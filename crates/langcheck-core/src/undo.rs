//! Immediate-undo state machine.
//!
//! Tracks at most one just-applied correction. If the very next *relevant* physical
//! input is Backspace in the same field, the correction is reversed and the pair is
//! suppressed for the session; any other input clears the pending undo. The time
//! window (default 2 s) is enforced by the coordinator, which clears a stale
//! pending correction before consulting this state machine (`blueprint.md`
//! Section 8.11). Platform-independent and unit-tested.
//!
//! Implemented in delivery Step 10 (Immediate Undo and Personal Dictionary).

use crate::session::Boundary;

/// A correction that was just applied and may be immediately undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCorrection {
    /// The field the correction was applied in.
    pub focus_id: u64,
    /// The original token (what undo restores).
    pub original: String,
    /// The replacement that is currently in the field.
    pub replacement: String,
    /// The boundary that followed the word.
    pub boundary: Boundary,
}

/// What to do with a pending undo when the next relevant input arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoDecision {
    /// Reverse this correction (and suppress the pair for the session).
    Undo(PendingCorrection),
    /// The pending correction is no longer eligible; it has been cleared.
    Cleared,
    /// Nothing was pending.
    Nothing,
}

/// Holds at most one immediately-undoable correction.
#[derive(Debug, Default)]
pub struct UndoState {
    pending: Option<PendingCorrection>,
}

impl UndoState {
    /// Create an empty undo state.
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// Record a freshly-applied correction as undoable.
    pub fn record(&mut self, correction: PendingCorrection) {
        self.pending = Some(correction);
    }

    /// Discard any pending correction (e.g. on timeout or focus change).
    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// Whether a correction is currently undoable.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Decide what to do for the next relevant input. An immediate Backspace in the
    /// same focus reverses the correction; any other input clears it. Either way the
    /// pending correction is consumed.
    pub fn on_next_input(&mut self, is_backspace: bool, focus_id: u64) -> UndoDecision {
        match self.pending.take() {
            Some(correction) if is_backspace && correction.focus_id == focus_id => {
                UndoDecision::Undo(correction)
            }
            Some(_) => UndoDecision::Cleared,
            None => UndoDecision::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correction(focus_id: u64) -> PendingCorrection {
        PendingCorrection {
            focus_id,
            original: "teh".to_owned(),
            replacement: "the".to_owned(),
            boundary: Boundary::Space,
        }
    }

    #[test]
    fn immediate_backspace_in_same_focus_undoes() {
        let mut undo = UndoState::new();
        undo.record(correction(7));
        assert!(undo.has_pending());
        match undo.on_next_input(true, 7) {
            UndoDecision::Undo(c) => assert_eq!(c.original, "teh"),
            other => panic!("expected Undo, got {other:?}"),
        }
        assert!(!undo.has_pending(), "pending consumed");
    }

    #[test]
    fn other_input_clears_pending() {
        let mut undo = UndoState::new();
        undo.record(correction(7));
        assert_eq!(undo.on_next_input(false, 7), UndoDecision::Cleared);
        assert!(!undo.has_pending());
    }

    #[test]
    fn backspace_in_a_different_focus_clears_not_undoes() {
        let mut undo = UndoState::new();
        undo.record(correction(7));
        assert_eq!(undo.on_next_input(true, 9), UndoDecision::Cleared);
    }

    #[test]
    fn nothing_pending_returns_nothing() {
        let mut undo = UndoState::new();
        assert_eq!(undo.on_next_input(true, 1), UndoDecision::Nothing);
    }
}
