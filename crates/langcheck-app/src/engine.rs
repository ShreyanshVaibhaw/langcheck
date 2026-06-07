//! Shared correction-decision logic and the IPC evaluation handler.
//!
//! The engine *decision* — assemble candidates, apply the confidence policy — is
//! identical whether it is reached from the keystroke path (the coordinator) or
//! the TSF adapter's single-token IPC path. [`decide`] is that shared core, so the
//! two paths can never diverge. [`evaluate_request`] adapts it to the IPC
//! [`Request`]/[`Response`] protocol; it is the broker-side handler, keeping the
//! broker the only process that holds language logic (`blueprint.md` §7.1, 11.4).

use std::time::Instant;

use langcheck_core::classify::normalize_lookup;
use langcheck_core::ipc::{Request, Response};
use langcheck_core::{
    case_pattern, classify_token, evaluate, Boundary, CandidateSource, CandidateWord,
    ConfidencePolicy, CorrectionDecision, RankWeights, WordSnapshot,
};
use langcheck_lexicon::{LanguageTag, LexiconProvider, PersonalDictionary, MAX_LEXICON_CANDIDATES};

/// The engine decision for an already-snapshotted word: assemble candidates
/// (lexicon, curated rules, user-forced) and apply the confidence policy. A
/// user-forced pair overrides "known"-ness; otherwise a dictionary or personal
/// word is left alone.
///
/// Shared by the coordinator (keystroke path) and the TSF IPC handler so they
/// cannot diverge. Pure aside from the (immutable) lexicon/personal-dictionary
/// lookups; the caller owns metrics, blocked-pair checks, freshness, and replacement.
pub fn decide(
    word: &WordSnapshot,
    lexicon: &dyn LexiconProvider,
    personal: &PersonalDictionary,
    weights: &RankWeights,
    policy: &ConfidencePolicy,
    deadline: Option<Instant>,
) -> CorrectionDecision {
    let normalized = &word.normalized;
    let forced = personal.forced_correction(normalized).map(str::to_owned);
    let is_known = forced.is_none()
        && (lexicon.contains(LanguageTag::EnUs, normalized) || personal.contains_word(normalized));

    let mut candidates: Vec<CandidateWord> = lexicon
        .candidates(
            LanguageTag::EnUs,
            normalized,
            MAX_LEXICON_CANDIDATES,
            deadline,
        )
        .unwrap_or_default()
        .into_iter()
        .map(|c| CandidateWord {
            word: c.word,
            edit_distance: c.edit_distance,
            frequency: c.frequency,
            source: CandidateSource::Lexicon,
        })
        .collect();
    if let Some(forced) = &forced {
        candidates.push(CandidateWord {
            word: forced.clone(),
            edit_distance: 1,
            frequency: u32::MAX,
            source: CandidateSource::UserPair,
        });
    }

    evaluate(word, is_known, &candidates, weights, policy, deadline)
}

/// Build a [`WordSnapshot`] from a raw token. The TSF adapter has no session state
/// machine, so the focus/generation/version fields are not meaningful here (the
/// adapter enforces its own freshness in-process) and are zeroed.
pub fn snapshot_for_token(token: &str) -> WordSnapshot {
    WordSnapshot {
        focus_id: 0,
        generation: 0,
        token_version: 0,
        text: token.to_owned(),
        normalized: normalize_lookup(token),
        case: case_pattern(token),
        class: classify_token(token),
    }
}

/// The broker-side IPC handler. Answers a `Ping` and, for an `Evaluate`, runs the
/// shared engine [`decide`] over the token and maps the result to a [`Response`].
///
/// Honours blocked pairs (a blocked correction becomes [`Response::Leave`]).
/// Non-eligible tokens (not a clean natural word) and anything short of an
/// auto-correct decision are left unchanged — the conservative default.
pub fn evaluate_request(
    request: Request,
    lexicon: &dyn LexiconProvider,
    personal: &PersonalDictionary,
    weights: &RankWeights,
    policy: &ConfidencePolicy,
) -> Response {
    let (token, _boundary): (String, Boundary) = match request {
        // Liveness and the focus beacon are both simple acknowledgements; the broker
        // records the beacon as activity (see `tsf_broker`) so the MVP path defers.
        Request::Ping | Request::Active => return Response::Pong,
        Request::Evaluate { token, boundary } => (token, boundary),
    };

    let word = snapshot_for_token(&token);
    if !word.is_autocorrect_eligible() {
        return Response::Leave;
    }
    match decide(&word, lexicon, personal, weights, policy, None) {
        CorrectionDecision::AutoCorrect { candidate } => {
            let replacement_key = normalize_lookup(&candidate.replacement);
            if personal.is_blocked(&word.normalized, &replacement_key) {
                Response::Leave
            } else {
                Response::Replace {
                    replacement: candidate.replacement,
                }
            }
        }
        _ => Response::Leave,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use langcheck_lexicon::compact_fst::CompactFstLexicon;

    fn lexicon() -> CompactFstLexicon<&'static [u8]> {
        CompactFstLexicon::production_en_us().expect("production lexicon")
    }

    fn eval(token: &str) -> Response {
        evaluate_request(
            Request::Evaluate {
                token: token.to_owned(),
                boundary: Boundary::Space,
            },
            &lexicon(),
            &PersonalDictionary::default(),
            &RankWeights::default(),
            &ConfidencePolicy::default(),
        )
    }

    #[test]
    fn ping_is_answered() {
        let response = evaluate_request(
            Request::Ping,
            &lexicon(),
            &PersonalDictionary::default(),
            &RankWeights::default(),
            &ConfidencePolicy::default(),
        );
        assert_eq!(response, Response::Pong);
    }

    #[test]
    fn curated_typo_is_corrected() {
        // A curated rule corrects this even though "wierd" is an archaic dictionary
        // entry — the same override the coordinator relies on.
        assert_eq!(
            eval("wierd"),
            Response::Replace {
                replacement: "weird".to_owned()
            }
        );
        assert_eq!(
            eval("teh"),
            Response::Replace {
                replacement: "the".to_owned()
            }
        );
    }

    #[test]
    fn known_word_is_left_alone() {
        assert_eq!(eval("the"), Response::Leave);
        assert_eq!(eval("language"), Response::Leave);
    }

    #[test]
    fn non_prose_token_is_left_alone() {
        // Not a clean natural word ⇒ never auto-corrected.
        assert_eq!(eval("http://x"), Response::Leave);
        assert_eq!(eval("v2.0"), Response::Leave);
    }

    #[test]
    fn blocked_pair_is_left_alone() {
        let mut personal = PersonalDictionary::default();
        personal.block_pair("wierd", "weird");
        let response = evaluate_request(
            Request::Evaluate {
                token: "wierd".to_owned(),
                boundary: Boundary::Space,
            },
            &lexicon(),
            &personal,
            &RankWeights::default(),
            &ConfidencePolicy::default(),
        );
        assert_eq!(response, Response::Leave);
    }
}
