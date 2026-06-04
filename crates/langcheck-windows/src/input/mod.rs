//! Keyboard observation. Receives physical key events on a dedicated thread with
//! a Windows message loop, ignores LangCheck-injected events, drops everything
//! while `capture_allowed` is false, and pushes compact events into a bounded
//! queue — never doing language work in the callback (see `blueprint.md`
//! Sections 8.1 and 11.1).
//!
//! Implemented in delivery Step 01 (Windows Input and Focus Feasibility Spike).
