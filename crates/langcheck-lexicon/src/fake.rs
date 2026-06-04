//! Deterministic, in-memory lexicon for tests and benchmarks.
//!
//! `FakeLexicon` lets engine and integration tests use a fixed, predictable
//! vocabulary instead of the real dictionary, whose evolving contents would make
//! tests brittle (`blueprint.md` Section 19.3). It is gated behind the `fake`
//! feature (and `cfg(test)`) so it never ships in a release build, and it
//! performs no I/O or networking.

use std::collections::BTreeMap;
use std::time::Instant;

use langcheck_core::classify::normalize_lookup;

use crate::{
    bounded_levenshtein, CandidateList, LanguageTag, LexiconCandidate, LexiconError,
    LexiconProvider, MAX_LEXICON_CANDIDATES,
};

/// A fixed-vocabulary lexicon backed by an in-memory map.
pub struct FakeLexicon {
    language: LanguageTag,
    words: BTreeMap<String, u32>,
}

impl FakeLexicon {
    /// Build a fake lexicon from `(word, frequency)` pairs (words are normalized).
    pub fn new(language: LanguageTag, entries: impl IntoIterator<Item = (String, u32)>) -> Self {
        let words = entries
            .into_iter()
            .map(|(w, f)| (normalize_lookup(&w), f))
            .collect();
        Self { language, words }
    }

    /// A small fixed `en-US` vocabulary sufficient for engine tests.
    pub fn en_us_default() -> Self {
        const WORDS: &[(&str, u32)] = &[
            ("the", 1000),
            ("word", 600),
            ("world", 550),
            ("would", 500),
            ("work", 480),
            ("there", 460),
            ("their", 450),
            ("they", 440),
            ("with", 700),
            ("that", 800),
            ("this", 780),
            ("have", 760),
            ("from", 680),
            ("about", 420),
            ("which", 410),
            ("people", 260),
            ("because", 250),
            ("really", 230),
            ("receive", 200),
            ("believe", 190),
            ("friend", 180),
            ("hello", 300),
            ("necessary", 110),
            ("separate", 95),
            ("definitely", 90),
        ];
        Self::new(
            LanguageTag::EnUs,
            WORDS.iter().map(|(w, f)| ((*w).to_string(), *f)),
        )
    }
}

impl LexiconProvider for FakeLexicon {
    fn language(&self) -> LanguageTag {
        self.language
    }

    fn contains(&self, language: LanguageTag, word: &str) -> bool {
        language == self.language && self.words.contains_key(&normalize_lookup(word))
    }

    fn candidates(
        &self,
        language: LanguageTag,
        word: &str,
        limit: usize,
        deadline: Option<Instant>,
    ) -> Result<CandidateList, LexiconError> {
        if language != self.language {
            return Err(LexiconError::UnsupportedLanguage(language));
        }
        if matches!(deadline, Some(dl) if Instant::now() >= dl) {
            return Err(LexiconError::Timeout);
        }
        let limit = limit.min(MAX_LEXICON_CANDIDATES);
        let normalized = normalize_lookup(word);
        let max_distance = if normalized.chars().count() >= 6 {
            2
        } else {
            1
        };

        let mut raw: Vec<(String, u8, u32)> = self
            .words
            .iter()
            .filter_map(|(candidate, &frequency)| {
                let distance = bounded_levenshtein(&normalized, candidate, max_distance);
                (distance <= max_distance).then(|| (candidate.clone(), distance, frequency))
            })
            .collect();
        raw.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then(b.2.cmp(&a.2))
                .then_with(|| a.0.cmp(&b.0))
        });

        Ok(raw
            .into_iter()
            .take(limit)
            .map(|(word, edit_distance, frequency)| LexiconCandidate {
                word,
                edit_distance,
                frequency,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_deterministic() {
        let lex = FakeLexicon::en_us_default();
        assert!(lex.contains(LanguageTag::EnUs, "the"));
        assert!(lex.contains(LanguageTag::EnUs, "THE"));
        assert!(!lex.contains(LanguageTag::EnUs, "teh"));
    }

    #[test]
    fn candidates_find_near_words() {
        let lex = FakeLexicon::en_us_default();
        // Distance-1 insertion (short word -> max distance 1).
        let cands = lex.candidates(LanguageTag::EnUs, "wrd", 5, None).unwrap();
        assert!(cands.iter().any(|c| c.word == "word"), "got {cands:?}");
        assert!(cands.len() <= 5);
    }
}
