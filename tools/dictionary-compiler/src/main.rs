//! `dictionary-compiler` — compiles an approved `word<TAB>count` list into
//! LangCheck's reproducible, versioned compact FST lexicon. It is a developer /
//! build tool and is never part of the resident application (see `blueprint.md`
//! Sections 8.5, 15, and ADR-0007).
//!
//! Usage: `cargo run -p dictionary-compiler [-- <input.txt> <out_dir>]`.
//! Defaults read `crates/langcheck-lexicon/data/en-US.wordfreq.txt` and write
//! `crates/langcheck-lexicon/dictionaries/{en-US.fst, en-US.meta.json}`.
//!
//! Counts are scaled linearly to `u32` weights (preserving ranking) so the lexicon
//! can read them as frequencies. The same input always yields the same FST bytes
//! and the same SHA-256, so the build is reproducible (`blueprint.md` Step 09).
#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use fst::MapBuilder;
use sha2::{Digest, Sha256};

const DICTIONARY_VERSION: &str = "en-US-1";
const DEFAULT_INPUT: &str = "crates/langcheck-lexicon/data/en-US.wordfreq.txt";
const DEFAULT_OUTDIR: &str = "crates/langcheck-lexicon/dictionaries";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = args
        .get(1)
        .map_or_else(|| PathBuf::from(DEFAULT_INPUT), PathBuf::from);
    let out_dir = args
        .get(2)
        .map_or_else(|| PathBuf::from(DEFAULT_OUTDIR), PathBuf::from);

    match build(&input, &out_dir) {
        Ok(report) => {
            println!(
                "compiled {} words from {} -> {} ({} bytes, sha256 {})",
                report.word_count,
                input.display(),
                out_dir.display(),
                report.fst_bytes,
                &report.sha256[..16],
            );
        }
        Err(e) => {
            eprintln!("dictionary-compiler failed: {e}");
            std::process::exit(1);
        }
    }
}

struct Report {
    word_count: usize,
    fst_bytes: usize,
    sha256: String,
}

fn build(input: &Path, out_dir: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(input)?;

    let mut entries: Vec<(String, u64)> = Vec::new();
    let mut max_count = 1u64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let word = fields.next().unwrap_or_default().to_ascii_lowercase();
        let count: u64 = fields
            .next()
            .and_then(|c| c.trim().parse().ok())
            .unwrap_or(1);
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') {
            continue;
        }
        max_count = max_count.max(count);
        entries.push((word, count));
    }
    entries.sort();
    entries.dedup_by(|a, b| a.0 == b.0);
    if entries.is_empty() {
        return Err("no valid entries in input".into());
    }

    let mut builder = MapBuilder::memory();
    for (word, count) in &entries {
        // Scale the raw count to a u32-range weight, preserving relative ranking.
        let weight = (u128::from(*count) * u128::from(u32::MAX) / u128::from(max_count)) as u64;
        builder.insert(word.as_bytes(), weight.max(1))?;
    }
    let bytes = builder.into_inner()?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));

    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("en-US.fst"), &bytes)?;
    let meta = format!(
        "{{\n  \"language\": \"en-US\",\n  \"version\": \"{DICTIONARY_VERSION}\",\n  \
         \"word_count\": {},\n  \"sha256\": \"{sha256}\",\n  \
         \"source\": \"data/en-US.wordfreq.txt\"\n}}\n",
        entries.len()
    );
    std::fs::write(out_dir.join("en-US.meta.json"), meta)?;

    Ok(Report {
        word_count: entries.len(),
        fst_bytes: bytes.len(),
        sha256,
    })
}
