//! User configuration loaded from `%APPDATA%\keyhop\config.toml`.
//!
//! The config is intentionally optional: if the file is missing or malformed,
//! [`Config::load_or_default`] logs a warning and returns sensible defaults
//! that match the hardcoded behavior shipped in v0.1.0. This keeps the tool
//! drop-in upgradeable — existing users who never open Settings see no
//! change in behavior.
//!
//! All user-facing customization (hotkeys, hint alphabet, overlay colors,
//! Windows startup) flows through this single struct so the Settings window
//! has exactly one round-trip target.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level configuration loaded from `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// Global hotkey bindings.
    pub hotkeys: HotkeyBindings,
    /// Hint label generation settings.
    pub hints: HintConfig,
    /// Overlay colors (element + window pickers).
    pub colors: ColorConfig,
    /// Windows startup integration (write to Run registry key).
    pub startup: StartupConfig,
}

/// Hotkey chord strings (parsed by [`crate::windows::hotkey::parse_hotkey`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HotkeyBindings {
    /// Pick element in foreground window. Default: `Ctrl+Shift+Space`.
    pub pick_element: String,
    /// Pick window across all monitors. Default: `Ctrl+Alt+Space`.
    pub pick_window: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            pick_element: "Ctrl+Shift+Space".to_string(),
            pick_window: "Ctrl+Alt+Space".to_string(),
        }
    }
}

/// Hint label generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HintConfig {
    /// Characters used to build hint labels. Default: home row `asdfghjkl`.
    pub alphabet: String,
}

impl Default for HintConfig {
    fn default() -> Self {
        Self {
            alphabet: crate::hint::DEFAULT_ALPHABET.to_string(),
        }
    }
}

/// Overlay color presets for both pickers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ColorConfig {
    /// Element picker colors.
    pub element: BadgeColors,
    /// Window picker colors.
    pub window: BadgeColors,
}

/// Hex color strings (`#RRGGBB`) for one badge style. Empty strings mean
/// "use the hardcoded default for this preset" so users can override one
/// color without listing all six.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct BadgeColors {
    /// Background color of the matchable badge.
    pub badge_bg: String,
    /// Text color of the matchable badge.
    pub badge_fg: String,
    /// Border color around both pills.
    pub border: String,
    /// Per-window opacity for the overlay, expressed as a percentage
    /// (0..=100). `0` means "use the preset default" so existing config
    /// files keep their previous behaviour. Values below ~50 quickly
    /// become unreadable; the settings UI clamps to a sensible range.
    pub opacity: u8,
    /// Tri-state override for "draw an arrow from each badge to the
    /// element it represents". `None` means "use the preset default"
    /// (on for elements, off for windows) so existing TOML files keep
    /// their previous look. Set explicitly to override per picker.
    pub show_leader: Option<bool>,
    /// Optional pen color for the leader line + arrowhead, as a hex
    /// `#RRGGBB`. Empty string → use the preset default.
    pub leader_color: String,
}

/// Windows startup integration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct StartupConfig {
    /// Whether the Run-registry shim is currently installed. The Settings
    /// window mirrors this back to the registry on Save; the in-process
    /// flag is here so we have a single source of truth for the UI.
    pub launch_at_startup: bool,
}

impl Config {
    /// Path to the config file under `%APPDATA%\keyhop\config.toml`.
    /// Returns `None` if `APPDATA` isn't set (very unusual on Windows; we
    /// just fall back to defaults in that case).
    pub fn file_path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        let mut path = PathBuf::from(appdata);
        path.push("keyhop");
        path.push("config.toml");
        Some(path)
    }

    /// Load config from disk, falling back to defaults on any failure.
    /// Logs a warning (via `tracing`) when the file exists but is invalid
    /// so users editing TOML by hand can spot mistakes in the log file.
    pub fn load_or_default() -> Self {
        match Self::try_load() {
            Ok(Some(cfg)) => {
                tracing::info!("loaded config from disk");
                cfg
            }
            Ok(None) => {
                tracing::info!("no config file found, using defaults");
                Self::default()
            }
            Err(e) => {
                tracing::warn!(error = ?e, "config file is invalid, using defaults");
                Self::default()
            }
        }
    }

    /// Attempt to load the config. Returns `Ok(None)` when the file simply
    /// doesn't exist (the common first-run case), `Err` for malformed TOML
    /// or I/O failures.
    pub fn try_load() -> anyhow::Result<Option<Self>> {
        let Some(path) = Self::file_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(Some(cfg))
    }

    /// Serialize and write the config to `%APPDATA%\keyhop\config.toml`,
    /// creating the directory if needed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::file_path()
            .ok_or_else(|| anyhow::anyhow!("APPDATA env var not set; cannot save config"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        tracing::info!(?path, "config saved");
        Ok(())
    }

    /// Delete the config file. Used by the "Reset to Defaults" button —
    /// next launch will load defaults again.
    pub fn delete_file() -> anyhow::Result<()> {
        let Some(path) = Self::file_path() else {
            return Ok(());
        };
        if path.exists() {
            std::fs::remove_file(&path)?;
            tracing::info!(?path, "config file deleted");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let text = "[hotkeys]\npick_element = \"Ctrl+Alt+K\"\n";
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(cfg.hotkeys.pick_element, "Ctrl+Alt+K");
        // Other fields keep their defaults.
        assert_eq!(cfg.hotkeys.pick_window, "Ctrl+Alt+Space");
        assert_eq!(cfg.hints.alphabet, crate::hint::DEFAULT_ALPHABET);
    }

    #[test]
    fn empty_toml_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg, Config::default());
    }
}
