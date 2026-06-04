//! The versioned typing-session state machine: maintains the active token and
//! input generation, and resets on focus change, navigation, paste, modifiers,
//! or any uncertainty (see `blueprint.md` Section 8.3).
//!
//! Implemented in delivery Step 02 (Core Token and Session State Machine).
