//! Per-user "only one keyhop running" guard.
//!
//! Two `keyhop` instances on the same desktop would race for the same
//! global hotkeys (whichever registers first wins; the second silently
//! "registers" nothing) and stack two tray icons in the notification area.
//! Both are user-visible footguns. We avoid them with a named mutex.
//!
//! The mutex name is *not* prefixed with `Global\`, so it lives in the
//! caller's session namespace — exactly the scope we want for a per-user
//! productivity tool. (Using `Global\` would require
//! `SeCreateGlobalPrivilege`, which standard accounts don't have.)

use anyhow::{Context, Result};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// RAII handle to the single-instance mutex. Drop releases it back to the
/// OS, freeing the slot for the next `keyhop` launch.
pub struct InstanceGuard {
    handle: HANDLE,
}

impl InstanceGuard {
    /// Try to acquire the lock. Returns `Ok(Some(_))` if this is the first
    /// running instance, `Ok(None)` if another instance already holds it,
    /// and `Err` only on real OS failures.
    pub fn acquire() -> Result<Option<Self>> {
        // SAFETY: passing default security, requesting initial ownership,
        // and a static UTF-16 name. CreateMutexW returns a non-null
        // handle on success; failure is reported via the Result.
        let handle = unsafe {
            CreateMutexW(None, true, w!("KeyhopSingleInstanceMutex"))
                .context("CreateMutexW for single-instance guard")?
        };
        // GetLastError must be checked *immediately* after CreateMutexW —
        // a non-null handle plus ERROR_ALREADY_EXISTS means we attached
        // to an existing mutex rather than creating a fresh one.
        let already_existed = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
        if already_existed {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(None);
        }
        Ok(Some(Self { handle }))
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `handle` came from a successful CreateMutexW and is only
        // closed once (here). Drop runs at most once per value.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
