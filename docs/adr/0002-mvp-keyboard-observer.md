# ADR-0002: `WH_KEYBOARD_LL` as the MVP keyboard observer

- **Status:** Accepted (provisional — on-hardware measurements pending; see checklist)
- **Date:** 2026-06-05
- **Deciders:** LangCheck maintainers

## Context

LangCheck must observe physical keyboard input cheaply, distinguish its own
injected events to avoid recursion, and gate capture on a fail-closed focus-safety
signal (`blueprint.md` Sections 8.1, 8.2, 11.1, 29). Two mechanisms were
considered: a low-level keyboard hook (`WH_KEYBOARD_LL`) and Raw Input.

This step is a feasibility spike. The empirical comparison (callback latency, idle
CPU/wakeups, and coverage across real applications) must run on a Windows desktop,
which the implementing environment cannot do; those measurements are listed below
as a required verification gate rather than completed here.

## Decision

Use **`WH_KEYBOARD_LL` on a dedicated thread with a Windows message loop** as the
MVP observer.

- The callback does the minimum work: it drops everything while `capture_allowed`
  is false, ignores events whose `dwExtraInfo` carries `LANGCHECK_INJECTED_MARKER`,
  stamps a monotonic generation, and pushes a compact `InputEvent` into a bounded
  channel (dropping, not blocking, when full).
- Focus safety is read separately on a dedicated COM/UI-Automation thread, which
  toggles `capture_allowed` — the hook is installed once for the process lifetime
  rather than repeatedly hooked/unhooked.
- Integrity-level checks (`integrity.rs`) detect higher-integrity targets so
  replacement (Step 05) can skip them; UIPI is never bypassed.

Raw Input remains a documented alternative (Microsoft recommends it for
asynchronous monitoring) and may be revisited if the measurements below fail.

## Consequences

- The hook callback must return within the `LowLevelHooksTimeout` or be silently
  removed; the design keeps it allocation-free and lock-light. Production replaces
  the prototype `std::sync::mpsc::sync_channel` with a lock-free SPSC ring (Step 06)
  and adds hook-health monitoring.
- `LLKHF_INJECTED` + the `dwExtraInfo` marker give reliable recursion prevention.
- Read-only detection in the focus inspector is currently approximated (enabled ⇒
  editable); the Value/Text-pattern read-only check is refined in Step 06.

## Manual verification checklist (required before this is trusted)

Run `langcheck --spike` and confirm on a real desktop:

- [ ] Callback duration p99 `< 100 µs`; idle CPU and wakeups within Section 5 budgets.
- [ ] Events captured in Notepad, a Chromium browser text area, and Edge.
- [ ] Focus on a **password** field reports `Sensitive` and capture stops.
- [ ] Terminal / code editor and unknown controls report non-`NormalProse`.
- [ ] A higher-integrity (elevated) target is detected and would be skipped.
- [ ] IME composition and dead keys do not produce spurious captured events.
- [ ] LangCheck-injected events (Step 05) are ignored — no recursion.

## Alternatives considered

- **Raw Input:** good for async monitoring, but needs extra work for text
  translation, injected-event handling, and coverage validation. Deferred.
- **`WH_KEYBOARD` (non-low-level):** requires a DLL injected into every process —
  rejected (conflicts with the "no DLL injection" invariant, `blueprint.md` 7.1).
