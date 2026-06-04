//! Redacted, in-memory observability.
//!
//! Bounded atomic counters only — events translated, session resets, decisions by
//! tier, corrections applied/failed, and commits cancelled. They never contain raw
//! typed text and are discarded when the process exits (`blueprint.md` Section 18).
//!
//! Implemented in delivery Step 06 (End-to-End Conservative Autocorrect).

use std::sync::atomic::{AtomicU64, Ordering};

/// Shared, thread-safe correction metrics.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Key events translated into session events.
    pub events_translated: AtomicU64,
    /// Sessions reset (any reason).
    pub sessions_reset: AtomicU64,
    /// Decisions: original already known.
    pub known: AtomicU64,
    /// Decisions: ignored.
    pub ignored: AtomicU64,
    /// Decisions: suggested (not applied in the MVP).
    pub suggested: AtomicU64,
    /// Decisions: autocorrect chosen.
    pub autocorrected: AtomicU64,
    /// Corrections actually applied to a field.
    pub corrections_applied: AtomicU64,
    /// Replacement attempts that failed/were skipped at execution.
    pub replace_failures: AtomicU64,
    /// Commits cancelled by a final safety check (stale, focus change, unsafe...).
    pub commits_cancelled: AtomicU64,
}

impl Metrics {
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// A consistent-enough point-in-time copy for display.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_translated: self.events_translated.load(Ordering::Relaxed),
            sessions_reset: self.sessions_reset.load(Ordering::Relaxed),
            known: self.known.load(Ordering::Relaxed),
            ignored: self.ignored.load(Ordering::Relaxed),
            suggested: self.suggested.load(Ordering::Relaxed),
            autocorrected: self.autocorrected.load(Ordering::Relaxed),
            corrections_applied: self.corrections_applied.load(Ordering::Relaxed),
            replace_failures: self.replace_failures.load(Ordering::Relaxed),
            commits_cancelled: self.commits_cancelled.load(Ordering::Relaxed),
        }
    }
}

/// A plain copy of the counters (no atomics), for printing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub events_translated: u64,
    pub sessions_reset: u64,
    pub known: u64,
    pub ignored: u64,
    pub suggested: u64,
    pub autocorrected: u64,
    pub corrections_applied: u64,
    pub replace_failures: u64,
    pub commits_cancelled: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment_and_snapshot() {
        let m = Metrics::default();
        Metrics::inc(&m.events_translated);
        Metrics::inc(&m.events_translated);
        Metrics::inc(&m.corrections_applied);
        let s = m.snapshot();
        assert_eq!(s.events_translated, 2);
        assert_eq!(s.corrections_applied, 1);
        assert_eq!(s.known, 0);
    }
}
