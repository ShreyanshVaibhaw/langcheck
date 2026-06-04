//! Personal dictionary and user correction rules — local, small, and
//! human-inspectable. Holds only entries the user explicitly adds or approves
//! (`user_words.txt`, `autocorrect.tsv`, `blocked_pairs.tsv`); never a history of
//! typed words (see `blueprint.md` Sections 8.12 and 12.1).
//!
//! Implemented in delivery Step 10 (Immediate Undo and Personal Dictionary).
