# Threat Model

Scope: the per-user LangCheck broker (`langcheck.exe`) on Windows, **and** the
post-MVP TSF precision adapter (`langcheck_tsf.dll`, an in-process COM text service)
plus the same-user IPC channel between them (ADR-0008). Derived from `blueprint.md`
Section 12.3; this file tracks each threat's mitigation and current status.

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
| T8 | Another user / remote process talks to the broker's IPC pipe | Pipe created with a DACL granting only the creating user's SID (+ SYSTEM); `PIPE_REJECT_REMOTE_CLIENTS`; name namespaced by SID; bounded message-mode buffers (`langcheck-ipc`) | Implemented; same-user/local-only verified by construction + round-trip test |
| T9 | Buggy in-process TSF adapter destabilises the host app | Fail-open everywhere (any COM error/uncertainty ⇒ no change); no language logic/persistence in the adapter; COM activate/advise/teardown exercised off-host (`--tsf-comtest`); adapter is opt-in and not registered by default | Implemented + host-verified in WordPad; broad app-matrix + 8h stability pending (ADR-0008) |
| T10 | TSF adapter replaces the wrong text | Before any edit, the word's range is re-derived and its text re-read and required to equal the detected token; mismatch ⇒ no edit; broker (not the adapter) makes the decision | Implemented; range re-verification in `langcheck-tsf` |
| T11 | TSF token leaks typed text via the IPC/diagnostics | Tokens are in-memory only, sent over the same-user pipe, never persisted; the broker's request log is a count only (never the token); no networking (offline audit) | Implemented; `--broker-serve` count is content-free |

## Trust boundaries

- **Hook callback** (system context): does the minimum, never allocates/logs/locks;
  drops everything unless capture is allowed.
- **Coordinator thread**: the only place language logic runs; revalidates focus,
  integrity, and input freshness before any replacement. Shares the engine decision
  with the TSF path (`engine::decide`) so neither can be more permissive.
- **TSF adapter** (`langcheck_tsf.dll`, in-process inside host apps): contains no
  language logic or persistence; only observes a token, asks the broker, and applies
  the broker's answer via an edit session. Fail-open; opt-in; never registered by
  default. The broker — not the adapter — owns every correction decision.
- **Broker IPC pipe**: same-user (SID-scoped DACL), local-only
  (`PIPE_REJECT_REMOTE_CLIENTS`), carries opaque protocol bytes only.
- **Dependency supply chain**: pinned via `Cargo.lock`; licenses + offline bans
  enforced by `cargo deny`; offline invariant additionally guarded by
  `scripts/offline-audit.ps1` in CI.

## Open items (tracked, none critical for the current stage)

- T5/T6 hardening (UIA per-call timeouts, dictionary hash/version) — Steps 09, 11.
- Full Section-19.6 behavioral security tests against the live app-matrix — Step 11
  (the logic-level fail-closed checks are unit-tested now).
- T9 (TSF host-app stability): broad app-matrix + 8-hour stability host testing with
  the adapter active — ADR-0008 checklist.
