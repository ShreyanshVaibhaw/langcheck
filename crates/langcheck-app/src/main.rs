//! LangCheck broker entry point (`langcheck.exe`).
//!
//! Bootstrap stage: this binary starts and exits cleanly. Keyboard observation,
//! focus inspection, the correction engine, the tray UI, and the `--background`
//! launch mode are introduced in later delivery steps (see `blueprint.md`
//! Section 24).
#![deny(unsafe_code)]

mod config;
mod coordinator;
mod diagnostics;
mod persistence;

fn main() {
    println!(
        "LangCheck {} (bootstrap build) — no correction functionality yet. See blueprint.md.",
        env!("CARGO_PKG_VERSION")
    );
}
