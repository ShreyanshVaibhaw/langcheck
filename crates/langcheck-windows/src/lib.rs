//! `langcheck-windows` — all Windows-specific integration for LangCheck.
//!
//! This crate isolates Win32, COM, and UI Automation usage behind typed, testable
//! interfaces: keyboard observation (`input`), focus-safety inspection (`focus`),
//! text replacement (`replace`), the native tray UI (`tray`), start-at-login
//! registration (`startup`), and integrity-level checks (`integrity`). `unsafe` is
//! denied at the crate root and re-enabled only at small, documented FFI
//! boundaries, each carrying a safety-invariant comment (see `blueprint.md`
//! Section 12.4).
//!
//! Real functionality arrives in delivery Steps 01 (input/focus), 05
//! (replacement), and 08 (tray/startup).
#![deny(unsafe_code)]

pub mod focus;
pub mod input;
pub mod integrity;
pub mod replace;
pub mod startup;
pub mod tray;
