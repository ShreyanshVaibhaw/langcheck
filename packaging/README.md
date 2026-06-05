# Packaging & Release

Per-user Windows packaging for LangCheck — **no administrator privileges**, no
service, no machine-wide changes (`blueprint.md` Sections 21, 13.4). LangCheck
performs **no update checks and contains no in-app updater**; new versions are
distributed as signed packages obtained out-of-band (Section 21.2).

## Files

| File | Purpose |
|---|---|
| [`install.ps1`](install.ps1) | Per-user install to `%LOCALAPPDATA%\Programs\LangCheck`; copies `langcheck_tsf.dll`; optional start-at-login / launch / `-RegisterTsf`. |
| [`uninstall.ps1`](uninstall.ps1) | Unregister the TSF adapter (if registered), remove app + startup; keep settings unless `-DeleteState`. |

Quick local install (after a release build):

```powershell
cargo build --release
packaging\install.ps1 -StartAtLogin -Launch
# optionally also enable rich/web-editor corrections (machine-wide; prompts for UAC):
packaging\install.ps1 -StartAtLogin -Launch -RegisterTsf
```

### The TSF adapter in packaging (ADR-0008)

`langcheck_tsf.dll` (the optional precision adapter for rich/web editors) is always
shipped next to `langcheck.exe` but is **inert until registered**. Registration is
opt-in and **machine-wide** (it writes under `HKLM`, like any IME), so it is the one
step that needs elevation — `install.ps1 -RegisterTsf` (or `langcheck --register-tsf`
later) self-elevates via UAC. `uninstall.ps1` detects an existing registration and
unregisters it (elevated, and *before* deleting the DLL) so no broken input method
is left behind. The kill switch `langcheck --tsf-disable` turns its corrections off
without unregistering.

## Release process (Step 12)

1. **Build** the optimized binary: `cargo build --workspace --release`
   (→ `target\release\langcheck.exe`; the production dictionary is embedded).
2. **Regenerate the dictionary** if its source changed:
   `cargo run -p dictionary-compiler` (commit the regenerated `en-US.fst` +
   `en-US.meta.json`; the build is reproducible — see ADR-0007).
3. **Pass the release gates** (`blueprint.md` Section 20): `cargo fmt --check`,
   `cargo clippy -D warnings`, `cargo test --workspace`, `cargo deny check`, and the
   offline-invariant + no-typing-history audits (`docs/privacy.md`).
4. **Code sign** `langcheck.exe` **and `langcheck_tsf.dll`** ⛏ *manual — needs your
   certificate*. Both must be signed — the DLL especially, since it loads into other
   processes as a registered TSF text service:
   `signtool sign /fd SHA256 /tr <timestamp-url> /td SHA256 /a target\release\langcheck.exe target\release\langcheck_tsf.dll`
5. **SBOM + third-party notices** ⛏ *manual — needs tooling*: generate an SBOM
   (e.g. `cargo cyclonedx`) and license notices (e.g. `cargo about generate`).
   (CI uploads a CycloneDX SBOM artifact on each build — see `.github/workflows`.)
6. **Package**: zip `langcheck.exe`, **`langcheck_tsf.dll`**, `LICENSE`,
   `install.ps1`, `uninstall.ps1` (+ SBOM/notices). Sign the package if distributing
   an installer executable.
7. **Test** ⛏ *manual — needs a clean machine*: clean install, upgrade-over-previous,
   uninstall (with and without `-DeleteState`); confirm no admin prompt except the
   opt-in TSF step, start-at-login round-trips, the TSF adapter registers/unregisters
   cleanly, and "delete all state" works.
8. **Rollback**: retain the previous signed package; to roll back, re-run its
   installer.

## Manual / user-provided gates (cannot be done in the build environment)

- [ ] Code-signing certificate (step 4) and a signed binary.
- [ ] SBOM + third-party license notices generated (step 5).
- [ ] Install / upgrade / uninstall tested on a clean Windows machine (step 7).
- [ ] Dictionary source + redistribution license approved (done — see ADR-0007).
- [ ] Privacy/security docs current (`docs/privacy.md`, `SECURITY.md`,
      `docs/threat-model.md`) and the offline + no-typing-history audits re-run.

A future signed, self-contained installer (e.g. Inno Setup / MSIX) can wrap these
steps; the per-user, no-admin, reversible-startup, clean-uninstall contract above
must be preserved.
