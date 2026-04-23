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
use std::fmt;

use anyhow::{Context, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};

use crate::config::HotkeyBindings;

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
    /// Open the settings dialog. Default: `Ctrl + Shift + ,`.
    OpenSettings,
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

    /// Register hotkeys from user config strings. Returns the registry plus
    /// a list of conflicts so the caller can surface them to the user
    /// without aborting startup — partial registration is better than no
    /// registration. Any chord that fails to parse or register is reported
    /// in [`RegistrationOutcome::conflicts`]; the corresponding action
    /// simply won't have a hotkey until the user fixes it in Settings.
    pub fn register_from_config(bindings: &HotkeyBindings) -> Result<RegistrationOutcome> {
        let mut me = Self::new()?;
        let mut conflicts = Vec::new();

        for (action, raw) in [
            (HotkeyAction::PickElement, &bindings.pick_element),
            (HotkeyAction::PickWindow, &bindings.pick_window),
            (HotkeyAction::OpenSettings, &bindings.open_settings),
        ] {
            match parse_hotkey(raw) {
                Ok((mods, code)) => {
                    if let Err(e) = me.register(mods, code, action) {
                        tracing::warn!(action = ?action, chord = %raw, error = ?e, "hotkey registration failed");
                        conflicts.push(HotkeyConflict {
                            action,
                            chord: raw.clone(),
                            reason: ConflictReason::AlreadyInUse,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(action = ?action, chord = %raw, error = ?e, "hotkey parse failed");
                    conflicts.push(HotkeyConflict {
                        action,
                        chord: raw.clone(),
                        reason: ConflictReason::ParseError(e.to_string()),
                    });
                }
            }
        }

        Ok(RegistrationOutcome {
            hotkeys: me,
            conflicts,
        })
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

impl fmt::Display for HotkeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyAction::PickElement => write!(f, "Pick element"),
            HotkeyAction::PickWindow => write!(f, "Pick window"),
            HotkeyAction::OpenSettings => write!(f, "Open settings"),
        }
    }
}

/// A hotkey that couldn't be registered. Surfaced to the user via tray
/// notification and the Settings window so they can pick a different
/// chord without digging through the log file.
#[derive(Debug, Clone)]
pub struct HotkeyConflict {
    /// Which logical action was affected.
    pub action: HotkeyAction,
    /// The raw chord string from config (e.g. `"Ctrl+Shift+Space"`).
    pub chord: String,
    /// Why registration failed.
    pub reason: ConflictReason,
}

/// Why a hotkey registration failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictReason {
    /// Some other process owns the chord (the common case — Discord,
    /// Slack, an IDE, an IME switcher, etc.).
    AlreadyInUse,
    /// The chord string in the config was malformed (typo,
    /// unsupported key name).
    ParseError(String),
}

impl fmt::Display for ConflictReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConflictReason::AlreadyInUse => write!(f, "already in use by another application"),
            ConflictReason::ParseError(e) => write!(f, "invalid chord ({e})"),
        }
    }
}

/// Result of [`Hotkeys::register_from_config`]: the (possibly partial)
/// registry plus everything that didn't make it.
pub struct RegistrationOutcome {
    /// The successfully-registered hotkeys.
    pub hotkeys: Hotkeys,
    /// Chords that failed parsing or OS registration.
    pub conflicts: Vec<HotkeyConflict>,
}

