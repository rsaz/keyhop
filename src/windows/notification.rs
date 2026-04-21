//! Lightweight user-facing notifications.
//!
//! Since the binary runs as a hidden GUI-subsystem process in release
//! builds (no console, no main window), we need a way to surface
//! recoverable problems and confirmations to the user. The simplest
//! always-works mechanism on Windows is the modal `MessageBoxW`:
//!
//! - Works in every session including locked-down enterprise builds.
//! - Respects high-DPI scaling automatically.
//! - Gives the user an unambiguous acknowledge step (so we know they
//!   actually saw it before we move on).
//!
//! Tray "balloons" / toasts would be less intrusive but require either
//! a custom Win32 `Shell_NotifyIconW` integration (the `tray-icon` crate
//! we use doesn't expose this) or a winrt toast pipeline (large extra
//! dependency for v0.2.0). MessageBox is good enough for the handful of
//! events we need to surface.
//!
//! Every notification also goes through `tracing` so users with the log
//! file open can see context the dialog box doesn't have room for.

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK,
};

/// Severity of a user notification. Maps to the appropriate `MessageBoxW`
/// icon style.
#[derive(Debug, Clone, Copy)]
pub enum Level {
    /// Confirmation / progress ("Settings saved").
    Info,
    /// Recoverable issue ("Hotkey conflict — using defaults").
    Warning,
    /// Hard failure ("Failed to write config").
    Error,
}

/// Show a blocking modal message box. Logs at the matching level so
/// headless callers (logs being read after the fact) get the full picture.
pub fn show(title: &str, body: &str, level: Level) {
    match level {
        Level::Info => tracing::info!(%title, %body, "user notification"),
        Level::Warning => tracing::warn!(%title, %body, "user notification"),
        Level::Error => tracing::error!(%title, %body, "user notification"),
    }

    let style = match level {
        Level::Info => MB_OK | MB_ICONINFORMATION,
        Level::Warning => MB_OK | MB_ICONWARNING,
        Level::Error => MB_OK | MB_ICONERROR,
    };
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let body_w: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(body_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            style,
        );
    }
}

/// Convenience for `show(_, _, Level::Info)`.
pub fn info(title: &str, body: &str) {
    show(title, body, Level::Info);
}

/// Convenience for `show(_, _, Level::Warning)`.
pub fn warn(title: &str, body: &str) {
    show(title, body, Level::Warning);
}

/// Convenience for `show(_, _, Level::Error)`.
pub fn error(title: &str, body: &str) {
    show(title, body, Level::Error);
}
