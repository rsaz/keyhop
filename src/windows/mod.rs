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
pub mod prewarm;
pub mod settings_window;
pub mod single_instance;
pub mod splash_screen;
pub mod startup;
pub mod tray;
pub mod window_picker;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use parking_lot::Mutex;

use uiautomation::{
    core::UICacheRequest,
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UILegacyIAccessiblePattern, UIPatternType,
        UIScrollPattern, UISelectionItemPattern, UITogglePattern,
    },
    types::{ControlType, Handle, ScrollAmount, UIProperty},
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

/// Bookkeeping for one [`walk_window`] invocation. Surfaced via
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

/// One element captured by [`walk_window`] before any backend-side
/// bookkeeping runs. Holds everything the public `Element` needs,
/// plus the live `UIElement` so the foreground backend can register
/// it for [`Backend::perform`] later.
///
/// IDs aren't assigned at walk time: parallel pid-group walks (Phase
/// 5) emit records on multiple threads, and we need a consistent
/// monotonic counter across the merged result. The merge step that
/// runs back on the main thread takes care of numbering.
pub(crate) struct WalkRecord {
    pub role: Role,
    pub name: Option<String>,
    pub bounds: Bounds,
    pub element: UIElement,
}

/// Send-shim for [`WalkRecord`] so it can ship across rayon worker
/// boundaries.
///
/// # Safety
///
/// `UIElement` wraps an MTA-only COM interface; the `windows`/`uiautomation`
/// crates conservatively leave it `!Send`. We uphold the COM rule by
/// guaranteeing every thread that ever touches one of these records —
/// the rayon worker that constructs it and the main thread that later
/// invokes patterns on it — has CoInitialized as MTA via
/// `UIAutomation::new()`. Both paths satisfy that: the per-pid worker
/// in `enumerate_windows_filtered` builds its own `UIAutomation` (which
/// registers MTA on the worker thread), and the main thread does the
/// same when [`WindowsBackend::with_shared_cache`] runs at startup.
struct SendWalkRecord(WalkRecord);
// SAFETY: see type-level comment above.
unsafe impl Send for SendWalkRecord {}

