//! Curated corpus and precision gate for the correction engine (blueprint.md
//! Sections 5.1, 19.3). Runs the full evaluate() pipeline over labelled examples
//! using the deterministic `FakeLexicon` so dictionary contents never make the
//! test flaky, and asserts the precision-over-recall contract:
//!
//! - common misspellings autocorrect to the expected word;
//! - correctly-spelled words are left alone (Known);
//! - ambiguous typos are suggested or ignored, never autocorrected;
//! - non-prose tokens are ineligible;
//! - unknown words with no close match invent nothing.
//!
//! The headline gate: zero harmful autocorrections (precision == 100%).

use langcheck_core::candidate::{CandidateSource, CandidateWord};
use langcheck_core::classify::{classify_token, normalize_lookup};
use langcheck_core::token::case_pattern;
use langcheck_core::{evaluate, ConfidencePolicy, CorrectionDecision, RankWeights, WordSnapshot};
use langcheck_lexicon::fake::FakeLexicon;
use langcheck_lexicon::{LanguageTag, LexiconProvider};

/// Build a snapshot exactly as the session would classify a typed token.
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

/// A controlled vocabulary covering every corpus case (including a deliberately
/// confusable `cat/car/can/cap/cab` cluster).
fn corpus_lexicon() -> FakeLexicon {
    const WORDS: &[(&str, u32)] = &[
        ("the", 1000),
        ("word", 600),
        ("world", 550),
        ("hello", 300),
        ("friend", 200),
        ("receive", 180),
        ("separate", 120),
        ("weird", 110),
        ("until", 130),
        ("believe", 150),
        ("because", 250),
        ("their", 400),
        ("which", 410),
        ("azure", 40),
        ("ten", 50),
        ("cat", 500),
        ("car", 480),
        ("can", 470),
        ("cap", 460),
        ("cab", 450),
    ];
    FakeLexicon::new(
        LanguageTag::EnUs,
        WORDS.iter().map(|(w, f)| ((*w).to_string(), *f)),
    )
}

/// Evaluate one token through the full pipeline backed by `lexicon`.
fn evaluate_token(lexicon: &FakeLexicon, text: &str) -> CorrectionDecision {
    let snap = snapshot(text);
    let known = lexicon.contains(LanguageTag::EnUs, text);
    let lexicon_candidates: Vec<CandidateWord> = lexicon
        .candidates(LanguageTag::EnUs, text, 8, None)
        .unwrap_or_default()
        .into_iter()
        .map(|c| CandidateWord {
            word: c.word,
            edit_distance: c.edit_distance,
            frequency: c.frequency,
            source: CandidateSource::Lexicon,
        })
        .collect();
    evaluate(
        &snap,
        known,
        &lexicon_candidates,
        &RankWeights::default(),
        &ConfidencePolicy::default(),
        None,
    )
}

fn is_autocorrect(decision: &CorrectionDecision) -> bool {
    matches!(decision, CorrectionDecision::AutoCorrect { .. })
}

#[test]
fn corpus_precision_and_behavior() {
    let lexicon = corpus_lexicon();

    // (typo, expected correction) — must autocorrect.
    let should_autocorrect = [
        ("teh", "the"), // curated rule + transposition
        ("recieve", "receive"),
        ("seperate", "separate"),
        ("freind", "friend"),
        ("wierd", "weird"),
        ("wrd", "word"), // lexicon single-edit insertion
        ("helo", "hello"),
        ("frend", "friend"),
    ];
    // Correctly-spelled words — must be left alone (Known).
    let known_words = ["hello", "friend", "the", "world", "because", "azure"];
    // Ambiguous typos (several near rivals) — must NOT autocorrect.
    let ambiguous = ["caz", "cas"];
    // Non-prose tokens — ineligible.
    let non_prose = ["user@host", "src/main", "foo_bar", "h3llo"];
    // Real-but-unknown word with no close match — must invent nothing.
    let no_match = ["fjord"];

    let mut autocorrections = 0usize;
    let mut harmful = Vec::new();
    let mut recalled = 0usize;

    for (typo, expected) in should_autocorrect {
        let decision = evaluate_token(&lexicon, typo);
        if let CorrectionDecision::AutoCorrect { candidate } = &decision {
            autocorrections += 1;
            recalled += 1;
            assert_eq!(
                candidate.replacement, expected,
                "'{typo}' corrected to '{}', expected '{expected}'",
                candidate.replacement
            );
        } else {
            panic!("'{typo}' should autocorrect to '{expected}', got {decision:?}");
        }
    }

    for word in known_words {
        let decision = evaluate_token(&lexicon, word);
        assert_eq!(
            decision,
            CorrectionDecision::Known,
            "'{word}' should be Known"
        );
        if is_autocorrect(&decision) {
            autocorrections += 1;
            harmful.push(word.to_string());
        }
    }

    for typo in ambiguous {
        let decision = evaluate_token(&lexicon, typo);
        assert!(
            !is_autocorrect(&decision),
            "ambiguous '{typo}' must not autocorrect, got {decision:?}"
        );
    }

    for token in non_prose {
        let decision = evaluate_token(&lexicon, token);
        assert!(
            matches!(decision, CorrectionDecision::Ignore(_)),
            "non-prose '{token}' must be ignored, got {decision:?}"
        );
        if is_autocorrect(&decision) {
            autocorrections += 1;
            harmful.push(token.to_string());
        }
    }

    for token in no_match {
        let decision = evaluate_token(&lexicon, token);
        assert!(
            !is_autocorrect(&decision),
            "unknown '{token}' with no close match must not autocorrect, got {decision:?}"
        );
    }

    let precision = if autocorrections == 0 {
        1.0
    } else {
        (autocorrections - harmful.len()) as f64 / autocorrections as f64
    };
    let recall = recalled as f64 / should_autocorrect.len() as f64;
    println!(
        "corpus: {autocorrections} autocorrections, {} harmful; precision={:.3}, recall={:.3}",
        harmful.len(),
        precision,
        recall
    );

    // Precision-over-recall: no harmful autocorrection is tolerated on the corpus.
    assert!(harmful.is_empty(), "harmful autocorrections: {harmful:?}");
    assert_eq!(precision, 1.0);
    assert_eq!(recall, 1.0, "every curated misspelling should be corrected");
}
