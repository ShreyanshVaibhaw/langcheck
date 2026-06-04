//! Integrity-level checks for UIPI safety. Determines whether a target window
//! belongs to a process at an equal or lower integrity level so replacement is
//! only attempted where it is permitted; LangCheck never bypasses UIPI (see
//! `blueprint.md` Sections 8.10 and 12.2).
//!
//! Implemented in delivery Steps 01 and 05.