/// UI Automation-backed implementation of [`Backend`] for Windows.
pub struct WindowsBackend {
    automation: UIAutomation,
    elements: HashMap<ElementId, UIElement>,
    next_id: u64,
    /// Per-HWND enumeration cache. Backs the "press Esc, retry"
    /// flow — same window, same elements, no UIA round-trip. Disabled
    /// by setting `enable_caching = false` in `config.toml`.
    ///
    /// Phase 4 of the perf plan introduces a pre-warm worker thread
    /// (`crate::windows::prewarm`) that owns its own
    /// [`WindowsBackend`] but writes into the *same* `CacheManager`
    /// as the synchronous hotkey path. The shared `Arc<Mutex<…>>`
    /// makes that hand-off lock-free for readers in the common case
    /// (`parking_lot::Mutex` is uncontended ~99 % of the time) and
    /// keeps the cache's existing API intact — callers just go
    /// through `self.cache.lock()` instead of touching it directly.
    cache: Arc<Mutex<CacheManager>>,
    /// Hard cap on elements per enumeration in non-active-window scope
    /// modes. The single-window walk caps are
    /// `MAX_ELEMENTS_DESKTOP`/`MAX_ELEMENTS_BROWSER`; this kicks in
    /// only when we're aggregating across multiple windows.
    max_elements_global: usize,
    /// Pre-built UIA cache request that prefetches every property and
    /// pattern the walker reads on each node. With this attached to
    /// `element_from_handle_build_cache` and the `*_build_cache`
    /// walker methods, every `get_cached_*` call inside `try_record*`
    /// resolves from in-process memory instead of a cross-process COM
    /// round-trip — the dominant cost of a UIA tree walk on real apps
    /// (each remote get is a few-hundred-microsecond IPC). Built once
    /// in [`Self::with_config`] and reused for every walk.
    cache_request: UICacheRequest,
    /// Pre-built UIA condition that pre-filters the desktop walk on
    /// the server side: "controls of an interactable type or any
    /// element advertising an action pattern, that is not currently
    /// scrolled offscreen". Phase 2 of the perf plan replaces the
    /// recursive control-view walker (one IPC per `get_first_child` /
    /// `get_next_sibling`, N hops total) with a single
    /// `find_all_build_cache(Subtree, …)` call against this condition,
    /// collapsing an O(N)-IPC walk into one IPC for desktop apps.
    /// Browser windows still go through the recursive walker because
    /// their DOM trees over-match every generic control-type predicate.
    desktop_condition: uiautomation::core::UICondition,
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
        let cache = Arc::new(Mutex::new(CacheManager::new(cache_ttl_ms, enable_caching)));
        Self::with_shared_cache(cache, max_elements_global)
    }

    /// Construct a backend whose `CacheManager` is shared with another
    /// `WindowsBackend` (typically the pre-warm worker — see
    /// [`crate::windows::prewarm`]). Both backends own their own
    /// `UIAutomation` client (UIA is per-thread / per-apartment) but
    /// hits one populates the other's cache for free.
    pub fn with_shared_cache(
        cache: Arc<Mutex<CacheManager>>,
        max_elements_global: usize,
    ) -> anyhow::Result<Self> {
        // PerMonitorV2 ensures `GetBoundingRectangle` returns physical
        // pixels matching the overlay's coordinate space on high-DPI
        // displays. Best-effort: ignore "already set" / older-OS errors.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let automation = UIAutomation::new().context("initializing UI Automation client")?;
        let cache_request = build_cache_request(&automation)
            .context("building UIA cache request for prefetched properties/patterns")?;
        let desktop_condition = build_desktop_condition(&automation)
            .context("building UIA desktop pre-filter condition for FindAll(Subtree, …)")?;
        Ok(Self {
            automation,
            elements: HashMap::new(),
            next_id: 0,
            cache,
            max_elements_global,
            cache_request,
            desktop_condition,
        })
    }

    /// Hand out a clone of the shared cache handle so the pre-warm
    /// worker can write into the same `CacheManager` the hotkey path
    /// reads from. Cheap (atomic refcount bump on the `Arc`).
    pub fn cache_handle(&self) -> Arc<Mutex<CacheManager>> {
        Arc::clone(&self.cache)
    }

    /// Mirror new cache settings from [`crate::Config`] into the live
    /// backend without restarting. Disabling caching also clears the
    /// existing entries.
    pub fn reconfigure_cache(&mut self, enable_caching: bool, cache_ttl_ms: u64) {
        self.cache.lock().reconfigure(cache_ttl_ms, enable_caching);
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
        // Phase 5: tightened prune. The previous "any pixel overlaps"
        // check pulled in windows that just barely touched the cursor
        // monitor (e.g. a sliver of an off-screen Slack window),
        // wasting a UIA walk per spurious match. Requiring 10% area
        // overlap drops those without losing any window the user
        // would actually consider "on this monitor".
        self.enumerate_windows_filtered(|bounds| {
            rect_overlap_fraction(&monitor_rect, bounds) >= 0.10
        })
    }

    fn enumerate_all_windows(&mut self) -> anyhow::Result<Vec<Element>> {
        self.enumerate_windows_filtered(|_| true)
    }

    /// Shared implementation behind the multi-window scope modes.
    ///
    /// Phase 5: parallelize the per-window walks across rayon worker
    /// threads, grouped by process id. Most of the time in this
    /// function is spent waiting on COM IPC against remote app
    /// processes, which is embarrassingly parallel — the only thing
    /// we *can't* do is hammer two HWNDs of the same app
    /// simultaneously (they share one UI thread on the target side),
    /// so the pid grouping serializes per-app work onto a single
    /// worker. The foreground window's pid group is walked locally
    /// to keep its UIElements bound to this backend's apartment for
    /// `perform()` (rayon workers register theirs on their own
    /// thread, which is fine for invocation thanks to MTA marshalling
    /// but risks subtle ordering bugs we don't need to court).
    ///
    /// Errors from a single window's walk are swallowed with a
    /// `tracing` warning — better to return what we did manage to
    /// enumerate than abort the whole pick because one tab's UIA
    /// tree was unhealthy.
    fn enumerate_windows_filtered<F>(&mut self, mut accept: F) -> anyhow::Result<Vec<Element>>
    where
        F: FnMut(&Bounds) -> bool,
    {
        use std::collections::BTreeMap;

        let candidates = crate::windows::window_picker::enumerate_visible()
            .context("listing top-level windows for multi-window scope")?;
        let cap = self.max_elements_global;
        let foreground = unsafe { GetForegroundWindow() };

        // Filter + group by pid. BTreeMap so iteration order is stable
        // across runs (helpful for test reproducibility and for
        // attributing trace logs to specific apps). The foreground pid
        // gets walked first; everything else fans out onto rayon.
        let mut by_pid: BTreeMap<u32, Vec<crate::windows::window_picker::TopLevelWindow>> =
            BTreeMap::new();
        let mut foreground_pid: Option<u32> = None;
        for win in candidates {
            if !accept(&win.bounds) {
                continue;
            }
            let pid = window_pid(win.hwnd);
            if win.hwnd == foreground {
                foreground_pid = Some(pid);
            }
            by_pid.entry(pid).or_default().push(win);
        }

        // Walk the foreground pid group locally (sequentially through
        // `enumerate_window`, which goes through the cache and binds
        // UIElements into `self.elements`). This guarantees the
        // hotkey-driven backend can invoke any element on the active
        // window via `perform()` even if a parallel worker had
        // pre-walked the same HWND.
        let mut out: Vec<Element> = Vec::with_capacity(64);
        if let Some(pid) = foreground_pid {
            if let Some(group) = by_pid.remove(&pid) {
                let mut group = group;
                group.sort_by_key(|w| if w.hwnd == foreground { 0 } else { 1 });
                for win in group {
                    if out.len() >= cap {
                        break;
                    }
                    match self.enumerate_window(win.hwnd) {
                        Ok(elements) => {
                            let remaining = cap - out.len();
                            out.extend(elements.into_iter().take(remaining));
                        }
                        Err(e) => {
                            tracing::warn!(?win.hwnd, title = %win.title, error = ?e,
                                "foreground-pid window walk failed; skipping");
                        }
                    }
                }
            }
        }

        // Remaining pid groups fan out across rayon workers. Each
        // worker constructs its own `UIAutomation` (the `with_config`
        // CoInitializeEx happens on the worker thread), then walks
        // every HWND in its group. Records come back wrapped in
        // [`SendWalkRecord`] (see safety note on that type).
        if !by_pid.is_empty() && out.len() < cap {
            use rayon::prelude::*;
            let groups: Vec<(u32, Vec<crate::windows::window_picker::TopLevelWindow>)> =
                by_pid.into_iter().collect();
            let walked: Vec<Vec<SendWalkRecord>> = groups
                .into_par_iter()
                .map(|(pid, windows)| {
                    let automation = match UIAutomation::new() {
                        Ok(a) => a,
                        Err(e) => {
                            tracing::warn!(pid, error = ?e,
                                "rayon worker: UIAutomation init failed");
                            return Vec::new();
                        }
                    };
                    let cache_request = match build_cache_request(&automation) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(pid, error = ?e,
                                "rayon worker: build_cache_request failed");
                            return Vec::new();
                        }
                    };
                    let desktop_condition = match build_desktop_condition(&automation) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(pid, error = ?e,
                                "rayon worker: build_desktop_condition failed");
                            return Vec::new();
                        }
                    };
                    let mut local: Vec<SendWalkRecord> = Vec::with_capacity(64);
                    for win in windows {
                        let is_browser = is_browser_window(win.hwnd);
                        match walk_window(
                            &automation,
                            &cache_request,
                            &desktop_condition,
                            win.hwnd,
                            is_browser,
                        ) {
                            Ok(records) => {
                                local.extend(records.into_iter().map(SendWalkRecord));
                            }
                            Err(e) => {
                                tracing::warn!(?win.hwnd, title = %win.title, error = ?e,
                                    "parallel window walk failed; skipping");
                            }
                        }
                    }
                    local
                })
                .collect();

            // Merge in pid-group order so logging stays
            // deterministic. `register_records` mints the global
            // `ElementId`s and stuffs UIElements into `self.elements`
            // so `perform()` works on every returned element.
            for group in walked {
                if out.len() >= cap {
                    break;
                }
                let remaining = cap - out.len();
                let take: Vec<WalkRecord> =
                    group.into_iter().map(|w| w.0).take(remaining).collect();
                let elements = self.register_records(take);
                out.extend(elements);
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
    /// multi-window scope modes and by the foreground path.
    pub fn enumerate_window(&mut self, hwnd: HWND) -> anyhow::Result<Vec<Element>> {
        let key = hwnd_to_key(hwnd);
        if let Some(cached) = self.cache.lock().get(key) {
            let all_known = cached.iter().all(|e| self.elements.contains_key(&e.id));
            if all_known {
                return Ok(cached.as_ref().to_vec());
            }
            // Cache hit but the IDs were minted by a different
            // backend (the pre-warm worker, or a different per-pid
            // worker). Fall through to a fresh walk so
            // `self.elements` has UIElement handles bound to *this*
            // backend for `perform()` to use.
        }

        let is_browser = is_browser_window(hwnd);
        let records = walk_window(
            &self.automation,
            &self.cache_request,
            &self.desktop_condition,
            hwnd,
            is_browser,
        )?;
        let elements = self.register_records(records);

        let shared: Arc<[Element]> = Arc::from(elements.clone().into_boxed_slice());
        self.cache.lock().insert(key, shared);
        Ok(elements)
    }

    /// Drain a batch of [`WalkRecord`]s into `self.elements`,
    /// minting fresh IDs and producing the public [`Element`] list
    /// in the same order. Centralized so the foreground-,
    /// single-window-, and multi-window-merge paths can't drift.
    fn register_records(&mut self, records: Vec<WalkRecord>) -> Vec<Element> {
        let mut out = Vec::with_capacity(records.len());
        for rec in records {
            let id = ElementId(self.next_id);
            self.next_id = self.next_id.wrapping_add(1);
            self.elements.insert(id, rec.element);
            out.push(Element {
                id,
                role: rec.role,
                name: rec.name,
                bounds: rec.bounds,
            });
        }
        out
    }
}

