//! Visual Settings dialog.
//!
//! Hand-rolled Win32 window built directly on `windows-rs` so we don't add
//! a heavyweight GUI framework just for one screen. The window is modal
//! relative to the tray (the message loop blocks in [`show`] until the
//! user closes it) and laid out manually in pixel coordinates.
//!
//! Layout (top-to-bottom):
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ Hotkeys                                      │
//! │   Pick element: [Ctrl+Shift+Space         ] │
//! │   Pick window:  [Ctrl+Alt+Space           ] │
//! │                                              │
//! │ Hints                                        │
//! │   Alphabet:     [asdfghjkl                ] │
//! │                                              │
//! │ Colors (#RRGGBB)                             │
//! │   Element bg:   [#FFE500                  ] │
//! │   Window bg:    [#33AAFF                  ] │
//! │                                              │
//! │ [x] Launch keyhop at Windows startup         │
//! │                                              │
//! │     [Save]   [Cancel]   [Reset to Defaults] │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! On Save the window:
//!   1. Reads each control's text via `WM_GETTEXT`.
//!   2. Validates hotkeys + colors via the existing parsers (so what the
//!      user can type is exactly what `config.toml` accepts).
//!   3. Writes `config.toml`.
//!   4. Mirrors the startup checkbox to the Run registry key.
//!   5. Closes the window — the caller surfaces a "restart to apply"
//!      hint since hot-reloading hotkeys mid-loop is out of scope for v0.2.0.

use std::cell::RefCell;
use std::ffi::c_void;

use anyhow::Result;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::BST_CHECKED;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SendMessageW, ShowWindow, TranslateMessage, UnregisterClassW, BM_GETCHECK, BM_SETCHECK,
    BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL,
    CB_SETCURSEL, CBS_DROPDOWNLIST, CBS_HASSTRINGS, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_LEFT, HMENU,
    IDC_ARROW, MSG, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU,
    WS_TABSTOP, WS_VSCROLL, WS_VISIBLE,
};

use crate::alphabet_presets::{self, ALL_PRESETS};
use crate::config::{
    AlphabetPreset, BadgeColors, ColorConfig, Config, HintConfig, HotkeyBindings,
    PerformanceConfig, ScopeConfig, ScopeMode, StartupConfig,
};
use crate::hint::{HintStrategy, DEFAULT_ALPHABET};
use crate::windows::{hotkey, notification, overlay, startup};

const WINDOW_W: i32 = 560;
const WINDOW_H: i32 = 980;
const LABEL_W: i32 = 160;
const FIELD_X: i32 = 190;
const FIELD_W: i32 = 340;
const ROW_H: i32 = 30;
const PADDING: i32 = 14;

const ID_PICK_ELEMENT: usize = 1001;
const ID_PICK_WINDOW: usize = 1002;
const ID_ALPHABET: usize = 1003;
const ID_ELEMENT_BG: usize = 1004;
const ID_WINDOW_BG: usize = 1005;
const ID_LAUNCH_STARTUP: usize = 1006;
const ID_ELEMENT_OPACITY: usize = 1007;
const ID_WINDOW_OPACITY: usize = 1008;
const ID_SHOW_LEADER: usize = 1009;
const ID_HINT_STRATEGY: usize = 1010;
const ID_ALPHABET_PRESET: usize = 1011;
const ID_INCLUDE_NUMBERS: usize = 1012;
const ID_INCLUDE_EXTENDED: usize = 1013;
const ID_EXCLUDE_AMBIGUOUS: usize = 1014;
const ID_CUSTOM_ADDITIONS: usize = 1015;
const ID_SCOPE_MODE: usize = 1016;
const ID_MAX_ELEMENTS: usize = 1017;
const ID_ENABLE_CACHING: usize = 1018;
const ID_CACHE_TTL_MS: usize = 1019;
const ID_MIN_SINGLES: usize = 1020;
const ID_SAVE: usize = 1100;
const ID_CANCEL: usize = 1101;
const ID_RESET: usize = 1102;

