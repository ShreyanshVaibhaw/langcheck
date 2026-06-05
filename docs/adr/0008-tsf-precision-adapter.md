# ADR-0008: TSF precision adapter for exact-range correction in rich/web editors

- **Status:** Accepted (host-verified working in WordPad; broader app-matrix + memory-overhead gates pending — see checklist)
- **Date:** 2026-06-06
- **Deciders:** LangCheck maintainers

## Context

The MVP corrects by synthesizing keystrokes (`SendInput`: backspaces + Unicode
re-type) after a safe boundary (ADR-0002, `blueprint.md` §8.1, 11.4). This is
reliable in plain Win32 `Edit` controls but **not** in rich/web/Electron editors
(many note apps, browser fields, some markdown editors), which intercept or
re-handle injected input — the correction silently fails to apply. Field testing in
earlier sessions confirmed this is a hard limitation of the synthetic-keystroke
approach, not a tuning issue.

Windows' native mechanism for editing text *in place* (by range, not by keystroke)
is the **Text Services Framework (TSF)**: an in-process COM **text service** (TIP)
loaded into the host app, which observes edits and replaces text through TSF
*edit sessions*. This is how IMEs and autocorrect engines integrate with
TSF-aware applications.

Two hard constraints shaped the design (`blueprint.md` §7.1, 11.4, 12.3):
the adapter runs **inside other processes**, so a bug can destabilise the host; and
**all language logic and persistence must stay in the broker** (the only trusted,
auditable process).

## Decision

Ship a **minimal, fail-open, opt-in TSF adapter** (`langcheck-tsf`, a `cdylib` COM
text service) that contains **no language logic**. It:

1. Registers per machine as an en-US keyboard TIP (COM CLSID under
   `HKLM\Software\Classes\CLSID` + a TSF profile via `ITfInputProcessorProfiles` /
   `ITfCategoryMgr`). Registration is **machine-wide and requires elevation** — like
   every IME — so the broker's `--register-tsf` self-elevates via UAC.
2. On activation, advises an `ITfThreadMgrEventSink` (focus) which advises an
   `ITfTextEditSink` on the focused context. In `OnEndEdit` it reads the text just
   before the caret (read-only), and when a word is completed by a boundary it asks
   the **broker** over a **same-user, local-only named pipe** (`langcheck-ipc`;
   protocol in `langcheck-core::ipc`).
3. If the broker returns a correction, it applies it through an **asynchronous
   read-write `ITfEditSession`** that `SetText`-replaces the word's range
   (`TF_ST_CORRECTION`).

The broker remains the sole owner of the dictionary, ranking, confidence policy,
and persistence; the same `engine::decide` serves the keystroke path and the TSF
path so they cannot diverge. A **kill switch** (`tsf_adapter_enabled` config,
`--tsf-enable`/`--tsf-disable`) disables TSF corrections without unregistering.

**Safety guarantees:**
- **Fail-open.** Any COM error, missing broker, or uncertainty leaves host text
  untouched. Loading/activating the adapter never blocks or alters input.
- **Never replace the wrong text.** Before any edit, the word's range is re-derived
  and its text is re-read and required to equal the detected token; only then is it
  handed to `SetText`.
- **No language logic / no persistence / no typed text logged** in the adapter or
  the diagnostics (the broker's request log is a count only).

## Consequences

- **+** Exact-range correction works in rich/web editors — host-verified: typing
  `wierd ` in WordPad with the TIP active auto-corrects to `weird ` via an edit
  session.
- **+** The broker stays the only process with language logic/persistence; the
  adapter is a thin, auditable shim (it links only `langcheck-core` +
  `langcheck-ipc`, never the broker's Win32 integration).
- **−** Registration needs one-time administrator elevation (machine-wide TIP).
- **−** In-process COM carries host-stability risk. Mitigated by fail-open, the
  range re-verification, an off-host COM harness (`--tsf-comtest`) that proves
  activate→advise→focus→deactivate runs without faulting, an audited panic-free
  runtime path (no unwrap/expect/panic; bounds clamped on host-provided lengths),
  and **panic containment** (`catch_unwind`) on the hottest callbacks (`OnEndEdit`,
  `DoEditSession`) so a panic can never unwind across the FFI boundary into the host.
- **−** The live edit path can only be fully verified inside a real editor; CI and
  the harness cover everything else (protocol, transport, decision logic, COM
  plumbing).

## Manual verification checklist (required before this is broadly trusted)

- [x] Elevated register/unregister round-trip is clean and reversible (HKLM TIP +
      CLSID written then removed).
- [x] COM activate/advise/teardown runs without faulting (`--tsf-comtest`).
- [x] In-DLL IPC reaches the broker (`--tsf-selftest`).
- [x] Live correction in a real editor (WordPad: `wierd `→`weird `).
- [ ] App-matrix: a Chromium browser field, Microsoft Edge, a notes/markdown
      editor, a chat app, Microsoft Word — correction applies (or fails open) and
      typing is never disrupted.
- [~] No double-correction when `--background` (the MVP `SendInput` path) and the
      TSF adapter are both active. *Mitigated in code:* the broker records which
      window the adapter is handling (from its Evaluate requests) and the MVP path
      defers there for a short interval (`SharedState::tsf_handling`); steady typing
      can't double-correct. The first word in a newly-focused TSF field could still
      race — confirm on a real desktop.
- [ ] Host-process memory overhead of `langcheck_tsf.dll` measured and approved.
- [ ] 8-hour stability with the TIP active in a host app: no crash, no stuck input.

## Alternatives considered

- **Keep `SendInput` only (the MVP).** Rejected for rich/web editors — proven
  unreliable there; the TSF adapter exists precisely for that gap. `SendInput`
  remains the path for plain Win32 fields.
- **UI Automation `TextPattern`.** Good for *reading* text, but range editing
  across apps is inconsistent and not the supported insertion mechanism; TSF is the
  Windows-sanctioned editing surface.
- **Per-application integrations** (browser extensions, app plugins). Unscalable and
  out of scope for a local OS-level utility.
- **A no-elevation, per-user TIP.** Not supported by TSF — text-service registration
  is machine-wide (`HKLM\SOFTWARE\Microsoft\CTF`); verified empirically that a
  non-elevated `Register` fails. Hence the elevated, opt-in install.
