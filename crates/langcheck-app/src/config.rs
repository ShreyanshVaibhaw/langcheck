//! Configuration manager: loads defaults, persisted settings, and app policies;
//! validates and migrates schema versions; persists via atomic replacement. The
//! `retain_typing_history` invariant must always remain `false` (see
//! `blueprint.md` Section 8.13).
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).
