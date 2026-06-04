//! Atomic local persistence of user-approved state under `%LOCALAPPDATA%\LangCheck`
//! (config, personal words, autocorrect/blocked pairs, excluded apps). Files are
//! rewritten atomically via a temporary file and replacement; malformed lines are
//! skipped and reported. No typing history is ever written (see `blueprint.md`
//! Sections 8.12 and 14).
//!
//! Implemented in delivery Steps 08 and 10.
