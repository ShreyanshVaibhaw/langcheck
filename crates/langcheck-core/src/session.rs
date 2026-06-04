//! The versioned typing-session state machine.
//!
//! It consumes abstract, already-translated input events and produces versioned
//! [`WordSnapshot`]s at safe boundaries, resetting on focus changes, navigation,
//! paste, modifiers, composition, queue overflow, or pause (see `blueprint.md`
//! Sections 8.3, 6.3, 6.4).
//!
//! This module is platform-independent. The Windows input observer is
//! responsible for translating physical key events into [`SessionEvent`]s and
//! for gating capture by field safety (`capture_allowed`); the core never sees
//! virtual-key codes, scan codes, or field metadata.

use crate::classify::{classify_token_bounded, normalize_lookup, TokenClass};
use crate::token::{case_pattern, CasePattern, TokenState, DEFAULT_MAX_TOKEN_CHARS};

/// A safe word boundary after which a completed token may be committed (the MVP
/// set; see `blueprint.md` Section 6.3). Enter and Tab are deliberately excluded
/// because focus or application state may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    Space,
    Period,
    Comma,
    Question,
    Exclamation,
    Colon,
    Semicolon,
}

impl Boundary {
    /// The literal character this boundary represents (re-emitted after a
    /// correction so the user's punctuation is preserved).
    pub fn as_char(self) -> char {
        match self {
            Boundary::Space => ' ',
            Boundary::Period => '.',
            Boundary::Comma => ',',
            Boundary::Question => '?',
            Boundary::Exclamation => '!',
            Boundary::Colon => ':',
            Boundary::Semicolon => ';',
        }
    }

    /// Map a character to a safe boundary, if it is one.
    pub fn from_char(c: char) -> Option<Boundary> {
        Some(match c {
            ' ' => Boundary::Space,
            '.' => Boundary::Period,
            ',' => Boundary::Comma,
            '?' => Boundary::Question,
            '!' => Boundary::Exclamation,
            ':' => Boundary::Colon,
            ';' => Boundary::Semicolon,
            _ => return None,
        })
    }
}

/// Why a typing session was reset. Resets discard the active token (and, in later
/// steps, any in-flight decision) so that uncertain edits never produce a
/// correction (see `blueprint.md` Section 8.3). Tracked for redacted metrics
/// (Section 18) — never accompanied by token text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    FocusChange,
    QueueOverflow,
    MouseOrSelection,
    Navigation,
    Deletion,
    Newline,
    Tab,
    PasteOrCut,
    ModifierActive,
    Composition,
    UnknownTranslation,
    Paused,
    /// Backspace arrived with no active token — an edit reaching into text the
    /// session is not tracking. (In Step 10 an immediately-following undo
    /// intercepts Backspace before this reset.)
    BackspacePastToken,
}

/// An abstract, already-translated input event consumed by the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    /// A translated character was typed.
    Char(char),
    /// A safe boundary character was typed.
    Boundary(Boundary),
    /// Backspace was pressed.
    Backspace,
    /// Focus moved to the field identified by `id`; clears the active token.
    FocusChange(u64),
    /// Any other condition that must reset the session.
    Reset(ResetReason),
}

/// A versioned snapshot of a token, tagged with the focus and input identifiers
/// needed to detect stale work before committing a correction (see `blueprint.md`
/// Sections 8.3 and 8.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSnapshot {
    /// Identifier of the focused field this token was typed in.
    pub focus_id: u64,
    /// Monotonic physical-input generation at the time of this snapshot.
    pub generation: u64,
    /// Monotonic version that changes whenever the token's content changes.
    pub token_version: u64,
    /// The token exactly as typed.
    pub text: String,
    /// The token normalized for lexicon lookup (ASCII-lowercased).
    pub normalized: String,
    /// The token's case shape.
    pub case: CasePattern,
    /// The token's structural class.
    pub class: TokenClass,
}

impl WordSnapshot {
    /// Whether this word is eligible for *automatic* correction in the MVP (only
    /// a structurally clean [`TokenClass::NaturalWord`]).
    pub fn is_autocorrect_eligible(&self) -> bool {
        self.class.is_autocorrect_eligible()
    }
}

