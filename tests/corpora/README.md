# Evaluation Corpora

Versioned corpora for measuring correction precision/recall and guarding against
regressions (see [`blueprint.md`](../../blueprint.md) Section 19.3). Populated from
delivery Step 04 onward:

- Common misspelling pairs.
- Correct uncommon words that must **not** be changed.
- Names, acronyms, URLs, paths, emails, and code identifiers.
- Ambiguous errors where autocorrection must not occur.
- A regression entry for every reported harmful correction.
