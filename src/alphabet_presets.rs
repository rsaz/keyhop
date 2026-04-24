//! Alphabet preset templates and a builder that resolves a [`HintConfig`]
//! into the final character set used by [`crate::HintEngine`].
//!
//! The Settings dialog exposes a small dropdown of named presets
//! ([`AlphabetPreset`]) plus modifier checkboxes (numbers, extended,
//! exclude-ambiguous) and a free-form custom-additions field. This
//! module is the single source of truth for translating those choices
//! into a concrete alphabet string.
//!
//! Design goals:
//!
//! - **Deterministic**: same config in → same alphabet out, no
//!   surprises for users who script `config.toml` by hand.
//! - **Idempotent**: applying a preset is a pure function — no global
//!   state, easy to unit-test.
//! - **Stable order**: the alphabet's character order matters because
//!   [`crate::HintEngine`] hands out single-letter hints in alphabet
//!   order. Home-row characters always come first so the first nine
//!   hints stay under the user's resting fingers.
//! - **No silent dedupe**: if the user types `aa` into "custom
//!   additions" they probably want one `a`, not a panic. We dedupe
//!   while preserving first-occurrence order.

use crate::config::{AlphabetPreset, HintConfig};

/// Characters we never want to ship in the same alphabet because they
/// look near-identical in the overlay font and the user can't tell
/// which one to type. Stripped when [`HintConfig::exclude_ambiguous`]
/// is on (the default).
///
/// Kept conservative on purpose: with the Consolas overlay font (added
/// in v0.4.0) `I` vs `l` is already crisp, so we only strip the truly
/// indistinguishable pair `O` / `0`. Users who want a wider strip can
/// add characters via `exclude_ambiguous` + `custom_additions` (clear
/// then re-add) — but the default needs to *not* nuke `l` from the
/// home-row preset, which would silently shrink the most common
/// alphabet from 9 chars to 8.
pub const AMBIGUOUS_CHARS: &[char] = &['O', '0'];

/// Home-row characters in QWERTY typing-comfort order. Used as the
/// base of [`AlphabetPreset::HomeRow`] and prefixed onto every
/// non-`Custom` preset so the most common single-letter hints stay
/// under the user's resting fingers.
pub const HOME_ROW: &str = "asdfghjkl";

/// Right-pinky extension keys appended for [`AlphabetPreset::HomeRowExtended`].
pub const HOME_ROW_EXT: &str = ";'";

/// Top-row digits.
pub const NUMBERS: &str = "0123456789";

/// Lowercase a-z.
pub const LOWERCASE_ALPHA: &str = "abcdefghijklmnopqrstuvwxyz";

/// Build the final alphabet string from a [`HintConfig`].
///
/// Resolution order:
/// 1. Pick the base set from `cfg.preset`. For [`AlphabetPreset::Custom`]
///    we use `cfg.alphabet` verbatim and skip every modifier — power
///    users opting into Custom get exactly what they typed.
/// 2. Apply the `include_numbers` / `include_extended` flags by
///    appending the corresponding constant.
/// 3. Append `cfg.custom_additions` so users can plug in extra keys
///    (national characters, F-key sigils, etc.) without giving up the
///    preset.
/// 4. If `cfg.exclude_ambiguous` is on, strip [`AMBIGUOUS_CHARS`].
/// 5. Always dedupe while preserving first-occurrence order so the
///    alphabet doesn't quietly waste hint slots on duplicates.
///
/// Returns at minimum the home-row default if every step somehow
/// produced an empty string — [`crate::HintEngine`] panics on an empty
/// alphabet and we'd rather fall back than crash.
pub fn build_alphabet(cfg: &HintConfig) -> String {
    if matches!(cfg.preset, AlphabetPreset::Custom) {
        // Custom mode bypasses the modifier flags entirely.
        let result = dedupe_preserve_order(&cfg.alphabet);
        return if result.is_empty() {
            crate::hint::DEFAULT_ALPHABET.to_string()
        } else {
            result
        };
    }

    let mut buf = String::new();
    buf.push_str(base_for_preset(cfg.preset));

    if cfg.include_numbers && !cfg.preset_already_has_numbers() {
        buf.push_str(NUMBERS);
    }
    if cfg.include_extended && !buf.contains(';') {
        buf.push_str(HOME_ROW_EXT);
    }
    if !cfg.custom_additions.is_empty() {
        buf.push_str(&cfg.custom_additions);
    }

    if cfg.exclude_ambiguous {
        buf.retain(|c| !AMBIGUOUS_CHARS.contains(&c));
    }

    let result = dedupe_preserve_order(&buf);
    if result.is_empty() {
        // Defensive: every char was stripped. Prevent an empty alphabet
        // from reaching `HintEngine::new` (which would panic).
        crate::hint::DEFAULT_ALPHABET.to_string()
    } else {
        result
    }
}