/// Parse a chord string like `"Ctrl+Shift+Space"` into modifiers + key.
///
/// Accepts:
/// - Modifiers: `Ctrl`/`Control`, `Shift`, `Alt`, `Win`/`Super`/`Meta`
///   (case-insensitive).
/// - Keys: `A`-`Z`, `0`-`9`, `F1`-`F24`, `Space`, `Enter`/`Return`,
///   `Tab`, `Esc`/`Escape`, `Backspace`, `Delete`, `Insert`, `Home`,
///   `End`, `PageUp`, `PageDown`, arrow keys (`Left`, `Right`, `Up`,
///   `Down`), `Comma`, `Period`, `Slash`, `Backslash`, `Semicolon`,
///   `Quote`, `BracketLeft`/`BracketRight`, `Minus`, `Equal`, `Backquote`.
/// - Literal punctuation aliases for the keys above: `,` `.` `/` `\` `;`
///   `'` `[` `]` `-` `=` `` ` `` — so users can write `Ctrl+\` or
///   `Ctrl+/` exactly the way it appears on their keyboard instead of
///   spelling out `Backslash` / `Slash`.
///
/// The trailing token is always the key; everything before is a modifier.
/// Whitespace and case are normalized. The literal `+` cannot be used as
/// the key (it's the segment separator) — use `Equal` or `=` to bind the
/// physical `+/=` key.
pub fn parse_hotkey(s: &str) -> Result<(Modifiers, Code)> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty hotkey string");
    }

    let parts: Vec<&str> = trimmed.split('+').map(|p| p.trim()).collect();
    if parts.iter().any(|p| p.is_empty()) {
        anyhow::bail!("malformed hotkey '{s}': empty segment");
    }

    let (key_str, mod_strs) = parts
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("missing key in '{s}'"))?;

    let mut modifiers = Modifiers::empty();
    for m in mod_strs {
        modifiers |=
            parse_modifier(m).with_context(|| format!("unrecognized modifier '{m}' in '{s}'"))?;
    }

    let code =
        parse_code(key_str).with_context(|| format!("unrecognized key '{key_str}' in '{s}'"))?;

    Ok((modifiers, code))
}

fn parse_modifier(s: &str) -> Result<Modifiers> {
    match s.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Ok(Modifiers::CONTROL),
        "shift" => Ok(Modifiers::SHIFT),
        "alt" => Ok(Modifiers::ALT),
        "win" | "super" | "meta" | "cmd" => Ok(Modifiers::META),
        other => anyhow::bail!("unknown modifier '{other}'"),
    }
}

