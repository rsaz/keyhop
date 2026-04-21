//! Windows "launch at login" integration via the Run registry key.
//!
//! We use `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` rather than
//! the system-wide `HKLM` variant so that:
//!
//! - No admin elevation is needed (per-user install pattern).
//! - Uninstalling/removing the binary won't leave orphaned `HKLM` entries.
//! - The setting follows the user across machines if they roam profiles.
//!
//! The value name is hardcoded to `keyhop` so we don't have to guess at
//! enable/disable time. The value data is the absolute path to the
//! currently-running executable, captured via [`std::env::current_exe`].

use anyhow::{Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "keyhop";

/// Whether keyhop is currently registered to launch with Windows. Returns
/// `Ok(false)` when the value is missing (the common state); `Err` only on
/// genuine registry failures (corruption, ACL surprises) since users may
/// poll this on UI open and we don't want a benign "missing" to look like
/// a real problem.
pub fn is_enabled() -> Result<bool> {
    let mut hkey = HKEY::default();
    let subkey = to_wide(RUN_KEY);
    let value_name = to_wide(VALUE_NAME);

    unsafe {
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if r.is_err() {
            return Err(anyhow::anyhow!(
                "RegOpenKeyExW failed: 0x{:08X}",
                r.0 as u32
            ));
        }

        let r = RegQueryValueExW(hkey, PCWSTR(value_name.as_ptr()), None, None, None, None);
        let _ = RegCloseKey(hkey);

        if r == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else if r.is_ok() {
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                "RegQueryValueExW failed: 0x{:08X}",
                r.0 as u32
            ))
        }
    }
}

/// Enable or disable Windows startup integration for the current user.
///
/// On enable: writes `HKCU\...\Run\keyhop = "<absolute exe path>"`.
/// On disable: deletes the value (idempotent — removing a missing value
/// is treated as success so the Settings window can call this
/// unconditionally on save).
pub fn set_enabled(enable: bool) -> Result<()> {
    if enable {
        enable_startup()
    } else {
        disable_startup()
    }
}

fn enable_startup() -> Result<()> {
    let exe = std::env::current_exe().context("getting current_exe path")?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("exe path is not valid UTF-8: {exe:?}"))?;

    let subkey = to_wide(RUN_KEY);
    let value_name = to_wide(VALUE_NAME);
    let mut value_data = to_wide(exe_str);

    let mut hkey = HKEY::default();

    unsafe {
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if r.is_err() {
            anyhow::bail!("RegOpenKeyExW failed: 0x{:08X}", r.0 as u32);
        }

        // Length in bytes including the trailing NUL.
        let byte_len = value_data.len() * std::mem::size_of::<u16>();
        let bytes = std::slice::from_raw_parts_mut(value_data.as_mut_ptr() as *mut u8, byte_len);
        let r = RegSetValueExW(hkey, PCWSTR(value_name.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);

        if r.is_err() {
            anyhow::bail!("RegSetValueExW failed: 0x{:08X}", r.0 as u32);
        }
    }

    tracing::info!(path = %exe_str, "registered for Windows startup");
    Ok(())
}

fn disable_startup() -> Result<()> {
    let subkey = to_wide(RUN_KEY);
    let value_name = to_wide(VALUE_NAME);

    let mut hkey = HKEY::default();

    unsafe {
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if r.is_err() {
            anyhow::bail!("RegOpenKeyExW failed: 0x{:08X}", r.0 as u32);
        }

        let r = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(hkey);

        if r == ERROR_FILE_NOT_FOUND {
            // Nothing to remove — that's fine.
            tracing::debug!("startup key already absent");
            return Ok(());
        }
        if r.is_err() {
            anyhow::bail!("RegDeleteValueW failed: 0x{:08X}", r.0 as u32);
        }
    }

    tracing::info!("removed Windows startup registration");
    Ok(())
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
