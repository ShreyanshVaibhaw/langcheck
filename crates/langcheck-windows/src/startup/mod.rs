//! Per-user start-at-login registration and single-instance enforcement.
//!
//! Start-at-login registers a quoted `langcheck.exe --background` command under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; it is off by default and
//! fully reversible, and never creates a service or elevates (`blueprint.md`
//! Section 13.4). A per-user named mutex enforces a single broker instance.
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).

use std::path::Path;

use windows::core::{Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, HANDLE};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS,
    REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;

/// The `Run` subkey under `HKEY_CURRENT_USER`.
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The value name LangCheck registers.
const RUN_VALUE_NAME: &str = "LangCheck";

/// The launch command registered for start-at-login: the quoted executable path
/// plus `--background` (`blueprint.md` Section 13.4). Pure and unit-tested.
pub fn launch_command(exe: &Path) -> String {
    format!("\"{}\" --background", exe.display())
}

/// Enable or disable starting LangCheck at sign-in. Uses the current executable's
/// path. Disabling removes only LangCheck's own value.
pub fn set_start_at_login(enable: bool) -> Result<()> {
    if enable {
        let exe = std::env::current_exe().map_err(|e| {
            windows::core::Error::new(windows::core::HRESULT(-1), format!("current_exe: {e}"))
        })?;
        registry_set_string(RUN_SUBKEY, RUN_VALUE_NAME, &launch_command(&exe))
    } else {
        registry_delete_value(RUN_SUBKEY, RUN_VALUE_NAME)
    }
}

/// Whether LangCheck is registered to start at sign-in.
pub fn is_start_at_login_enabled() -> bool {
    registry_value_exists(RUN_SUBKEY, RUN_VALUE_NAME)
}

fn registry_set_string(subkey: &str, name: &str, value: &str) -> Result<()> {
    let key = create_key(subkey, KEY_SET_VALUE)?;
    // REG_SZ data is the UTF-16 string including its NUL terminator, as bytes.
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: reinterpreting the `u16` buffer as bytes; the length is exactly
    // `wide.len() * 2` and `wide` outlives `bytes` (both live only in this call).
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
    let name = HSTRING::from(name);
    // SAFETY: `key` is a valid open key; `name` is a valid wide string; `bytes` is
    // a valid REG_SZ payload for its declared length.
    let result = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)) };
    close_key(key);
    result.ok()
}

fn registry_delete_value(subkey: &str, name: &str) -> Result<()> {
    let key = open_key(subkey, KEY_SET_VALUE)?;
    let name = HSTRING::from(name);
    // SAFETY: `key` is valid; `name` is a valid wide string.
    let status = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
    close_key(key);
    // Deleting an absent value is treated as success (idempotent disable).
    if status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        status.ok()
    }
}

fn registry_value_exists(subkey: &str, name: &str) -> bool {
    let Ok(key) = open_key(subkey, KEY_QUERY_VALUE) else {
        return false;
    };
    let name = HSTRING::from(name);
    // SAFETY: `key`/`name` are valid; null out-params query only existence/type.
    let status = unsafe { RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, None) };
    close_key(key);
    status.is_ok()
}

fn open_key(subkey: &str, access: REG_SAM_FLAGS) -> Result<HKEY> {
    let mut key = HKEY::default();
    let subkey = HSTRING::from(subkey);
    // SAFETY: HKEY_CURRENT_USER is a predefined key; `subkey` is a valid wide
    // string; `key` is written on success.
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), 0, access, &mut key).ok()? };
    Ok(key)
}

/// Create-or-open a subkey under `HKEY_CURRENT_USER` for writing.
fn create_key(subkey: &str, access: REG_SAM_FLAGS) -> Result<HKEY> {
    let mut key = HKEY::default();
    let subkey = HSTRING::from(subkey);
    // SAFETY: HKEY_CURRENT_USER is predefined; `subkey` is a valid wide string;
    // `key` is written on success. No class or security attributes are supplied.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            access,
            None,
            &mut key,
            None,
        )
        .ok()?
    };
    Ok(key)
}

fn close_key(key: HKEY) {
    // SAFETY: closing a key we opened.
    unsafe {
        let _ = RegCloseKey(key);
    }
}

/// A held single-instance lock (a named mutex). Dropping it releases the mutex.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// Try to become the single instance for `name` (per-user). Returns `None` if
    /// another instance already holds it.
    pub fn acquire(name: &str) -> Option<Self> {
        let wide = HSTRING::from(name);
        // SAFETY: creating/opening a named mutex; `name` is a valid wide string.
        let handle = unsafe { CreateMutexW(None, true, PCWSTR(wide.as_ptr())) }.ok()?;
        // SAFETY: reading the thread-local last error immediately after the call.
        let already = unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS;
        if already {
            // SAFETY: closing the handle we just opened; we are not the owner.
            unsafe {
                let _ = CloseHandle(handle);
            }
            None
        } else {
            Some(Self { handle })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: closing the mutex handle we own (also releases ownership).
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_command_is_quoted_and_background() {
        let cmd = launch_command(Path::new(r"C:\Program Files\LangCheck\langcheck.exe"));
        assert_eq!(
            cmd,
            "\"C:\\Program Files\\LangCheck\\langcheck.exe\" --background"
        );
    }

    #[test]
    fn registry_string_round_trips_in_isolated_subkey() {
        use windows::Win32::System::Registry::RegDeleteKeyW;

        // A dedicated, throwaway subkey so neither the real Run key nor any other
        // state is touched (robust across environments and CI).
        let subkey = format!(r"Software\LangCheckTest-{}", std::process::id());
        let name = "value";

        assert!(!registry_value_exists(&subkey, name));
        registry_set_string(&subkey, name, "hello").expect("set");
        assert!(registry_value_exists(&subkey, name));
        registry_delete_value(&subkey, name).expect("delete");
        assert!(!registry_value_exists(&subkey, name));
        // Deleting an absent value is not an error.
        registry_delete_value(&subkey, name).expect("idempotent delete");

        // Clean up the (now empty) test subkey.
        let wide = HSTRING::from(subkey.as_str());
        // SAFETY: deleting the empty test subkey we created above.
        unsafe {
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
        }
    }

    #[test]
    fn single_instance_blocks_a_second_acquire() {
        let name = format!("Local\\langcheck-test-{}", std::process::id());
        let first = SingleInstance::acquire(&name).expect("first acquire");
        assert!(SingleInstance::acquire(&name).is_none(), "second must fail");
        drop(first);
        // After releasing, acquisition succeeds again.
        assert!(SingleInstance::acquire(&name).is_some());
    }
}