/// The result of processing a single [`SessionEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The active token grew or shrank; the snapshot may be pre-evaluated so a
    /// decision is ready by the time a boundary arrives.
    Building(WordSnapshot),
    /// A safe boundary completed the active token — the commit point.
    Completed {
        word: WordSnapshot,
        boundary: Boundary,
    },
    /// Nothing actionable (e.g. a boundary or backspace with no active token).
    Idle,
    /// The session reset; any in-flight token and decision must be discarded.
    Reset(ResetReason),
}

/// Tunable session bounds (see `blueprint.md` Section 8.13 `[performance]`).
#[derive(Debug, Clone, Copy)]
pub struct SessionConfig {
    /// Maximum characters retained for the active token.
    pub max_token_chars: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_token_chars: DEFAULT_MAX_TOKEN_CHARS,
        }
    }
}

/// The typing-session state machine. Single-writer; not internally synchronized
/// (owned by the coordinator thread; see `blueprint.md` Section 9).
#[derive(Debug, Clone)]
pub struct Session {
    config: SessionConfig,
    focus_id: u64,
    generation: u64,
    token_version: u64,
    token: TokenState,
}

impl Session {
    /// Create a fresh session with the given bounds.
    pub fn new(config: SessionConfig) -> Self {
        Self {
            token: TokenState::new(config.max_token_chars),
            config,
            focus_id: 0,
            generation: 0,
            token_version: 0,
        }
    }

    /// The id of the currently focused field.
    pub fn focus_id(&self) -> u64 {
        self.focus_id
    }

    /// The current physical-input generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The current token version.
    pub fn token_version(&self) -> u64 {
        self.token_version
    }

    /// The active token text (empty when idle).
    pub fn active_token(&self) -> &str {
        self.token.as_str()
    }

    /// Process one event and return the resulting outcome.
    ///
    /// Every call advances the physical-input generation by one, so a decision
    /// computed for generation `N` is committed only by the immediately following
    /// boundary at generation `N + 1` (see `blueprint.md` Section 8.9).
    pub fn handle(&mut self, event: SessionEvent) -> SessionOutcome {
        self.generation = self.generation.wrapping_add(1);
        match event {
            SessionEvent::Char(ch) => {
                self.token.push(ch);
                self.token_version = self.token_version.wrapping_add(1);
                SessionOutcome::Building(self.snapshot())
            }
            SessionEvent::Boundary(boundary) => {
                if self.token.is_empty() {
                    SessionOutcome::Idle
                } else {
                    let word = self.snapshot();
                    self.token.clear();
                    SessionOutcome::Completed { word, boundary }
                }
            }
            SessionEvent::Backspace => {
                if self.token.is_empty() {
                    SessionOutcome::Reset(ResetReason::BackspacePastToken)
                } else {
                    self.token.pop();
                    self.token_version = self.token_version.wrapping_add(1);
                    if self.token.is_empty() {
                        SessionOutcome::Idle
                    } else {
                        SessionOutcome::Building(self.snapshot())
                    }
                }
            }
            SessionEvent::FocusChange(new_id) => {
                self.focus_id = new_id;
                self.token.clear();
                SessionOutcome::Reset(ResetReason::FocusChange)
            }
            SessionEvent::Reset(reason) => {
                self.token.clear();
                SessionOutcome::Reset(reason)
            }
        }
    }

