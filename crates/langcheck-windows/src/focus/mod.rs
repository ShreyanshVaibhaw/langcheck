//! Focus safety inspection. Tracks the focused control and foreground process via
//! UI Automation on a dedicated COM thread, positively classifies a field as
//! normal prose before enabling capture, and fails closed on any uncertainty.
//! Password and other sensitive field values are never read (see `blueprint.md`
//! Sections 8.2 and 12.2).
//!
//! Implemented in delivery Step 01 (Windows Input and Focus Feasibility Spike).
