//! Session coordinator: the end-to-end correction loop.
//!
//! Runs on a dedicated thread, draining the observer's bounded queue. For each key
//! event it (1) reconciles focus, (2) translates the key, (3) drives the core
//! session state machine, and (4) on a completed word at a safe boundary, runs the
//! final safety checks and — only if they all pass — evaluates the engine and
//! applies a high-confidence correction via the replacement executor.
//!
//! Stale work is cancelled rather than applied: a correction commits only if the
//! boundary is still the most recent physical input (no newer keystroke has been
//! queued), focus is unchanged, capture is still allowed, and LangCheck is enabled
//! and not paused (`blueprint.md` Sections 8.3, 8.9, 10). The pure [`commit_gate`]
//! encodes those checks and is unit-tested.
//!
//! Implemented in delivery Step 06 (End-to-End Conservative Autocorrect).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use langcheck_core::classify::normalize_lookup;
use langcheck_core::{
    evaluate, CandidateSource, CandidateWord, ConfidencePolicy, CorrectionDecision,
    PendingCorrection, RankWeights, Session, SessionConfig, SessionEvent, SessionOutcome,
    UndoDecision, UndoState, WordSnapshot,
};
use langcheck_lexicon::{LanguageTag, LexiconProvider, PersonalDictionary, MAX_LEXICON_CANDIDATES};
use langcheck_windows::input::translate::{KeyTranslator, Translated};
use langcheck_windows::input::{self, InputEvent};
use langcheck_windows::replace::{ReplacementExecutor, ReplacementPlan, SendInputExecutor};

use crate::diagnostics::Metrics;

/// Per-request engine deadline (`blueprint.md` Section 8.13 `[performance]`).
const DECISION_DEADLINE_MS: u64 = 15;

/// State shared between the focus thread (writer of `focus_id`), the coordinator,
/// and the UI/main thread (the enable/pause kill switch).
#[derive(Debug)]
pub struct SharedState {
    /// Coarse identity of the focused window (foreground HWND). Focus thread writes.
    pub focus_id: AtomicU64,
    /// Global kill switch.
    pub enabled: AtomicBool,
    /// Pause (global or per-app); suspends correction without disabling.
    pub paused: AtomicBool,
    /// Shutdown signal for the coordinator and focus threads.
    pub shutdown: AtomicBool,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            focus_id: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Inputs to the final commit decision (all already-read booleans), so the gate is
/// pure and testable.
#[derive(Debug, Clone, Copy)]
pub struct CommitContext {
    pub enabled: bool,
    pub paused: bool,
    pub eligible: bool,
    pub capture_allowed: bool,
    pub focus_unchanged: bool,
    pub generation_fresh: bool,
}

/// Why a commit was cancelled (redacted; for metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    Disabled,
    Paused,
    NotEligible,
    Unsafe,
    FocusChanged,
    StaleGeneration,
}

/// The result of the final commit gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitGate {
    Proceed,
    Cancel(CancelReason),
}

/// Final safety checks before evaluating/replacing (`blueprint.md` Section 8.9).
/// Pure: every input is a boolean read by the caller.
pub fn commit_gate(ctx: &CommitContext) -> CommitGate {
    if !ctx.enabled {
        return CommitGate::Cancel(CancelReason::Disabled);
    }
    if ctx.paused {
        return CommitGate::Cancel(CancelReason::Paused);
    }
    if !ctx.eligible {
        return CommitGate::Cancel(CancelReason::NotEligible);
    }
    if !ctx.capture_allowed {
        return CommitGate::Cancel(CancelReason::Unsafe);
    }
    if !ctx.focus_unchanged {
        return CommitGate::Cancel(CancelReason::FocusChanged);
    }
    if !ctx.generation_fresh {
        return CommitGate::Cancel(CancelReason::StaleGeneration);
    }
    CommitGate::Proceed
}

/// The end-to-end coordinator. Owns the session, translator, lexicon, and executor;
/// single-threaded on its own thread.
pub struct Coordinator {
    session: Session,
    translator: KeyTranslator,
    lexicon: Box<dyn LexiconProvider>,
    personal: PersonalDictionary,
    executor: SendInputExecutor,
    weights: RankWeights,
    policy: ConfidencePolicy,
    shared: Arc<SharedState>,
    metrics: Arc<Metrics>,
    undo: UndoState,
    undo_recorded_at: Option<Instant>,
    undo_window: Duration,
    /// Pairs rejected via undo this session; never re-applied until restart.
    session_blocklist: HashSet<(String, String)>,
}

