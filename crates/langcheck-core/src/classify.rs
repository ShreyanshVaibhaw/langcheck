//! Token classification and lookup normalization.
//!
//! Only [`TokenClass::NaturalWord`] is eligible for *automatic* correction in the
//! MVP; every other class is left untouched. Classification is purely structural
//! (characters, length, and case) — refinements that need the personal
//! dictionary, such as learned mixed-case words ([`TokenClass::KnownPersonalWord`]),
//! are applied by the engine in later steps (see `blueprint.md` Sections 6.4, 8.4).

use crate::token::{case_pattern, CasePattern, DEFAULT_MAX_TOKEN_CHARS};

/// Minimum length, in characters, for a token to be a correctable natural word.
pub const MIN_NATURAL_WORD_LEN: usize = 3;

/// The structural class of a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    /// A clean ASCII word eligible for automatic correction.
    NaturalWord,
    /// A word the user has explicitly added to their personal dictionary.
    ///
    /// Never produced by [`classify_token`]; assigned by the engine after a
    /// personal-dictionary hit (delivery Step 10).
    KnownPersonalWord,
    /// Looks like an email address or URL.
    EmailOrUrl,
    /// Looks like a filesystem path.
    Path,
    /// Looks like source code or an identifier (snake_case, camelCase, CONSTANT).
    CodeOrIdentifier,
    /// Contains digits mixed with other characters.
    NumericOrMixed,
    /// Contains non-ASCII characters (out of scope for the English MVP).
    UnsupportedUnicode,
    /// Shorter than [`MIN_NATURAL_WORD_LEN`].
    TooShort,
    /// Longer than the configured token bound.
    TooLong,
}

impl TokenClass {
    /// Whether this class is eligible for *automatic* correction in the MVP.
    pub fn is_autocorrect_eligible(self) -> bool {
        matches!(self, TokenClass::NaturalWord)
    }
}

/// Classify a raw token using the default token bound.
pub fn classify_token(token: &str) -> TokenClass {
    classify_token_bounded(token, DEFAULT_MAX_TOKEN_CHARS)
}

/// Classify a raw token with an explicit maximum length (matching the session's
/// configured token bound).
///
/// Case and length rules are folded into the result: an all-uppercase or
/// mixed/camel-case alphabetic token is treated as identifier-like
/// ([`TokenClass::CodeOrIdentifier`]) rather than a natural word, so it is never
/// autocorrected.
pub fn classify_token_bounded(token: &str, max_chars: usize) -> TokenClass {
    if token.is_empty() {
        return TokenClass::TooShort;
    }
    let char_count = token.chars().count();
    if char_count > max_chars {
        return TokenClass::TooLong;
    }
    if !token.is_ascii() {
        return TokenClass::UnsupportedUnicode;
    }
    // Structural markers take precedence over prose classification.
    if token.contains('@') || token.contains("://") {
        return TokenClass::EmailOrUrl;
    }
    if token.contains('/') || token.contains('\\') {
        return TokenClass::Path;
    }
    if token.chars().any(|c| c.is_ascii_digit()) {
        return TokenClass::NumericOrMixed;
    }
    if token.contains('_') {
        return TokenClass::CodeOrIdentifier;
    }
    // What remains may contain only ASCII letters and at most one *internal*
    // apostrophe; anything else is treated as non-prose.
    let mut apostrophes = 0usize;
    for (idx, c) in token.chars().enumerate() {
        match c {
            '\'' => {
                apostrophes += 1;
                if idx == 0 || idx == char_count - 1 {
                    return TokenClass::CodeOrIdentifier;
                }
            }
            '.' => return TokenClass::EmailOrUrl,
            c if c.is_ascii_alphabetic() => {}
            _ => return TokenClass::CodeOrIdentifier,
        }
    }
    if apostrophes > 1 {
        return TokenClass::CodeOrIdentifier;
    }
    if char_count < MIN_NATURAL_WORD_LEN {
        return TokenClass::TooShort;
    }
    match case_pattern(token) {
        CasePattern::AllLower | CasePattern::Capitalized => TokenClass::NaturalWord,
        CasePattern::AllUpper | CasePattern::Mixed => TokenClass::CodeOrIdentifier,
    }
}

/// Normalize a token for dictionary lookup: ASCII-lowercased, preserving an
/// internal apostrophe. (English MVP; non-ASCII tokens are never looked up.)
pub fn normalize_lookup(token: &str) -> String {
    token.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_words_are_recognized() {
        assert_eq!(classify_token("the"), TokenClass::NaturalWord);
        assert_eq!(classify_token("teh"), TokenClass::NaturalWord);
        assert_eq!(classify_token("The"), TokenClass::NaturalWord);
        assert_eq!(classify_token("don't"), TokenClass::NaturalWord);
        assert_eq!(classify_token("I'm"), TokenClass::NaturalWord);
    }

    #[test]
    fn uppercase_and_mixed_case_are_excluded() {
        assert_eq!(classify_token("THE"), TokenClass::CodeOrIdentifier);
        assert_eq!(classify_token("tHe"), TokenClass::CodeOrIdentifier);
        assert_eq!(classify_token("camelCase"), TokenClass::CodeOrIdentifier);
    }

    #[test]
    fn length_rules() {
        assert_eq!(classify_token(""), TokenClass::TooShort);
        assert_eq!(classify_token("hi"), TokenClass::TooShort);
        assert_eq!(classify_token(&"a".repeat(40)), TokenClass::TooLong);
        assert_eq!(classify_token_bounded("abcdef", 4), TokenClass::TooLong);
    }

    #[test]
    fn structural_classes() {
        assert_eq!(classify_token("user@host"), TokenClass::EmailOrUrl);
        assert_eq!(classify_token("http://x.io"), TokenClass::EmailOrUrl);
        assert_eq!(classify_token("e.g"), TokenClass::EmailOrUrl);
        assert_eq!(classify_token("a/b"), TokenClass::Path);
        assert_eq!(classify_token("c:\\win"), TokenClass::Path);
        assert_eq!(classify_token("h3llo"), TokenClass::NumericOrMixed);
        assert_eq!(classify_token("foo_bar"), TokenClass::CodeOrIdentifier);
        assert_eq!(classify_token("well-known"), TokenClass::CodeOrIdentifier);
        assert_eq!(classify_token("café"), TokenClass::UnsupportedUnicode);
        assert_eq!(classify_token("'tis"), TokenClass::CodeOrIdentifier);
        assert_eq!(classify_token("rock'n'roll"), TokenClass::CodeOrIdentifier);
    }

    #[test]
    fn eligibility_matches_natural_word() {
        assert!(classify_token("teh").is_autocorrect_eligible());
        assert!(!classify_token("THE").is_autocorrect_eligible());
        assert!(!classify_token("hi").is_autocorrect_eligible());
        assert!(!classify_token("user@host").is_autocorrect_eligible());
    }

    #[test]
    fn normalization_lowercases_ascii() {
        assert_eq!(normalize_lookup("The"), "the");
        assert_eq!(normalize_lookup("DON'T"), "don't");
        assert_eq!(normalize_lookup("teh"), "teh");
    }
}
