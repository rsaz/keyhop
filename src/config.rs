//! User configuration loaded from `%APPDATA%\keyhop\config.toml`.
//!
//! The config is intentionally optional: if the file is missing or malformed,
//! [`Config::load_or_default`] logs a warning and returns sensible defaults
//! that match the hardcoded behavior shipped in v0.1.0. This keeps the tool
//! drop-in upgradeable — existing users who never open Settings see no
//! change in behavior.
//!
//! All user-facing customization (hotkeys, hint alphabet, overlay colors,
//! Windows startup, scope, performance) flows through this single struct
//! so the Settings window has exactly one round-trip target.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hint::HintStrategy;

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
    /// Element-targeting scope (active window vs all monitors etc.).
    pub scope: ScopeConfig,
    /// Performance toggles (caching).
    pub performance: PerformanceConfig,
}

/// Hotkey chord strings (parsed by [`crate::windows::hotkey::parse_hotkey`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HotkeyBindings {
    /// Pick element in foreground window. Default: `Ctrl+Shift+Space`.
    pub pick_element: String,
    /// Pick window across all monitors. Default: `Ctrl+Alt+Space`.
    pub pick_window: String,
    /// Open settings dialog. Default: `Ctrl+Shift+,`.
    pub open_settings: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            pick_element: "Ctrl+Shift+Space".to_string(),
            pick_window: "Ctrl+Alt+Space".to_string(),
            open_settings: "Ctrl+Shift+,".to_string(),
        }
    }
}

/// Hint label generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HintConfig {
    /// Characters used to build hint labels. Default: home row `asdfghjkl`.
    /// When empty (and a `preset` is set), the alphabet is derived from
    /// the preset/builder fields below.
    pub alphabet: String,
    /// Allocation strategy. Default: [`HintStrategy::ShortestFirst`].
    pub strategy: HintStrategy,
    /// Built-in alphabet preset selected from a small menu in Settings.
    /// When set to anything other than [`AlphabetPreset::Custom`] the
    /// `alphabet` field is rebuilt from this preset plus the modifier
    /// flags below on Save.
    pub preset: AlphabetPreset,
    /// Append the digits `0123456789` to the preset alphabet.
    pub include_numbers: bool,
    /// Append the right-hand extension keys `;'` to the preset alphabet.
    pub include_extended: bool,
    /// Strip ambiguous characters (`I`, `l`, `O`, `0`) from the final
    /// alphabet so the user can't confuse them on a busy overlay.
    pub exclude_ambiguous: bool,
    /// Free-form characters appended to the preset alphabet. Useful for
    /// keyboard layouts with handy non-ASCII keys (e.g. dead keys on
    /// AZERTY) that would otherwise be unreachable.
    pub custom_additions: String,
    /// Minimum number of single-character hints to *guarantee* on every
    /// scene, even when math-optimal allocation would skip them.
    ///
    /// When a scene has many targets (e.g. > 73 with the 9-char home
    /// row), the Vimium-style allocator skips length-1 hints because
    /// promoting one would force `n` length-`L` hints to grow by one
    /// character — net cost across the scene is higher. That's
    /// average-keystroke optimal but ergonomically surprising: users
    /// expect "shortest first" to mean "at least one single-key hint".
    ///
    /// Setting this to a positive number reserves that many length-1
    /// hints from `alphabet[0..min_singles]`, then runs the Vimium
    /// allocator over the remaining `n − min_singles` prefix slots for
    /// the rest of the count. Trades a small average-typing penalty
    /// for guaranteed one-key reach on the most-likely targets.
    ///
    /// Capped at `n − 1` at runtime (we always leave at least one
    /// prefix slot for multi-char hints when count > n). Default `8`
    /// matches the home-row alphabet's "all letters as singles, last
    /// one reserved as prefix" convention that vim-style hint tools
    /// (Vimium, Surfingkeys, easymotion) ship with — set to `0` to
    /// fall back to the math-optimal Vimium allocation that
    /// minimises average keystrokes at the cost of one-key reach.
    #[serde(default = "default_min_singles")]
    pub min_singles: usize,
}

/// Default for [`HintConfig::min_singles`]. Lifted to a free function so
/// `#[serde(default = "...")]` can call it on a per-field basis when
/// the user's `config.toml` is missing the entry — important because
/// users upgrading from earlier [Unreleased] builds where the default
/// was `0` would otherwise lose the new behaviour silently.
fn default_min_singles() -> usize {
    8
}

