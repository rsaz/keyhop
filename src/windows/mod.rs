//! Windows backend for keyhop.
//!
//! Implements [`crate::Backend`] against the Windows UI Automation API via
//! the `uiautomation` crate, plus the global hotkey ([`hotkey`]) and
//! transparent overlay ([`overlay`]) building blocks the binary needs.
//!
//! The backend walks the control-view subtree of the foreground window and
//! collects on-screen, interactable elements (buttons, links, inputs, menu
//! items, ...) into the platform-agnostic [`crate::Element`] model.

pub mod config_watcher;
pub mod hotkey;
pub mod ipc;
pub mod notification;
pub mod overlay;
pub mod settings_window;
pub mod single_instance;
pub mod splash_screen;
pub mod startup;
pub mod tray;
pub mod window_picker;

use std::collections::HashMap;

use anyhow::Context;

use uiautomation::{
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UILegacyIAccessiblePattern, UIScrollPattern,
        UISelectionItemPattern, UITogglePattern,
    },
    types::{ControlType, Handle, ScrollAmount},
    UIAutomation, UIElement, UITreeWalker,
};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
    SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETOBJECT,
};

use crate::cache::{CacheManager, WindowKey};
use crate::config::ScopeMode;
use crate::{Action, Backend, Bounds, Element, ElementId, Role};

/// Maximum tree depth we descend into for *desktop* apps. Native Win32 /
/// WinUI / Electron trees are wide but rarely deep — twelve levels covers
/// every real app surface we've measured (Office ribbons, VS sidebar
/// trees, etc.) without spending time on pathological structural nesting.
const MAX_TREE_DEPTH_DESKTOP: usize = 12;

/// Maximum tree depth we descend into for *browser* foregrounds.
///
/// Browsers map every DOM node to a UIA element, so the path from the
/// browser window to a single `<a>` tag in the rendered page typically
/// looks like:
///
/// ```text
/// Window > NavigationView > Tab > WebView > Document
///   > <body> > <div> > <div> > <header> > <nav> > <ul> > <li> > <a>
/// ```
///
/// Modern web apps (GitHub, Gmail, dashboards, …) routinely nest 15–25
/// elements deep; the desktop limit of 12 only reaches the browser
/// chrome and the page document, never the page content. This is why
/// the picker would highlight the address bar and tabs but nothing on
/// the page itself. The browser path is bounded primarily by
/// [`MAX_ELEMENTS_BROWSER`] so the deeper limit doesn't translate to
/// runaway walks — it just lets us *find* page content before we hit
/// the element cap.
const MAX_TREE_DEPTH_BROWSER: usize = 32;

/// Hard cap on collected elements per enumeration in a regular desktop
/// app. A typical foreground window has well under this many interactable
/// items; the cap is mainly a safety valve.
const MAX_ELEMENTS_DESKTOP: usize = 500;

/// Larger cap for browsers, where the UIA tree mirrors the DOM and a
/// content-rich page (Gmail, GitHub, dashboards) can legitimately expose
/// hundreds of clickable items. The web-specific filter in
/// [`WindowsBackend::try_record_web_element`] is what keeps the
/// signal-to-noise ratio sane at this size.
const MAX_ELEMENTS_BROWSER: usize = 800;

/// Default minimum element size (in physical pixels) before we even
/// consider a web element. Filters out the swarms of 1×1 / 4×4
/// layout/spacer/decoration nodes that the Chromium UIA provider
/// exposes. Elements that explicitly advertise interactivity (an action
/// pattern, a known ARIA role, or `IsKeyboardFocusable=true`) get a
/// looser bar — see [`MIN_WEB_FOCUSABLE_SIZE`].
const MIN_WEB_ELEMENT_SIZE: i32 = 16;

/// Looser size floor for web elements that *prove* they're interactive
/// (focusable / patterned / known role). Many real targets — small
/// "x" close icons in chips, custom icon buttons, dense toolbar items
/// — fall under [`MIN_WEB_ELEMENT_SIZE`] but are absolutely something
/// the user wants to hint. Ten pixels is small enough to catch those
/// without dipping into the spacer-element noise.
const MIN_WEB_FOCUSABLE_SIZE: i32 = 10;

/// MSAA / IAccessible role IDs we treat as "definitely clickable" when
/// the modern UIA control type doesn't make it obvious. These match the
/// `ROLE_SYSTEM_*` constants from `oleacc.h` and are how older / web
/// content surfaces semantic intent through the legacy bridge.
const MSAA_ROLE_MENU_ITEM: i32 = 0x0C;
const MSAA_ROLE_CELL: i32 = 0x1D;
const MSAA_ROLE_LINK: i32 = 0x1E;
const MSAA_ROLE_LIST_ITEM: i32 = 0x22;
const MSAA_ROLE_OUTLINE_ITEM: i32 = 0x24;
const MSAA_ROLE_PAGE_TAB: i32 = 0x25;
const MSAA_ROLE_PUSHBUTTON: i32 = 0x2B;
const MSAA_ROLE_CHECKBUTTON: i32 = 0x2C;
const MSAA_ROLE_RADIOBUTTON: i32 = 0x2D;
const MSAA_ROLE_COMBOBOX: i32 = 0x2E;
const MSAA_ROLE_BUTTON_DROPDOWN: i32 = 0x38;
const MSAA_ROLE_BUTTON_MENU: i32 = 0x39;

