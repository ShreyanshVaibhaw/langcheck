//! `langcheck-lexicon` — dictionary lookup behind a small trait.
//!
//! Release builds use LangCheck's own bundled, read-only, memory-mapped compact
//! FST lexicon. The runtime never sends words to a system, third-party, or remote
//! spell provider — that is what guarantees typed words never leave the device
//! (see `blueprint.md` Section 8.5 and ADR-004).
//!
//! Real functionality arrives in delivery Steps 03 (prototype FST) and 09
//! (production lexicon). `unsafe` is denied except at the reviewed memory-mapping
//! boundary introduced in Step 03.
#![deny(unsafe_code)]

pub mod compact_fst;
pub mod personal;

// Rejected runtime backend, retained only as an isolated developer benchmark and
// gated behind a non-default feature so it can never ship (blueprint Section 8.5).
#[cfg(feature = "dev-windows-spell")]
pub mod windows_spell;
