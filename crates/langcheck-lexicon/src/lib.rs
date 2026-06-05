//! `langcheck-lexicon` — dictionary lookup behind a small trait.
//!
//! Release builds use LangCheck's own bundled, read-only compact FST lexicon. The
//! runtime never sends words to a system, third-party, or remote spell provider —
//! that is what guarantees typed words never leave the device (see `blueprint.md`
//! Section 8.5 and ADR-004).
//!
//! All lookups are synchronous and meant to run on the coordinator thread, never
//! on the input callback (`blueprint.md` Sections 8.1, 8.5). `candidates` accepts
//! an explicit `limit` and optional `deadline` so a slow or pathological search
//! produces no correction rather than stalling the hot path.
//!
//! Implemented in delivery Step 03 (prototype FST) and refined in Step 09
//! (production lexicon). `unsafe` is denied here; the memory-mapped production
//! backend introduces its single reviewed `unsafe` boundary in Step 09.
#![deny(unsafe_code)]

use std::fmt;
use std::time::Instant;

use smallvec::SmallVec;

pub mod compact_fst;
pub mod personal;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

// Rejected runtime backend, retained only as an isolated developer benchmark and
// gated behind a non-default feature so it can never ship (blueprint Section 8.5).
#[cfg(feature = "dev-windows-spell")]
pub mod windows_spell;

pub use compact_fst::CompactFstLexicon;
pub use personal::PersonalDictionary;

/// Maximum number of candidates a lexicon backend may return for one query
/// (`blueprint.md` Section 8.6).
pub const MAX_LEXICON_CANDIDATES: usize = 8;

/// A language supported by the lexicon. The MVP ships only `en-US`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageTag {
    /// English (United States).
    EnUs,
}

impl LanguageTag {
    /// The BCP-47-style tag string.
    pub fn as_str(self) -> &'static str {
        match self {
            LanguageTag::EnUs => "en-US",
        }
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single dictionary suggestion: a known word, its edit distance from the
/// query, and a frequency used later for ranking (`blueprint.md` Sections 8.5,
/// 8.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconCandidate {
    /// The candidate word as stored in the lexicon (normalized, lowercase).
    pub word: String,
    /// Edit distance from the (normalized) query.
    pub edit_distance: u8,
    /// Relative frequency / weight; higher is more common.
    pub frequency: u32,
}

/// A bounded list of candidates that avoids heap allocation for the common case.
pub type CandidateList = SmallVec<[LexiconCandidate; MAX_LEXICON_CANDIDATES]>;

/// Errors a lexicon backend may return. Every error maps to "make no correction"
/// at the engine level (`blueprint.md` Sections 8.6, 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexiconError {
    /// The backend does not serve the requested language.
    UnsupportedLanguage(LanguageTag),
    /// The per-request deadline elapsed before the search finished.
    Timeout,
    /// The dictionary failed to load or validate, or the backend errored.
    Backend(String),
}

impl fmt::Display for LexiconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexiconError::UnsupportedLanguage(lang) => {
                write!(f, "unsupported language: {lang}")
            }
            LexiconError::Timeout => f.write_str("lexicon lookup deadline elapsed"),
            LexiconError::Backend(msg) => write!(f, "lexicon backend error: {msg}"),
        }
    }
}

impl std::error::Error for LexiconError {}

/// Dictionary lookup behind a backend-agnostic interface so the engine never
/// depends on a concrete dictionary (`blueprint.md` Section 8.5).
///
/// Implementations must be local and offline: a backend must never transmit the
/// query or hand it to an external process or provider.
pub trait LexiconProvider: Send + Sync {
    /// The language this provider serves.
    fn language(&self) -> LanguageTag;

    /// Whether `word` is a known word in `language`. Returns `false` for an
    /// unsupported language (fail closed).
    fn contains(&self, language: LanguageTag, word: &str) -> bool;

    /// Generate up to `limit` bounded candidate suggestions for `word`.
    ///
    /// Returns at most `min(limit, MAX_LEXICON_CANDIDATES)` candidates ordered by
    /// increasing edit distance, then decreasing frequency. If `deadline` is set
    /// and elapses, returns [`LexiconError::Timeout`] and the engine makes no
    /// correction.
    fn candidates(
        &self,
        language: LanguageTag,
        word: &str,
        limit: usize,
        deadline: Option<Instant>,
    ) -> Result<CandidateList, LexiconError>;
}

/// Levenshtein edit distance between `a` and `b`, saturating at `max + 1` (so a
/// caller can cheaply test "within `max`"). Operates over Unicode scalar values;
/// inputs are bounded (token length ≤ 32) so the O(n·m) table is small.
pub(crate) fn bounded_levenshtein(a: &str, b: &str, max: u8) -> u8 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let max = max as usize;
    if n.abs_diff(m) > max {
        return saturate(max);
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        if row_min > max {
            return saturate(max);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    let d = prev[m];
    if d > max {
        saturate(max)
    } else {
        d as u8
    }
}

fn saturate(max: usize) -> u8 {
    (max + 1).min(u8::MAX as usize) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tag_string() {
        assert_eq!(LanguageTag::EnUs.as_str(), "en-US");
        assert_eq!(LanguageTag::EnUs.to_string(), "en-US");
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(bounded_levenshtein("the", "the", 2), 0);
        assert_eq!(bounded_levenshtein("teh", "the", 2), 2); // transposition = 2 edits
        assert_eq!(bounded_levenshtein("wrd", "word", 2), 1); // insertion
        assert_eq!(bounded_levenshtein("cat", "dog", 2), 3); // saturates to max+1
        assert_eq!(bounded_levenshtein("recieve", "receive", 2), 2);
    }

    #[test]
    fn error_display_is_redacted_of_input() {
        assert_eq!(
            LexiconError::UnsupportedLanguage(LanguageTag::EnUs).to_string(),
            "unsupported language: en-US"
        );
        assert_eq!(
            LexiconError::Timeout.to_string(),
            "lexicon lookup deadline elapsed"
        );
    }
}
