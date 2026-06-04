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
- **No privilege escalation.** LangCheck never elevates, bypasses UIPI, runs as a
  Windows service, injects DLLs into arbitrary processes, or uses the clipboard
  for replacement.

These are release-blocking invariants (see [`blueprint.md`](blueprint.md)
Sections 1.1 and 12).

## Supported versions

LangCheck is pre-release. During development only the latest `main` is supported.

## Reporting a vulnerability

Please report security vulnerabilities **privately** using GitHub's
[Report a vulnerability](https://github.com/ShreyanshVaibhaw/langcheck/security/advisories/new)
flow (the repository's **Security → Advisories** tab). Do **not** open a public
issue for a security report.

Please include the affected component, reproduction steps, and impact. Because
LangCheck has no telemetry, we rely entirely on user reports for field issues.