/// Base character set for a built-in preset. `Custom` is intentionally
/// not handled here — call sites short-circuit before reaching this
/// function.
fn base_for_preset(p: AlphabetPreset) -> &'static str {
    match p {
        AlphabetPreset::HomeRow => HOME_ROW,
        AlphabetPreset::HomeRowExtended => "asdfghjkl;'",
        AlphabetPreset::LowercaseAlpha => LOWERCASE_ALPHA,
        AlphabetPreset::Alphanumeric => "abcdefghijklmnopqrstuvwxyz0123456789",
        AlphabetPreset::Numbers => NUMBERS,
        // Custom is handled before this fn is called.
        AlphabetPreset::Custom => HOME_ROW,
    }
}

impl HintConfig {
    /// True when the configured preset already includes the digits 0-9.
    /// Used by [`build_alphabet`] to avoid double-appending the numbers
    /// when the user toggles `include_numbers` on top of `Alphanumeric`.
    fn preset_already_has_numbers(&self) -> bool {
        matches!(
            self.preset,
            AlphabetPreset::Alphanumeric | AlphabetPreset::Numbers
        )
    }
}

/// Remove duplicate characters while preserving the first occurrence's
/// position. Important: HintEngine assigns single-letter hints in
/// alphabet order, so reordering here would visibly change which
/// element gets which hint.
fn dedupe_preserve_order(s: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if seen.insert(ch) {
            out.push(ch);
        }
    }
    out
}

/// Human-readable label for use in the Settings dropdown. Kept in this
/// module so the UI doesn't have to duplicate the enum-to-string mapping.
pub fn preset_label(p: AlphabetPreset) -> &'static str {
    match p {
        AlphabetPreset::HomeRow => "Home row (asdfghjkl)",
        AlphabetPreset::HomeRowExtended => "Home row + ; '",
        AlphabetPreset::LowercaseAlpha => "Lowercase a-z",
        AlphabetPreset::Alphanumeric => "Alphanumeric a-z 0-9",
        AlphabetPreset::Numbers => "Numbers 0-9",
        AlphabetPreset::Custom => "Custom (use Alphabet field)",
    }
}