/// Hint-strategy dropdown entries, stable index → enum mapping.
const STRATEGY_OPTIONS: &[(HintStrategy, &str)] = &[
    (HintStrategy::ShortestFirst, "Shortest first (recommended)"),
    (HintStrategy::FixedLength, "Fixed length"),
];

/// Scope-mode dropdown entries, stable index → enum mapping.
const SCOPE_OPTIONS: &[(ScopeMode, &str)] = &[
    (ScopeMode::ActiveWindow, "Active window only"),
    (ScopeMode::ActiveMonitor, "All windows on active monitor"),
    (ScopeMode::AllWindows, "All visible windows (every monitor)"),
];

/// Per-window state stashed in a thread_local. Holds child HWNDs we need
/// to query on Save and the most recent button outcome.
struct State {
    pick_element: HWND,
    pick_window: HWND,
    alphabet: HWND,
    hint_strategy: HWND,
    alphabet_preset: HWND,
    include_numbers: HWND,
    include_extended: HWND,
    exclude_ambiguous: HWND,
    custom_additions: HWND,
    min_singles: HWND,
    element_bg: HWND,
    window_bg: HWND,
    element_opacity: HWND,
    window_opacity: HWND,
    show_leader: HWND,
    scope_mode: HWND,
    max_elements: HWND,
    enable_caching: HWND,
    cache_ttl_ms: HWND,
    launch_startup: HWND,
    /// Snapshot of the leader-line preference at dialog open. Used to
    /// decide whether to write `show_leader = Some(...)` or leave it as
    /// `None` (preset default) — we only persist an explicit override
    /// when the user actually toggled the checkbox.
    initial_show_leader: Option<bool>,
    /// Set by the Save handler so [`show`] can return whether the config
    /// was actually written (vs. user cancelled).
    saved: bool,
}

thread_local! {
    /// The window proc runs on the same thread as [`show`] and we only
    /// ever have one settings window at a time, so a thread-local is
    /// simpler than smuggling pointers through `CREATESTRUCT`.
    static STATE: RefCell<Option<Box<State>>> = const { RefCell::new(None) };
}

/// Open the modal Settings window. Blocks until the user closes it.
/// Returns `Ok(true)` if the user clicked Save (config persisted),
/// `Ok(false)` if they cancelled or closed the window.
pub fn show(initial: &Config) -> Result<bool> {
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class_name = w!("KeyhopSettingsWindow");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name,
            w!("keyhop Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_W,
            WINDOW_H,
            None,
            None,
            hinstance,
            None,
        )?;

        build_controls(hwnd, hinstance, initial);
        let _ = ShowWindow(hwnd, SW_SHOW);

        // Modal message pump: only this window's messages drive the loop,
        // so the caller's main message loop is paused for the dialog's
        // lifetime — exactly the modal behaviour the user expects.
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let saved = STATE.with(|s| s.borrow().as_ref().map(|st| st.saved).unwrap_or(false));
        STATE.with(|s| {
            s.borrow_mut().take();
        });
        let _ = UnregisterClassW(class_name, hinstance);

        Ok(saved)
    }
}