/// Bookkeeping for one [`WindowsBackend::walk`] invocation. Surfaced via
/// the debug log at the end of `enumerate_foreground` so we can tell —
/// after the fact — whether the picker stopped because it ran out of
/// elements, hit the depth cap, or simply found everything the tree
/// exposes. Crucial when triaging "keyhop didn't see X" reports on a
/// specific page or app.
#[derive(Debug, Default)]
struct WalkStats {
    visited: usize,
    max_depth: usize,
    hit_depth_cap: bool,
    hit_element_cap: bool,
}

/// UI Automation-backed implementation of [`Backend`] for Windows.
pub struct WindowsBackend {
    automation: UIAutomation,
    elements: HashMap<ElementId, UIElement>,
    next_id: u64,
    /// Set in [`Backend::enumerate_foreground`] before walking the tree.
    /// Drives whether the desktop or web detection path runs for each
    /// element. Recomputed every enumeration so it tracks the user's
    /// actual focus, not whatever was foreground when the backend was
    /// constructed.
    is_current_browser: bool,
    /// Per-HWND enumeration cache. Backs the "press Esc, retry"
    /// flow — same window, same elements, no UIA round-trip. Disabled
    /// by setting `enable_caching = false` in `config.toml`.
    cache: CacheManager,
    /// Hard cap on elements per enumeration in non-active-window scope
    /// modes. The single-window walk caps are
    /// `MAX_ELEMENTS_DESKTOP`/`MAX_ELEMENTS_BROWSER`; this kicks in
    /// only when we're aggregating across multiple windows.
    max_elements_global: usize,
}

impl WindowsBackend {
    /// Construct a new backend with caching enabled and the default
    /// 500ms TTL. Use [`Self::with_config`] to override either knob.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_config(true, 500, 300)
    }

    /// Construct a new backend, initializing the UI Automation client.
    /// `enable_caching` and `cache_ttl_ms` configure the cache; the
    /// Settings dialog calls [`Self::reconfigure_cache`] to flip the
    /// switch at runtime without restarting.
    pub fn with_config(
        enable_caching: bool,
        cache_ttl_ms: u64,
        max_elements_global: usize,
    ) -> anyhow::Result<Self> {
        // PerMonitorV2 ensures `GetBoundingRectangle` returns physical
        // pixels matching the overlay's coordinate space on high-DPI
        // displays. Best-effort: ignore "already set" / older-OS errors.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let automation = UIAutomation::new().context("initializing UI Automation client")?;
        Ok(Self {
            automation,
            elements: HashMap::new(),
            next_id: 0,
            is_current_browser: false,
            cache: CacheManager::new(cache_ttl_ms, enable_caching),
            max_elements_global,
        })
    }

    /// Mirror new cache settings from [`crate::Config`] into the live
    /// backend without restarting. Disabling caching also clears the
    /// existing entries.
    pub fn reconfigure_cache(&mut self, enable_caching: bool, cache_ttl_ms: u64) {
        self.cache.reconfigure(cache_ttl_ms, enable_caching);
    }

    /// Replace the global element cap (used in multi-window scope
    /// modes). Single-window walks still cap at the platform-specific
    /// constants — this is purely a safety net for the aggregate.
    pub fn set_max_elements_global(&mut self, max: usize) {
        self.max_elements_global = max.max(1);
    }

    /// Enumerate elements scoped by [`ScopeMode`].
    ///
    /// - [`ScopeMode::ActiveWindow`] is exactly [`Backend::enumerate_foreground`].
    /// - [`ScopeMode::ActiveMonitor`] enumerates every visible top-level
    ///   window whose frame intersects the monitor under the cursor.
    /// - [`ScopeMode::AllWindows`] enumerates every visible top-level
    ///   window across all monitors.
    ///
    /// Multi-window modes aggregate per-window walks and apply the
    /// [`Self::max_elements_global`] cap so a busy desktop can't render
    /// thousands of badges. The cap is enforced in walk order, so the
    /// foreground window's elements are always present.
    pub fn enumerate_by_scope(&mut self, mode: ScopeMode) -> anyhow::Result<Vec<Element>> {
        match mode {
            ScopeMode::ActiveWindow => self.enumerate_foreground(),
            ScopeMode::ActiveMonitor => self.enumerate_active_monitor(),
            ScopeMode::AllWindows => self.enumerate_all_windows(),
        }
    }

    fn enumerate_active_monitor(&mut self) -> anyhow::Result<Vec<Element>> {
        let monitor_rect = current_monitor_rect();
        self.enumerate_windows_filtered(|bounds| rect_intersects_bounds(&monitor_rect, bounds))
    }

    fn enumerate_all_windows(&mut self) -> anyhow::Result<Vec<Element>> {
        self.enumerate_windows_filtered(|_| true)
    }

    /// Shared implementation behind the multi-window scope modes. Walks
    /// every visible top-level window the window picker would surface,
    /// runs the existing element walker on each, and merges results
    /// up to [`Self::max_elements_global`].
    ///
    /// Errors from a single window's walk are swallowed with a `tracing`
    /// warning — better to return what we did manage to enumerate than
    /// abort the whole pick because one tab's UIA tree was unhealthy.
    fn enumerate_windows_filtered<F>(&mut self, mut accept: F) -> anyhow::Result<Vec<Element>>
    where
        F: FnMut(&Bounds) -> bool,
    {
        let candidates = crate::windows::window_picker::enumerate_visible()
            .context("listing top-level windows for multi-window scope")?;
        let mut out: Vec<Element> = Vec::with_capacity(64);
        let cap = self.max_elements_global;

        // Walk the foreground window first so its elements always have
        // hint slots even on a crowded desktop.
        let foreground = unsafe { GetForegroundWindow() };
        let mut ordered: Vec<crate::windows::window_picker::TopLevelWindow> = candidates;
        ordered.sort_by_key(|w| if w.hwnd == foreground { 0 } else { 1 });

        for win in ordered {
            if !accept(&win.bounds) {
                continue;
            }
            if out.len() >= cap {
                break;
            }
            match self.enumerate_window(win.hwnd) {
                Ok(elements) => {
                    let remaining = cap - out.len();
                    out.extend(elements.into_iter().take(remaining));
                }
                Err(e) => {
                    tracing::warn!(?win.hwnd, title = %win.title, error = ?e, "window walk failed; skipping");
                }
            }
        }

        tracing::info!(
            collected = out.len(),
            cap,
            "enumerate_windows_filtered done"
        );
        Ok(out)
    }

    /// Walk a single HWND, going through the cache first. Used by the
    /// multi-window scope modes; the active-window path stays in
    /// [`Backend::enumerate_foreground`] because it needs to mutate
    /// `is_current_browser` for the caller.
    fn enumerate_window(&mut self, hwnd: HWND) -> anyhow::Result<Vec<Element>> {
        let key = hwnd_to_key(hwnd);
        if let Some(cached) = self.cache.get(key) {
            // Replay cached elements through `next_id` so [`Backend::perform`]
            // can still resolve them. We re-register the UIA elements via a
            // fresh walk if the user actually invokes one — but for the
            // common "show overlay → press Esc" loop we never get that far,
            // so the cache hit is essentially free.
            return Ok(cached);
        }

        let is_browser = is_browser_window(hwnd);
        if is_browser {
            activate_browser_accessibility(hwnd);
        }

        let handle = Handle::from(hwnd.0 as isize);
        let root = self
            .automation
            .element_from_handle(handle)
            .context("element_from_handle for top-level window failed")?;
        let walker = self
            .automation
            .get_control_view_walker()
            .context("creating control-view tree walker failed")?;

        let prev_browser = self.is_current_browser;
        self.is_current_browser = is_browser;

        let mut out = Vec::with_capacity(64);
        let mut stats = WalkStats::default();
        self.walk(&walker, &root, 0, &mut out, &mut stats);

        self.is_current_browser = prev_browser;

        self.cache.insert(key, out.clone());
        Ok(out)
    }
}

