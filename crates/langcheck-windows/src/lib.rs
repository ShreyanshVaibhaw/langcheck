//! `langcheck-windows` — all Windows-specific integration for LangCheck.
//!
//! This crate isolates Win32, COM, and UI Automation usage behind typed, testable
//! interfaces: keyboard observation (`input`), focus-safety inspection (`focus`),
//! text replacement (`replace`), the native tray UI (`tray`), start-at-login
//! registration (`startup`), and integrity-level checks (`integrity`).
//!
//! Because this crate is the FFI boundary, `unsafe` is used pervasively for Win32
//! calls; rather than forbidding it, we *require every unsafe block to carry a
//! `// SAFETY:` comment* (enforced by `clippy::undocumented_unsafe_blocks`), per
//! `blueprint.md` Section 12.4. Pure logic (event/field classification, integrity
//! comparison) is factored out into safe, unit-tested functions.
//!
//! Implemented in delivery Steps 01 (input/focus/integrity), 05 (replacement), and
//! 08 (tray/startup).

// FFI needs `unsafe`; enforce documentation of every unsafe block instead of
// forbidding it (blueprint Section 12.4).
#![deny(clippy::undocumented_unsafe_blocks)]

pub mod focus;
pub mod input;
pub mod integrity;
pub mod policy;
pub mod replace;
pub mod startup;
pub mod tray;

/// Marker stored in `dwExtraInfo` on every input event LangCheck injects, so the
/// observer can recognise and ignore its own synthetic input and never recurse
/// (see `blueprint.md` Sections 8.1 and 8.10). The replacement executor (Step 05)
/// writes this value; the input observer (Step 01) skips events carrying it.
pub const LANGCHECK_INJECTED_MARKER: usize = 0x4C43_4B31; // 'L','C','K','1'
