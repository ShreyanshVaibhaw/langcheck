//! Rejected runtime backend: the Windows Spell Checking API.
//!
//! This API may load external spell-check providers that LangCheck cannot
//! guarantee are offline, which conflicts with the hard guarantee that typed
//! words never leave the device (see `blueprint.md` Section 8.5). It is therefore
//! **never** used by release builds. This module exists only as an isolated
//! developer benchmark target, gated behind the non-default `dev-windows-spell`
//! feature, and must never process real user typing.
