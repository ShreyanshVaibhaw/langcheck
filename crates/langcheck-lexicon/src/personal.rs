//! Personal dictionary and user correction rules.
//!
//! Holds only entries the user explicitly approves — added words
//! (`user_words.txt`), forced replacement pairs (`autocorrect.tsv`), and blocked
//! pairs (`blocked_pairs.tsv`) — never a history of typed words (`blueprint.md`
//! Sections 8.12, 12.1). Files are small, human-inspectable, and rewritten
//! atomically; malformed lines are skipped. Parsing and lookups are pure and
//! unit-tested.
//!
//! Implemented in delivery Step 10 (Immediate Undo and Personal Dictionary).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use langcheck_core::classify::normalize_lookup;

/// File names under the per-user `state` directory.
pub const USER_WORDS_FILE: &str = "user_words.txt";
pub const AUTOCORRECT_FILE: &str = "autocorrect.tsv";
pub const BLOCKED_PAIRS_FILE: &str = "blocked_pairs.tsv";

const MAX_PERSONAL_ENTRIES: usize = 100_000;
const MAX_PERSONAL_WORD_LEN: usize = 64;

/// A user's personal words and correction rules.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PersonalDictionary {
    words: BTreeSet<String>,
    forced: BTreeMap<String, String>,
    blocked: BTreeSet<(String, String)>,
}

impl PersonalDictionary {
    /// An empty personal dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there are no entries at all.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.forced.is_empty() && self.blocked.is_empty()
    }

    /// Whether the user has added `word` as known (so it is never "corrected").
    pub fn contains_word(&self, word: &str) -> bool {
        self.words.contains(&normalize_lookup(word))
    }

    /// A user-forced replacement for `word`, if any.
    pub fn forced_correction(&self, word: &str) -> Option<&str> {
        self.forced.get(&normalize_lookup(word)).map(String::as_str)
    }

    /// Whether the user has blocked correcting `original` to `replacement`.
    pub fn is_blocked(&self, original: &str, replacement: &str) -> bool {
        self.blocked
            .contains(&(normalize_lookup(original), normalize_lookup(replacement)))
    }

    /// Add a personal word (normalized; ignored if invalid or at the entry bound).
    pub fn add_word(&mut self, word: &str) {
        let word = normalize_lookup(word);
        if is_valid_word(&word) && self.words.len() < MAX_PERSONAL_ENTRIES {
            self.words.insert(word);
        }
    }

    /// Add a forced replacement pair.
    pub fn add_forced(&mut self, original: &str, replacement: &str) {
        let (original, replacement) = (normalize_lookup(original), normalize_lookup(replacement));
        if is_valid_word(&original)
            && is_valid_word(&replacement)
            && self.forced.len() < MAX_PERSONAL_ENTRIES
        {
            self.forced.insert(original, replacement);
        }
    }

    /// Block correcting `original` to `replacement` permanently.
    pub fn block_pair(&mut self, original: &str, replacement: &str) {
        let (original, replacement) = (normalize_lookup(original), normalize_lookup(replacement));
        if is_valid_word(&original)
            && is_valid_word(&replacement)
            && self.blocked.len() < MAX_PERSONAL_ENTRIES
        {
            self.blocked.insert((original, replacement));
        }
    }

    /// Parse a dictionary from the three file contents (any may be empty).
    pub fn from_parts(user_words: &str, autocorrect_tsv: &str, blocked_tsv: &str) -> Self {
        let mut dict = Self::new();
        for line in user_words.lines().take(MAX_PERSONAL_ENTRIES) {
            if let Some(word) = parse_word_line(line) {
                dict.add_word(&word);
            }
        }
        for line in autocorrect_tsv.lines().take(MAX_PERSONAL_ENTRIES) {
            if let Some((original, replacement)) = parse_pair_line(line) {
                dict.add_forced(&original, &replacement);
            }
        }
        for line in blocked_tsv.lines().take(MAX_PERSONAL_ENTRIES) {
            if let Some((original, replacement)) = parse_pair_line(line) {
                dict.block_pair(&original, &replacement);
            }
        }
        dict
    }

    /// Load from the three files in `dir`; a missing/unreadable file is empty.
    pub fn load_dir(dir: &Path) -> Self {
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap_or_default();
        Self::from_parts(
            &read(USER_WORDS_FILE),
            &read(AUTOCORRECT_FILE),
            &read(BLOCKED_PAIRS_FILE),
        )
    }

    /// Atomically write the three files into `dir`.
    pub fn save_dir(&self, dir: &Path) -> io::Result<()> {
        atomic_write(&dir.join(USER_WORDS_FILE), &self.user_words_text())?;
        atomic_write(&dir.join(AUTOCORRECT_FILE), &self.autocorrect_tsv())?;
        atomic_write(&dir.join(BLOCKED_PAIRS_FILE), &self.blocked_pairs_tsv())
    }

    /// Serialize the added words (one per line).
    pub fn user_words_text(&self) -> String {
        let mut out = String::new();
        for word in &self.words {
            out.push_str(word);
            out.push('\n');
        }
        out
    }

    /// Serialize the forced pairs as TSV.
    pub fn autocorrect_tsv(&self) -> String {
        let mut out = String::new();
        for (original, replacement) in &self.forced {
            out.push_str(original);
            out.push('\t');
            out.push_str(replacement);
            out.push('\n');
        }
        out
    }

    /// Serialize the blocked pairs as TSV.
    pub fn blocked_pairs_tsv(&self) -> String {
        let mut out = String::new();
        for (original, replacement) in &self.blocked {
            out.push_str(original);
            out.push('\t');
            out.push_str(replacement);
            out.push('\n');
        }
        out
    }
}