/// Build the per-process UIA cache request once, attaching every property
/// and pattern the walker / `try_record*` paths consult. This is the spine
/// of Phase 1 of the perf plan: by sending the entire shopping list to UIA
/// up front, every subsequent `get_cached_*` call reads from in-process
/// memory instead of round-tripping to the target app's UI thread.
///
/// Anything fetched by the walker must be added here — the cached getter
/// returns an error otherwise. We deliberately *over-cache* a couple of
/// rarely-used properties (LocalizedControlType, AutomationId) so future
/// debug logging can pick them up without forcing a re-roll of the
/// request.
fn build_cache_request(automation: &UIAutomation) -> uiautomation::Result<UICacheRequest> {
    let req = automation.create_cache_request()?;

    // Properties read by `try_record`, `try_record_desktop_element`,
    // `try_record_web_element`, and `create_element`.
    for prop in [
        UIProperty::ControlType,
        UIProperty::BoundingRectangle,
        UIProperty::IsOffscreen,
        UIProperty::IsEnabled,
        UIProperty::IsKeyboardFocusable,
        UIProperty::IsControlElement,
        UIProperty::Name,
        UIProperty::AutomationId,
        UIProperty::LocalizedControlType,
        UIProperty::ClassName,
    ] {
        req.add_property(prop)?;
    }

    // Patterns probed by `has_any_action_pattern` / `looks_clickable_web`.
    // `Scroll` is here so that the future `Action::Scroll` path can stay
    // on cached pattern handles too. `LegacyIAccessible` is needed because
    // `looks_clickable_web` reads its `get_cached_role()`.
    for pat in [
        UIPatternType::Invoke,
        UIPatternType::Toggle,
        UIPatternType::SelectionItem,
        UIPatternType::ExpandCollapse,
        UIPatternType::LegacyIAccessible,
        UIPatternType::Scroll,
    ] {
        req.add_pattern(pat)?;
    }

    Ok(req)
}

