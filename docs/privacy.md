# Privacy

LangCheck's privacy guarantees are **release-blocking invariants**, not features
(`blueprint.md` Sections 1.1 and 12). This document is authoritative; it is kept in
sync as the implementation evolves and is re-audited at each release gate
(Section 20).

## Guarantees

- **No network.** No release contains any inbound or outbound networking,
  telemetry, analytics, crash upload, remote logging, cloud sync, update check, or
  in-app download.
- **No typing history.** Typed words, sentences, field contents, and corrections
  are never persisted. Only user-approved settings, personal words, forced
  replacement rules, and blocked pairs are stored (delivery Steps 08 and 10).
- **Sensitive fields are inert.** Password, PIN/OTP, payment, authentication,
  recovery-code, private-key, security-answer, non-prose, and unknown fields are
  never queued, translated, buffered, checked, logged, or modified. Protected field
  *values* are never read.
- **No clipboard.** Replacement uses synthetic keystrokes (`SendInput`), never the
  clipboard.
- **No full-field/full-document collection.** Only the single active token is held,
  in volatile memory, and it is cleared on focus change, navigation, pause, or exit.
- **Metrics are numeric-only.** In-memory counters never contain typed text and are
  discarded on exit (`crates/langcheck-app/src/diagnostics.rs` holds only `u64`
  atomics).

## How the guarantees are enforced

| Guarantee | Mechanism |
|---|---|
| Capture off by default | `input::CAPTURE_ALLOWED` starts `false`; enabled only for a positively-classified prose field (`focus::classify_field`, fail-closed). |
| Sensitive/unknown fields skipped | `classify_field` → `Sensitive`/`NonProse`/`Unknown` ⇒ no capture; any UIA read failure ⇒ `Unknown`. |
| Excluded app categories | `policy::is_default_excluded` (terminals, password managers, remote-desktop/VM, IDEs); unknown foreground process fails closed. |
| Never inject into higher integrity | `integrity::is_target_higher` gate before `SendInput` (UIPI never bypassed). |
| No recursion | LangCheck-injected events carry `LANGCHECK_INJECTED_MARKER` and are ignored by the observer. |
| No networking deps | `deny.toml` bans HTTP/gRPC/async-runtime/telemetry crates; CI `cargo deny check` enforces it. |

## Dependency audit (offline guarantee)

The release dependency tree contains no networking, telemetry, or async-runtime
crates. As of this writing the third-party runtime dependencies are:

```
fst, utf8-ranges            # offline FST lexicon (no I/O beyond the mapped bytes)
smallvec                    # bounded collections
windows, windows-core,      # Win32 / COM / UI Automation FFI
windows-result, windows-strings, windows-targets, windows_x86_64_msvc,
windows-implement, windows-interface   # (+ proc-macro2, quote, syn, unicode-ident as build-time macro deps)
```

Verify locally:

```powershell
cargo tree --workspace            # inspect every dependency
cargo deny check                  # enforce the license + offline-ban policy
```

## Verifying the runtime behavior

- `langcheck --spike` prints aggregate counters and the focus classification only —
  never keystrokes. Focusing a password field shows `Sensitive` and capture stops.
- `langcheck --run` corrects only in safe prose fields; sensitive/unknown/excluded
  fields and higher-integrity targets are skipped.

See [`SECURITY.md`](../SECURITY.md), [`docs/threat-model.md`](threat-model.md), and
`blueprint.md` Sections 1.1, 12, and 19.6.
