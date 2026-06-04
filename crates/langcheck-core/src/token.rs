//! Token state and tokenization: building the single word currently being typed
//! with a bounded buffer, plus casing analysis used both to reject non-prose
//! casings and to restore case after a lowercase lexicon candidate is chosen.
//!
//! Implemented in delivery Step 02 (Core Token and Session State Machine).

/// Maximum number of characters retained for the active token in the MVP. Longer
/// tokens are flagged as overflowed and classified as [`TokenClass::TooLong`];
/// the buffer is never grown without bound (see `blueprint.md` Sections 8.3, 8.4).
///
/// [`TokenClass::TooLong`]: crate::classify::TokenClass::TooLong
pub const DEFAULT_MAX_TOKEN_CHARS: usize = 32;

/// The case shape of a token.
///
/// Only [`AllLower`](CasePattern::AllLower) and
/// [`Capitalized`](CasePattern::Capitalized) tokens are eligible for automatic
/// correction; all-uppercase and mixed/camel-case tokens are conservatively
/// excluded (see `blueprint.md` Section 6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasePattern {
    /// All letters lowercase, e.g. `the`.
    AllLower,
    /// First letter uppercase, the remaining letters lowercase, e.g. `The`.
    Capitalized,
    /// All letters uppercase, e.g. `THE`.
    AllUpper,
    /// Any other shape (camelCase, sTudly caps, interior capitals), e.g. `tHe`.
    Mixed,
}

/// Determine the [`CasePattern`] of a token, considering only ASCII letters and
/// ignoring apostrophes or any other characters.
pub fn case_pattern(token: &str) -> CasePattern {
    let mut letters = 0usize;
    let mut first_upper = false;
    let mut rest_has_upper = false;
    let mut any_upper = false;
    let mut any_lower = false;
    for c in token.chars().filter(|c| c.is_ascii_alphabetic()) {
        let upper = c.is_ascii_uppercase();
        if letters == 0 {
            first_upper = upper;
        } else if upper {
            rest_has_upper = true;
        }
        any_upper |= upper;
        any_lower |= !upper;
        letters += 1;
    }
    if letters == 0 {
        return CasePattern::Mixed;
    }
    match (any_upper, any_lower) {
        (false, _) => CasePattern::AllLower,
        (true, false) => CasePattern::AllUpper,
        (true, true) if first_upper && !rest_has_upper => CasePattern::Capitalized,
        (true, true) => CasePattern::Mixed,
    }
}

/// Restore the case of `replacement` (assumed lowercase) to match the case shape
/// of `original`. Returns `None` for [`CasePattern::Mixed`], which is never
/// autocorrected in the MVP, and for an empty `replacement`.
pub fn restore_case(original: &str, replacement: &str) -> Option<String> {
    if replacement.is_empty() {
        return None;
    }
    match case_pattern(original) {
        CasePattern::AllLower => Some(replacement.to_ascii_lowercase()),
        CasePattern::AllUpper => Some(replacement.to_ascii_uppercase()),
        CasePattern::Capitalized => Some(capitalize_ascii(replacement)),
        CasePattern::Mixed => None,
    }
}

/// Uppercase the first character and lowercase the rest (ASCII).
fn capitalize_ascii(s: &str) -> String {
    let mut chars = s.chars();
    let mut out = String::with_capacity(s.len());
    if let Some(first) = chars.next() {
        out.push(first.to_ascii_uppercase());
        for c in chars {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// The active token buffer. Bounded to `max_chars`; once additional characters
/// arrive the token is flagged as overflowed and will be classified as
/// `TooLong`, so its memory never grows beyond the bound.
#[derive(Debug, Clone)]
pub struct TokenState {
    buf: String,
    char_len: usize,
    max_chars: usize,
    overflowed: bool,
}

impl TokenState {
    /// Create an empty token buffer bounded to `max_chars` (at least 1).
    pub fn new(max_chars: usize) -> Self {
        Self {
            buf: String::new(),
            char_len: 0,
            max_chars: max_chars.max(1),
            overflowed: false,
        }
    }

    /// Whether the token is currently empty.
    pub fn is_empty(&self) -> bool {
        self.char_len == 0
    }

    /// The number of characters currently held.
    pub fn len(&self) -> usize {
        self.char_len
    }

    /// Whether more characters arrived than the bound allowed (sticky until
    /// [`clear`](TokenState::clear)). An overflowed token is never eligible.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The current token text.
    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// Reset to an empty, non-overflowed token.
    pub fn clear(&mut self) {
        self.buf.clear();
        self.char_len = 0;
        self.overflowed = false;
    }

    /// Append a character. Returns `false` (and sets the overflow flag) if the
    /// token is already at its bound, dropping the character.
    pub fn push(&mut self, ch: char) -> bool {
        if self.char_len >= self.max_chars {
            self.overflowed = true;
            return false;
        }
        self.buf.push(ch);
        self.char_len += 1;
        true
    }

    /// Remove the last character. Returns `false` if the token was already empty.
    pub fn pop(&mut self) -> bool {
        if self.buf.pop().is_some() {
            self.char_len -= 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_patterns_are_detected() {
        assert_eq!(case_pattern("the"), CasePattern::AllLower);
        assert_eq!(case_pattern("The"), CasePattern::Capitalized);
        assert_eq!(case_pattern("THE"), CasePattern::AllUpper);
        assert_eq!(case_pattern("tHe"), CasePattern::Mixed);
        assert_eq!(case_pattern("camelCase"), CasePattern::Mixed);
        assert_eq!(case_pattern("don't"), CasePattern::AllLower);
        assert_eq!(case_pattern("Don't"), CasePattern::Capitalized);
        assert_eq!(case_pattern(""), CasePattern::Mixed);
    }

    #[test]
    fn restore_case_maps_clean_patterns_only() {
        assert_eq!(restore_case("teh", "the").as_deref(), Some("the"));
        assert_eq!(restore_case("Teh", "the").as_deref(), Some("The"));
        assert_eq!(restore_case("TEH", "the").as_deref(), Some("THE"));
        assert_eq!(restore_case("tEh", "the"), None);
        assert_eq!(restore_case("teh", ""), None);
    }

    #[test]
    fn token_buffer_is_bounded_and_overflow_is_sticky() {
        let mut t = TokenState::new(4);
        for c in "abcdefgh".chars() {
            t.push(c);
        }
        assert_eq!(t.len(), 4);
        assert!(t.overflowed());
        assert_eq!(t.as_str(), "abcd");
        // Popping below the bound does not clear the overflow flag.
        assert!(t.pop());
        assert!(t.overflowed());
        t.clear();
        assert!(!t.overflowed());
        assert!(t.is_empty());
    }

    #[test]
    fn pop_on_empty_is_false() {
        let mut t = TokenState::new(8);
        assert!(!t.pop());
        t.push('a');
        assert!(t.pop());
        assert!(t.is_empty());
    }
}
