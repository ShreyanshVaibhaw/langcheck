//! Bundled, read-only compact FST lexicon — the release backend.
//!
//! The word list and frequency data are compiled into a finite-state transducer
//! (an `fst::Map` from word to frequency). Candidate suggestions come from the
//! bounded Levenshtein automaton over the FST, giving deterministic latency and
//! memory behavior (see `blueprint.md` Section 8.5 and ADR-004).
//!
//! Two backings share one generic type: [`CompactFstLexicon<Vec<u8>>`] builds an
//! FST in memory from the small embedded prototype list (dev/tests), and
//! [`CompactFstLexicon<&'static [u8]>`] loads the production FST embedded via
//! `include_bytes!` (compiled offline by `tools/dictionary-compiler`). Embedding
//! keeps the crate free of `unsafe` — no file and no `mmap` — while shipping the
//! dictionary inside the executable (see ADR-0007).

use std::time::Instant;

use fst::automaton::Levenshtein;
use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use langcheck_core::classify::normalize_lookup;

use crate::{
    bounded_levenshtein, CandidateList, LanguageTag, LexiconCandidate, LexiconError,
    LexiconProvider, MAX_LEXICON_CANDIDATES,
};

/// The embedded prototype word list (`word [frequency]` per line; `#` comments).
const PROTOTYPE_EN_US: &str = include_str!("prototype_en_us.txt");

/// Version stamp for the prototype dictionary (production versioning: Step 09).
const PROTOTYPE_EN_US_VERSION: &str = "prototype-en-US-1";

/// Cap on raw automaton matches collected before ranking/truncation, bounding
/// work on large dictionaries (`blueprint.md` Section 8.6).
const RAW_MATCH_CAP: usize = 64;

/// Defensive input bounds for dictionary loading (`blueprint.md` Sections 14, 19.6):
/// a malformed or oversized word list must never exhaust memory.
const MAX_DICTIONARY_WORD_LEN: usize = 64;
const MAX_DICTIONARY_LINES: usize = 2_000_000;

/// A read-only compact FST lexicon mapping each known word to a frequency. The
/// backing `D` is `Vec<u8>` for the in-memory prototype/dev lexicon and
/// `&'static [u8]` for the bundled production lexicon.
pub struct CompactFstLexicon<D: AsRef<[u8]> = Vec<u8>> {
    language: LanguageTag,
    map: Map<D>,
    word_count: usize,
    version: &'static str,
}

impl CompactFstLexicon<Vec<u8>> {
    /// Build the prototype `en-US` lexicon from the embedded word list.
    pub fn prototype_en_us() -> Result<Self, LexiconError> {
        Self::from_word_list(LanguageTag::EnUs, PROTOTYPE_EN_US, PROTOTYPE_EN_US_VERSION)
    }

    /// Build a lexicon from a `word [frequency]` text list. Malformed lines are
    /// skipped (`blueprint.md` Section 8.12); duplicate words keep the highest
    /// frequency.
    pub fn from_word_list(
        language: LanguageTag,
        list: &str,
        version: &'static str,
    ) -> Result<Self, LexiconError> {
        let mut entries: Vec<(String, u64)> = list
            .lines()
            .take(MAX_DICTIONARY_LINES)
            .filter_map(parse_line)
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|next, kept| {
            if next.0 == kept.0 {
                kept.1 = kept.1.max(next.1);
                true
            } else {
                false
            }
        });
        if entries.is_empty() {
            return Err(LexiconError::Backend("lexicon word list is empty".into()));
        }

        let mut builder = MapBuilder::memory();
        for (word, freq) in &entries {
            builder
                .insert(word.as_bytes(), *freq)
                .map_err(|e| LexiconError::Backend(e.to_string()))?;
        }
        let bytes = builder
            .into_inner()
            .map_err(|e| LexiconError::Backend(e.to_string()))?;
        let map = Map::new(bytes).map_err(|e| LexiconError::Backend(e.to_string()))?;

        Ok(Self {
            language,
            word_count: entries.len(),
            map,
            version,
        })
    }
}