    fn snapshot(&self) -> WordSnapshot {
        let text = self.token.as_str().to_owned();
        let class = if self.token.overflowed() {
            TokenClass::TooLong
        } else {
            classify_token_bounded(&text, self.config.max_token_chars)
        };
        WordSnapshot {
            focus_id: self.focus_id,
            generation: self.generation,
            token_version: self.token_version,
            normalized: normalize_lookup(&text),
            case: case_pattern(&text),
            class,
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess() -> Session {
        Session::new(SessionConfig::default())
    }

    /// Feed a string, mapping safe-boundary characters to `Boundary` events and
    /// everything else to `Char` events.
    fn type_str(s: &mut Session, text: &str) -> Vec<SessionOutcome> {
        text.chars()
            .map(|c| match Boundary::from_char(c) {
                Some(b) => s.handle(SessionEvent::Boundary(b)),
                None => s.handle(SessionEvent::Char(c)),
            })
            .collect()
    }

    #[test]
    fn completes_word_at_space() {
        let mut s = sess();
        let outs = type_str(&mut s, "teh ");
        match outs.last().unwrap() {
            SessionOutcome::Completed { word, boundary } => {
                assert_eq!(word.text, "teh");
                assert_eq!(word.normalized, "teh");
                assert_eq!(*boundary, Boundary::Space);
                assert!(word.is_autocorrect_eligible());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        // The token buffer is cleared after completion.
        assert!(s.active_token().is_empty());
    }

    #[test]
    fn boundary_generation_follows_decision_generation() {
        let mut s = sess();
        s.handle(SessionEvent::Char('t'));
        s.handle(SessionEvent::Char('e'));
        let decision_gen = match s.handle(SessionEvent::Char('h')) {
            SessionOutcome::Building(w) => w.generation,
            other => panic!("expected Building, got {other:?}"),
        };
        assert_eq!(decision_gen, 3);
        match s.handle(SessionEvent::Boundary(Boundary::Space)) {
            // The decision tagged at generation N is committed by the boundary at N+1.
            SessionOutcome::Completed { word, .. } => assert_eq!(word.generation, decision_gen + 1),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn focus_change_resets_and_updates_focus() {
        let mut s = sess();
        type_str(&mut s, "teh");
        assert_eq!(
            s.handle(SessionEvent::FocusChange(42)),
            SessionOutcome::Reset(ResetReason::FocusChange)
        );
        assert_eq!(s.focus_id(), 42);
        assert!(s.active_token().is_empty());
    }

    #[test]
    fn reset_clears_token_then_next_word_builds_fresh() {
        let mut s = sess();
        type_str(&mut s, "wrod");
        assert_eq!(
            s.handle(SessionEvent::Reset(ResetReason::Navigation)),
            SessionOutcome::Reset(ResetReason::Navigation)
        );
        assert!(s.active_token().is_empty());
        let outs = type_str(&mut s, "teh ");
        assert!(matches!(
            outs.last().unwrap(),
            SessionOutcome::Completed { .. }
        ));
    }

    #[test]
    fn boundary_with_empty_token_is_idle() {
        let mut s = sess();
        assert_eq!(
            s.handle(SessionEvent::Boundary(Boundary::Space)),
            SessionOutcome::Idle
        );
    }

    #[test]
    fn backspace_edits_then_resets_past_token() {
        let mut s = sess();
        type_str(&mut s, "ab");
        assert!(matches!(
            s.handle(SessionEvent::Backspace),
            SessionOutcome::Building(_)
        ));
        assert_eq!(s.active_token(), "a");
        assert_eq!(s.handle(SessionEvent::Backspace), SessionOutcome::Idle);
        assert_eq!(
            s.handle(SessionEvent::Backspace),
            SessionOutcome::Reset(ResetReason::BackspacePastToken)
        );
    }

    #[test]
    fn token_is_bounded_and_marked_too_long() {
        let mut s = sess();
        let outs = type_str(&mut s, &"a".repeat(100));
        assert!(s.active_token().chars().count() <= SessionConfig::default().max_token_chars);
        match outs.last().unwrap() {
            SessionOutcome::Building(w) => {
                assert_eq!(w.class, TokenClass::TooLong);
                assert!(!w.is_autocorrect_eligible());
            }
            other => panic!("expected Building, got {other:?}"),
        }
    }

    #[test]
    fn clean_lowercase_words_of_every_length_are_eligible() {
        let max = SessionConfig::default().max_token_chars;
        for len in MIN_NATURAL_WORD_LEN_FOR_TEST..=max {
            let mut s = sess();
            for c in "a".repeat(len).chars() {
                s.handle(SessionEvent::Char(c));
            }
            match s.handle(SessionEvent::Boundary(Boundary::Period)) {
                SessionOutcome::Completed { word, .. } => {
                    assert_eq!(word.class, TokenClass::NaturalWord, "len {len}");
                    assert!(word.is_autocorrect_eligible());
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }
    }

    const MIN_NATURAL_WORD_LEN_FOR_TEST: usize = 3;
}

/// Deterministic randomized invariant ("fuzz") tests: arbitrary event streams,
/// including arbitrary Unicode, must never panic or break the session's bounds
/// and versioning invariants (see `blueprint.md` Section 19.2).
#[cfg(test)]
mod fuzz {
    use super::*;

    /// Small reproducible PRNG (xorshift64*), so CI failures are deterministic.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u32) -> u32 {
            (self.next_u64() % u64::from(n)) as u32
        }
    }

    fn random_event(rng: &mut Rng) -> SessionEvent {
        match rng.below(7) {
            0..=2 => SessionEvent::Char(random_char(rng)),
            3 => {
                let bs = [
                    Boundary::Space,
                    Boundary::Period,
                    Boundary::Comma,
                    Boundary::Question,
                    Boundary::Exclamation,
                    Boundary::Colon,
                    Boundary::Semicolon,
                ];
                SessionEvent::Boundary(bs[rng.below(bs.len() as u32) as usize])
            }
            4 => SessionEvent::Backspace,
            5 => SessionEvent::FocusChange(rng.next_u64()),
            _ => {
                let rs = [
                    ResetReason::QueueOverflow,
                    ResetReason::MouseOrSelection,
                    ResetReason::Navigation,
                    ResetReason::Deletion,
                    ResetReason::Newline,
                    ResetReason::Tab,
                    ResetReason::PasteOrCut,
                    ResetReason::ModifierActive,
                    ResetReason::Composition,
                    ResetReason::UnknownTranslation,
                    ResetReason::Paused,
                ];
                SessionEvent::Reset(rs[rng.below(rs.len() as u32) as usize])
            }
        }
    }

    fn random_char(rng: &mut Rng) -> char {
        match rng.below(10) {
            0..=5 => {
                let base = if rng.below(2) == 0 { b'a' } else { b'A' };
                (base + rng.below(26) as u8) as char
            }
            6..=7 => {
                // Arbitrary Unicode scalar value (skip the surrogate range).
                let mut cp = rng.below(0x0011_0000);
                while (0xD800..=0xDFFF).contains(&cp) {
                    cp = rng.below(0x0011_0000);
                }
                char::from_u32(cp).unwrap_or('a')
            }
            _ => {
                let syms = b"_/@\\.#!-'";
                syms[rng.below(syms.len() as u32) as usize] as char
            }
        }
    }

    #[test]
    fn arbitrary_event_streams_stay_bounded_and_never_panic() {
        let cfg = SessionConfig::default();
        let mut rng = Rng(0x1234_5678_9ABC_DEF0);
        for _ in 0..2_000 {
            let mut s = Session::new(cfg);
            let mut prev_gen = s.generation();
            let mut prev_ver = s.token_version();
            let steps = 1 + rng.below(256);
            for _ in 0..steps {
                let event = random_event(&mut rng);
                let resets = matches!(event, SessionEvent::Reset(_) | SessionEvent::FocusChange(_));
                let out = s.handle(event);

                // Generation advances by exactly one per event.
                assert_eq!(s.generation(), prev_gen + 1);
                // Token version never decreases.
                assert!(s.token_version() >= prev_ver);
                // The active token never exceeds the configured bound.
                assert!(s.active_token().chars().count() <= cfg.max_token_chars);
                // Resets always clear the token and report a reset.
                if resets {
                    assert!(s.active_token().is_empty());
                    assert!(matches!(out, SessionOutcome::Reset(_)));
                }
                // Any snapshot is likewise bounded.
                match &out {
                    SessionOutcome::Building(w) | SessionOutcome::Completed { word: w, .. } => {
                        assert!(w.text.chars().count() <= cfg.max_token_chars);
                    }
                    _ => {}
                }

                prev_gen = s.generation();
                prev_ver = s.token_version();
            }
        }
    }
}
