//! Confidence policy: separates the best suggestion from what is safe to apply
//! automatically. The automatic tier is deliberately conservative — precision is
//! valued far above recall — and every decision carries a reason code for metrics
//! and tests (`blueprint.md` Sections 5.1, 8.7, 18).
//!
//! Implemented in delivery Step 04 (Candidate Ranking and Confidence Engine).

use std::time::Instant;

use crate::candidate::{assemble, CandidateSource, CandidateWord};
use crate::classify::TokenClass;
use crate::rank::{rank, RankWeights, ScoredCandidate};
use crate::session::WordSnapshot;
use crate::token::restore_case;

/// Thresholds governing the automatic / suggest / ignore tiers. Scores come from
/// [`RankWeights`]; lower is better. Defaults pair with `RankWeights::default()`.
#[derive(Debug, Clone, Copy)]
pub struct ConfidencePolicy {
    /// The top candidate must score at or below this to be eligible for automatic
    /// correction. Tuned to admit a single edit but not a two-edit candidate.
    pub max_auto_score: f64,
    /// The top candidate must score at or below this to be offered as a suggestion.
    pub max_suggest_score: f64,
    /// The top candidate must beat the second by at least this margin to autocorrect
    /// (prevents correcting ambiguous typos with several near rivals).
    pub min_margin: f64,
}

impl Default for ConfidencePolicy {
    fn default() -> Self {
        Self {
            max_auto_score: 1_100.0,
            max_suggest_score: 2_100.0,
            min_margin: 250.0,
        }
    }
}

/// A chosen correction, with the case-restored replacement ready to inject.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The original token, exactly as typed.
    pub original: String,
    /// The replacement with the original token's case reapplied.
    pub replacement: String,
    /// Edit distance from the original (transposition counts as 1).
    pub edit_distance: u8,
    /// Where the candidate came from.
    pub source: CandidateSource,
    /// The candidate's score (lower is better).
    pub score: f64,
}

/// Why a token was only suggested rather than autocorrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestReason {
    /// The best candidate was plausible but above the automatic-correction score.
    BelowAutoThreshold,
    /// Two or more candidates were too close to choose safely.
    AmbiguousMargin,
}

/// Why no correction was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    /// The token's class is not eligible for automatic correction.
    NotEligible(TokenClass),
    /// No candidate corrections were produced.
    NoCandidates,
    /// The best candidate scored below the suggestion threshold.
    LowConfidence,
    /// The original's case could not be cleanly reapplied to the candidate.
    CasingMismatch,
    /// The per-request deadline had already elapsed.
    DeadlineExceeded,
}

/// The outcome of evaluating a token (`blueprint.md` Section 8.7). Every variant is
/// a reason code in itself; `Ignore`/`Suggest` carry a more specific reason.
#[derive(Debug, Clone, PartialEq)]
pub enum CorrectionDecision {
    /// The original token is already a known word; nothing to do.
    Known,
    /// No correction; see the reason.
    Ignore(IgnoreReason),
    /// Detected a likely correction but did not apply it automatically.
    Suggest {
        candidate: Candidate,
        reason: SuggestReason,
    },
    /// A high-confidence automatic correction.
    AutoCorrect { candidate: Candidate },
}

/// Full evaluation pipeline: deadline and eligibility checks, then candidate
/// assembly, ranking, and the confidence decision.
///
/// `is_original_known` and `lexicon_candidates` are supplied by the coordinator
/// from the lexicon backend, so the engine itself never depends on a dictionary.
pub fn evaluate(
    snapshot: &WordSnapshot,
    is_original_known: bool,
    lexicon_candidates: &[CandidateWord],
    weights: &RankWeights,
    policy: &ConfidencePolicy,
    deadline: Option<Instant>,
) -> CorrectionDecision {
    if matches!(deadline, Some(dl) if Instant::now() >= dl) {
        return CorrectionDecision::Ignore(IgnoreReason::DeadlineExceeded);
    }
    if !snapshot.is_autocorrect_eligible() {
        return CorrectionDecision::Ignore(IgnoreReason::NotEligible(snapshot.class));
    }
    let candidates = assemble(&snapshot.normalized, lexicon_candidates);
    // A curated common-typo rule or an explicit user pair overrides "known"-ness: a
    // known but commonly-mistyped form (e.g. an archaic dictionary entry like
    // "wierd") is still corrected. Without such a rule, a known word is left alone.
    let has_overriding_rule = candidates.iter().any(|candidate| {
        matches!(
            candidate.source,
            CandidateSource::CommonTypoRule | CandidateSource::UserPair
        )
    });
    if is_original_known && !has_overriding_rule {
        return CorrectionDecision::Known;
    }
    let scored = rank(snapshot, &candidates, weights);
    decide(snapshot, &scored, policy)
}

