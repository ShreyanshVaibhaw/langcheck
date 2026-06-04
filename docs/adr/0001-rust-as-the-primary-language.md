# ADR-0001: Rust as the primary language

- **Status:** Accepted
- **Date:** 2026-06-04
- **Deciders:** LangCheck maintainers

## Context

LangCheck must run continuously in the background with very low idle CPU and
memory, make sub-10 ms spelling decisions, integrate tightly with Win32, COM, and
UI Automation, and uphold strict memory- and concurrency-safety guarantees. The
correction engine should also be reusable and unit-testable without Windows APIs.
See `blueprint.md` Sections 5 (budgets) and 1.1 (invariants).

## Decision

Use Rust for the broker, correction engine, Windows integration, build tools, and
the future TSF adapter, unless a narrow C++ bridge is proven necessary by
measurement.

## Consequences

- Native performance and low memory with a strong default safety model.
- `unsafe` is confined to small, reviewed FFI boundaries, each carrying a
  safety-invariant comment (see `blueprint.md` Section 12.4); `langcheck-core`
  forbids `unsafe` entirely.
- The platform-independent core is unit-testable and reusable across lexicon
  backends, and its portability is enforced in CI by building it on Linux.
- The team must maintain Rust and `windows-rs` expertise.

## Alternatives considered

- **C++:** maximal Win32/COM/TSF ergonomics, but weaker default safety guarantees
  and a larger manual-correctness burden for the concurrency model.
- **C# / .NET:** faster UI development, but a larger runtime footprint and GC
  pauses conflict with the idle-memory and latency budgets in Section 5.
