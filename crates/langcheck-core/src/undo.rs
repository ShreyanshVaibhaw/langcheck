//! Undo state machine: reverses the most recent automatic correction when the
//! user immediately rejects it, and suppresses the rejected pair for the session
//! (see `blueprint.md` Section 8.11).
//!
//! Implemented in delivery Step 10 (Immediate Undo and Personal Dictionary).
