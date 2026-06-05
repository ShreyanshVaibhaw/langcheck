//! Per-user (un)registration of the post-MVP TSF adapter DLL.
//!
//! The broker registers `langcheck_tsf.dll` by loading it and calling its COM
//! self-(un)registration entry points **in-process**, so the writes land in the
//! invoking user's `HKCU` — unlike `regsvr32`, which auto-elevates and would target
//! the *elevated* user's hive. No elevation and no service are ever used; the
//! adapter is opt-in and never registered automatically (`blueprint.md` Step 13,
//! Sections 7.1, 13.4).
//!
//! This only loads our own DLL and calls a zero-argument export; it contains no
//! language logic. The actual registration effect (COM CLSID + TSF profile) lives
//! in `langcheck-tsf` and must be verified on a real desktop.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{s, Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// Signature of the DLL's `DllRegisterServer` / `DllUnregisterServer` exports.
type SelfRegFn = unsafe extern "system" fn() -> HRESULT;

/// Register the TSF adapter for the current user by invoking the DLL's
/// `DllRegisterServer`. `dll_path` is the full path to `langcheck_tsf.dll`.
pub fn register(dll_path: &Path) -> Result<()> {
    call_self_reg(dll_path, true)
}

/// Unregister the TSF adapter for the current user (`DllUnregisterServer`).
pub fn unregister(dll_path: &Path) -> Result<()> {
    call_self_reg(dll_path, false)
}

/// Build a `langcheck-windows`-style error with a human-readable message.
fn err(message: &str) -> Error {
    Error::new(HRESULT(-1), message)
}

/// Load `dll_path`, call its self-(un)registration export, then free it. The
/// registration writes persist independently of the loaded module.
fn call_self_reg(dll_path: &Path, register: bool) -> Result<()> {
    let wide: Vec<u16> = dll_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid NUL-terminated wide path that outlives the call.
    let module = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }?;
    let outcome = call_loaded(module, register);
    // SAFETY: `module` was just loaded; release our reference.
    unsafe {
        let _ = FreeLibrary(module);
    }
    outcome
}

/// Resolve and call the self-(un)registration export on an already-loaded module.
fn call_loaded(module: HMODULE, register: bool) -> Result<()> {
    let name = if register {
        s!("DllRegisterServer")
    } else {
        s!("DllUnregisterServer")
    };
    // SAFETY: `module` is a valid loaded module; `name` is a static NUL-terminated
    // ASCII export name.
    let proc = unsafe { GetProcAddress(module, name) }
        .ok_or_else(|| err("TSF adapter is missing its registration entry point"))?;
    // SAFETY: the export's ABI is `extern "system" fn() -> HRESULT` by contract (it
    // is our own DLL), so transmuting the resolved function pointer is sound.
    let func: SelfRegFn = unsafe { std::mem::transmute(proc) };
    // SAFETY: calling the zero-argument COM self-(un)registration entry point.
    let hr = unsafe { func() };
    hr.ok()
}