/// Build the desktop pre-filter once per backend lifetime. Phase 2 of the
/// perf plan replaces the recursive control-view tree walk with a single
/// `find_all_build_cache(TreeScope::Subtree, …)` call against this
/// condition; UIA evaluates it on the target app's UI thread and ships
/// back only the elements that match (already cached per
/// [`build_cache_request`]).
///
/// Conceptually the condition is:
///
/// ```text
/// And(
///   IsControlElement = true,
///   Or(
///     ControlType in { Button, SplitButton, Hyperlink, Edit, MenuItem,
///                       TabItem, CheckBox, RadioButton, ComboBox,
///                       ListItem, TreeItem, DataItem, Image },
///     IsInvokePatternAvailable = true,
///     IsTogglePatternAvailable = true,
///     IsSelectionItemPatternAvailable = true,
///     IsExpandCollapsePatternAvailable = true,
///   ),
///   Not(IsOffscreen = true),
/// )
/// ```
///
/// The `uiautomation` crate exposes the boolean combinators in binary
/// form, so the implementation reduces lists with a small `fold` helper.
fn build_desktop_condition(
    automation: &UIAutomation,
) -> uiautomation::Result<uiautomation::core::UICondition> {
    use uiautomation::core::UICondition;
    use uiautomation::types::PropertyConditionFlags;
    use uiautomation::variants::Variant;

    fn or_all(
        automation: &UIAutomation,
        mut conds: Vec<UICondition>,
    ) -> uiautomation::Result<UICondition> {
        let mut acc = conds.remove(0);
        for c in conds {
            acc = automation.create_or_condition(acc, c)?;
        }
        Ok(acc)
    }

    let prop = |property: UIProperty, value: Variant| -> uiautomation::Result<UICondition> {
        automation.create_property_condition(property, value, Some(PropertyConditionFlags::None))
    };

    // ControlTypes worth hinting on a desktop app. Mirrors the
    // accept-list inside `map_role` plus `Image` (which the desktop
    // path also accepts via the structural fallback). UIA stores
    // ControlType as the `UIA_*ControlTypeId` integer, so we pass the
    // enum's `i32` discriminant.
    let ct_ids: [ControlType; 13] = [
        ControlType::Button,
        ControlType::SplitButton,
        ControlType::Hyperlink,
        ControlType::Edit,
        ControlType::MenuItem,
        ControlType::TabItem,
        ControlType::CheckBox,
        ControlType::RadioButton,
        ControlType::ComboBox,
        ControlType::ListItem,
        ControlType::TreeItem,
        ControlType::DataItem,
        ControlType::Image,
    ];
    let mut control_type_conds: Vec<UICondition> = Vec::with_capacity(ct_ids.len());
    for ct in ct_ids {
        control_type_conds.push(prop(UIProperty::ControlType, Variant::from(ct as i32))?);
    }
    let by_control_type = or_all(automation, control_type_conds)?;

    // Pattern-availability fallback: catches custom controls whose
    // ControlType is structural (Pane / Group / Custom) but that still
    // expose an action pattern. Stays in lock-step with
    // `has_any_action_pattern`.
    let pattern_conds = vec![
        prop(UIProperty::IsInvokePatternAvailable, Variant::from(true))?,
        prop(UIProperty::IsTogglePatternAvailable, Variant::from(true))?,
        prop(
            UIProperty::IsSelectionItemPatternAvailable,
            Variant::from(true),
        )?,
        prop(
            UIProperty::IsExpandCollapsePatternAvailable,
            Variant::from(true),
        )?,
    ];
    let by_pattern = or_all(automation, pattern_conds)?;

    let interactable = automation.create_or_condition(by_control_type, by_pattern)?;

    let is_control_element = prop(UIProperty::IsControlElement, Variant::from(true))?;

    let offscreen = prop(UIProperty::IsOffscreen, Variant::from(true))?;
    let on_screen = automation.create_not_condition(offscreen)?;

    let with_control_element =
        automation.create_and_condition(is_control_element, interactable)?;
    automation.create_and_condition(with_control_element, on_screen)
}