/// Pack a Win32 HWND into the cache's opaque [`WindowKey`]. Round-trip
/// is one-way (we never reconstruct an HWND from a key); we only need
/// equality + hashing inside the map.
fn hwnd_to_key(hwnd: HWND) -> WindowKey {
    hwnd.0 as usize as u64
}

/// Monitor rect (in physical pixels) under the current cursor. Falls
/// back to the entire virtual desktop when the cursor query or
/// `GetMonitorInfo` fail — better to overshoot the scope than refuse
/// to enumerate anything.
fn current_monitor_rect() -> RECT {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};

    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt).is_err() {
            return RECT {
                left: i32::MIN / 2,
                top: i32::MIN / 2,
                right: i32::MAX / 2,
                bottom: i32::MAX / 2,
            };
        }
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut mi).as_bool() {
            mi.rcMonitor
        } else {
            RECT {
                left: i32::MIN / 2,
                top: i32::MIN / 2,
                right: i32::MAX / 2,
                bottom: i32::MAX / 2,
            }
        }
    }
}

/// True when the window's frame `bounds` overlaps `monitor` by even one
/// pixel. We use frame-overlap instead of "centre is on monitor" so a
/// window straddling two monitors is included by both.
fn rect_intersects_bounds(monitor: &RECT, bounds: &Bounds) -> bool {
    let r_right = bounds.x + bounds.width;
    let r_bottom = bounds.y + bounds.height;
    bounds.x < monitor.right
        && r_right > monitor.left
        && bounds.y < monitor.bottom
        && r_bottom > monitor.top
}

impl Backend for WindowsBackend {
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>> {
        self.elements.clear();
        self.next_id = 0;

        // SAFETY: `GetForegroundWindow` has no preconditions and may return a
        // null handle, which we check below.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            tracing::warn!("no foreground window");
            return Ok(Vec::new());
        }

