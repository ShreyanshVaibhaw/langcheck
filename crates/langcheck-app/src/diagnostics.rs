//! Redacted, in-memory observability: bounded counters (events seen/dropped,
//! session resets by reason, decision latency histogram, replacement outcomes)
//! that never contain raw typed text and are never transmitted. Diagnostics
//! export is user-initiated and local-only (see `blueprint.md` Sections 18).
//!
//! Implemented in delivery Steps 06 and 08.
