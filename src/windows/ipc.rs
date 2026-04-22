//! Tiny inter-process channel between the running `keyhop` instance and
//! a freshly-launched `keyhop --close` invocation.
//!
//! Why bother:
//!
//! - The single-instance mutex (see [`crate::windows::single_instance`])
//!   stops a *second* keyhop from booting, but gives the user no way to
//!   tell the *first* one to stop. Without this module the only options
//!   are the tray menu or Task Manager, both of which break the
//!   "everything from a terminal" workflow keyhop is built around.
//!
//! - We can't just `taskkill /F` from the closer process: that skips
//!   `Drop` on the instance guard and on the global hotkeys, leaving
//!   the OS-level hotkey registrations stuck until the next sign-out.
//!
//! Mechanism: the running instance creates a hidden top-level window
//! with a known class name. The closer process calls
//! [`send_close_signal`], which uses [`FindWindowW`] to locate that
//! window and posts `WM_CLOSE` to it. The window proc translates
//! `WM_CLOSE` into [`PostQuitMessage`], which the main message loop
//! turns into a clean shutdown.
//!
//! Hidden / `WS_POPUP` (rather than message-only `HWND_MESSAGE`)
//! because top-level windows are findable with the much simpler
//! [`FindWindowW`]; message-only windows would force the closer side
//! to enumerate `HWND_MESSAGE` children.

use anyhow::{Context, Result};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, FindWindowW, PostMessageW, PostQuitMessage, RegisterClassExW,
    CW_USEDEFAULT, HWND_DESKTOP, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WNDCLASSEXW, WS_OVERLAPPED,
};

/// Class name for the hidden IPC window. Must match between the running
/// instance (which registers it) and the closer process (which queries
/// it via [`FindWindowW`]). Keep it specific enough that no other app
/// could plausibly pick the same string.
pub const IPC_CLASS: PCWSTR = w!("KeyhopIpcWindowClass_v1");
/// Window title for the hidden IPC window. Doesn't have to match across
/// processes (we look up by class), but a recognisable name helps when
/// inspecting the process tree with Spy++ or similar.
pub const IPC_TITLE: PCWSTR = w!("Keyhop IPC");

/// Create the hidden IPC window in the *running* instance. Call this
/// once during startup, after the message loop's owning thread has been
/// chosen — the window must live on the same thread that pumps
/// `GetMessageW`, otherwise `WM_CLOSE` will be dispatched to a thread
/// that doesn't exist.
///
/// Returns the `HWND` so the caller can keep it alive for the lifetime
/// of the message loop. Dropping the returned [`IpcWindow`] destroys
/// the window and unregisters the class, freeing the slot for the next
/// keyhop launch.
pub fn create() -> Result<IpcWindow> {
    unsafe {
        let h_instance = GetModuleHandleW(None).context("GetModuleHandleW for IPC window")?;

        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(h_instance.0),
            lpszClassName: IPC_CLASS,
            ..Default::default()
        };
        // Re-registering the class on a relaunch is fine — RegisterClassExW
        // returns 0 with ERROR_CLASS_ALREADY_EXISTS, which we treat as
        // success. The class lives until the process exits.
        let _ = RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            IPC_CLASS,
            IPC_TITLE,
            // WS_OVERLAPPED + never calling ShowWindow keeps the window
            // off the taskbar and out of Alt-Tab while still being a
            // top-level window FindWindowW can locate.
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            HWND_DESKTOP,
            None,
            HINSTANCE(h_instance.0),
            None,
        )
        .context("CreateWindowExW for IPC window")?;

        tracing::debug!(?hwnd, "IPC window created");
        Ok(IpcWindow { hwnd })
    }
}

/// RAII handle to the hidden IPC window. Drop destroys the window so a
/// freshly-relaunched keyhop can re-create it without colliding.
pub struct IpcWindow {
    hwnd: HWND,
}

impl Drop for IpcWindow {
    fn drop(&mut self) {
        unsafe {
            // Best-effort: if the window was already destroyed (e.g. by
            // PostQuitMessage indirectly tearing things down), the call
            // will fail and we don't care.
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // The closer process posted this — translate it into a clean
            // shutdown of the message loop. PostQuitMessage queues
            // WM_QUIT, which the main `GetMessageW` loop in `main.rs`
            // detects and breaks on.
            tracing::info!("IPC: WM_CLOSE received — shutting down");
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Look up a running keyhop instance and ask it to shut down.
///
/// Returns `true` if a running instance was found and signalled,
/// `false` if no instance window exists. Errors propagate from the
/// underlying Win32 calls.
///
/// Used by the `--close` CLI flag: a freshly-launched keyhop calls
/// this and exits before initialising any of the heavier subsystems
/// (tray, hotkeys, UI Automation).
pub fn send_close_signal() -> Result<bool> {
    unsafe {
        let hwnd = FindWindowW(IPC_CLASS, IPC_TITLE);
        let Ok(hwnd) = hwnd else {
            return Ok(false);
        };
        if hwnd.0.is_null() {
            return Ok(false);
        }
        // PostMessage (not SendMessage) so we don't block on the running
        // instance's UI thread — particularly useful if it's mid-overlay.
        PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0))
            .context("PostMessageW(WM_CLOSE) to running keyhop instance")?;
        Ok(true)
    }
}