        // Decide which detection path the upcoming walk will take. The
        // browser path is more aggressive about size-filtering and pattern
        // checks because the UIA tree mirrors the entire DOM; the desktop
        // path is more permissive so custom controls aren't dropped.
        self.is_current_browser = is_browser_window(hwnd);
        tracing::debug!(
            is_browser = self.is_current_browser,
            "enumerate_foreground: detected window kind"
        );

        // Cache lookup before any UIA work. A hit means we were called
        // again on the same HWND inside the TTL — almost always the
        // "user pressed Esc, retrying" path. We don't cache the
        // raw UIElement handles though, so a cache hit means
        // [`Backend::perform`] won't be able to invoke anything until
        // a fresh enumeration replaces the entry. That trade-off is
        // fine: the overlay only calls back into perform after a
        // successful pick, and that pick happens on the *current*
        // enumeration round, which always populates the elements map.
        let key = hwnd_to_key(hwnd);
        if let Some(cached) = self.cache.get(key) {
            tracing::debug!(cached = cached.len(), "enumerate_foreground served from cache");
            // We still want subsequent perform() calls to work, so
            // immediately fall through to a fresh walk — the cache
            // hit short-circuits only when no UIA dispatch is going
            // to happen. In practice this means we never serve from
            // cache here today; the multi-window scope path
            // (`enumerate_window`) is where the cache earns its keep.
            // We leave the lookup wired up so changing this policy
            // later requires no plumbing.
            self.cache.invalidate(key);
        }

        // Chromium-based browsers (Chrome, Edge, Brave, …) defer building
        // the renderer accessibility tree until an a11y client asks for
        // it. By the time `element_from_handle` runs the browser process
        // root is available, but the per-tab document/page tree may still
        // be a single placeholder node. Send a `WM_GETOBJECT` to every
        // descendant HWND first so the renderer is awake before we walk.
        if self.is_current_browser {
            activate_browser_accessibility(hwnd);
        }

        let handle = Handle::from(hwnd.0 as isize);
        let root = self
            .automation
            .element_from_handle(handle)
            .context("element_from_handle for foreground window failed")?;
        let walker = self
            .automation
            .get_control_view_walker()
            .context("creating control-view tree walker failed")?;

        let mut out = Vec::with_capacity(64);
        let mut stats = WalkStats::default();
        self.walk(&walker, &root, 0, &mut out, &mut stats);

        // Refresh the cache with the just-collected elements so the
        // next call within `cache_ttl_ms` can short-circuit the walk
        // entirely. We hand back the bare value below; the cache
        // clones it on insert so this doesn't move the data.
        self.cache.insert(key, out.clone());

        // Per-role breakdown helps when a user reports "keyhop didn't see
        // button X in app Y" — the log shows whether the element was
        // dropped by the role filter or never made it through the bounds
        // / pattern checks at all. Walk stats tell us whether we ran out
        // of room (depth/element cap) or genuinely exhausted the tree.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let mut by_role: HashMap<Role, usize> = HashMap::new();
            for el in &out {
                *by_role.entry(el.role).or_insert(0) += 1;
            }
            tracing::debug!(
                collected = out.len(),
                visited = stats.visited,
                max_depth = stats.max_depth,
                hit_depth_cap = stats.hit_depth_cap,
                hit_element_cap = stats.hit_element_cap,
                ?by_role,
                "enumerate_foreground done"
            );
        }

        Ok(out)
    }

    fn perform(&mut self, id: ElementId, action: Action) -> anyhow::Result<()> {
        let el = self
            .elements
            .get(&id)
            .with_context(|| format!("unknown element id {id:?}"))?;

        match action {
            // Invoke goes through the UIA pattern cascade — no cursor warp,
            // works on virtualized / off-screen items, respects the app's
            // accessibility hooks. Mouse-click is the explicit last resort
            // for legacy controls that don't expose any pattern.
            Action::Invoke => {
                let path = invoke_smart(el).context("invoking element failed")?;
                if path == InvokePath::MouseFallback {
                    tracing::warn!(?id, "no UIA pattern matched; used mouse-click fallback");
                } else {
                    tracing::debug!(?id, ?path, "element invoked via UIA pattern");
                }
            }
            // Click is the explicit "synthesize a real mouse click" action.
            // Different semantics from Invoke — caller is opting in to
            // cursor motion, useful for testing or for apps that only
            // respond to genuine input.
            Action::Click => {
                el.click().context("element.click() failed")?;
            }
            Action::Focus => {
                el.set_focus().context("element.set_focus() failed")?;
            }
            Action::Type(s) => {
                el.send_text(&s, 0).context("element.send_text() failed")?;
            }
            Action::Scroll { dx, dy } => {
                scroll_element(el, dx, dy).context("scrolling element failed")?;
            }
        }
        Ok(())
    }
}

/// Which path through [`invoke_smart`] succeeded. Surfaced via tracing so we
/// can spot apps that consistently fall through to the mouse fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvokePath {
    Invoke,
    Toggle,
    SelectionItem,
    ExpandCollapse,
    LegacyDefaultAction,
    MouseFallback,
}