fn is_valid_word(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= MAX_PERSONAL_WORD_LEN
        && word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'')
}

fn parse_word_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    Some(line.to_owned())
}

fn parse_pair_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split('\t');
    let original = fields.next()?.trim();
    let replacement = fields.next()?.trim();
    if original.is_empty() || replacement.is_empty() {
        return None;
    }
    Some((original.to_owned(), replacement.to_owned()))
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    fs::write(&temp, contents)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_looks_up() {
        let dict = PersonalDictionary::from_parts(
            "# my words\nfoobar\nWidget\n",
            "teh\tthe\nwont\twon't\n",
            "wrd\tward\n",
        );
        assert!(dict.contains_word("FooBar")); // normalized
        assert!(dict.contains_word("widget"));
        assert!(!dict.contains_word("hello"));
        assert_eq!(dict.forced_correction("TEH"), Some("the"));
        assert_eq!(dict.forced_correction("wont"), Some("won't"));
        assert!(dict.is_blocked("Wrd", "Ward"));
        assert!(!dict.is_blocked("wrd", "word"));
    }

    #[test]
    fn skips_malformed_lines() {
        let dict = PersonalDictionary::from_parts("ok\n123bad\n\n  \n", "onlyone\n\tnoorig\n", "");
        assert!(dict.contains_word("ok"));
        assert!(!dict.contains_word("123bad")); // digits invalid
        assert_eq!(dict.forced_correction("onlyone"), None); // missing replacement
    }

    #[test]
    fn round_trips_through_files() {
        let dir = std::env::temp_dir().join(format!("langcheck-personal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut dict = PersonalDictionary::new();
        dict.add_word("foobar");
        dict.add_forced("teh", "the");
        dict.block_pair("wrd", "ward");
        dict.save_dir(&dir).expect("save");
        let loaded = PersonalDictionary::load_dir(&dir);
        assert_eq!(loaded, dict);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_loads_empty() {
        let dir = std::env::temp_dir().join("langcheck-personal-does-not-exist-xyz");
        assert!(PersonalDictionary::load_dir(&dir).is_empty());
    }
}