/// Read a cached boolean property as a `Result<bool>`. The `uiautomation`
/// crate doesn't expose a direct `get_cached_is_*` for arbitrary boolean
/// properties — only for a handful of named ones — so we go through the
/// generic `get_cached_property_value` path and convert via `Variant`.
/// Non-bool variants are surfaced as `Err`; callers fall back to the
/// safe default themselves (usually "treat as enabled / on-screen" so a
/// quirky control isn't dropped silently).
fn cached_bool(el: &UIElement, prop: UIProperty) -> uiautomation::Result<bool> {
    let v = el.get_cached_property_value(prop)?;
    v.try_into()
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

/// Fraction of the window's frame area that overlaps `monitor`, in
/// [0.0, 1.0]. Phase 5 replaces the old "any pixel overlaps" check
/// for `enumerate_active_monitor` so a window that's only just
/// touching the active monitor (off-screen Slack, half-snapped Edge)
/// is no longer worth a UIA walk. The all-windows scope mode skips
/// this check entirely — it wants *every* visible HWND.
fn rect_overlap_fraction(monitor: &RECT, bounds: &Bounds) -> f32 {
    let area = (bounds.width as i64) * (bounds.height as i64);
    if area <= 0 {
        return 0.0;
    }
    let r_right = bounds.x + bounds.width;
    let r_bottom = bounds.y + bounds.height;
    let ix1 = bounds.x.max(monitor.left);
    let iy1 = bounds.y.max(monitor.top);
    let ix2 = r_right.min(monitor.right);
    let iy2 = r_bottom.min(monitor.bottom);
    let iw = (ix2 - ix1).max(0) as i64;
    let ih = (iy2 - iy1).max(0) as i64;
    let intersection = iw * ih;
    intersection as f32 / area as f32
}

/// Look up the owning process id of `hwnd`. Returns `0` if the call
/// fails (typically because the HWND was destroyed between
/// enumeration and pid lookup); callers treat `0` as a synthetic
/// "unknown" group so those windows still get walked, just on their
/// own worker.
fn window_pid(hwnd: HWND) -> u32 {
    let mut pid: u32 = 0;
    // SAFETY: `GetWindowThreadProcessId` writes through `&mut pid`
    // and tolerates a null hwnd (returns 0).
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
    }
    pid
}

