//! Per-user start-at-login registration. Registers a quoted launch command under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` that starts
//! `langcheck.exe --background`; reversible and off by default. Never creates a
//! Windows service or elevates (see `blueprint.md` Section 13.4).
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).
