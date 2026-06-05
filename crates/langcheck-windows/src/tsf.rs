//! (Un)registration of the post-MVP TSF adapter DLL.
//!
//! The broker registers `langcheck_tsf.dll` by loading it and calling its COM
//! self-(un)registration entry point **in-process** (rather than via `regsvr32`),
//! which keeps error reporting in our hands and adds no external dependency.
//!
//! TSF text-service registration is **machine-wide and requires administrator
//! elevation** (it writes under `HKLM\SOFTWARE\Microsoft\CTF` and
//! `HKLM\Software\Classes\CLSID`) — like every IME. [`is_elevated`] +
//! [`relaunch_elevated`] let the broker re-launch itself via UAC when needed. It
//! is opt-in and never registered automatically (`blueprint.md` Step 13, §7.1).
//!
//! This module only loads our own DLL and calls zero-argument exports; it contains
//! no language logic. The registration effect (COM CLSID + TSF profile) lives in
//! `langcheck-tsf`.

use core::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{s, w, Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, FreeLibrary, HANDLE, HMODULE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;

/// Signature of the DLL's `DllRegisterServer` / `DllUnregisterServer` exports.
type SelfRegFn = unsafe extern "system" fn() -> HRESULT;

/// Register the TSF adapter (machine-wide) by invoking the DLL's
/// `DllRegisterServer`. Requires elevation. `dll_path` is the full path to
/// `langcheck_tsf.dll`.
pub fn register(dll_path: &Path) -> Result<()> {
    call_self_reg(dll_path, true)
}

/// Unregister the TSF adapter (machine-wide) via `DllUnregisterServer`. Requires
/// elevation.
pub fn unregister(dll_path: &Path) -> Result<()> {
    call_self_reg(dll_path, false)
}

/// Build a `langcheck-windows`-style error with a human-readable message.
fn err(message: &str) -> Error {
    Error::new(HRESULT(-1), message)
}

/// Whether the current process is running elevated (an admin token).
///
/// TSF text-service registration is machine-wide and requires elevation, so the
/// broker uses this to decide whether to relaunch itself via UAC.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess is a pseudo-handle; `token` is written on success.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: `token` is valid; we pass a correctly sized TOKEN_ELEVATION buffer
    // and its length, and an out-param for the bytes written.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: `token` is a valid open handle obtained above.
    unsafe {
        let _ = CloseHandle(token);
    }
    queried.is_ok() && elevation.TokenIsElevated != 0
}

/// Relaunch this executable elevated (UAC "runas") with a single argument, e.g.
/// `--register-tsf`. Returns once the elevated process has been *launched*; its
/// work then proceeds independently. Errors if the user declines the UAC prompt.
pub fn relaunch_elevated(arg: &str) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| err(&format!("current_exe: {e}")))?;
    let exe_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = arg.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: all string pointers are valid NUL-terminated wide strings that
    // outlive the call; the "runas" verb triggers the UAC elevation prompt.
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("runas"),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_NORMAL,
        )
    };
    // ShellExecuteW returns an HINSTANCE whose value is > 32 on success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(err("elevation request failed or was declined"))
    }
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