impl Backend for WindowsBackend {
    fn enumerate_foreground(&mut self) -> anyhow::Result<Vec<Element>> {
        // SAFETY: `GetForegroundWindow` has no preconditions and may return a
        // null handle, which we check below.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            tracing::warn!("no foreground window");
            return Ok(Vec::new());
        }

        let is_browser = is_browser_window(hwnd);
        tracing::debug!(is_browser, "enumerate_foreground: detected window kind");

        // Cache lookup before any UIA work. A hit means we were called
        // again on the same HWND inside the TTL — either the
        // "user pressed Esc, retrying" path or, with Phase 4 in
        // place, the pre-warm worker beating the user to the walk
        // after a foreground change. We only honor a hit when this
        // backend already owns UIElement handles for every cached ID;
        // otherwise [`Self::perform`] would fail with "unknown element
        // id" when the user picks.
        let key = hwnd_to_key(hwnd);
        if let Some(cached) = self.cache.lock().get(key) {
            let all_known = cached.iter().all(|e| self.elements.contains_key(&e.id));
            if all_known {
                tracing::debug!(
                    cached = cached.len(),
                    "enumerate_foreground served from cache"
                );
                return Ok(cached.as_ref().to_vec());
            }
            tracing::debug!(
                cached = cached.len(),
                "cache hit but IDs not registered locally; rewalking to bind UIElements"
            );
        }

        // Reset the local UIElement registry on a real miss; we hand
        // out fresh IDs starting from zero so old badges from the
        // previous walk can't be invoked by mistake.
        self.elements.clear();
        self.next_id = 0;

        let records = walk_window(
            &self.automation,
            &self.cache_request,
            &self.desktop_condition,
            hwnd,
            is_browser,
        )?;
        let out = self.register_records(records);

        // Refresh the cache with the just-collected elements so the
        // next call within `cache_ttl_ms` can short-circuit the walk
        // entirely. The cache holds an `Arc<[Element]>` so subsequent
        // cache hits are a single refcount bump even for a 2k-element
        // browser walk.
        let shared: Arc<[Element]> = Arc::from(out.clone().into_boxed_slice());
        self.cache.lock().insert(key, shared);

