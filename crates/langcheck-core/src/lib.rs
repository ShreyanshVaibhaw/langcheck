//! `langcheck-core` — the platform-independent heart of LangCheck.
//!
//! This crate contains tokenization, token classification, candidate generation,
//! ranking, the confidence policy, the typing-session state machine, and undo
//! bookkeeping. It must remain free of Windows APIs, UI frameworks, filesystem
//! layout, and any concrete lexicon backend so the correction logic stays
//! deterministic and unit-testable on every platform (enforced in CI by building
//! and testing this crate on Linux).
//!
//! Real functionality arrives in later delivery steps (see `blueprint.md`
//! Section 24); at the bootstrap stage these modules are intentionally empty.
#![forbid(unsafe_code)]

pub mod candidate;
pub mod classify;
pub mod confidence;
pub mod rank;
pub mod session;
pub mod token;
pub mod undo;