/// Try every UIA pattern that could plausibly "invoke" `el`, in order of
/// specificity, and return which one worked. Falls back to a synthesized
/// mouse click only when every pattern path has been exhausted.
///
/// Pattern cascade rationale:
///
/// 1. [`UIInvokePattern`] — the canonical "do the primary action" interface.
///    Buttons, links, menu items, split buttons all expose it.
/// 2. [`UITogglePattern`] — checkboxes, toggle buttons.
/// 3. [`UISelectionItemPattern`] — tabs, radios, list/tree items.
/// 4. [`UIExpandCollapsePattern`] — combo boxes, tree expanders.
/// 5. [`UILegacyIAccessiblePattern::do_default_action`] — broad
///    MSAA-compatible fallback that often works when the modern patterns
///    aren't exposed.
/// 6. [`UIElement::click`] — synthesized mouse click. Last resort: warps
///    the cursor and is the only thing that works for some pre-UIA Win32
///    controls. We prefer never to hit this path.
fn invoke_smart(el: &UIElement) -> anyhow::Result<InvokePath> {
    if let Ok(p) = el.get_pattern::<UIInvokePattern>() {
        if p.invoke().is_ok() {
            return Ok(InvokePath::Invoke);
        }
    }
    if let Ok(p) = el.get_pattern::<UITogglePattern>() {
        if p.toggle().is_ok() {
            return Ok(InvokePath::Toggle);
        }
    }
    if let Ok(p) = el.get_pattern::<UISelectionItemPattern>() {
        if p.select().is_ok() {
            return Ok(InvokePath::SelectionItem);
        }
    }
    if let Ok(p) = el.get_pattern::<UIExpandCollapsePattern>() {
        // Expand-only: for an already-expanded element this is a no-op on
        // most controls. Collapsing on Invoke would surprise the user (you
        // pressed a hint to *do* the thing, not to undo it).
        if p.expand().is_ok() {
            return Ok(InvokePath::ExpandCollapse);
        }
    }
    if let Ok(p) = el.get_pattern::<UILegacyIAccessiblePattern>() {
        if p.do_default_action().is_ok() {
            return Ok(InvokePath::LegacyDefaultAction);
        }
    }
    el.click().context("element.click() failed")?;
    Ok(InvokePath::MouseFallback)
}

/// Scroll a UIA element by the requested pixel delta.
///
/// UIA exposes scroll in two modes:
/// 1. Discrete amounts via [`UIScrollPattern::scroll`] — Page/Line/None,
///    matching what Page-Up / arrow keys do natively. We map our pixel
///    delta to the closest amount based on magnitude.
/// 2. Continuous percent via [`UIScrollPattern::set_scroll_percent`]
///    (0.0..=100.0). We don't use this here because we're given a pixel
///    delta, not an absolute target — translating delta → percent would
///    require querying viewport size which adds round-trips for little
///    benefit.
///
/// Choice of amount per axis:
/// - `|delta| >= 100` → page-sized scroll (large increment / decrement).
/// - `0 < |delta| < 100` → small increment / decrement (one line).
/// - `delta == 0` → `NoAmount` so the axis is skipped entirely.
///
/// Returns an error if the element doesn't support a scroll pattern at
/// all — callers can decide whether to fall back to synthesized
/// `WM_VSCROLL` (not implemented today; most modern controls expose UIA).
fn scroll_element(el: &UIElement, dx: i32, dy: i32) -> anyhow::Result<()> {
    let pattern = el
        .get_pattern::<UIScrollPattern>()
        .context("element does not support UIScrollPattern")?;

    let h = pixel_delta_to_amount(dx);
    let v = pixel_delta_to_amount(dy);
    pattern
        .scroll(h, v)
        .context("UIScrollPattern::scroll failed")?;
    tracing::debug!(dx, dy, ?h, ?v, "scrolled element via UIA");
    Ok(())
}

fn pixel_delta_to_amount(delta: i32) -> ScrollAmount {
    // Threshold chosen to match the "one mouse-wheel notch ≈ 100px"
    // convention Windows uses for `WM_MOUSEWHEEL` (`WHEEL_DELTA == 120`).
    // Anything bigger than that warrants a page-sized scroll; anything
    // smaller is a line-sized nudge.
    const PAGE_THRESHOLD: i32 = 100;
    match delta {
        0 => ScrollAmount::NoAmount,
        d if d >= PAGE_THRESHOLD => ScrollAmount::LargeIncrement,
        d if d <= -PAGE_THRESHOLD => ScrollAmount::LargeDecrement,
        d if d > 0 => ScrollAmount::SmallIncrement,
        _ => ScrollAmount::SmallDecrement,
    }
}

impl WindowsBackend {
    fn walk(
        &mut self,
        walker: &UITreeWalker,
        el: &UIElement,
        depth: usize,
        out: &mut Vec<Element>,
        stats: &mut WalkStats,
    ) {
        stats.visited += 1;
        if depth > stats.max_depth {
            stats.max_depth = depth;
        }
        let (max_elements, max_depth) = if self.is_current_browser {
            (MAX_ELEMENTS_BROWSER, MAX_TREE_DEPTH_BROWSER)
        } else {
            (MAX_ELEMENTS_DESKTOP, MAX_TREE_DEPTH_DESKTOP)
        };
        if out.len() >= max_elements {
            stats.hit_element_cap = true;
            return;
        }
        if let Some(record) = self.try_record(el) {
            out.push(record);
        }
        if depth >= max_depth {
            stats.hit_depth_cap = true;
            return;
        }
        if let Ok(first) = walker.get_first_child(el) {
            let mut current = first;
            loop {
                self.walk(walker, &current, depth + 1, out, stats);
                match walker.get_next_sibling(&current) {
                    Ok(next) => current = next,
                    Err(_) => break,
                }
            }
        }
    }

