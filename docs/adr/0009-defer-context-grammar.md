# ADR-0009: Defer optional context/grammar (Step 14 research outcome)

- **Status:** Accepted (deferred — research outcome; revisitable)
- **Date:** 2026-06-06
- **Deciders:** LangCheck maintainers

## Context

`blueprint.md` Step 14 is an explicit **research phase, not a product commitment**:
"determine whether a very small local context model can improve ambiguity and
grammar handling without breaking memory, privacy, or precision budgets," and its
acceptance criteria state the research "can be rejected without changing [the]
spelling architecture."

A context model (e.g. a compact n-gram) could help cases the current word-at-a-time
engine deliberately leaves alone — homophones and confusions that are only resolvable
from surrounding words (`there`/`their`, `hause`→`house` vs a name, etc.). Against
that potential benefit stand hard constraints:

- **Privacy.** LangCheck's invariants forbid persisting/transmitting typing history,
  and only the single active token is held in memory. Any context window is *more*
  retained text. The config already **forces `context_mode = false`** and treats it
  as an invariant, not a toggle (`config.rs::validate`). Shipping context would
  require explicit privacy UX (acceptance criterion) and a product decision.
- **Precision.** The product's promise is high-precision, conservative correction
  with a 0-harmful-correction gate. A weak context model can *introduce* confident-
  but-wrong corrections — the worst failure mode.
- **Resources.** Idle CPU/working-set/wakeup budgets are tight (Section 5); a resident
  model + per-keystroke context lookups risk them.
- **New surface.** Even ephemeral in-memory context is a new threat-model item.

## Decision

**Defer.** Ship no context or grammar feature now; keep `context_mode` forced off and
ship no model assets. The research conclusion is that the cost (privacy surface,
precision risk, resource budget, UX work) is not justified without (a) a concrete,
measured quality gap that *only* context can close, and (b) an explicit opt-in
privacy-UX design. None of those exist today, and the conservative word-level engine
already meets the MVP's precision goal.

This is exactly the outcome the brief permits: rejecting the research changes nothing
about the spelling architecture.

## Consequences

- **+** The architecture stays simple, fully local, fast, and private; no new
  attack/privacy surface; the 0-harmful-correction gate is not put at risk.
- **−** Context-dependent corrections (homophones, light grammar) remain out of
  scope. This is acceptable and on-brand: the engine leaves low-confidence words
  **unchanged** rather than guessing (e.g. it correctly leaves `hause` alone).
- **Reversible.** If revisited, the *minimum* bar is: a measured quality gap; a
  bounded, **memory-only**, never-persisted/never-transmitted context window; an
  explicit user opt-in with clear privacy UX; and a re-run of the resource +
  harmful-correction gates. Until then, `context_mode` stays an enforced-off invariant.

## Alternatives considered

- **Ship a small n-gram context model now.** Rejected: premature without a measured
  need, a privacy-UX design, and resource validation; risks precision and the privacy
  invariants.
- **Cloud/LLM grammar.** Rejected outright — violates the non-negotiable
  fully-local/no-network invariant (ADR is unnecessary; it is simply out of scope).
