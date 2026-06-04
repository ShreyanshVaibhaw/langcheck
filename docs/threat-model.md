# Threat Model

Scope: the per-user LangCheck broker (`langcheck.exe`) on Windows. The post-MVP
TSF adapter is out of scope until Step 13. Derived from `blueprint.md` Section 12.3;
this file tracks each threat's mitigation and current status.

## Assets

- The user's typed text (especially secrets typed into sensitive fields).
- The user's machine integrity (LangCheck must not weaken it).

## Threats and mitigations

| # | Threat | Mitigation | Status |
|---|---|---|---|
| T1 | Accidental capture of secrets (passwords, OTPs) | Fail-closed focus classification; capture off by default; sensitive/unknown ⇒ no capture; field values never read | Logic in place + unit-tested; real-field behavior pending manual verification (ADR-0002) |
| T2 | Persistence/transmission of typing history | No networking code path; no typing-history persistence; metrics are numeric-only | Enforced (no persistence yet; `deny.toml` + audit) |
| T3 | Incorrect replacement in a sensitive/destructive field | Conservative confidence policy; app-category exclusions; capture gating; corpus precision gate (0 harmful) | Engine verified; field gating logic tested |
| T4 | Infinite recursion from observing injected events | Injected events carry `LANGCHECK_INJECTED_MARKER`; observer ignores them | Logic in place; manual verification with `--run` |
| T5 | Malicious/broken UI Automation provider hangs the inspector | UIA runs on a dedicated thread; reads fail closed to `Unknown` | Dedicated thread + fail-closed mapping; per-call timeouts hardened in Step 11 |
| T6 | Tampered dictionary / oversized input | Bounded word length and line count on load; malformed lines skipped; hash/version validation | Bounds in place; hash/version validation lands with the production lexicon (Step 09) |
| T7 | Injection into a higher-integrity (elevated) target | Integrity-level check before `SendInput`; never bypass UIPI; fail closed on read failure | Logic in place + unit-tested |
| T8 | Compromised same-user process drives a future TSF adapter | Out of scope until Step 13 (same-user authenticated IPC, remote-client rejection) | Deferred (Step 13) |

## Trust boundaries

- **Hook callback** (system context): does the minimum, never allocates/logs/locks;
  drops everything unless capture is allowed.
- **Coordinator thread**: the only place language logic runs; revalidates focus,
  integrity, and input freshness before any replacement.
- **Dependency supply chain**: pinned via `Cargo.lock`; licenses + offline bans
  enforced by `cargo deny`.

## Open items (tracked, none critical for the current stage)

- T5/T6 hardening (UIA per-call timeouts, dictionary hash/version) — Steps 09, 11.
- Full Section-19.6 behavioral security tests against the live app-matrix — Step 11
  (the logic-level fail-closed checks are unit-tested now).
- T8 (TSF) — Step 13.
