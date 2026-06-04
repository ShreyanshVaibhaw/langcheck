//! `dictionary-compiler` — build-time tool that compiles an approved English word
//! list and frequency data into LangCheck's reproducible, versioned compact FST
//! lexicon. It is a developer/build tool and is never part of the resident
//! application (see `blueprint.md` Sections 8.5 and 15).
//!
//! Implemented in delivery Step 09 (Production Compact FST Lexicon).
#![deny(unsafe_code)]

fn main() {
    eprintln!(
        "dictionary-compiler {} — not implemented yet (see blueprint.md Step 09).",
        env!("CARGO_PKG_VERSION")
    );
}
