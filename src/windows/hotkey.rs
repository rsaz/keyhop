//! Multi-binding registry on top of the [`global_hotkey`] crate.
//!
//! [`Hotkeys`] owns a single [`GlobalHotKeyManager`] and a small map from
//! the manager-assigned numeric id to a logical [`HotkeyAction`]. The owner
//! polls [`Hotkeys::poll_actions`] inside its Win32 message loop and gets
//! back a list of actions triggered since the last poll.
//!
//! The underlying [`GlobalHotKeyManager`] uses a hidden Win32 message-only
//! window to receive `WM_HOTKEY`. The owning thread must therefore run a
//! Win32 message loop (`GetMessageW` / `DispatchMessageW`) for events to be
//! delivered. The `keyhop` binary takes care of that.

use std::collections::HashMap;

use anyhow::{Context, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};

/// What a registered chord should *do*. Decoupled from the specific
/// modifier+key combination so we can rebind defaults later (or load from
/// config) without touching dispatch code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    /// Show the hint overlay for elements inside the current foreground
    /// window. Default: `Ctrl + Shift + Space`.
    PickElement,
    /// Show the hint overlay for every visible top-level window across all
    /// monitors. Default: `Ctrl + Alt + Space`.
    PickWindow,
}

/// Registered hotkeys. Drop releases all OS registrations.
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    bindings: HashMap<u32, HotkeyAction>,
    /// Held so we can unregister on drop.
    registered: Vec<HotKey>,
}

impl Hotkeys {
    /// Register keyhop's two default chords:
    ///
    /// - `Ctrl + Shift + Space` → [`HotkeyAction::PickElement`]
    /// - `Ctrl + Alt + Space`   → [`HotkeyAction::PickWindow`]
    ///
    /// Chosen because they're rarely globally bound, share a memorable
    /// "Ctrl + modifier + Space" pattern, and avoid the famous
    /// `Ctrl+Shift+W` collision with browsers' "close window".
    pub fn register_defaults() -> Result<Self> {
        let mut me = Self::new()?;
        me.register(
            Modifiers::CONTROL | Modifiers::SHIFT,
            Code::Space,
            HotkeyAction::PickElement,
        )?;
        me.register(
            Modifiers::CONTROL | Modifiers::ALT,
            Code::Space,
            HotkeyAction::PickWindow,
        )?;
        Ok(me)
    }

    /// Construct an empty registry. Use [`Self::register`] to add bindings.
    pub fn new() -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("creating GlobalHotKeyManager failed")?;
        Ok(Self {
            manager,
            bindings: HashMap::new(),
            registered: Vec::new(),
        })
    }

    /// Add a single chord → action binding.
    pub fn register(
        &mut self,
        modifiers: Modifiers,
        code: Code,
        action: HotkeyAction,
    ) -> Result<()> {
        let hotkey = HotKey::new(Some(modifiers), code);
        self.manager
            .register(hotkey)
            .with_context(|| format!("registering hotkey for {action:?}"))?;
        tracing::info!(
            ?modifiers,
            ?code,
            ?action,
            id = hotkey.id(),
            "hotkey registered"
        );
        self.bindings.insert(hotkey.id(), action);
        self.registered.push(hotkey);
        Ok(())
    }

    /// Drain the global hotkey channel and return every action that fired
    /// (in arrival order). Released events are ignored — keyhop only cares
    /// about the press edge.
    pub fn poll_actions(&self) -> Vec<HotkeyAction> {
        let receiver = GlobalHotKeyEvent::receiver();
        let mut out = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if let Some(&action) = self.bindings.get(&event.id) {
                out.push(action);
            }
        }
        out
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        for hk in self.registered.drain(..) {
            if let Err(e) = self.manager.unregister(hk) {
                tracing::warn!(error = %e, "failed to unregister hotkey");
            }
        }
    }
}
