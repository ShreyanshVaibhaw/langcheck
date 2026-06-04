//! Replacement executor. Applies a correction with a single `SendInput` batch
//! (backspaces + corrected word + original boundary), tags injected events via
//! `dwExtraInfo` so the observer ignores them, never uses the clipboard, and never
//! targets a higher-integrity process (see `blueprint.md` Sections 8.10 and 11).
//!
//! Implemented in delivery Step 05 (Safe Replacement Executor).