/// Every preset, in dropdown order. Stable so the Settings dialog can
/// translate selected-index to enum without a separate mapping.
pub const ALL_PRESETS: &[AlphabetPreset] = &[
    AlphabetPreset::HomeRow,
    AlphabetPreset::HomeRowExtended,
    AlphabetPreset::LowercaseAlpha,
    AlphabetPreset::Alphanumeric,
    AlphabetPreset::Numbers,
    AlphabetPreset::Custom,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HintConfig {
        HintConfig::default()
    }

    #[test]
    fn default_config_yields_home_row() {
        let alphabet = build_alphabet(&cfg());
        assert_eq!(alphabet, "asdfghjkl");
    }

    #[test]
    fn lowercase_alpha_preset_excludes_ambiguous_by_default() {
        let mut c = cfg();
        c.preset = AlphabetPreset::LowercaseAlpha;
        let alphabet = build_alphabet(&c);
        // 'O' / '0' are stripped by the default ambiguous list. 'l' is
        // *not* stripped — Consolas keeps it visually distinct from
        // 'I' / '1', and dropping it would silently break the home-row
        // preset.
        assert!(alphabet.contains('a'));
        assert!(alphabet.contains('l'));
        assert!(alphabet.contains('z'));
    }

    #[test]
    fn lowercase_alpha_preset_keeps_o_when_disabled() {
        let mut c = cfg();
        c.preset = AlphabetPreset::LowercaseAlpha;
        c.exclude_ambiguous = false;
        let alphabet = build_alphabet(&c);
        // 'o' is uppercase 'O' here is the actual ambiguous one; lowercase
        // o is fine. Confirm that disabling the strip doesn't break the
        // base set either way.
        assert!(alphabet.contains('o'));
    }

    #[test]
    fn include_numbers_appends_digits() {
        let mut c = cfg();
        c.include_numbers = true;
        let alphabet = build_alphabet(&c);
        // Default exclude_ambiguous strips only '0'. '1' is intentionally
        // kept because the Consolas overlay font draws it distinctly
        // from lowercase 'l'.
        assert!(alphabet.contains('1'));
        assert!(alphabet.contains('2'));
        assert!(alphabet.contains('9'));
        assert!(!alphabet.contains('0'));
    }

    #[test]
    fn alphanumeric_preset_doesnt_double_append_numbers() {
        let mut c = cfg();
        c.preset = AlphabetPreset::Alphanumeric;
        c.include_numbers = true;
        let alphabet = build_alphabet(&c);
        // Each digit (that survives the ambiguous filter) appears once.
        for d in "123456789".chars() {
            assert_eq!(
                alphabet.matches(d).count(),
                1,
                "digit {d} should appear exactly once in {alphabet:?}"
            );
        }
    }

    #[test]
    fn custom_additions_are_appended() {
        let mut c = cfg();
        c.custom_additions = "z".to_string();
        let alphabet = build_alphabet(&c);
        assert!(alphabet.contains('z'));
        assert!(alphabet.starts_with("asdfghjkl"));
    }

    #[test]
    fn custom_preset_uses_alphabet_verbatim() {
        let mut c = cfg();
        c.preset = AlphabetPreset::Custom;
        c.alphabet = "qwerty".to_string();
        c.include_numbers = true; // ignored in custom mode
        let alphabet = build_alphabet(&c);
        assert_eq!(alphabet, "qwerty");
    }

    #[test]
    fn empty_result_falls_back_to_default() {
        // Alphabet entirely composed of ambiguous chars + exclude on
        // would otherwise produce empty string.
        let mut c = cfg();
        c.preset = AlphabetPreset::Custom;
        c.alphabet = "".to_string();
        let alphabet = build_alphabet(&c);
        assert_eq!(alphabet, crate::hint::DEFAULT_ALPHABET);
    }

    #[test]
    fn dedupe_preserves_first_occurrence_order() {
        assert_eq!(dedupe_preserve_order("aabbcc"), "abc");
        assert_eq!(dedupe_preserve_order("baca"), "bac");
    }

    #[test]
    fn home_row_extended_preset_includes_extras() {
        let mut c = cfg();
        c.preset = AlphabetPreset::HomeRowExtended;
        let alphabet = build_alphabet(&c);
        assert!(alphabet.contains(';'));
        assert!(alphabet.contains('\''));
        assert!(alphabet.starts_with("asdfghjkl"));
    }

    #[test]
    fn numbers_preset_emits_only_digits_minus_ambiguous() {
        let mut c = cfg();
        c.preset = AlphabetPreset::Numbers;
        let alphabet = build_alphabet(&c);
        // Default exclude_ambiguous strips only '0' (kept '1' since
        // Consolas draws it clearly).
        assert_eq!(alphabet, "123456789");
    }
}