/// The bundled production FST, embedded in the binary and compiled offline by
/// `tools/dictionary-compiler` from `data/en-US.wordfreq.txt` (see ADR-0007).
/// Embedding ships the lexicon inside the (signed) executable: it is demand-paged
/// from the image, needs no separate file or `mmap`, and cannot be tampered with
/// independently — so no runtime hash check is required.
const EN_US_FST: &[u8] = include_bytes!("../dictionaries/en-US.fst");
const PRODUCTION_EN_US_VERSION: &str = "en-US-1";

impl CompactFstLexicon<&'static [u8]> {
    /// Load the bundled production `en-US` lexicon.
    pub fn production_en_us() -> Result<Self, LexiconError> {
        let map = Map::new(EN_US_FST).map_err(|e| LexiconError::Backend(e.to_string()))?;
        let word_count = map.len();
        Ok(Self {
            language: LanguageTag::EnUs,
            map,
            word_count,
            version: PRODUCTION_EN_US_VERSION,
        })
    }
}

impl<D: AsRef<[u8]>> CompactFstLexicon<D> {
    /// Number of distinct words in the lexicon.
    pub fn word_count(&self) -> usize {
        self.word_count
    }

    /// Dictionary version stamp.
    pub fn version(&self) -> &str {
        self.version
    }
}

impl<D: AsRef<[u8]> + Send + Sync> LexiconProvider for CompactFstLexicon<D> {
    fn language(&self) -> LanguageTag {
        self.language
    }

    fn contains(&self, language: LanguageTag, word: &str) -> bool {
        if language != self.language {
            return false;
        }
        let normalized = normalize_lookup(word);
        self.map.get(normalized.as_bytes()).is_some()
    }

    fn candidates(
        &self,
        language: LanguageTag,
        word: &str,
        limit: usize,
        deadline: Option<Instant>,
    ) -> Result<CandidateList, LexiconError> {
        if language != self.language {
            return Err(LexiconError::UnsupportedLanguage(language));
        }
        if deadline_elapsed(deadline) {
            return Err(LexiconError::Timeout);
        }
        let limit = limit.min(MAX_LEXICON_CANDIDATES);
        if limit == 0 {
            return Ok(CandidateList::new());
        }

        let normalized = normalize_lookup(word);
        let max_distance = max_edit_distance(&normalized);
        let automaton = Levenshtein::new(&normalized, u32::from(max_distance))
            .map_err(|e| LexiconError::Backend(e.to_string()))?;
        let mut stream = self.map.search(automaton).into_stream();

        let mut raw: Vec<(String, u8, u32)> = Vec::new();
        let mut seen = 0u32;
        while let Some((key, freq)) = stream.next() {
            seen += 1;
            if seen.is_multiple_of(64) && deadline_elapsed(deadline) {
                return Err(LexiconError::Timeout);
            }
            let candidate = match std::str::from_utf8(key) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let distance = bounded_levenshtein(&normalized, candidate, max_distance);
            let frequency = u32::try_from(freq).unwrap_or(u32::MAX);
            raw.push((candidate.to_owned(), distance, frequency));
            if raw.len() >= RAW_MATCH_CAP {
                break;
            }
        }

        // Damerau-style adjacent transpositions: a single swap is edit distance 2
        // under plain Levenshtein, so the distance-1 policy for short words would
        // miss the most common typo of all ("teh" -> "the"). Generate each adjacent
        // swap, validate it against the FST, and record it as a single edit.
        collect_transpositions(&self.map, &normalized, &mut raw);

        // Best first: nearest edit distance, then most frequent, then alphabetical.
        raw.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then(b.2.cmp(&a.2))
                .then_with(|| a.0.cmp(&b.0))
        });

        Ok(raw
            .into_iter()
            .take(limit)
            .map(|(word, edit_distance, frequency)| LexiconCandidate {
                word,
                edit_distance,
                frequency,
            })
            .collect())
    }
}