    /// Decide whether to record `el`, branching on whether the foreground
    /// window is a browser. Returns `Some` for elements the user should be
    /// able to hint, `None` otherwise.
    fn try_record(&mut self, el: &UIElement) -> Option<Element> {
        // Common preconditions: invisible / zero-sized / disabled elements
        // are never useful to hint, regardless of which path runs.
        if el.is_offscreen().unwrap_or(true) {
            return None;
        }
        let bounds = el.get_bounding_rectangle().ok()?;
        if bounds.get_width() <= 0 || bounds.get_height() <= 0 {
            return None;
        }
        // `is_enabled` is best-effort: some controls don't implement the
        // property and return Err. Treat those as enabled — better to
        // include a stale-state badge than silently drop a valid target.
        if !el.is_enabled().unwrap_or(true) {
            return None;
        }

        let ct = el.get_control_type().ok()?;
        if self.is_current_browser {
            self.try_record_web_element(el, ct)
        } else {
            self.try_record_desktop_element(el, ct)
        }
    }

    /// Desktop-app detection. Matches the legacy v0.2.0 behaviour for
    /// known-interactable ControlTypes, then falls back to a pattern
    /// check so custom controls (Pane/Group with InvokePattern, common
    /// in modern Win32 / WinUI / Electron apps) get picked up too.
    fn try_record_desktop_element(&mut self, el: &UIElement, ct: ControlType) -> Option<Element> {
        if let Some(role) = map_role(ct) {
            return self.create_element(el, role);
        }
        // Pattern fallback: anything that exposes an action pattern is
        // worth showing, even if its ControlType is "structural". We
        // gate on a small minimum size so we don't spam badges over
        // every 1-pixel separator that happens to expose Toggle.
        if has_any_action_pattern(el) {
            let bounds = el.get_bounding_rectangle().ok()?;
            if bounds.get_width() >= 10 && bounds.get_height() >= 10 {
                return self.create_element(el, Role::Other);
            }
        }
        None
    }

    /// Web-page / browser-content detection. Stricter than the desktop
    /// path because the UIA tree mirrors the entire DOM and includes
    /// piles of layout containers and decorative nodes.
    ///
    /// Detection cascade:
    ///
    /// 1. **High-confidence ControlTypes** (`Button`, `Hyperlink`,
    ///    `Edit`, …) win immediately — these are unambiguous.
    /// 2. **Structural types** (`Pane`, `Group`, `Image`, `Custom`,
    ///    `Text`, `Document`) are only kept when they *also* expose an
    ///    action pattern, an ARIA-mapped MSAA role, or
    ///    `IsKeyboardFocusable=true`. This is the path that catches
    ///    `<div role="button">`, `<a>` tags Chromium classifies as
    ///    `Custom`, and click-handler-only divs that are still
    ///    keyboard-tabbable (the modern accessibility floor).
    /// 3. **Anything else** — last-chance ARIA / focus heuristic so
    ///    the picker doesn't drop a target just because Chromium
    ///    picked an exotic ControlType for it.
    ///
    /// Size filtering is split: known-interactive elements only need
    /// to clear [`MIN_WEB_FOCUSABLE_SIZE`] (10px), so we still pick up
    /// small icon buttons and chip "×" controls. Everything else has to
    /// clear the stricter [`MIN_WEB_ELEMENT_SIZE`] (16px) — that's
    /// where the spacer-element noise lives.
    fn try_record_web_element(&mut self, el: &UIElement, ct: ControlType) -> Option<Element> {
        use ControlType::*;

        let bounds = el.get_bounding_rectangle().ok()?;
        let w = bounds.get_width();
        let h = bounds.get_height();
        // Hard floor: anything smaller than `MIN_WEB_FOCUSABLE_SIZE` is
        // almost certainly noise even for "interactive" elements. This
        // catches accessibility nodes Chromium materialises with a
        // 1×1 / 4×4 hit-box for offscreen content.
        if w < MIN_WEB_FOCUSABLE_SIZE || h < MIN_WEB_FOCUSABLE_SIZE {
            return None;
        }
        let big_enough_for_anything = w >= MIN_WEB_ELEMENT_SIZE && h >= MIN_WEB_ELEMENT_SIZE;

        // Phase 1: high-confidence ControlTypes always win — even at
        // the smaller 10px size, since something the browser explicitly
        // labelled as e.g. a Button is clearly clickable.
        let role = match ct {
            Button | SplitButton => Some(Role::Button),
            Hyperlink => Some(Role::Link),
            Edit => Some(Role::TextInput),
            CheckBox => Some(Role::Checkbox),
            RadioButton => Some(Role::Radio),
            ComboBox => Some(Role::ComboBox),
            MenuItem => Some(Role::MenuItem),
            TabItem => Some(Role::Tab),
            ListItem | TreeItem | DataItem => Some(Role::ListItem),

            // Phase 2: ambiguous / structural types need *some* signal
            // that they're interactive before we count them.
            Image | Custom | Pane | Group | Text | Document => {
                let interactive = has_any_action_pattern(el)
                    || looks_clickable_web(el)
                    || el.is_keyboard_focusable().unwrap_or(false);
                if !interactive {
                    None
                } else if matches!(ct, Image) {
                    Some(Role::Button)
                } else {
                    Some(Role::Other)
                }
            }
            _ => None,
        };

        if let Some(role) = role {
            // Big_enough_for_anything is only enforced for Phase 2 noise:
            // a real `Button` ControlType clears the smaller floor by
            // design, even if the browser sized it down.
            let needs_big = matches!(ct, Image | Custom | Pane | Group | Text | Document);
            if needs_big && !big_enough_for_anything {
                return None;
            }
            return self.create_element(el, role);
        }

        // Phase 3: last-chance heuristic for ControlTypes we didn't
        // match above. Catches things like ARIA `role="button"` exposed
        // only via the legacy IAccessible bridge, or pure-JS clickable
        // elements that show up under exotic ControlTypes but are
        // tabbable through keyboard navigation.
        if big_enough_for_anything
            && (looks_clickable_web(el) || el.is_keyboard_focusable().unwrap_or(false))
        {
            return self.create_element(el, Role::Button);
        }

        None
    }

