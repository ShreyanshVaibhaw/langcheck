//! Bundled, read-only, memory-mapped compact FST lexicon — the release backend.
//!
//! The word list and frequency data are compiled (by `tools/dictionary-compiler`)
//! into a finite-state transducer that is memory-mapped rather than deserialized
//! into a large heap structure, giving deterministic latency and memory behavior
//! (see `blueprint.md` Section 8.5 and ADR-004).
//!
//! Implemented in delivery Step 03 (prototype) and Step 09 (production lexicon).