/// Maximum edit distance policy (`blueprint.md` Section 8.6): distance 1 for
/// short tokens, 2 for tokens of length ≥ 6.
fn max_edit_distance(word: &str) -> u8 {
    if word.chars().count() >= 6 {
        2
    } else {
        1
    }
}

/// Append dictionary words reachable from `normalized` by a single adjacent
/// transposition, as edit-distance-1 candidates. If a word is already present
/// (found by the Levenshtein pass at a higher distance), lower its distance to 1.
fn collect_transpositions<D: AsRef<[u8]>>(
    map: &Map<D>,
    normalized: &str,
    raw: &mut Vec<(String, u8, u32)>,
) {
    let chars: Vec<char> = normalized.chars().collect();
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i] == chars[i + 1] {
            continue;
        }
        let mut swapped = chars.clone();
        swapped.swap(i, i + 1);
        let swapped: String = swapped.into_iter().collect();
        if let Some(freq) = map.get(swapped.as_bytes()) {
            let frequency = u32::try_from(freq).unwrap_or(u32::MAX);
            if let Some(pos) = raw.iter().position(|(w, _, _)| *w == swapped) {
                raw[pos].1 = raw[pos].1.min(1);
            } else if raw.len() < RAW_MATCH_CAP {
                raw.push((swapped, 1, frequency));
            }
        }
    }
}

fn deadline_elapsed(deadline: Option<Instant>) -> bool {
    matches!(deadline, Some(dl) if Instant::now() >= dl)
}