fn parse_code(s: &str) -> Result<Code> {
    let upper = s.to_ascii_uppercase();
    let code = match upper.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "F13" => Code::F13,
        "F14" => Code::F14,
        "F15" => Code::F15,
        "F16" => Code::F16,
        "F17" => Code::F17,
        "F18" => Code::F18,
        "F19" => Code::F19,
        "F20" => Code::F20,
        "F21" => Code::F21,
        "F22" => Code::F22,
        "F23" => Code::F23,
        "F24" => Code::F24,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "TAB" => Code::Tab,
        "ESC" | "ESCAPE" => Code::Escape,
        "BACKSPACE" => Code::Backspace,
        "DELETE" | "DEL" => Code::Delete,
        "INSERT" | "INS" => Code::Insert,
        "HOME" => Code::Home,
        "END" => Code::End,
        "PAGEUP" | "PGUP" => Code::PageUp,
        "PAGEDOWN" | "PGDN" => Code::PageDown,
        "LEFT" => Code::ArrowLeft,
        "RIGHT" => Code::ArrowRight,
        "UP" => Code::ArrowUp,
        "DOWN" => Code::ArrowDown,
        "COMMA" | "," => Code::Comma,
        "PERIOD" | "DOT" | "." => Code::Period,
        "SLASH" | "/" => Code::Slash,
        "BACKSLASH" | "\\" => Code::Backslash,
        "SEMICOLON" | ";" => Code::Semicolon,
        "QUOTE" | "APOSTROPHE" | "'" => Code::Quote,
        "BRACKETLEFT" | "[" => Code::BracketLeft,
        "BRACKETRIGHT" | "]" => Code::BracketRight,
        "MINUS" | "-" => Code::Minus,
        "EQUAL" | "=" => Code::Equal,
        "BACKQUOTE" | "GRAVE" | "`" => Code::Backquote,
        other => anyhow::bail!("unknown key '{other}'"),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_pick_element() {
        let (m, c) = parse_hotkey("Ctrl+Shift+Space").unwrap();
        assert_eq!(m, Modifiers::CONTROL | Modifiers::SHIFT);
        assert_eq!(c, Code::Space);
    }

    #[test]
    fn parses_default_pick_window() {
        let (m, c) = parse_hotkey("Ctrl+Alt+Space").unwrap();
        assert_eq!(m, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(c, Code::Space);
    }

    #[test]
    fn case_insensitive() {
        let (m, c) = parse_hotkey("ctrl+shift+f1").unwrap();
        assert_eq!(m, Modifiers::CONTROL | Modifiers::SHIFT);
        assert_eq!(c, Code::F1);
    }

    #[test]
    fn ignores_whitespace() {
        let (m, c) = parse_hotkey("  Ctrl + Alt + K  ").unwrap();
        assert_eq!(m, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(c, Code::KeyK);
    }

    #[test]
    fn key_only_no_modifiers() {
        let (m, c) = parse_hotkey("F12").unwrap();
        assert!(m.is_empty());
        assert_eq!(c, Code::F12);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("   ").is_err());
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(parse_hotkey("Hyper+A").is_err());
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(parse_hotkey("Ctrl+NotAKey").is_err());
    }

    #[test]
    fn rejects_empty_segment() {
        assert!(parse_hotkey("Ctrl++Space").is_err());
    }

    #[test]
    fn alt_aliases() {
        let (m, _) = parse_hotkey("Win+A").unwrap();
        assert_eq!(m, Modifiers::META);
        let (m, _) = parse_hotkey("Super+A").unwrap();
        assert_eq!(m, Modifiers::META);
    }

    /// Regression for #3: `Ctrl+\` is on both sides of an ANSI keyboard
    /// and was rejected because the parser only knew the spelled-out
    /// `Backslash` form.
    #[test]
    fn parses_backslash_literal() {
        let (m, c) = parse_hotkey(r"Ctrl+\").unwrap();
        assert_eq!(m, Modifiers::CONTROL);
        assert_eq!(c, Code::Backslash);
    }

    #[test]
    fn parses_backslash_named_form_still_works() {
        let (m, c) = parse_hotkey("Ctrl+Backslash").unwrap();
        assert_eq!(m, Modifiers::CONTROL);
        assert_eq!(c, Code::Backslash);
    }

    #[test]
    fn parses_punctuation_literals() {
        // Every printable punctuation alias should resolve to the same
        // `Code` as its spelled-out twin.
        let pairs: &[(&str, &str, Code)] = &[
            ("Ctrl+,", "Ctrl+Comma", Code::Comma),
            ("Ctrl+.", "Ctrl+Period", Code::Period),
            ("Ctrl+/", "Ctrl+Slash", Code::Slash),
            (r"Ctrl+\", "Ctrl+Backslash", Code::Backslash),
            ("Ctrl+;", "Ctrl+Semicolon", Code::Semicolon),
            ("Ctrl+'", "Ctrl+Quote", Code::Quote),
            ("Ctrl+[", "Ctrl+BracketLeft", Code::BracketLeft),
            ("Ctrl+]", "Ctrl+BracketRight", Code::BracketRight),
            ("Ctrl+-", "Ctrl+Minus", Code::Minus),
            ("Ctrl+=", "Ctrl+Equal", Code::Equal),
            ("Ctrl+`", "Ctrl+Backquote", Code::Backquote),
        ];
        for (literal, named, expected) in pairs {
            let (lm, lc) = parse_hotkey(literal)
                .unwrap_or_else(|e| panic!("{literal} should parse but errored: {e}"));
            let (nm, nc) = parse_hotkey(named)
                .unwrap_or_else(|e| panic!("{named} should parse but errored: {e}"));
            assert_eq!(lm, Modifiers::CONTROL, "{literal} modifiers");
            assert_eq!(nm, Modifiers::CONTROL, "{named} modifiers");
            assert_eq!(lc, *expected, "{literal} code");
            assert_eq!(nc, *expected, "{named} code");
            assert_eq!(lc, nc, "{literal} should equal {named}");
        }
    }

    /// `Ctrl+\` plus surrounding whitespace — the kind of thing the
    /// Settings dialog might hand us if the user pads the input.
    #[test]
    fn backslash_with_whitespace() {
        let (m, c) = parse_hotkey(r"  Ctrl  +  \  ").unwrap();
        assert_eq!(m, Modifiers::CONTROL);
        assert_eq!(c, Code::Backslash);
    }
}