impl Default for HintConfig {
    fn default() -> Self {
        Self {
            alphabet: crate::hint::DEFAULT_ALPHABET.to_string(),
            strategy: HintStrategy::default(),
            preset: AlphabetPreset::default(),
            include_numbers: false,
            include_extended: false,
            exclude_ambiguous: true,
            custom_additions: String::new(),
            min_singles: default_min_singles(),
        }
    }
}

/// Pre-built alphabet templates the Settings dialog exposes via a
/// dropdown. Mapping to actual characters lives in
/// [`crate::alphabet_presets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlphabetPreset {
    /// Home row (`asdfghjkl`). Default — minimum hand movement.
    #[default]
    HomeRow,
    /// Home row plus right-hand extension keys (`asdfghjkl;'`).
    HomeRowExtended,
    /// Lowercase a-z.
    LowercaseAlpha,
    /// Lowercase a-z plus 0-9.
    Alphanumeric,
    /// Top-row digits 0-9.
    Numbers,
    /// Don't apply any preset — use the `alphabet` field verbatim. The
    /// modifier flags (`include_numbers`, `exclude_ambiguous`, …) are
    /// also ignored in this mode so power-users get exactly what they
    /// typed.
    Custom,
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

/// Element-targeting scope. Picks which set of windows the element
/// picker enumerates when the user fires its leader chord.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScopeConfig {
    /// Scope mode. See [`ScopeMode`].
    pub mode: ScopeMode,
    /// Hard cap on elements collected per enumeration, applied across
    /// the entire scope. Prevents "all windows" mode on a busy desktop
    /// from producing a wall of badges that can't be parsed visually.
    pub max_elements: usize,
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            mode: ScopeMode::default(),
            max_elements: 300,
        }
    }
}

/// Which set of windows the element picker enumerates. Defaults to
/// [`Self::ActiveWindow`] (the v0.3.0 behaviour) so existing users see
/// no change after upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScopeMode {
    /// Only the foreground window (legacy behaviour).
    #[default]
    ActiveWindow,
    /// All visible top-level windows on the monitor that currently
    /// contains the cursor.
    ActiveMonitor,
    /// All visible top-level windows across every monitor.
    AllWindows,
}

/// Performance / caching toggles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PerformanceConfig {
    /// When `true`, the backend caches enumeration results per HWND for
    /// `cache_ttl_ms` milliseconds. Repeated invocations on the same
    /// window (e.g. cancelling and retrying) skip the UIA tree walk
    /// entirely.
    pub enable_caching: bool,
    /// Lifetime of a cache entry in milliseconds. Lower values are
    /// safer (the UI is less likely to drift from reality) but reduce
    /// the cache hit rate.
    pub cache_ttl_ms: u64,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enable_caching: true,
            cache_ttl_ms: 500,
        }
    }
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
        assert_eq!(cfg.hints.strategy, HintStrategy::ShortestFirst);
        assert_eq!(cfg.scope.mode, ScopeMode::ActiveWindow);
        assert_eq!(cfg.scope.max_elements, 300);
        assert!(cfg.performance.enable_caching);
        assert_eq!(cfg.performance.cache_ttl_ms, 500);
    }

    #[test]
    fn empty_toml_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn scope_mode_serializes_as_snake_case() {
        let cfg = ScopeConfig {
            mode: ScopeMode::AllWindows,
            max_elements: 100,
        };
        let text = toml::to_string(&cfg).unwrap();
        assert!(
            text.contains("all_windows"),
            "expected snake_case in {text:?}"
        );
    }

    #[test]
    fn legacy_config_without_scope_or_performance_loads() {
        // Older config files only had hotkeys, hints, colors, startup.
        // They must continue to deserialize cleanly into the new struct
        // — anything missing falls back to defaults.
        let text = r#"
            [hotkeys]
            pick_element = "Ctrl+Shift+Space"
            pick_window  = "Ctrl+Alt+Space"
            [hints]
            alphabet = "asdfghjkl"
            [colors.element]
            badge_bg = ""
            [colors.window]
            badge_bg = ""
            [startup]
            launch_at_startup = false
        "#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(cfg.scope, ScopeConfig::default());
        assert_eq!(cfg.performance, PerformanceConfig::default());
    }
}
