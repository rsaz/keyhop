//! Generation of short keyboard labels (the strings shown over hints).
//!
//! The engine produces fixed-length labels in a base-N encoding over a
//! configurable alphabet. The default alphabet uses the home row so labels
//! stay easy to type without moving the hands.

/// Default home-row alphabet, ordered by typing comfort for QWERTY layouts.
pub const DEFAULT_ALPHABET: &str = "asdfghjkl";

/// Produces short label strings for a given number of targets.
#[derive(Debug, Clone)]
pub struct HintEngine {
    alphabet: Vec<char>,
}

impl Default for HintEngine {
    fn default() -> Self {
        Self::new(DEFAULT_ALPHABET)
    }
}

impl HintEngine {
    /// Build an engine over the given alphabet. Duplicate characters are
    /// preserved as-is; callers are responsible for deduping if desired.
    ///
    /// # Panics
    ///
    /// Panics if `alphabet` is empty.
    pub fn new(alphabet: &str) -> Self {
        assert!(!alphabet.is_empty(), "hint alphabet must not be empty");
        Self {
            alphabet: alphabet.chars().collect(),
        }
    }

    /// Return the configured alphabet.
    pub fn alphabet(&self) -> &[char] {
        &self.alphabet
    }

    /// Generate `count` distinct fixed-length labels.
    ///
    /// Label length is the smallest L such that `alphabet.len()^L >= count`,
    /// so all labels share the same length — this avoids ambiguous prefixes
    /// during interactive matching.
    pub fn generate(&self, count: usize) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }
        let n = self.alphabet.len();
        let mut len = 1usize;
        let mut cap = n;
        while cap < count {
            len += 1;
            cap = cap.saturating_mul(n);
            if cap == usize::MAX {
                break;
            }
        }
        (0..count).map(|i| self.encode(i, len)).collect()
    }

    fn encode(&self, mut idx: usize, len: usize) -> String {
        let n = self.alphabet.len();
        let mut chars = vec![self.alphabet[0]; len];
        for pos in (0..len).rev() {
            chars[pos] = self.alphabet[idx % n];
            idx /= n;
        }
        chars.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn empty_count_returns_empty() {
        let e = HintEngine::default();
        assert!(e.generate(0).is_empty());
    }

    #[test]
    fn single_char_labels_when_count_fits_alphabet() {
        let e = HintEngine::new("abc");
        let labels = e.generate(3);
        assert_eq!(labels, vec!["a", "b", "c"]);
    }

    #[test]
    fn promotes_to_two_chars_when_count_exceeds_alphabet() {
        let e = HintEngine::new("ab");
        let labels = e.generate(4);
        assert_eq!(labels, vec!["aa", "ab", "ba", "bb"]);
    }

    #[test]
    fn all_labels_are_unique_and_same_length() {
        let e = HintEngine::default();
        let labels = e.generate(120);
        let len = labels[0].len();
        assert!(labels.iter().all(|l| l.len() == len));
        let set: HashSet<_> = labels.iter().collect();
        assert_eq!(set.len(), labels.len());
    }

    #[test]
    #[should_panic]
    fn empty_alphabet_panics() {
        let _ = HintEngine::new("");
    }
}