/// The confidence decision over already-ranked candidates. Assumes the token is
/// eligible and the original is not already known (see [`evaluate`]).
pub fn decide(
    snapshot: &WordSnapshot,
    scored: &[ScoredCandidate],
    policy: &ConfidencePolicy,
) -> CorrectionDecision {
    let Some(top) = scored.first() else {
        return CorrectionDecision::Ignore(IgnoreReason::NoCandidates);
    };
    let Some(replacement) = restore_case(&snapshot.text, &top.candidate.word) else {
        return CorrectionDecision::Ignore(IgnoreReason::CasingMismatch);
    };
    let candidate = Candidate {
        original: snapshot.text.clone(),
        replacement,
        edit_distance: top.candidate.edit_distance,
        source: top.candidate.source,
        score: top.score,
    };

    if top.score > policy.max_suggest_score {
        return CorrectionDecision::Ignore(IgnoreReason::LowConfidence);
    }

    let margin_ok = match scored.get(1) {
        Some(second) => (second.score - top.score) >= policy.min_margin,
        None => true,
    };
    let auto_ok = top.score <= policy.max_auto_score;

    if auto_ok && margin_ok {
        CorrectionDecision::AutoCorrect { candidate }
    } else {
        let reason = if auto_ok {
            SuggestReason::AmbiguousMargin
        } else {
            SuggestReason::BelowAutoThreshold
        };
        CorrectionDecision::Suggest { candidate, reason }
    }
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

    fn lex(word: &str, edit_distance: u8, frequency: u32) -> CandidateWord {
        CandidateWord {
            word: word.to_string(),
            edit_distance,
            frequency,
            source: CandidateSource::Lexicon,
        }
    }

    fn eval(snap: &WordSnapshot, known: bool, cands: &[CandidateWord]) -> CorrectionDecision {
        evaluate(
            snap,
            known,
            cands,
            &RankWeights::default(),
            &ConfidencePolicy::default(),
            None,
        )
    }

    #[test]
    fn known_word_is_left_alone() {
        assert_eq!(
            eval(&snapshot("hello"), true, &[]),
            CorrectionDecision::Known
        );
    }

    #[test]
    fn curated_rule_overrides_a_known_word() {
        // "wierd" is a built-in common typo that also exists in comprehensive
        // dictionaries; the rule must still correct it even when is_original_known.
        match eval(&snapshot("wierd"), true, &[]) {
            CorrectionDecision::AutoCorrect { candidate } => {
                assert_eq!(candidate.replacement, "weird");
            }
            other => panic!("expected AutoCorrect weird, got {other:?}"),
        }
    }

    #[test]
    fn non_prose_token_is_ineligible() {
        let decision = eval(&snapshot("user@host"), false, &[lex("user", 1, 100)]);
        assert_eq!(
            decision,
            CorrectionDecision::Ignore(IgnoreReason::NotEligible(TokenClass::EmailOrUrl))
        );
    }

    #[test]
    fn single_clear_candidate_autocorrects_with_restored_case() {
        // One single-edit candidate, no near rival -> automatic.
        let decision = eval(&snapshot("Wrd"), false, &[lex("word", 1, 1000)]);
        match decision {
            CorrectionDecision::AutoCorrect { candidate } => {
                assert_eq!(candidate.replacement, "Word"); // capitalization reapplied
            }
            other => panic!("expected AutoCorrect, got {other:?}"),
        }
    }

    #[test]
    fn curated_rule_autocorrects() {
        // "teh" with no lexicon candidates still corrects via the curated rule.
        match eval(&snapshot("teh"), false, &[]) {
            CorrectionDecision::AutoCorrect { candidate } => {
                assert_eq!(candidate.replacement, "the")
            }
            other => panic!("expected AutoCorrect, got {other:?}"),
        }
    }

    #[test]
    fn several_near_rivals_are_suggested_not_autocorrected() {
        // Four equally-close, similarly-frequent candidates -> ambiguous.
        let decision = eval(
            &snapshot("caz"),
            false,
            &[
                lex("cat", 1, 500),
                lex("car", 1, 480),
                lex("can", 1, 470),
                lex("cap", 1, 460),
            ],
        );
        match decision {
            CorrectionDecision::Suggest { reason, .. } => {
                assert_eq!(reason, SuggestReason::AmbiguousMargin);
            }
            other => panic!("expected ambiguous Suggest, got {other:?}"),
        }
    }

    #[test]
    fn no_candidates_is_ignored_with_reason() {
        assert_eq!(
            eval(&snapshot("xyzzy"), false, &[]),
            CorrectionDecision::Ignore(IgnoreReason::NoCandidates)
        );
    }

    #[test]
    fn elapsed_deadline_makes_no_correction() {
        let decision = evaluate(
            &snapshot("teh"),
            false,
            &[],
            &RankWeights::default(),
            &ConfidencePolicy::default(),
            Some(Instant::now()),
        );
        assert_eq!(
            decision,
            CorrectionDecision::Ignore(IgnoreReason::DeadlineExceeded)
        );
    }
}
