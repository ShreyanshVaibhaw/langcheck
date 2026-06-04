//! Confidence policy: separates the best suggestion from what is safe to
//! autocorrect. The automatic tier is deliberately conservative — precision is
//! valued far above recall (see `blueprint.md` Sections 5.1 and 8.7).
//!
//! Implemented in delivery Step 04 (Candidate Ranking and Confidence Engine).
