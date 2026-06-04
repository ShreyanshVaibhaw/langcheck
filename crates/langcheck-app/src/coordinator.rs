//! Session coordinator: wires the input observer, focus cache, session state,
//! correction engine, decision cache, commit checks, and replacement executor
//! using bounded queues and a small fixed set of threads (see `blueprint.md`
//! Sections 8.3, 8.9, and 9).
//!
//! Implemented in delivery Step 06 (End-to-End Conservative Autocorrect).
