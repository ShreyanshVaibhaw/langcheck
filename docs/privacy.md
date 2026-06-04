# Privacy

> **Status:** placeholder. Authoritative privacy documentation is written during
> delivery Step 07 (Privacy and Safety Hardening). See [`blueprint.md`](../blueprint.md)
> Section 12.

LangCheck's privacy guarantees are release-blocking invariants:

- No inbound or outbound network functionality in any release.
- No typed text, history, settings, diagnostics, or identifiers leave the device.
- No telemetry, analytics, crash upload, cloud sync, remote logging, or update check.
- No full-field or full-document collection, and no clipboard access.
- No typing history is persisted; only user-approved settings, personal words,
  forced replacement rules, and blocked pairs are stored.
- All LangCheck state can be deleted from the settings UI.

See [`SECURITY.md`](../SECURITY.md) and `blueprint.md` Sections 1.1 and 12.
