//! Thin wrapper around the [`global_hotkey`] crate that registers keyhop's
//! leader chord and exposes a non-blocking poller for `Pressed` events.
//!
//! The underlying [`GlobalHotKeyManager`] uses a hidden Win32 message-only
//! window to receive `WM_HOTKEY`. The owning thread must therefore run a
//! Win32 message loop (`GetMessageW` / `DispatchMessageW`) for events to be
//! delivered. The `keyhop` binary takes care of that.

use anyhow::{Context, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};

/// Owns a registered global hotkey for keyhop's leader. Drop releases the
/// registration with the OS.
pub struct LeaderHotkey {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

impl LeaderHotkey {
    /// Register keyhop's default leader chord: `Ctrl + Shift + Space`.
    /// Chosen because it is rarely globally bound, has three modifiers (low
    /// accidental-trigger risk), and is free of AltGr conflicts on European
    /// keyboard layouts.
    pub fn register_default() -> Result<Self> {
        Self::register(Modifiers::CONTROL | Modifiers::SHIFT, Code::Space)
    }

    /// Register an arbitrary modifier + key combination.
    pub fn register(modifiers: Modifiers, code: Code) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("creating GlobalHotKeyManager failed")?;
        let hotkey = HotKey::new(Some(modifiers), code);
        manager
            .register(hotkey)
            .context("registering global hotkey failed")?;
        tracing::info!(
            ?modifiers,
            ?code,
            id = hotkey.id(),
            "leader hotkey registered"
        );
        Ok(Self { manager, hotkey })
    }

    /// The registered hotkey's id, useful for filtering events when multiple
    /// hotkeys are registered.
    pub fn id(&self) -> u32 {
        self.hotkey.id()
    }

    /// Drain the global hotkey channel and return `true` if our leader was
    /// pressed since the last poll. Released events are ignored.
    pub fn poll_pressed(&self) -> bool {
        let receiver = GlobalHotKeyEvent::receiver();
        let mut pressed = false;
        while let Ok(event) = receiver.try_recv() {
            if event.id == self.hotkey.id() && event.state == HotKeyState::Pressed {
                pressed = true;
            }
        }
        pressed
    }
}

impl Drop for LeaderHotkey {
    fn drop(&mut self) {
        if let Err(e) = self.manager.unregister(self.hotkey) {
            tracing::warn!(error = %e, "failed to unregister leader hotkey");
        }
    }
}