unsafe fn build_controls(hwnd: HWND, hinstance: HINSTANCE, initial: &Config) {
    let mut y = PADDING;

    create_section_label(hwnd, hinstance, "Hotkeys", y);
    y += 22;

    create_label(hwnd, hinstance, "Pick element:", y);
    let pick_element = create_edit(
        hwnd,
        hinstance,
        &initial.hotkeys.pick_element,
        ID_PICK_ELEMENT,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Pick window:", y);
    let pick_window = create_edit(
        hwnd,
        hinstance,
        &initial.hotkeys.pick_window,
        ID_PICK_WINDOW,
        y,
    );
    y += ROW_H + PADDING;

    create_section_label(hwnd, hinstance, "Hints", y);
    y += 22;

    create_label(hwnd, hinstance, "Strategy:", y);
    let hint_strategy = create_combo(
        hwnd,
        hinstance,
        STRATEGY_OPTIONS.iter().map(|(_, label)| *label),
        STRATEGY_OPTIONS
            .iter()
            .position(|(s, _)| *s == initial.hints.strategy)
            .unwrap_or(0),
        ID_HINT_STRATEGY,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Alphabet preset:", y);
    let alphabet_preset = create_combo(
        hwnd,
        hinstance,
        ALL_PRESETS.iter().map(|p| alphabet_presets::preset_label(*p)),
        ALL_PRESETS
            .iter()
            .position(|p| *p == initial.hints.preset)
            .unwrap_or(0),
        ID_ALPHABET_PRESET,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Custom alphabet:", y);
    let alphabet = create_edit(hwnd, hinstance, &initial.hints.alphabet, ID_ALPHABET, y);
    y += ROW_H;

    create_label(hwnd, hinstance, "Custom additions:", y);
    let custom_additions = create_edit(
        hwnd,
        hinstance,
        &initial.hints.custom_additions,
        ID_CUSTOM_ADDITIONS,
        y,
    );
    y += ROW_H;

    let include_numbers = create_checkbox(
        hwnd,
        hinstance,
        "Include numbers (0-9)",
        initial.hints.include_numbers,
        ID_INCLUDE_NUMBERS,
        y,
    );
    y += ROW_H;
    let include_extended = create_checkbox(
        hwnd,
        hinstance,
        "Include extended keys (; ')",
        initial.hints.include_extended,
        ID_INCLUDE_EXTENDED,
        y,
    );
    y += ROW_H;
    let exclude_ambiguous = create_checkbox(
        hwnd,
        hinstance,
        "Exclude ambiguous characters (O 0)",
        initial.hints.exclude_ambiguous,
        ID_EXCLUDE_AMBIGUOUS,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Min single-key hints:", y);
    let min_singles = create_edit(
        hwnd,
        hinstance,
        &initial.hints.min_singles.to_string(),
        ID_MIN_SINGLES,
        y,
    );
    y += ROW_H + PADDING;

    create_section_label(hwnd, hinstance, "Colors (#RRGGBB)", y);
    y += 22;

    create_label(hwnd, hinstance, "Element badge bg:", y);
    let element_bg = create_edit(
        hwnd,
        hinstance,
        &color_or_default(&initial.colors.element.badge_bg, "#FFE500"),
        ID_ELEMENT_BG,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Window badge bg:", y);
    let window_bg = create_edit(
        hwnd,
        hinstance,
        &color_or_default(&initial.colors.window.badge_bg, "#33AAFF"),
        ID_WINDOW_BG,
        y,
    );
    y += ROW_H + PADDING;

    create_section_label(hwnd, hinstance, "Opacity (0-100, 0 = preset default)", y);
    y += 22;

    create_label(hwnd, hinstance, "Element opacity:", y);
    let element_opacity = create_edit(
        hwnd,
        hinstance,
        &opacity_or_blank(initial.colors.element.opacity),
        ID_ELEMENT_OPACITY,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Window opacity:", y);
    let window_opacity = create_edit(
        hwnd,
        hinstance,
        &opacity_or_blank(initial.colors.window.opacity),
        ID_WINDOW_OPACITY,
        y,
    );
    y += ROW_H + PADDING;

    // Single source of truth for the leader pref: if either picker has
    // an explicit value set, surface that (preferring `element` since
    // that's where the feature is most visible). When both are `None`,
    // the checkbox starts in its preset-default state (on).
    let initial_show_leader = initial
        .colors
        .element
        .show_leader
        .or(initial.colors.window.show_leader);
    let show_leader_checked = initial_show_leader.unwrap_or(true);
    let show_leader = create_checkbox(
        hwnd,
        hinstance,
        "Draw arrow from each badge to its target element",
        show_leader_checked,
        ID_SHOW_LEADER,
        y,
    );
    y += ROW_H + PADDING;

    create_section_label(hwnd, hinstance, "Scope", y);
    y += 22;

    create_label(hwnd, hinstance, "Target windows:", y);
    let scope_mode = create_combo(
        hwnd,
        hinstance,
        SCOPE_OPTIONS.iter().map(|(_, label)| *label),
        SCOPE_OPTIONS
            .iter()
            .position(|(m, _)| *m == initial.scope.mode)
            .unwrap_or(0),
        ID_SCOPE_MODE,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Max elements:", y);
    let max_elements = create_edit(
        hwnd,
        hinstance,
        &initial.scope.max_elements.to_string(),
        ID_MAX_ELEMENTS,
        y,
    );
    y += ROW_H + PADDING;

    create_section_label(hwnd, hinstance, "Performance", y);
    y += 22;

    let enable_caching = create_checkbox(
        hwnd,
        hinstance,
        "Cache element enumeration (faster repeated picks)",
        initial.performance.enable_caching,
        ID_ENABLE_CACHING,
        y,
    );
    y += ROW_H;

    create_label(hwnd, hinstance, "Cache TTL (ms):", y);
    let cache_ttl_ms = create_edit(
        hwnd,
        hinstance,
        &initial.performance.cache_ttl_ms.to_string(),
        ID_CACHE_TTL_MS,
        y,
    );
    y += ROW_H + PADDING;

    let startup_now = startup::is_enabled().unwrap_or(initial.startup.launch_at_startup);
    let launch_startup = create_checkbox(
        hwnd,
        hinstance,
        "Launch keyhop at Windows startup",
        startup_now,
        ID_LAUNCH_STARTUP,
        y,
    );
    y += ROW_H + PADDING * 2;

    create_button(hwnd, hinstance, "Save", ID_SAVE, PADDING, y, true);
    create_button(
        hwnd,
        hinstance,
        "Cancel",
        ID_CANCEL,
        PADDING + 130,
        y,
        false,
    );
    create_button(
        hwnd,
        hinstance,
        "Reset to Defaults",
        ID_RESET,
        PADDING + 260,
        y,
        false,
    );

    let state = Box::new(State {
        pick_element,
        pick_window,
        alphabet,
        hint_strategy,
        alphabet_preset,
        include_numbers,
        include_extended,
        exclude_ambiguous,
        custom_additions,
        min_singles,
        element_bg,
        window_bg,
        element_opacity,
        window_opacity,
        show_leader,
        scope_mode,
        max_elements,
        enable_caching,
        cache_ttl_ms,
        launch_startup,
        initial_show_leader,
        saved: false,
    });
    STATE.with(|s| *s.borrow_mut() = Some(state));
}

fn color_or_default(value: &str, default_hex: &str) -> String {
    if value.trim().is_empty() {
        default_hex.to_string()
    } else {
        value.to_string()
    }
}

/// Render a 0..=100 opacity value into the edit field. `0` is the
/// "use preset default" sentinel and is shown as an empty string so
/// users see a clean field rather than a confusing zero.
fn opacity_or_blank(value: u8) -> String {
    if value == 0 {
        String::new()
    } else {
        value.to_string()
    }
}

/// Parse the opacity edit field back into the 0..=100 representation.
/// Empty strings (and anything unparseable) become `0` = "preset
/// default", matching what the placeholder text implies.
fn parse_opacity(text: &str) -> u8 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }
    trimmed
        .parse::<u32>()
        .map(|v| v.min(100) as u8)
        .unwrap_or(0)
}

unsafe fn create_section_label(parent: HWND, hinstance: HINSTANCE, text: &str, y: i32) {
    let text_w = to_wide(text);
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("STATIC"),
        PCWSTR(text_w.as_ptr()),
        WS_CHILD | WS_VISIBLE,
        PADDING,
        y,
        WINDOW_W - PADDING * 2,
        20,
        parent,
        HMENU::default(),
        hinstance,
        None,
    );
}

unsafe fn create_label(parent: HWND, hinstance: HINSTANCE, text: &str, y: i32) {
    let text_w = to_wide(text);
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("STATIC"),
        PCWSTR(text_w.as_ptr()),
        WS_CHILD | WS_VISIBLE,
        PADDING,
        y + 4,
        LABEL_W,
        20,
        parent,
        HMENU::default(),
        hinstance,
        None,
    );
}

unsafe fn create_edit(
    parent: HWND,
    hinstance: HINSTANCE,
    initial_text: &str,
    id: usize,
    y: i32,
) -> HWND {
    let text_w = to_wide(initial_text);
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("EDIT"),
        PCWSTR(text_w.as_ptr()),
        WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | WS_TABSTOP
            | WINDOW_STYLE((ES_AUTOHSCROLL | ES_LEFT) as u32),
        FIELD_X,
        y,
        FIELD_W,
        24,
        parent,
        HMENU(id as *mut c_void),
        hinstance,
        None,
    )
    .unwrap_or(HWND(std::ptr::null_mut()))
}

unsafe fn create_checkbox(
    parent: HWND,
    hinstance: HINSTANCE,
    text: &str,
    checked: bool,
    id: usize,
    y: i32,
) -> HWND {
    let text_w = to_wide(text);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        PCWSTR(text_w.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        PADDING,
        y,
        WINDOW_W - PADDING * 2,
        24,
        parent,
        HMENU(id as *mut c_void),
        hinstance,
        None,
    )
    .unwrap_or(HWND(std::ptr::null_mut()));
    if checked && !hwnd.0.is_null() {
        SendMessageW(hwnd, BM_SETCHECK, WPARAM(BST_CHECKED.0 as usize), LPARAM(0));
    }
    hwnd
}

/// Create a non-editable dropdown (CBS_DROPDOWNLIST) with the given
/// items and pre-select `selected_index`. Returns the combobox HWND so
/// the caller can read the active selection on Save.
unsafe fn create_combo<'a, I: IntoIterator<Item = &'a str>>(
    parent: HWND,
    hinstance: HINSTANCE,
    items: I,
    selected_index: usize,
    id: usize,
    y: i32,
) -> HWND {
    // CBS_DROPDOWNLIST = read-only dropdown (no edit field), CBS_HASSTRINGS
    // is required so CB_ADDSTRING actually stores the strings rather
    // than expecting owner-draw item data.
    let style = CBS_DROPDOWNLIST | CBS_HASSTRINGS;
    // Tall enough that the dropdown list shows 6 items without scrolling.
    let height = 200;
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("COMBOBOX"),
        PCWSTR::null(),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WS_VSCROLL
            | WINDOW_STYLE(style as u32),
        FIELD_X,
        y,
        FIELD_W,
        height,
        parent,
        HMENU(id as *mut c_void),
        hinstance,
        None,
    )
    .unwrap_or(HWND(std::ptr::null_mut()));
    if hwnd.0.is_null() {
        return hwnd;
    }
    for item in items {
        let wide = to_wide(item);
        SendMessageW(hwnd, CB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
    }
    SendMessageW(
        hwnd,
        CB_SETCURSEL,
        WPARAM(selected_index),
        LPARAM(0),
    );
    hwnd
}

unsafe fn create_button(
    parent: HWND,
    hinstance: HINSTANCE,
    text: &str,
    id: usize,
    x: i32,
    y: i32,
    default: bool,
) {
    let text_w = to_wide(text);
    let style_bits = if default {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        PCWSTR(text_w.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(style_bits as u32),
        x,
        y,
        120,
        30,
        parent,
        HMENU(id as *mut c_void),
        hinstance,
        None,
    );
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            let notif = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if notif == BN_CLICKED {
                match id {
                    ID_SAVE => on_save(hwnd),
                    ID_CANCEL => {
                        let _ = DestroyWindow(hwnd);
                    }
                    ID_RESET => on_reset(hwnd),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn on_save(hwnd: HWND) {
    let (cfg, launch_startup) = match collect_config() {
        Some(v) => v,
        None => return,
    };

    if let Err(e) = validate(&cfg) {
        notification::error(
            "Invalid settings",
            &format!("{e}\n\nPlease fix and try again."),
        );
        return;
    }

    if let Err(e) = cfg.save() {
        notification::error("Failed to save config", &format!("{e}"));
        return;
    }

    if let Err(e) = startup::set_enabled(launch_startup) {
        notification::warn(
            "Couldn't update Windows startup",
            &format!("Settings were saved, but the startup registry update failed:\n{e}"),
        );
    }

    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.saved = true;
        }
    });

    let _ = DestroyWindow(hwnd);
}

unsafe fn on_reset(hwnd: HWND) {
    if let Err(e) = Config::delete_file() {
        notification::error("Couldn't reset config", &format!("{e}"));
        return;
    }
    if let Err(e) = startup::set_enabled(false) {
        notification::warn(
            "Reset partially complete",
            &format!("Config was deleted, but disabling startup failed:\n{e}"),
        );
    }
    notification::info(
        "Settings reset",
        "Config file deleted. Defaults will apply on next launch.",
    );
    let _ = DestroyWindow(hwnd);
}

fn collect_config() -> Option<(Config, bool)> {
    STATE.with(|s| {
        let state = s.borrow();
        let st = state.as_ref()?;
        let pick_element = unsafe { read_text(st.pick_element) };
        let pick_window = unsafe { read_text(st.pick_window) };
        let alphabet = unsafe { read_text(st.alphabet) };
        let custom_additions = unsafe { read_text(st.custom_additions) };
        let element_bg = unsafe { read_text(st.element_bg) };
        let window_bg = unsafe { read_text(st.window_bg) };
        let element_opacity = parse_opacity(&unsafe { read_text(st.element_opacity) });
        let window_opacity = parse_opacity(&unsafe { read_text(st.window_opacity) });
        let strategy_idx = unsafe { combo_selection(st.hint_strategy) };
        let strategy = STRATEGY_OPTIONS
            .get(strategy_idx)
            .map(|(s, _)| *s)
            .unwrap_or_default();
        let preset_idx = unsafe { combo_selection(st.alphabet_preset) };
        let preset = ALL_PRESETS
            .get(preset_idx)
            .copied()
            .unwrap_or(AlphabetPreset::HomeRow);
        let scope_idx = unsafe { combo_selection(st.scope_mode) };
        let scope_mode = SCOPE_OPTIONS
            .get(scope_idx)
            .map(|(m, _)| *m)
            .unwrap_or_default();
        let max_elements = unsafe { read_text(st.max_elements) }
            .trim()
            .parse::<usize>()
            .unwrap_or(300)
            .max(1);
        let cache_ttl_ms = unsafe { read_text(st.cache_ttl_ms) }
            .trim()
            .parse::<u64>()
            .unwrap_or(500);
        let include_numbers = unsafe { is_checked(st.include_numbers) };
        let include_extended = unsafe { is_checked(st.include_extended) };
        let exclude_ambiguous = unsafe { is_checked(st.exclude_ambiguous) };
        let min_singles = unsafe { read_text(st.min_singles) }
            .trim()
            .parse::<usize>()
            .unwrap_or(0);
        let enable_caching = unsafe { is_checked(st.enable_caching) };
        let show_leader_checked = unsafe { is_checked(st.show_leader) };
        // Keep the preset default (None) unless the user actually
        // changed the checkbox from how we showed it. This way upgrade
        // paths don't suddenly hard-pin everyone to a value, and the
        // window picker stays leader-less unless the user explicitly
        // opts in.
        let show_leader = if Some(show_leader_checked) == st.initial_show_leader {
            st.initial_show_leader
        } else {
            Some(show_leader_checked)
        };
        let launch_startup = unsafe { is_checked(st.launch_startup) };

        // For non-Custom presets the "custom alphabet" field is a
        // preview / power-user override only — we always rebuild the
        // final alphabet from the preset on Save so the dropdown is
        // the source of truth. Users wanting a fully bespoke alphabet
        // pick the "Custom" preset, which preserves the field
        // verbatim through `build_alphabet`.
        let hints_partial = HintConfig {
            alphabet: if alphabet.trim().is_empty() {
                DEFAULT_ALPHABET.to_string()
            } else {
                alphabet
            },
            strategy,
            preset,
            include_numbers,
            include_extended,
            exclude_ambiguous,
            custom_additions,
            min_singles,
        };
        // Materialise the preset choices into a concrete alphabet
        // string before we save. This way `config.toml` always has a
        // ready-to-use alphabet — even if a future build can't read
        // the preset enum (or someone hand-edits the field), the
        // engine still gets a usable character set.
        let resolved_alphabet = alphabet_presets::build_alphabet(&hints_partial);
        let hints = HintConfig {
            alphabet: resolved_alphabet,
            ..hints_partial
        };

        let cfg = Config {
            hotkeys: HotkeyBindings {
                pick_element,
                pick_window,
            },
            hints,
            colors: ColorConfig {
                element: BadgeColors {
                    badge_bg: element_bg,
                    badge_fg: String::new(),
                    border: String::new(),
                    opacity: element_opacity,
                    show_leader,
                    leader_color: String::new(),
                },
                window: BadgeColors {
                    badge_bg: window_bg,
                    badge_fg: String::new(),
                    border: String::new(),
                    opacity: window_opacity,
                    show_leader,
                    leader_color: String::new(),
                },
            },
            startup: StartupConfig {
                launch_at_startup: launch_startup,
            },
            scope: ScopeConfig {
                mode: scope_mode,
                max_elements,
            },
            performance: PerformanceConfig {
                enable_caching,
                cache_ttl_ms,
            },
        };
        Some((cfg, launch_startup))
    })
}

/// Read the currently selected index of a CBS_DROPDOWNLIST combobox.
/// Returns 0 when the box is empty or in error so the caller can
/// dispatch through `STRATEGY_OPTIONS` / `SCOPE_OPTIONS` arrays
/// without needing an Option dance.
unsafe fn combo_selection(hwnd: HWND) -> usize {
    let r = SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0));
    if r.0 < 0 {
        0
    } else {
        r.0 as usize
    }
}

/// True when the BS_AUTOCHECKBOX `hwnd` is currently checked.
unsafe fn is_checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize
}

fn validate(cfg: &Config) -> Result<()> {
    hotkey::parse_hotkey(&cfg.hotkeys.pick_element)
        .map_err(|e| anyhow::anyhow!("Pick element hotkey is invalid:\n  {e}"))?;
    hotkey::parse_hotkey(&cfg.hotkeys.pick_window)
        .map_err(|e| anyhow::anyhow!("Pick window hotkey is invalid:\n  {e}"))?;

    if cfg.hints.alphabet.trim().is_empty() {
        anyhow::bail!("Hint alphabet must not be empty.");
    }

    if !cfg.colors.element.badge_bg.trim().is_empty() {
        overlay::parse_hex_color(&cfg.colors.element.badge_bg)
            .map_err(|e| anyhow::anyhow!("Element badge color is invalid:\n  {e}"))?;
    }
    if !cfg.colors.window.badge_bg.trim().is_empty() {
        overlay::parse_hex_color(&cfg.colors.window.badge_bg)
            .map_err(|e| anyhow::anyhow!("Window badge color is invalid:\n  {e}"))?;
    }

    Ok(())
}

unsafe fn read_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let read = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..read as usize])
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