        // Per-role breakdown helps when a user reports "keyhop didn't see
        // button X in app Y" — the log shows whether the element was
        // dropped by the role filter or never made it through the bounds
        // / pattern checks at all.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let mut by_role: HashMap<Role, usize> = HashMap::new();
            for el in &out {
                *by_role.entry(el.role).or_insert(0) += 1;
            }
            tracing::debug!(collected = out.len(), ?by_role, "enumerate_foreground done");
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

/// Phase-5 free walker. Walks one HWND with the supplied UIA client
/// state and returns the captured [`WalkRecord`]s. Stateless w.r.t.
/// `WindowsBackend` so this can run on a rayon worker that owns its
/// own `UIAutomation` (each thread is MTA-initialized when its
/// `UIAutomation` is constructed). The caller assigns `ElementId`s
/// and registers `UIElement`s in the backend after merging.
pub(crate) fn walk_window(
    automation: &UIAutomation,
    cache_request: &UICacheRequest,
    desktop_condition: &uiautomation::core::UICondition,
    hwnd: HWND,
    is_browser: bool,
) -> anyhow::Result<Vec<WalkRecord>> {
    if is_browser {
        activate_browser_accessibility(hwnd);
    }
    let handle = Handle::from(hwnd.0 as isize);
    let root = automation
        .element_from_handle_build_cache(handle, cache_request)
        .context("element_from_handle_build_cache failed")?;

    let mut out = Vec::with_capacity(64);
    let mut stats = WalkStats::default();
    if is_browser {
        let walker = automation
            .get_control_view_walker()
            .context("creating control-view tree walker failed")?;
        walk_recurse(
            &walker,
            cache_request,
            &root,
            0,
            true,
            &mut out,
            &mut stats,
        );
    } else {
        find_all_desktop(
            automation,
            cache_request,
            desktop_condition,
            &root,
            &mut out,
            &mut stats,
        );
    }
    if tracing::enabled!(tracing::Level::DEBUG) {
        tracing::debug!(
            collected = out.len(),
            visited = stats.visited,
            max_depth = stats.max_depth,
            hit_depth_cap = stats.hit_depth_cap,
            hit_element_cap = stats.hit_element_cap,
            ?hwnd,
            is_browser,
            "walk_window done"
        );
    }
    Ok(out)
}

/// Recursive control-view walker. Used for browser windows where
/// `FindAll(Subtree, …)` would over-match on DOM nodes.
fn walk_recurse(
    walker: &UITreeWalker,
    cache_request: &UICacheRequest,
    el: &UIElement,
    depth: usize,
    is_browser: bool,
    out: &mut Vec<WalkRecord>,
    stats: &mut WalkStats,
) {
    stats.visited += 1;
    if depth > stats.max_depth {
        stats.max_depth = depth;
    }
    let (max_elements, max_depth) = if is_browser {
        (MAX_ELEMENTS_BROWSER, MAX_TREE_DEPTH_BROWSER)
    } else {
        (MAX_ELEMENTS_DESKTOP, MAX_TREE_DEPTH_DESKTOP)
    };
    if out.len() >= max_elements {
        stats.hit_element_cap = true;
        return;
    }
    if let Some(record) = try_record(el, is_browser) {
        out.push(record);
    }
    if depth >= max_depth {
        stats.hit_depth_cap = true;
        return;
    }
    // `*_build_cache` walker variants prefetch the configured
    // properties/patterns on the returned child/sibling in the same
    // IPC that produces the new node. Without this, every
    // `get_cached_*` inside `try_record` would fail because the new
    // element wasn't in the cache scope of the original
    // `element_from_handle_build_cache` call (cache scope is
    // per-element, not transitive across walker hops).
    if let Ok(first) = walker.get_first_child_build_cache(el, cache_request) {
        let mut current = first;
        loop {
            walk_recurse(
                walker,
                cache_request,
                &current,
                depth + 1,
                is_browser,
                out,
                stats,
            );
            match walker.get_next_sibling_build_cache(&current, cache_request) {
                Ok(next) => current = next,
                Err(_) => break,
            }
        }
    }
}

/// Phase-2 desktop walker: hand the whole subtree filter to UIA via
/// `find_all_build_cache(TreeScope::Subtree, …)` and let the target
/// process do the filtering on its own UI thread.
fn find_all_desktop(
    automation: &UIAutomation,
    cache_request: &UICacheRequest,
    desktop_condition: &uiautomation::core::UICondition,
    root: &UIElement,
    out: &mut Vec<WalkRecord>,
    stats: &mut WalkStats,
) {
    match root.find_all_build_cache(
        uiautomation::types::TreeScope::Subtree,
        desktop_condition,
        cache_request,
    ) {
        Ok(elements) => {
            stats.visited = elements.len();
            for el in elements.iter() {
                if out.len() >= MAX_ELEMENTS_DESKTOP {
                    stats.hit_element_cap = true;
                    break;
                }
                if let Some(record) = try_record_desktop_after_filter(el) {
                    out.push(record);
                }
            }
        }
        Err(e) => {
            // `FindAll` over a busy subtree can time out, or fail
            // when an Electron renderer reattaches its accessibility
            // tree mid-walk. Falling back keeps the picker functional.
            tracing::warn!(error = ?e,
                "find_all_build_cache failed on desktop subtree; falling back to recursive walk");
            if let Ok(walker) = automation.get_control_view_walker() {
                walk_recurse(&walker, cache_request, root, 0, false, out, stats);
            }
        }
    }
}

/// Slim variant of [`try_record`] used after the server-side pre-filter
/// has already discarded offscreen / non-control elements.
fn try_record_desktop_after_filter(el: &UIElement) -> Option<WalkRecord> {
    let bounds = el.get_cached_bounding_rectangle().ok()?;
    if bounds.get_width() <= 0 || bounds.get_height() <= 0 {
        return None;
    }
    if !cached_bool(el, UIProperty::IsEnabled).unwrap_or(true) {
        return None;
    }
    let ct = el.get_cached_control_type().ok()?;
    try_record_desktop_element(el, ct)
}

/// Decide whether to record `el`, branching on browser vs desktop.
fn try_record(el: &UIElement, is_browser: bool) -> Option<WalkRecord> {
    if cached_bool(el, UIProperty::IsOffscreen).unwrap_or(false) {
        return None;
    }
    let bounds = el.get_cached_bounding_rectangle().ok()?;
    if bounds.get_width() <= 0 || bounds.get_height() <= 0 {
        return None;
    }
    if !cached_bool(el, UIProperty::IsEnabled).unwrap_or(true) {
        return None;
    }
    let ct = el.get_cached_control_type().ok()?;
    if is_browser {
        try_record_web_element(el, ct)
    } else {
        try_record_desktop_element(el, ct)
    }
}

/// Desktop-app detection. Matches v0.2.0 behaviour for known
/// interactable ControlTypes, plus a pattern fallback for custom
/// controls (Pane / Group / Custom that nonetheless expose an action
/// pattern).
fn try_record_desktop_element(el: &UIElement, ct: ControlType) -> Option<WalkRecord> {
    if let Some(role) = map_role(ct) {
        return create_walk_record(el, role);
    }
    if has_any_action_pattern(el) {
        let bounds = el.get_cached_bounding_rectangle().ok()?;
        if bounds.get_width() >= 10 && bounds.get_height() >= 10 {
            return create_walk_record(el, Role::Other);
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
fn try_record_web_element(el: &UIElement, ct: ControlType) -> Option<WalkRecord> {
    use ControlType::*;

        let bounds = el.get_cached_bounding_rectangle().ok()?;
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
                    || cached_bool(el, UIProperty::IsKeyboardFocusable).unwrap_or(false);
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
        return create_walk_record(el, role);
    }

    // Phase 3: last-chance heuristic for ControlTypes we didn't
    // match above. Catches things like ARIA `role="button"` exposed
    // only via the legacy IAccessible bridge, or pure-JS clickable
    // elements that show up under exotic ControlTypes but are
    // tabbable through keyboard navigation.
    if big_enough_for_anything
        && (looks_clickable_web(el)
            || cached_bool(el, UIProperty::IsKeyboardFocusable).unwrap_or(false))
    {
        return create_walk_record(el, Role::Button);
    }

    None
}

/// Build a [`WalkRecord`] for a recorded UIA element. ID assignment
/// and registration in [`WindowsBackend::elements`] happen later, on
/// the merging thread; this function only does in-process reads
/// (cached bounding rect + name) so it is safe to call from any
/// MTA-initialized worker.
fn create_walk_record(el: &UIElement, role: Role) -> Option<WalkRecord> {
    let bounds = el.get_cached_bounding_rectangle().ok()?;
    let name = el.get_cached_name().ok().filter(|s| !s.is_empty());
    Some(WalkRecord {
        role,
        name,
        bounds: Bounds {
            x: bounds.get_left(),
            y: bounds.get_top(),
            width: bounds.get_width(),
            height: bounds.get_height(),
        },
        element: el.clone(),
    })
}

/// True when `el` advertises any of the patterns we know how to invoke.
/// Used by the desktop pattern-fallback path to catch custom controls
/// whose ControlType doesn't tell us they're interactable.
fn has_any_action_pattern(el: &UIElement) -> bool {
    // `get_cached_pattern` is the in-process counterpart of `get_pattern`:
    // it succeeds iff the cache request enabled the pattern *and* the
    // remote element actually advertised it. The four patterns here must
    // stay in lock-step with the `add_pattern` calls in
    // `build_cache_request` — otherwise this function silently returns
    // `false` for everything and the desktop pattern-fallback path goes
    // dark.
    el.get_cached_pattern::<UIInvokePattern>().is_ok()
        || el.get_cached_pattern::<UITogglePattern>().is_ok()
        || el.get_cached_pattern::<UISelectionItemPattern>().is_ok()
        || el.get_cached_pattern::<UIExpandCollapsePattern>().is_ok()
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
    let Ok(p) = el.get_cached_pattern::<UILegacyIAccessiblePattern>() else {
        return false;
    };
    let Ok(role) = p.get_cached_role() else {
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
