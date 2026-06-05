# ADR-0007: Production dictionary source and embedded FST

- **Status:** Accepted
- **Date:** 2026-06-05
- **Deciders:** LangCheck maintainers (source choice delegated to the implementer)

## Context

Delivery Step 09 requires a real, redistributable English word list with frequency
data, compiled into the compact FST lexicon. The blueprint (Section 16, ADR-004)
provisionally chose a memory-mapped (`memmap2`) FST loaded from a file, pending the
licensing and benchmark gate.

## Decision

### Source (public-domain / permissive)

- **Word membership:** [dwyl/english-words](https://github.com/dwyl/english-words)
  `words_alpha.txt` — a curated list of valid English words, **Unlicense**
  (public domain).
- **Frequencies:** Peter Norvig's `count_1w.txt` (Google Web Trillion Word Corpus
  counts). Word-frequency *counts are factual and not copyrightable*, and are
  distributed freely; they are used purely as ranking weights.
- **Build:** the top 30 000 `count_1w` words that are valid `words_alpha` words
  (ASCII `a–z`, length 2–20), in descending frequency, plus a curated set of common
  contractions (which `words_alpha` omits). Crucially, filtering through
  `words_alpha` keeps web-corpus **misspellings out of the dictionary** (e.g.
  `teh` is in `count_1w` but not in `words_alpha`).

The processed list is committed as
[`crates/langcheck-lexicon/data/en-US.wordfreq.txt`](../../crates/langcheck-lexicon/data/en-US.wordfreq.txt)
and is the source of truth. `tools/dictionary-compiler` builds the FST from it
**reproducibly** (same input → same bytes → same SHA-256, recorded in
`en-US.meta.json`).

### Backend: embed the FST (supersedes the provisional `memmap2` choice)

The compiled FST (~288 KB for 30 040 words) is **embedded in the binary** via
`include_bytes!`, rather than memory-mapped from a separate file.

Rationale:

- Equivalent low-memory behavior — the read-only FST is demand-paged from the
  executable image, so it is not all resident.
- **No `unsafe`** — `memmap2` requires an `unsafe` mapping boundary; `include_bytes!`
  + `fst::Map::new(&'static [u8])` is entirely safe.
- **Tamper-resistant** — the dictionary is part of the (eventually signed)
  executable and cannot be replaced independently, so no runtime hash/version check
  is needed (the SHA-256 in `en-US.meta.json` documents reproducibility).
- No installer file management for the dictionary.

## Consequences

- The production lexicon adds ~288 KB to the binary — far within the < 20 MB
  installed-size budget (Section 5).
- Updating the dictionary means editing the source list and re-running the
  compiler, then committing the regenerated `en-US.fst` + `en-US.meta.json`.
- The lexicon keeps `#![deny(unsafe_code)]` (no `mmap`). If a much larger
  dictionary later strains binary size, a file-based memory-mapped backend can be
  reconsidered behind the same `LexiconProvider` trait.

## Alternatives considered

- **`memmap2` from a file (provisional):** rejected for the MVP — adds an `unsafe`
  boundary, a separate installed file, and an independent tamper surface for no
  memory benefit at this size.
- **Raw `count_1w` as the dictionary:** rejected — web-corpus frequency lists
  contain misspellings, which would make the speller treat typos as valid words.