    /// Build the public [`Element`] for a recorded UIA element, assign it
    /// a stable ID, and register it for later [`Backend::perform`] lookups.
    /// Centralising this means the desktop and web paths can't drift apart
    /// on what an Element actually contains.
    fn create_element(&mut self, el: &UIElement, role: Role) -> Option<Element> {
        let bounds = el.get_bounding_rectangle().ok()?;
        let name = el.get_name().ok().filter(|s| !s.is_empty());

        let id = ElementId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.elements.insert(id, el.clone());

        Some(Element {
            id,
            role,
            name,
            bounds: Bounds {
                x: bounds.get_left(),
                y: bounds.get_top(),
                width: bounds.get_width(),
                height: bounds.get_height(),
            },
        })
    }
}

/// True when `el` advertises any of the patterns we know how to invoke.
/// Used by the desktop pattern-fallback path to catch custom controls
/// whose ControlType doesn't tell us they're interactable.
fn has_any_action_pattern(el: &UIElement) -> bool {
    el.get_pattern::<UIInvokePattern>().is_ok()
        || el.get_pattern::<UITogglePattern>().is_ok()
        || el.get_pattern::<UISelectionItemPattern>().is_ok()
        || el.get_pattern::<UIExpandCollapsePattern>().is_ok()
}

/// Heuristic for "this looks clickable even though no UIA pattern says so".
///
/// Web pages with ARIA `role="button"` / `role="link"` typically end up
/// exposing the role through the legacy IAccessible bridge but don't
/// publish an InvokePattern. Without this check those elements would be
/// invisible to the picker even though they're the entire reason a user
/// hits the hotkey on a web page.
///
/// We also pick up the broader interactive-but-stateful set (tabs,
/// tree items, grid cells, dropdown buttons, comboboxes) that
/// frequently show up only via the legacy bridge in modern web apps
/// (Gmail's row toggles, GitHub's file tree, dashboard grids, …).
fn looks_clickable_web(el: &UIElement) -> bool {
    let Ok(p) = el.get_pattern::<UILegacyIAccessiblePattern>() else {
        return false;
    };
    let Ok(role) = p.get_role() else {
        return false;
    };
    matches!(
        role as i32,
        MSAA_ROLE_LINK
            | MSAA_ROLE_PUSHBUTTON
            | MSAA_ROLE_CHECKBUTTON
            | MSAA_ROLE_RADIOBUTTON
            | MSAA_ROLE_MENU_ITEM
            | MSAA_ROLE_PAGE_TAB
            | MSAA_ROLE_OUTLINE_ITEM
            | MSAA_ROLE_LIST_ITEM
            | MSAA_ROLE_CELL
            | MSAA_ROLE_COMBOBOX
            | MSAA_ROLE_BUTTON_DROPDOWN
            | MSAA_ROLE_BUTTON_MENU
    )
}

fn map_role(ct: ControlType) -> Option<Role> {
    use ControlType::*;
    Some(match ct {
        // Canonical interactive controls — these always count.
        Button | SplitButton => Role::Button,
        Hyperlink => Role::Link,
        Edit => Role::TextInput,
        MenuItem => Role::MenuItem,
        TabItem => Role::Tab,
        CheckBox => Role::Checkbox,
        RadioButton => Role::Radio,
        ComboBox => Role::ComboBox,
        ListItem => Role::ListItem,
        TreeItem => Role::TreeItem,

        // Often-clickable controls in modern Win32 / WinUI / Electron
        // apps. Including them by ControlType (rather than via the
        // pattern fallback) means we catch them even when their app
        // forgets to publish InvokePattern.
        DataItem => Role::ListItem,
        Image => Role::Button,

        // Structural / non-interactable controls (Pane, Group, Text,
        // Document, Window, TitleBar, …) are skipped here on purpose —
        // they're handled by the pattern fallback in `try_record_desktop_element`,
        // which only records them when they actually expose an action.
        _ => return None,
    })
}

