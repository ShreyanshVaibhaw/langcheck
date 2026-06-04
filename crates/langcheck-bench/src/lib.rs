//! `langcheck-bench` — benchmark harnesses for LangCheck.
//!
//! Criterion benchmarks of the hot path (tokenizer, ranking, lexicon lookup) and
//! resource budgets are added alongside the components they measure (delivery
//! Steps 03, 04, 06, and 11). See `blueprint.md` Sections 5 and 19.5.
#![deny(unsafe_code)]
