# Architecture Decision Records

This directory records significant, hard-to-reverse decisions. Each ADR is a
numbered Markdown file (`NNNN-short-title.md`) based on
[`0000-template.md`](0000-template.md).

Per [`blueprint.md`](../../blueprint.md) Section 26, changes to **privacy
invariants, integrity behavior, input capture, replacement, TSF, dictionary
licensing, or startup behavior** require an ADR.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-rust-as-the-primary-language.md) | Rust as the primary language | Accepted |
| [0002](0002-mvp-keyboard-observer.md) | `WH_KEYBOARD_LL` as the MVP keyboard observer | Accepted (provisional) |
| [0007](0007-production-dictionary.md) | Production dictionary source and embedded FST | Accepted |

Decisions ADR-003 through ADR-006 are currently recorded inline in
[`blueprint.md`](../../blueprint.md) Section 23 and will be migrated into this
directory as they are next revisited.