/// Parse a `word [frequency]` line; return `None` for blank/comment/malformed
/// lines (skipped rather than failing the whole load).
fn parse_line(line: &str) -> Option<(String, u64)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split_whitespace();
    let word = fields.next()?.to_ascii_lowercase();
    if word.is_empty()
        || word.len() > MAX_DICTIONARY_WORD_LEN
        || !word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'')
    {
        return None;
    }
    let frequency = fields
        .next()
        .and_then(|f| f.parse::<u64>().ok())
        .unwrap_or(1);
    Some((word, frequency))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex() -> CompactFstLexicon {
        CompactFstLexicon::prototype_en_us().expect("prototype lexicon builds")
    }

    #[test]
    fn builds_and_reports_metadata() {
        let l = lex();
        assert!(
            l.word_count() > 100,
            "prototype should have a useful vocabulary, got {}",
            l.word_count()
        );
        assert_eq!(l.version(), "prototype-en-US-1");
        assert_eq!(l.language(), LanguageTag::EnUs);
    }

    #[test]
    fn production_lexicon_loads_with_a_real_vocabulary() {
        let l = CompactFstLexicon::production_en_us().expect("production lexicon loads");
        assert!(l.word_count() > 20_000, "got {}", l.word_count());
        assert_eq!(l.version(), "en-US-1");
        assert!(l.contains(LanguageTag::EnUs, "the"));
        assert!(l.contains(LanguageTag::EnUs, "receive"));
        assert!(l.contains(LanguageTag::EnUs, "friend"));
        assert!(l.contains(LanguageTag::EnUs, "don't"));
        // A common misspelling must NOT be in the dictionary.
        assert!(!l.contains(LanguageTag::EnUs, "teh"));
        // Candidate generation works on the real FST.
        let cands = l.candidates(LanguageTag::EnUs, "recieve", 8, None).unwrap();
        assert!(cands.iter().any(|c| c.word == "receive"), "got {cands:?}");
    }

    #[test]
    fn contains_known_and_unknown() {
        let l = lex();
        assert!(l.contains(LanguageTag::EnUs, "the"));
        assert!(l.contains(LanguageTag::EnUs, "The")); // normalized to lowercase
        assert!(l.contains(LanguageTag::EnUs, "receive"));
        assert!(!l.contains(LanguageTag::EnUs, "teh"));
        assert!(!l.contains(LanguageTag::EnUs, "zzzzx"));
    }

    #[test]
    fn candidates_cover_within_distance_edits() {
        let l = lex();
        // Distance-1 insertion on a short word (len 3..=5 -> max distance 1).
        let cands = l.candidates(LanguageTag::EnUs, "wrd", 8, None).unwrap();
        assert!(cands.iter().any(|c| c.word == "word"), "got {cands:?}");

        // Distance-2 double substitution on a longer word (len >= 6 -> max distance 2).
        let cands = l.candidates(LanguageTag::EnUs, "recieve", 8, None).unwrap();
        assert!(cands.iter().any(|c| c.word == "receive"), "got {cands:?}");
    }

    #[test]
    fn transpositions_are_surfaced_as_single_edits() {
        let l = lex();
        // A pure adjacent transposition ("teh" -> "the") is Levenshtein distance 2,
        // which the distance-1 policy for a 3-letter word would miss; the
        // transposition pass surfaces it as a single edit.
        let cands = l.candidates(LanguageTag::EnUs, "teh", 8, None).unwrap();
        let the = cands.iter().find(|c| c.word == "the");
        assert!(the.is_some(), "expected 'the' for 'teh', got {cands:?}");
        assert_eq!(the.unwrap().edit_distance, 1);
    }

    #[test]
    fn candidates_are_ordered_and_bounded() {
        let l = lex();
        let cands = l.candidates(LanguageTag::EnUs, "wrd", 3, None).unwrap();
        assert!(cands.len() <= 3);
        for pair in cands.windows(2) {
            assert!(
                pair[0].edit_distance <= pair[1].edit_distance,
                "got {cands:?}"
            );
        }
    }

    #[test]
    fn timeout_and_zero_limit_make_no_correction() {
        let l = lex();
        let past = Instant::now();
        assert_eq!(
            l.candidates(LanguageTag::EnUs, "teh", 8, Some(past)),
            Err(LexiconError::Timeout)
        );
        assert!(l
            .candidates(LanguageTag::EnUs, "teh", 0, None)
            .unwrap()
            .is_empty());
    }

    // Release-only latency measurement of `contains`/`candidates`, not a CI gate:
    // the blueprint's budgets (Section 5) apply to optimized release builds, and
    // wall-clock gating belongs on the defined hardware in Step 11. Marked
    // `#[ignore]` so the slow debug path never runs in the normal test suite.
    // Run with:
    //   cargo test -p langcheck-lexicon --release -- --ignored --nocapture
    #[test]
    #[ignore = "release-only latency measurement; run with --release --ignored --nocapture"]
    fn lookup_latency_measurement() {
        // Measure the PRODUCTION lexicon (what ships), not the prototype.
        let l = CompactFstLexicon::production_en_us().expect("production lexicon");
        let words = [
            "the", "wrd", "recieve", "hello", "world", "beleive", "friend", "xyzzy", "separate",
            "tomorow",
        ];
        let reps = 300usize;
        let mut sink = 0usize;

        // contains() is uniformly tiny — report its mean.
        let start = Instant::now();
        for _ in 0..reps {
            for w in &words {
                sink += usize::from(l.contains(LanguageTag::EnUs, w));
            }
        }
        let contains_mean = start.elapsed() / (reps * words.len()) as u32;

        // candidates() — report percentiles, the metric the budget is stated in
        // (blueprint.md Section 5: p50 < 2 ms, p95 < 5 ms, p99 < 10 ms).
        let mut samples: Vec<std::time::Duration> = Vec::with_capacity(reps * words.len());
        for _ in 0..reps {
            for w in &words {
                let t = Instant::now();
                let c = l.candidates(LanguageTag::EnUs, w, 8, None).unwrap();
                samples.push(t.elapsed());
                sink += std::hint::black_box(c).len();
            }
        }
        samples.sort_unstable();
        let pct = |p: f64| samples[(((samples.len() - 1) as f64) * p) as usize];
        println!(
            "lexicon latency (release): contains_mean={contains_mean:?}; candidates \
             p50={:?} p95={:?} p99={:?} max={:?} (sink={sink})",
            pct(0.50),
            pct(0.95),
            pct(0.99),
            samples.last().unwrap()
        );
    }
}
