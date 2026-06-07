//! Bounded candidate assembly.
//!
//! The engine combines dictionary-validated suggestions (produced by the lexicon
//! backend and passed in by the coordinator) with a small curated table of
//! built-in high-confidence common typos, and — from Step 10 — explicit user
//! pairs. Dictionary-dependent generation (edit-distance neighbours, transposition
//! variants) lives in the lexicon, which owns the dictionary; this module only
//! merges, de-duplicates, and bounds the candidate set so the engine stays
//! independent of any concrete backend (see `blueprint.md` Sections 8.6, 8.5).
//!
//! Implemented in delivery Step 04 (Candidate Ranking and Confidence Engine).

/// Maximum number of candidates retained before ranking (`blueprint.md` Section 8.6).
pub const MAX_CANDIDATES: usize = 32;

/// Default frequency assigned to a curated-rule correction (treated as common so a
/// rare-word penalty never suppresses a hand-verified typo fix).
const RULE_FREQUENCY: u32 = 1_000_000;

/// Where a candidate came from. Used by the ranker to apply source bonuses and by
/// diagnostics to attribute decisions (`blueprint.md` Sections 8.6, 8.7, 18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    /// A dictionary suggestion from the lexicon backend.
    Lexicon,
    /// A built-in curated common-typo rule.
    CommonTypoRule,
    /// An explicit user-defined replacement pair (delivery Step 10).
    UserPair,
}

impl CandidateSource {
    /// Priority for de-duplication: a higher-priority source is kept when the same
    /// candidate word arrives from multiple sources.
    fn priority(self) -> u8 {
        match self {
            CandidateSource::UserPair => 2,
            CandidateSource::CommonTypoRule => 1,
            CandidateSource::Lexicon => 0,
        }
    }
}

/// A single candidate correction word fed into the ranker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateWord {
    /// The candidate word, normalized to lowercase.
    pub word: String,
    /// Edit distance from the (normalized) original (transposition counts as 1).
    pub edit_distance: u8,
    /// Relative frequency / weight; higher is more common.
    pub frequency: u32,
    /// Where this candidate came from.
    pub source: CandidateSource,
}

/// Curated, high-confidence common English misspellings. Correction targets are
/// real words; the table is intentionally small (general coverage comes from the
/// lexicon's edit-distance search).
const BUILTIN_TYPOS: &[(&str, &str)] = &[
    ("teh", "the"),
    ("recieve", "receive"),
    ("seperate", "separate"),
    ("definately", "definitely"),
    ("freind", "friend"),
    ("wierd", "weird"),
    ("untill", "until"),
    ("tomorow", "tomorrow"),
    ("beleive", "believe"),
    ("becuase", "because"),
    ("thier", "their"),
    ("wich", "which"),
    ("occured", "occurred"),
    ("neccessary", "necessary"),
    ("enviroment", "environment"),
    ("accomodate", "accommodate"),
    ("acheive", "achieve"),
    ("arguement", "argument"),
    ("begining", "beginning"),
    ("collegue", "colleague"),
    ("comming", "coming"),
    ("concious", "conscious"),
    ("embarass", "embarrass"),
    ("existance", "existence"),
    ("familar", "familiar"),
    ("foriegn", "foreign"),
    ("goverment", "government"),
    ("harrass", "harass"),
    ("independant", "independent"),
    ("occassion", "occasion"),
    ("persistant", "persistent"),
    ("posession", "possession"),
    ("prefered", "preferred"),
    ("recomend", "recommend"),
    ("relevent", "relevant"),
    ("religous", "religious"),
    ("remeber", "remember"),
    ("succesful", "successful"),
    ("suprise", "surprise"),
    ("truely", "truly"),
    ("unfortunatly", "unfortunately"),
];

/// Look up a curated correction for a normalized token, if one exists.
pub fn builtin_typo_correction(normalized: &str) -> Option<&'static str> {
    BUILTIN_TYPOS
        .iter()
        .find(|(misspelling, _)| *misspelling == normalized)
        .map(|(_, correction)| *correction)
}

