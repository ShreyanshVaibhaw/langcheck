# LangCheck

> A Windows-first, local-only, ultra-fast spelling autocorrect utility written in Rust.

LangCheck runs quietly in the background, observes physical keyboard input **only**
in fields it can positively confirm are safe prose, and applies **only
high-confidence** spelling corrections after a safe word boundary — with an
immediate undo. It has **no network capability of any kind**.

> **Status:** MVP feature set (delivery Steps 00–10) **and the TSF precision
> adapter (Step 13)** are built and CI-green: keyboard observer, UI-Automation focus
> safety, the offline 30k-word compact-FST engine, conservative ranking/confidence,
> `SendInput` replacement, tray + persistence, immediate undo, personal dictionary —
> plus an opt-in in-process COM text service that corrects in rich/web editors via
> TSF edit-session replacement (host-verified in WordPad: `wierd `→`weird `). Steps
> 11–12 (compatibility/perf hardening, **signed** installer) need on-hardware
> verification and a code-signing certificate. See [`blueprint.md`](blueprint.md)
> Section 27 for live step status and [`docs/compatibility.md`](docs/compatibility.md)
> for how correction behaves and which apps are supported.

## Running it

```powershell
cargo build --release
.\target\release\langcheck.exe --background   # tray app (right-click the tray icon)
.\target\release\langcheck.exe --run          # console mode with live metrics
```

A typo is corrected when you type a word, then **space/period, then pause briefly**
(see "How correction behaves" in [`docs/compatibility.md`](docs/compatibility.md)).
Correction is reliable in standard Win32 text fields out of the box. For rich/web
editors, the **opt-in TSF adapter** (`langcheck --register-tsf`, then select the
"LangCheck" input method) corrects in place via TSF — see "The TSF precision
adapter" in the compatibility doc. Sensitive fields, terminals, password managers,
and elevated windows are never touched.

## Non-negotiable invariants

1. **Fully local** — no inbound/outbound networking, telemetry, cloud sync, crash
   upload, remote diagnostics, update checks, or downloads.
2. **No typing history** — never persists typed words, sentences, fields, or
   corrections. Only user-approved settings and dictionary/rule entries are stored.
3. **Sensitive fields are inert** — password, PIN/OTP, payment, authentication,
   non-prose, and unknown fields are never queued, buffered, checked, or modified.
4. **User-controlled background operation** — tray icon, optional start-at-login;
   closing settings does not exit.
5. **No security bypass** — never bypasses UIPI or runs as a service, and never
   elevates in normal operation. The only elevation is the **optional** TSF adapter's
   one-time, opt-in, clearly-prompted registration (machine-wide, like any IME).

See [`blueprint.md`](blueprint.md) Section 1.1 and [`SECURITY.md`](SECURITY.md).

## Workspace layout

| Crate / path | Purpose |
|---|---|
| [`crates/langcheck-core`](crates/langcheck-core) | Platform-independent token/session/ranking engine. No OS deps. |
| [`crates/langcheck-lexicon`](crates/langcheck-lexicon) | Dictionary lookup behind a trait; bundled offline compact FST. |
| [`crates/langcheck-windows`](crates/langcheck-windows) | Windows integration: input, focus, replacement, tray, startup. |
| [`crates/langcheck-app`](crates/langcheck-app) | The `langcheck.exe` broker: coordinator, config, persistence, TSF broker. |
| [`crates/langcheck-ipc`](crates/langcheck-ipc) | Same-user, local-only named-pipe transport for the broker ↔ TSF-adapter channel. |
| [`crates/langcheck-tsf`](crates/langcheck-tsf) | Opt-in TSF precision adapter (in-process COM text service) for rich/web editors. |
| [`crates/langcheck-bench`](crates/langcheck-bench) | Benchmarks for the hot path and lexicon. |
| [`tools/dictionary-compiler`](tools/dictionary-compiler) | Build-time tool that compiles the FST lexicon. |
| [`docs/adr`](docs/adr) | Architecture Decision Records. |

`langcheck-core` must never depend on Windows APIs, UI frameworks, or a concrete
lexicon backend; CI enforces this by building and testing it on Linux.

## Building

Requires the toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml)
(Rust 1.93.1, `x86_64-pc-windows-msvc`). On Windows:

```powershell
cargo build --workspace
cargo test  --workspace
cargo build --workspace --release
```

The same quality gates run in CI on every push and pull request:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                  # dependency licenses & advisories
pwsh scripts\offline-audit.ps1    # offline invariant: no networking in source
```

CI additionally builds `langcheck-core` + `langcheck-lexicon` on Linux (platform
independence) and uploads a CycloneDX SBOM.

## License

Licensed under the [MIT License](LICENSE). MIT was chosen as a permissive default
for the bootstrap; revisit before public release if patent terms or dual
(MIT OR Apache-2.0) licensing are desired.