/// True iff `hwnd` belongs to a process whose executable name looks
/// like a known browser. Cheap (one process-handle open + one image-name
/// query) and runs once per overlay invocation.
///
/// We match on substrings of the lower-cased exe filename. Misses:
///  - Browsers shipped under exotic names (Yandex, Vivaldi, etc.) — easy
///    to add when reported.
///  - Electron apps that *embed* Chromium (VS Code, Discord, …) — these
///    are explicitly *not* browsers for our purposes; their chrome is
///    native and the web-strict filter would over-prune their controls.
fn is_browser_window(hwnd: HWND) -> bool {
    let mut process_id: u32 = 0;
    // SAFETY: `GetWindowThreadProcessId` writes through `&mut process_id`
    // and tolerates a null `hwnd` (returns 0). We guarantee `process_id`
    // is a valid stack location for the duration of the call.
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id as *mut u32));
    }
    if process_id == 0 {
        return false;
    }

    let exe_path = unsafe {
        let h_process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let mut buf = vec![0u16; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            h_process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(h_process);
        if ok.is_err() || size == 0 {
            return false;
        }
        String::from_utf16_lossy(&buf[..size as usize]).to_lowercase()
    };

    // Match on the basename only — full paths add noise (e.g. `chrome` is
    // legitimately part of `\chrome\` directories for non-browser apps).
    let basename = exe_path.rsplit(['\\', '/']).next().unwrap_or(&exe_path);
    matches!(
        basename,
        "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "brave.exe"
            | "opera.exe"
            | "vivaldi.exe"
            | "iexplore.exe"
            | "arc.exe"
    )
}

/// MSAA object id for the UI Automation root, returned by Chromium's
/// `WM_GETOBJECT` handler. Defined as `-25` in `UIAutomationCore.h`.
const UIA_ROOT_OBJECT_ID: i32 = -25;
/// MSAA object id for the standard `IAccessible` root. Documented at
/// <https://learn.microsoft.com/en-us/windows/win32/winauto/object-identifiers>.
const OBJID_CLIENT: i32 = -4;
/// Wait at most this long for a single `WM_GETOBJECT` round trip when
/// activating browser accessibility. Browser UI threads can occasionally
/// stall (loading a giant page, devtools open, …); a short timeout means
/// our hotkey path stays snappy even when the browser doesn't reply.
const ACCESSIBILITY_ACTIVATION_TIMEOUT_MS: u32 = 50;

/// Nudge a Chromium-based browser into building its full per-tab
/// accessibility tree.
///
/// Chrome / Edge / Brave / Opera / Vivaldi / Arc all share Chromium's
/// `BrowserAccessibilityState`, which only enables the renderer-side
/// accessibility tree on demand. Without this, `EnumWindows`-style
/// walks see the browser chrome but the per-tab `Document` element
/// is a single placeholder with no children — exactly the symptom the
/// user reported (badges on tabs / address bar but never on the page).
///
/// The activation channel is `WM_GETOBJECT`. Sending it with
/// [`UIA_ROOT_OBJECT_ID`] (`-25`) tells Chromium "a UIA client is about
/// to query you", which sets the right accessibility mode flags.
/// We send to every descendant HWND, not just the top-level browser
/// window, because each tab's renderer lives in its own
/// `Chrome_RenderWidgetHostHWND` child window.
///
/// We also send [`OBJID_CLIENT`] as a belt-and-braces signal — Firefox
/// and IE classic respond to that path, while modern Chromium prefers
/// the UIA root id. The message is fire-and-forget; we ignore the
/// returned IAccessible pointer.
fn activate_browser_accessibility(hwnd: HWND) {
    unsafe {
        send_accessibility_probe(hwnd);
        // Walk the child HWNDs too — each renderer process surfaces
        // the page document via its own child window.
        let _ = EnumChildWindows(hwnd, Some(activation_enum_proc), LPARAM(0));
    }
}

unsafe extern "system" fn activation_enum_proc(
    hwnd: HWND,
    _lparam: LPARAM,
) -> windows::Win32::Foundation::BOOL {
    send_accessibility_probe(hwnd);
    windows::Win32::Foundation::BOOL(1)
}

/// Send the two `WM_GETOBJECT` flavors browsers care about, with a
/// short timeout so a wedged target window can't stall the picker.
unsafe fn send_accessibility_probe(hwnd: HWND) {
    for object_id in [UIA_ROOT_OBJECT_ID, OBJID_CLIENT] {
        let _ = SendMessageTimeoutW(
            hwnd,
            WM_GETOBJECT,
            WPARAM(0),
            LPARAM(object_id as isize),
            SMTO_ABORTIFHUNG,
            ACCESSIBILITY_ACTIVATION_TIMEOUT_MS,
            None,
        );
    }
}