impl Coordinator {
    pub fn new(
        lexicon: Box<dyn LexiconProvider>,
        personal: PersonalDictionary,
        undo_window: Duration,
        shared: Arc<SharedState>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            session: Session::new(SessionConfig::default()),
            translator: KeyTranslator::default(),
            lexicon,
            personal,
            executor: SendInputExecutor,
            weights: RankWeights::default(),
            policy: ConfidencePolicy::default(),
            shared,
            metrics,
            undo: UndoState::new(),
            undo_recorded_at: None,
            undo_window,
            session_blocklist: HashSet::new(),
        }
    }

    /// Drain and process events until shutdown is signalled or the channel
    /// disconnects. Polls with a short timeout so shutdown is responsive even
    /// though the observer's sender lives for the process lifetime.
    pub fn run(&mut self, events: &Receiver<InputEvent>) {
        while !self.shared.is_shutdown() {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => self.process(&event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn process(&mut self, event: &InputEvent) {
        // Reconcile focus: a foreground change resets the session and clears undo.
        let current_focus = self.shared.focus_id.load(Ordering::SeqCst);
        if current_focus != self.session.focus_id() {
            self.session
                .handle(SessionEvent::FocusChange(current_focus));
            Metrics::inc(&self.metrics.sessions_reset);
            self.clear_undo();
        }

        let translated = self.translator.translate(event);
        if matches!(translated, Translated::Ignore) {
            return; // modifier/ignored key: no token effect, not a "relevant" input
        }

        // Immediate-undo handling on the first relevant input after a correction.
        if self.undo.has_pending() {
            let expired = self
                .undo_recorded_at
                .is_some_and(|recorded| recorded.elapsed() > self.undo_window);
            if expired {
                self.clear_undo();
            } else {
                let is_backspace = matches!(translated, Translated::Backspace);
                match self.undo.on_next_input(is_backspace, current_focus) {
                    UndoDecision::Undo(correction) => {
                        self.perform_undo(&correction);
                        self.undo_recorded_at = None;
                        return; // consume the Backspace
                    }
                    UndoDecision::Cleared => self.undo_recorded_at = None,
                    UndoDecision::Nothing => {}
                }
            }
        }

        let session_event = match translated {
            Translated::Char(c) => SessionEvent::Char(c),
            Translated::Boundary(b) => SessionEvent::Boundary(b),
            Translated::Backspace => SessionEvent::Backspace,
            Translated::Reset(reason) => SessionEvent::Reset(reason),
            Translated::Ignore => return,
        };
        Metrics::inc(&self.metrics.events_translated);

        match self.session.handle(session_event) {
            SessionOutcome::Completed { word, boundary } => {
                self.try_commit(&word, boundary, event.generation);
            }
            SessionOutcome::Reset(_) => {
                Metrics::inc(&self.metrics.sessions_reset);
                self.clear_undo();
            }
            SessionOutcome::Building(_) | SessionOutcome::Idle => {}
        }
    }

    fn clear_undo(&mut self) {
        self.undo.clear();
        self.undo_recorded_at = None;
    }

    /// Reverse a just-applied correction and suppress the pair for the session.
    fn perform_undo(&mut self, correction: &PendingCorrection) {
        match self.executor.execute_undo(
            &correction.original,
            &correction.replacement,
            correction.boundary,
        ) {
            Ok(()) => {
                self.session_blocklist.insert((
                    normalize_lookup(&correction.original),
                    normalize_lookup(&correction.replacement),
                ));
                Metrics::inc(&self.metrics.corrections_undone);
            }
            Err(_) => Metrics::inc(&self.metrics.replace_failures),
        }
    }

    fn try_commit(
        &mut self,
        word: &WordSnapshot,
        boundary: langcheck_core::Boundary,
        boundary_gen: u64,
    ) {
        let ctx = CommitContext {
            enabled: self.shared.enabled(),
            paused: self.shared.paused(),
            eligible: word.is_autocorrect_eligible(),
            capture_allowed: input::capture_allowed(),
            focus_unchanged: self.shared.focus_id.load(Ordering::SeqCst) == word.focus_id,
            generation_fresh: input::generation() == boundary_gen,
        };
        if let CommitGate::Cancel(_) = commit_gate(&ctx) {
            Metrics::inc(&self.metrics.commits_cancelled);
            return;
        }

        let deadline = Some(Instant::now() + Duration::from_millis(DECISION_DEADLINE_MS));
        let normalized = word.normalized.clone();

        // A user-forced pair overrides "known"-ness; otherwise a personal word or a
        // dictionary word is treated as already correct and left alone.
        let forced = self
            .personal
            .forced_correction(&normalized)
            .map(str::to_owned);
        let is_known = forced.is_none()
            && (self.lexicon.contains(LanguageTag::EnUs, &normalized)
                || self.personal.contains_word(&normalized));

        let mut candidates: Vec<CandidateWord> = self
            .lexicon
            .candidates(
                LanguageTag::EnUs,
                &normalized,
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

        let decision = evaluate(
            word,
            is_known,
            &candidates,
            &self.weights,
            &self.policy,
            deadline,
        );
        match &decision {
            CorrectionDecision::Known => Metrics::inc(&self.metrics.known),
            CorrectionDecision::Ignore(_) => Metrics::inc(&self.metrics.ignored),
            CorrectionDecision::Suggest { .. } => Metrics::inc(&self.metrics.suggested),
            CorrectionDecision::AutoCorrect { .. } => Metrics::inc(&self.metrics.autocorrected),
        }

        let CorrectionDecision::AutoCorrect { candidate } = decision else {
            return;
        };

        // Honour blocked pairs and session suppression (pairs rejected via undo).
        let pair = (normalized, normalize_lookup(&candidate.replacement));
        if self.personal.is_blocked(&pair.0, &pair.1) || self.session_blocklist.contains(&pair) {
            Metrics::inc(&self.metrics.commits_cancelled);
            return;
        }

        // Re-check freshness after evaluation (which may have taken a moment): if
        // any newer physical input arrived, cancel rather than apply stale work.
        if input::generation() != boundary_gen {
            Metrics::inc(&self.metrics.commits_cancelled);
            return;
        }

        let plan = ReplacementPlan {
            focus_id: word.focus_id,
            expected_generation: boundary_gen,
            original: candidate.original,
            replacement: candidate.replacement,
            boundary,
        };
        // Partial/failed insertion is counted but never retried blindly.
        match self.executor.execute(&plan) {
            Ok(_undo) => {
                Metrics::inc(&self.metrics.corrections_applied);
                self.undo.record(PendingCorrection {
                    focus_id: plan.focus_id,
                    original: plan.original.clone(),
                    replacement: plan.replacement.clone(),
                    boundary,
                });
                self.undo_recorded_at = Some(Instant::now());
            }
            Err(_) => Metrics::inc(&self.metrics.replace_failures),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CommitContext {
        CommitContext {
            enabled: true,
            paused: false,
            eligible: true,
            capture_allowed: true,
            focus_unchanged: true,
            generation_fresh: true,
        }
    }

    #[test]
    fn all_checks_pass_proceeds() {
        assert_eq!(commit_gate(&ctx()), CommitGate::Proceed);
    }

    #[test]
    fn each_failing_check_cancels_with_reason() {
        assert_eq!(
            commit_gate(&CommitContext {
                enabled: false,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::Disabled)
        );
        assert_eq!(
            commit_gate(&CommitContext {
                paused: true,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::Paused)
        );
        assert_eq!(
            commit_gate(&CommitContext {
                eligible: false,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::NotEligible)
        );
        assert_eq!(
            commit_gate(&CommitContext {
                capture_allowed: false,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::Unsafe)
        );
        assert_eq!(
            commit_gate(&CommitContext {
                focus_unchanged: false,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::FocusChanged)
        );
        assert_eq!(
            commit_gate(&CommitContext {
                generation_fresh: false,
                ..ctx()
            }),
            CommitGate::Cancel(CancelReason::StaleGeneration)
        );
    }
}
