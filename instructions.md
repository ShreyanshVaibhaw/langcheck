# AI Agent Coding Instructions

## Source of Truth
- Read `blueprint.md` before changing code.
- Implement only the active delivery step from `blueprint.md` Section 24.
- Treat Sections 1.1, 12, 16, 19, 26, 27, and 31 as release-blocking.
- If implementation invalidates the blueprint, stop dependent work and update the blueprint plus an ADR; never silently weaken an invariant.

## Non-Negotiable Invariants
- Product runtime is fully local: no sockets, HTTP, telemetry, analytics, cloud sync, crash upload, remote logging, update checks, downloads, or networked plugins.
- Never persist typing history, raw typed words, sentences, field contents, correction history, or applications typed in.
- Persist only settings and personal dictionary/rule entries explicitly approved by the user.
- `capture_allowed` defaults to `false`.
- Sensitive, non-prose, and unknown fields must not queue, translate, buffer, check, log, or modify keystrokes.
- Never read password or protected-field values.
- Normal user typing always takes priority over correction.
- Never elevate, bypass UIPI, run as a Windows service, inject arbitrary DLLs, or use the clipboard for replacement.

## Architecture Boundaries
- Use Rust and the workspace structure defined in `blueprint.md` Section 15.
- Keep `langcheck-core` platform-independent and deterministic.
- Keep Windows APIs inside `langcheck-windows`.
- Keep language logic and persistence out of the future TSF adapter.
- Use the bundled, read-only, memory-mapped compact FST lexicon.
- Do not use Windows or third-party spell-check providers at runtime.
- Use bounded queues, token lengths, candidate counts, context, and deadlines.
- Prefer fixed threads/message loops; do not add a general async runtime without an approved ADR.

## Input and Safety Rules
- Input callbacks may only classify minimally, copy a compact event, and return.
- No allocation, logging, COM, UI Automation, dictionary lookup, filesystem I/O, blocking, or contended locking in input callbacks.
- Queue overflow, stale focus, unknown input, timeout, or provider failure resets state and disables correction until a clean safe state exists.
- A focus change clears token/context state and sets `capture_allowed = false` before inspecting the new field.
- Revalidate focus, generation, safety, integrity level, and decision freshness immediately before replacement or undo.
- Cancel stale work; never retry a partial replacement blindly.

## Privacy and Persistence
- Keep active tokens and optional future context in volatile memory only.
- Clear token/context data on focus change, uncertainty, pause, exit, or error.
- Never place raw tokens in normal logs, metrics, panic reports, or diagnostics.
- Diagnostics are local, redacted, user-initiated, and never transmitted.
- Write approved state atomically and validate schema, size, and contents.
- Reject dependencies that add networking, telemetry, crash upload, remote logging, cloud sync, update clients, or network-capable plugins.

## Background Lifecycle
- Run as one per-user background process with a visible native tray icon.
- Closing settings must leave the background checker running.
- Explicit `Exit LangCheck` must stop it.
- `Start with Windows` is a reversible per-user option using the quoted HKCU Run command and `langcheck.exe --background`.
- Startup must not open settings, elevate, create a service, or start duplicate broker instances.

## Rust and Windows Code
- Safe Rust is the default.
- Keep `unsafe` blocks small and add a concise safety-invariant comment.
- Wrap Win32 handles and COM lifetimes; convert failures into typed errors.
- Never hold a lock while calling COM, UI Automation, `SendInput`, or the filesystem.
- Preserve user changes and avoid unrelated refactors or dependency churn.

## Expected Commands
Run applicable commands after the Rust workspace exists:

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Use focused checks during development:

```powershell
cargo test -p <crate> <test_name>
cargo clippy -p <crate> --all-targets -- -D warnings
```

## Completion Gates
- Add focused unit/integration tests for every behavior change and regression.
- Test sensitive, non-prose, unknown, elevated, and secure contexts fail closed.
- Test no key event is buffered while `capture_allowed` is false.
- Test injected input cannot recurse and stale decisions cannot commit.
- Verify no raw typing history is persisted or logged.
- Verify the built runtime has no networking or telemetry behavior.
- Measure hot-path latency, idle CPU, memory, and queue-overflow behavior for changes affecting the resident process.
- Do not mark work complete with failing tests, safety gaps, or unexplained performance regressions.

## Commits and Delivery
- Keep each change scoped to one delivery step and its acceptance criteria.
- Include tests, measurements, compatibility impact, and rollback notes where applicable.
- AI commits must include:
  `Co-Authored-By: <agent model name> <noreply@example.com>`
