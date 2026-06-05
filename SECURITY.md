# Security Policy

LangCheck is a local input utility that observes keystrokes and injects text
corrections. Security and privacy are product requirements, not features. This
document describes the security model and how to report issues.

## Security & privacy model

LangCheck is designed to **fail closed**:

- **No network.** No release contains any inbound or outbound networking,
  telemetry, analytics, crash upload, remote logging, update check, or download.
- **No typing history.** Typed words, sentences, field contents, and corrections
  are never persisted. Only user-approved settings, personal words, forced
  replacements, and blocked pairs are stored locally.
- **Sensitive fields are inert.** Password, PIN/OTP, payment, authentication,
  recovery-code, private-key, security-answer, non-prose, and unknown fields are
  never queued, translated, buffered, checked, logged, or modified. Protected
  field values are never read.
- **No privilege escalation in normal use.** The broker never elevates, bypasses
  UIPI, runs as a Windows service, injects DLLs into arbitrary processes, or uses
  the clipboard. The one exception is the **optional** TSF adapter's one-time
  registration (below), which is opt-in and clearly prompted.

These are release-blocking invariants (see [`blueprint.md`](blueprint.md)
Sections 1.1 and 12).

## Optional TSF precision adapter

The post-MVP TSF adapter (`langcheck_tsf.dll`) is an opt-in, off-by-default
precision path for rich/web editors (ADR-0008). Its security properties:

- **Standard text service, not DLL injection.** It is a registered TSF text
  service that Windows loads into an app only when the user selects the "LangCheck"
  input method — not a DLL injected into arbitrary processes.
- **One-time, opt-in, machine-wide registration needs admin** (like every IME — TSF
  text services live under `HKLM`). `langcheck --register-tsf` self-elevates with a
  clear UAC prompt; nothing registers automatically. `--unregister-tsf` reverses it;
  `--tsf-disable` is a kill switch that needs no elevation.
- **No language logic or persistence in-process.** The adapter only reads the active
  token, asks the broker over a **same-user, local-only** named pipe
  (SID-scoped DACL + `PIPE_REJECT_REMOTE_CLIENTS`), and applies the answer. It logs
  and transmits nothing.
- **Fail-open + never-wrong-text.** Any error leaves host text untouched, and a
  range is edited only after its text is re-verified to equal the detected token.

## Supported versions

LangCheck is pre-release. During development only the latest `main` is supported.

## Reporting a vulnerability

Please report security vulnerabilities **privately** using GitHub's
[Report a vulnerability](https://github.com/ShreyanshVaibhaw/langcheck/security/advisories/new)
flow (the repository's **Security → Advisories** tab). Do **not** open a public
issue for a security report.

Please include the affected component, reproduction steps, and impact. Because
LangCheck has no telemetry, we rely entirely on user reports for field issues.
