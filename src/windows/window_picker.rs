//! Pick any visible top-level window across all monitors using a
//! Vimium-style hint overlay.
//!
//! The flow is:
//!
//! 1. [`enumerate_visible`] walks every top-level HWND with [`EnumWindows`]
//!    and filters out the noise (cloaked UWP shells, tool windows, the
//!    desktop, zero-size or off-screen windows).
//! 2. [`pick`] generates short labels via [`crate::HintEngine`], shows the
//!    overlay with [`crate::windows::overlay::pick_hint`], and on success
//!    returns the chosen window's HWND.
//! 3. [`focus`] brings the chosen window to the foreground.
//!
//! The picker is intentionally agnostic to *what* you do with the chosen
//! window — `main.rs` calls [`focus`] after [`pick`], but a future
//! "navigate inside this window" mode could chain into the element picker
//! instead.

use anyhow::{Context, Result};

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GA_ROOTOWNER, GWL_EXSTYLE,
    SW_RESTORE, WS_EX_TOOLWINDOW,
};

use crate::windows::overlay::{pick_hint, Hint, HintStyle};
use crate::{Bounds, HintEngine};

/// Metadata about a candidate top-level window.
#[derive(Debug, Clone)]
pub struct TopLevelWindow {
    /// Native window handle.
    pub hwnd: HWND,
    /// Title bar text. May be empty for some apps; we filter those out
    /// during enumeration so callers never see them.
    pub title: String,
    /// Visible frame in screen coordinates. We use DWM's "extended frame
    /// bounds" when available, which excludes the invisible 7px shadow
    /// border `GetWindowRect` reports on Win10+.
    pub bounds: Bounds,
}

// SAFETY: HWND is just a wrapper around a raw pointer used as an opaque
// handle. We never deref it; we hand it back to Win32 APIs. Sending it
// across threads is fine in practice.
unsafe impl Send for TopLevelWindow {}

/// Enumerate every visible, user-pickable top-level window.
///
/// Filters applied (in order, cheapest first):
///
/// - `IsWindowVisible` must be true.
/// - Window must not be cloaked (`DWMWA_CLOAKED == 0`). This skips the
///   ghost windows UWP/Win10+ leaves behind for closed Store apps.
/// - Must be its own root owner (`GetAncestor(GA_ROOTOWNER) == hwnd`).
///   Filters out child popups / tooltips that EnumWindows still surfaces.
/// - `WS_EX_TOOLWINDOW` must not be set. Filters trayed utilities and
///   floating tool palettes that don't show in Alt-Tab either.
/// - Class name must not be the desktop shell (`Progman`, `WorkerW`).
/// - Title must be non-empty (a window with no title is rarely something
///   the user means to pick).
/// - Frame must have positive area.
pub fn enumerate_visible() -> Result<Vec<TopLevelWindow>> {
    let mut out: Vec<TopLevelWindow> = Vec::with_capacity(32);
    let out_ptr: *mut Vec<TopLevelWindow> = &mut out;

    // SAFETY: We pass our Vec via LPARAM; the callback's lifetime is
    // bounded by EnumWindows itself, which is synchronous.
    unsafe {
        EnumWindows(Some(enum_proc), LPARAM(out_ptr as isize)).context("EnumWindows failed")?;
    }

    tracing::debug!(count = out.len(), "enumerate_visible");
    Ok(out)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<TopLevelWindow>);

    if let Some(w) = describe(hwnd) {
        out.push(w);
    }
    // Always continue enumeration, even on errors for individual windows.
    BOOL(1)
}

unsafe fn describe(hwnd: HWND) -> Option<TopLevelWindow> {
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }

    // Skip cloaked windows (closed UWP apps leave these around).
    let mut cloaked: u32 = 0;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut _ as *mut _,
        std::mem::size_of::<u32>() as u32,
    );
    if cloaked != 0 {
        return None;
    }

    // Skip child popups / tool tips that EnumWindows still walks.
    if GetAncestor(hwnd, GA_ROOTOWNER) != hwnd {
        return None;
    }

    // Skip tool windows (tray utilities, floating palettes).
    let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return None;
    }

    // Skip the desktop shell and its worker windows.
    let class = window_class_name(hwnd);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
        return None;
    }

    let title = window_title(hwnd);
    if title.is_empty() {
        return None;
    }

    let bounds = visible_frame(hwnd)?;
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }

    Some(TopLevelWindow {
        hwnd,
        title,
        bounds,
    })
}

unsafe fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut buf);
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

unsafe fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buf);
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Prefer DWM's extended frame bounds over `GetWindowRect`. The latter
/// includes the invisible drop-shadow margin Win10+ adds around top-level
/// windows (~7px on each side), which would offset our hint badge into
/// empty space.
unsafe fn visible_frame(hwnd: HWND) -> Option<Bounds> {
    let mut rect = RECT::default();
    let dwm = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut rect as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
    );
    if dwm.is_err() {
        // Fall back to plain GetWindowRect on older Windows or on errors.
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return None;
        }
    }
    Some(Bounds {
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    })
}

/// Show the window picker overlay and return the chosen window's HWND, or
/// `None` if the user cancelled / no candidates exist. Uses the default
/// hint engine and window style — call [`pick_with_style`] when the
/// caller needs to inject a config-driven style.
///
/// The labels are placed at the top-left of each window's visible frame
/// (i.e. anchored to the title bar), which is predictable and rarely
/// hidden by the window's own content.
pub fn pick(windows: Vec<TopLevelWindow>) -> Result<Option<TopLevelWindow>> {
    pick_with(windows, &HintEngine::default(), HintStyle::windows())
}

/// Variant of [`pick`] that uses a caller-provided [`HintStyle`]. The
/// hint alphabet still comes from the default engine — Settings only
/// exposes alphabet via the element picker side, but both pickers share
/// it so behaviour stays consistent.
pub fn pick_with_style(
    windows: Vec<TopLevelWindow>,
    style: HintStyle,
) -> Result<Option<TopLevelWindow>> {
    pick_with(windows, &HintEngine::default(), style)
}

/// Fully-parameterised picker. Used internally by [`pick`] and
/// [`pick_with_style`]; exposed in case future callers need to override
/// both the engine and the style at the same time.
pub fn pick_with(
    windows: Vec<TopLevelWindow>,
    engine: &HintEngine,
    style: HintStyle,
) -> Result<Option<TopLevelWindow>> {
    if windows.is_empty() {
        return Ok(None);
    }

    let labels = engine.generate(windows.len());
    let hints: Vec<Hint> = windows
        .iter()
        .zip(labels.iter())
        .map(|(w, l)| Hint {
            bounds: w.bounds,
            label: l.clone(),
            // Title pill makes window picker readable when two maximized
            // windows on the same monitor would otherwise share an anchor.
            extra: Some(w.title.clone()),
        })
        .collect();

    match pick_hint(hints, style)? {
        Some(idx) => Ok(Some(windows.into_iter().nth(idx).unwrap())),
        None => Ok(None),
    }
}

/// Bring the given window to the foreground, restoring it first if it was
/// minimized.
///
/// Note: Windows is restrictive about which process may steal foreground
/// focus. Because we call this immediately after our overlay was the
/// foreground window (and the overlay belongs to *our* process), the
/// SetForegroundWindow call is normally allowed.
pub fn focus(hwnd: HWND) -> Result<()> {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            // ShowWindow returns BOOL = previous-visible state, not an
            // error code; we don't care about the value.
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            tracing::warn!(?hwnd, "SetForegroundWindow returned false");
        }
    }
    Ok(())
}
