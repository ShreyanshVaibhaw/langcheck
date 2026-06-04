//! `langcheck-core` — the platform-independent heart of LangCheck.
//!
//! This crate contains tokenization, token classification, candidate generation,
//! ranking, the confidence policy, the typing-session state machine, and undo
//! bookkeeping. It must remain free of Windows APIs, UI frameworks, filesystem
//! layout, and any concrete lexicon backend so the correction logic stays
//! deterministic and unit-testable on every platform (enforced in CI by building
//! and testing this crate on Linux).
//!
//! Implemented so far (delivery Step 02): tokenization and casing ([`token`]),
//! token classification and lookup normalization ([`classify`]), and the
//! versioned typing-session state machine ([`session`]). Candidate generation,
//! ranking, the confidence policy, and undo arrive in later steps (see
//! `blueprint.md` Section 24).
#![forbid(unsafe_code)]

pub mod candidate;
pub mod classify;
pub mod confidence;
pub mod rank;
pub mod session;
pub mod token;
pub mod undo;

// Convenience re-exports of the most commonly used types.
pub use classify::{classify_token, TokenClass};
pub use session::{
    Boundary, ResetReason, Session, SessionConfig, SessionEvent, SessionOutcome, WordSnapshot,
};
pub use token::{case_pattern, restore_case, CasePattern};
