# Compatibility Matrix

Delivery Step 11. The **policy rows below are enforced in code** (`langcheck-windows`
`policy.rs` + the fail-closed focus inspector). The **per-application support levels
are provisional**: they reflect the intended/default behavior and must be confirmed
by the on-hardware app-matrix run described under "Manual gates" (which the build
environment cannot perform). See `blueprint.md` Section 22.

## How correction behaves (and how to diagnose it)

Two consequences of the MVP design (post-boundary `SendInput` replacement,
ADR-0002) are important for expectations:

1. **It corrects after you pause.** A correction commits only if the word + boundary
   is still your most recent keystroke. If you keep typing the next word
   immediately, the newer keystrokes invalidate the (now stale) edit and it is
   cancelled — applying it would delete the wrong characters because the caret has
   moved. So a typo is fixed when you type `word` + space and **pause briefly**, not
   mid-flow. This is intentional (precision/safety over recall).
2. **It is reliable only in fields that accept synthetic keystrokes.** Replacement
   is backspaces + Unicode input via `SendInput`. Plain Win32 `Edit` fields handle
   this faithfully. Rich/web/Electron editors (many note apps, some markdown
   editors) often intercept or re-handle injected input, so the backspace/retype
   does not produce the expected result. Exact-range editing for those apps is the
   purpose of the post-MVP **TSF adapter (Step 13)**.

**Diagnosing:** `langcheck --run` prints a per-reason cancellation breakdown, e.g.
`cancelled=12 [stale=12 focus=0 unsafe=0 blocked=0]`. `stale` dominating means
fluent typing — pause after the word. `unsafe`/`focus` mean the field/app was not a
safe capture target. `langcheck --spike` shows the live focus classification per app.

## Levels

| Level | Meaning |
|---|---|
| Supported | Autocorrection enabled by default; intended to pass the app-matrix tests. |
| Suggestion only | Detection may work, but automatic replacement is not yet trusted. |
| Disabled by default | Sensitive or destructive category; capture is off (enforced in `policy.rs`). |
| Unsupported | The field/platform cannot be handled safely. |

## Matrix

| Application / control | Observer | Focus safety | Replacement | Default policy | Notes |
|---|---|---|---|---|---|
| Notepad, Win32 `Edit` | LL hook | UIA `Edit`, editable | `SendInput` | **Supported** | Primary target; the simplest case. |
| WordPad / Rich Edit | LL hook | UIA `Edit`/`Document` | `SendInput` | **Supported** (confirm) | Rich Edit selection nuances to verify. |
| Chromium browser text area | LL hook | UIA `Document`/`Edit` | `SendInput` | **Supported** (confirm) | Per-site field semantics vary; fail closed when unknown. |
| Microsoft Edge text area | LL hook | UIA `Document`/`Edit` | `SendInput` | **Supported** (confirm) | As above. |
| Microsoft Word | LL hook | UIA `Document` | `SendInput` | **Suggestion only** (confirm) | Autolayout/autocorrect interactions; verify before enabling auto. |
| Chat field (Slack/Teams/etc.) | LL hook | UIA varies | `SendInput` | **Supported** (confirm) | Electron UIA quality varies. |
| Markdown / web / Electron note editors | LL hook | UIA `Document`/varies | `SendInput` (unreliable) | **Suggestion only** | Field-tested: synthetic-keystroke replacement is often intercepted/undone. Exact-range editing needs the TSF adapter (Step 13). |
| Windows Terminal / cmd / PowerShell | — | — | — | **Disabled by default** | `policy.rs` exclusion (destructive/non-prose). |
| Code editors / IDEs (VS Code, JetBrains, …) | — | — | — | **Disabled by default** | `policy.rs` exclusion (code, not prose). |
| Password managers (KeePass, 1Password, …) | — | — | — | **Disabled by default** | `policy.rs` exclusion. |
| Remote desktop / VM (mstsc, vmconnect, …) | — | — | — | **Disabled by default** | `policy.rs` exclusion; injection unsafe. |
| Password / sensitive field (any app) | drops events | UIA `IsPassword` ⇒ `Sensitive` | — | **Disabled** | Fail closed; value never read. |
| Elevated / higher-integrity window | — | — | skipped | **Unsupported** | Integrity check skips it (UIPI never bypassed). |
| Unknown control / failed UIA read | drops events | `Unknown` (fail closed) | — | **Disabled** | No capture unless positively `NormalProse`. |
| IME composition / dead keys | resets | — | — | **Unsupported** (MVP) | English-only MVP; resets the session. |

## Performance (measured)

Release build, this development machine (the defined low-spec gate is a Step 11
manual item):

| Metric | Measured | Budget (blueprint §5) |
|---|---|---|
| Lexicon `contains` | ~150 ns | — |
| Engine candidate gen p50 / p95 / p99 | 0.86 ms / 3.85 ms / 4.50 ms | p50 < 2 ms / p95 < 5 ms / p99 < 10 ms |
| Production FST size (embedded) | 288 KB (30 040 words) | installed < 20 MB |

The hook callback is allocation-free and does no language work; the bounded channel
drops on overflow. Idle CPU/working-set/wakeups and end-to-end replacement-dispatch
latency must be measured on the running app (see below).

## Manual gates (require a real desktop / defined hardware)

- [ ] App-matrix run: confirm each "(confirm)" row's support level (Notepad, a
      Chromium browser, Edge, Word, a chat app).
- [ ] Default-disabled categories stay disabled (terminal, IDE, password manager).
- [ ] Idle CPU < 0.1% over 60 s; working set < 25 MB; idle wakeups < 2/s (Task
      Manager / `Get-Counter` while `langcheck --background` runs).
- [ ] Rapid typing ≥ 200 WPM: cancellation, no mis-correction; focus-switch storms.
- [ ] 8-hour stability run: no crash, no stuck input observer.
- [ ] Low-spec Windows test machine passes the same gates.

Record results here and promote "(confirm)" rows to a firm level (or down to
Suggestion only / Disabled) as the measurements dictate.
