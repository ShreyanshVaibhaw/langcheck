//! Candidate ranking.
//!
//! Produces a deterministic score for each candidate (lower is better) from the
//! inputs in `blueprint.md` Section 8.7: weighted edit distance, keyboard-adjacency
//! of a substitution, word frequency, casing compatibility, and source bonuses for
//! curated/user rules. Context-based scoring is intentionally absent in the MVP.
//!
//! Implemented in delivery Step 04 (Candidate Ranking and Confidence Engine).

use crate::candidate::{CandidateSource, CandidateWord};
use crate::session::WordSnapshot;
use crate::token::restore_case;

/// Weights for the scoring model (`blueprint.md` Section 8.7). Defaults are tuned
/// so that, in practice, automatic correction fires only for a clearly-closest
/// candidate (a single edit with no comparably-close rival) or a curated rule.
#[derive(Debug, Clone, Copy)]
pub struct RankWeights {
    /// Cost per edit. Large, so edit distance dominates the ordering.
    pub edit: f64,
    /// Weight of the (log) frequency reward; acts as a tie-breaker within a distance.
    pub frequency: f64,
    /// Bonus subtracted for a built-in curated typo rule.
    pub rule_bonus: f64,
    /// Bonus subtracted for an explicit user pair (delivery Step 10).
    pub user_bonus: f64,
    /// Bonus subtracted when a single substitution is between keyboard-adjacent keys.
    pub keyboard_bonus: f64,
    /// Penalty added when the original's case cannot be cleanly reapplied.
    pub casing_penalty: f64,
}

impl Default for RankWeights {
    fn default() -> Self {
        Self {
            edit: 1000.0,
            frequency: 12.0,
            rule_bonus: 600.0,
            user_bonus: 2000.0,
            keyboard_bonus: 80.0,
            casing_penalty: 100_000.0,
        }
    }
}

/// A candidate with its computed score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    /// The candidate being scored.
    pub candidate: CandidateWord,
    /// Score; lower is better.
    pub score: f64,
}

/// Score and sort `candidates` for `snapshot` (best first). Ties are broken
/// alphabetically so the ordering is fully deterministic.
pub fn rank(
    snapshot: &WordSnapshot,
    candidates: &[CandidateWord],
    weights: &RankWeights,
) -> Vec<ScoredCandidate> {
    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .map(|candidate| ScoredCandidate {
            score: score(snapshot, candidate, weights),
            candidate: candidate.clone(),
        })
        .collect();
    scored.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.candidate.word.cmp(&b.candidate.word))
    });
    scored
}

fn score(snapshot: &WordSnapshot, candidate: &CandidateWord, weights: &RankWeights) -> f64 {
    let mut score = weights.edit * f64::from(candidate.edit_distance);
    score -= weights.frequency * (f64::from(candidate.frequency) + 1.0).ln();
    match candidate.source {
        CandidateSource::CommonTypoRule => score -= weights.rule_bonus,
        CandidateSource::UserPair => score -= weights.user_bonus,
        CandidateSource::Lexicon => {}
    }
    if let Some((from, to)) = single_substitution(&snapshot.normalized, &candidate.word) {
        if keyboard_adjacent(from, to) {
            score -= weights.keyboard_bonus;
        }
    }
    if restore_case(&snapshot.text, &candidate.word).is_none() {
        score += weights.casing_penalty;
    }
    score
}

/// If `a` and `b` have equal length and differ at exactly one position, return the
/// `(a_char, b_char)` pair at that position; otherwise `None`.
fn single_substitution(a: &str, b: &str) -> Option<(char, char)> {
    if a.chars().count() != b.chars().count() {
        return None;
    }
    let mut diff = None;
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            if diff.is_some() {
                return None;
            }
            diff = Some((x, y));
        }
    }
    diff
}

/// Whether two letters are adjacent on a QWERTY keyboard (a common substitution
/// typo). Non-letters are never adjacent.
fn keyboard_adjacent(a: char, b: char) -> bool {
    const ADJACENCY: &[(char, &str)] = &[
        ('q', "wa"),
        ('w', "qeas"),
        ('e', "wrsd"),
        ('r', "etdf"),
        ('t', "ryfg"),
        ('y', "tugh"),
        ('u', "yijh"),
        ('i', "uojk"),
        ('o', "ipkl"),
        ('p', "ol"),
        ('a', "qwsz"),
        ('s', "awedzx"),
        ('d', "serfcx"),
        ('f', "drtgvc"),
        ('g', "ftyhbv"),
        ('h', "gyujnb"),
        ('j', "huiknm"),
        ('k', "jiolm"),
        ('l', "kop"),
        ('z', "asx"),
        ('x', "zsdc"),
        ('c', "xdfv"),
        ('v', "cfgb"),
        ('b', "vghn"),
        ('n', "bhjm"),
        ('m', "njk"),
    ];
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if !a.is_ascii_lowercase() || !b.is_ascii_lowercase() {
        return false;
    }
    ADJACENCY
        .iter()
        .find(|(key, _)| *key == a)
        .is_some_and(|(_, neighbours)| neighbours.contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{classify_token, normalize_lookup};
    use crate::token::case_pattern;

    fn snapshot(text: &str) -> WordSnapshot {
        WordSnapshot {
            focus_id: 1,
            generation: 1,
            token_version: 1,
            normalized: normalize_lookup(text),
            case: case_pattern(text),
            class: classify_token(text),
            text: text.to_string(),
        }
    }

    fn cand(
        word: &str,
        edit_distance: u8,
        frequency: u32,
        source: CandidateSource,
    ) -> CandidateWord {
        CandidateWord {
            word: word.to_string(),
            edit_distance,
            frequency,
            source,
        }
    }

    #[test]
    fn keyboard_adjacency() {
        assert!(keyboard_adjacent('a', 's'));
        assert!(keyboard_adjacent('S', 'A'));
        assert!(!keyboard_adjacent('a', 'p'));
        assert!(!keyboard_adjacent('a', '1'));
    }

    #[test]
    fn single_substitution_detection() {
        assert_eq!(single_substitution("cat", "car"), Some(('t', 'r')));
        assert_eq!(single_substitution("cat", "cat"), None);
        assert_eq!(single_substitution("cat", "cars"), None);
        assert_eq!(single_substitution("cat", "dog"), None);
    }

    #[test]
    fn closer_and_more_frequent_candidates_rank_first() {
        let snap = snapshot("wrd");
        let ranked = rank(
            &snap,
            &[
                cand("word", 1, 1000, CandidateSource::Lexicon),
                cand("world", 2, 5000, CandidateSource::Lexicon),
            ],
            &RankWeights::default(),
        );
        // Distance dominates: the single-edit "word" beats the two-edit "world".
        assert_eq!(ranked[0].candidate.word, "word");
        assert!(ranked[0].score < ranked[1].score);
    }

    #[test]
    fn rule_candidate_beats_a_plain_lexicon_rival() {
        let snap = snapshot("teh");
        let ranked = rank(
            &snap,
            &[
                cand("the", 1, 1000, CandidateSource::CommonTypoRule),
                cand("ten", 1, 1000, CandidateSource::Lexicon),
            ],
            &RankWeights::default(),
        );
        assert_eq!(ranked[0].candidate.word, "the");
    }
}