/// Merge curated-rule and lexicon candidates into a bounded, de-duplicated list.
///
/// A candidate equal to the original is dropped (never "correct" a word to
/// itself). When the same word arrives from multiple sources, the highest-priority
/// source and the minimum edit distance / maximum frequency are kept.
pub fn assemble(normalized: &str, lexicon: &[CandidateWord]) -> Vec<CandidateWord> {
    let mut out: Vec<CandidateWord> = Vec::new();

    if let Some(correction) = builtin_typo_correction(normalized) {
        if correction != normalized {
            out.push(CandidateWord {
                word: correction.to_string(),
                edit_distance: osa_distance(normalized, correction),
                frequency: RULE_FREQUENCY,
                source: CandidateSource::CommonTypoRule,
            });
        }
    }

    for candidate in lexicon {
        if candidate.word == normalized {
            continue;
        }
        if let Some(existing) = out.iter_mut().find(|e| e.word == candidate.word) {
            existing.edit_distance = existing.edit_distance.min(candidate.edit_distance);
            existing.frequency = existing.frequency.max(candidate.frequency);
            if candidate.source.priority() > existing.source.priority() {
                existing.source = candidate.source;
            }
        } else if out.len() < MAX_CANDIDATES {
            out.push(candidate.clone());
        }
    }

    out.truncate(MAX_CANDIDATES);
    out
}

/// Optimal string alignment (restricted Damerau-Levenshtein) distance, counting an
/// adjacent transposition as a single edit. Inputs are bounded (token length ≤ 32)
/// so the O(n·m) table is small; saturates into `u8`.
pub fn osa_distance(a: &str, b: &str) -> u8 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    u8::try_from(d[n][m]).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(word: &str, edit_distance: u8, frequency: u32) -> CandidateWord {
        CandidateWord {
            word: word.to_string(),
            edit_distance,
            frequency,
            source: CandidateSource::Lexicon,
        }
    }

    #[test]
    fn osa_counts_transposition_as_one() {
        assert_eq!(osa_distance("teh", "the"), 1);
        assert_eq!(osa_distance("the", "the"), 0);
        assert_eq!(osa_distance("wrd", "word"), 1);
        assert_eq!(osa_distance("cat", "dog"), 3);
    }

    #[test]
    fn builtin_rule_lookup() {
        assert_eq!(builtin_typo_correction("teh"), Some("the"));
        assert_eq!(builtin_typo_correction("recieve"), Some("receive"));
        assert_eq!(builtin_typo_correction("hello"), None);
    }

    #[test]
    fn assemble_adds_rule_and_dedupes_with_lexicon() {
        // "teh" has both a curated rule (-> the) and a lexicon transposition (-> the).
        let candidates = assemble("teh", &[lex("the", 1, 1000), lex("ten", 1, 50)]);
        let the = candidates.iter().find(|c| c.word == "the").unwrap();
        assert_eq!(the.source, CandidateSource::CommonTypoRule); // rule wins priority
        assert_eq!(the.edit_distance, 1);
        assert!(candidates.iter().any(|c| c.word == "ten"));
        // de-duplicated: only one "the".
        assert_eq!(candidates.iter().filter(|c| c.word == "the").count(), 1);
    }

    #[test]
    fn assemble_drops_identity_and_bounds() {
        let many: Vec<CandidateWord> = (0..40).map(|i| lex(&format!("w{i:02}"), 1, 1)).collect();
        let candidates = assemble("origin", &many);
        assert!(candidates.len() <= MAX_CANDIDATES);

        // A candidate equal to the original is never produced.
        let candidates = assemble("word", &[lex("word", 0, 100), lex("ward", 1, 10)]);
        assert!(candidates.iter().all(|c| c.word != "word"));
        assert!(candidates.iter().any(|c| c.word == "ward"));
    }
}
