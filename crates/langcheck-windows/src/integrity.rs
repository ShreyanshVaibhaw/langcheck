//! Integrity-level checks for UIPI safety.
//!
//! Windows User Interface Privilege Isolation lets a process inject input only into
//! processes at an equal or lower integrity level. LangCheck must detect a target
//! at a *higher* integrity level and skip it; it never attempts to bypass UIPI
//! (see `blueprint.md` Sections 8.10 and 12.2). The numeric comparison is pure and
//! unit-tested; only the token/SID reads touch Win32.
//!
//! Implemented in delivery Step 01 (Windows Input and Focus Feasibility Spike).

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// A process integrity level, represented by its mandatory-label RID. Higher RID
/// means higher integrity, so the derived ordering is the integrity ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IntegrityLevel(pub u32);

impl IntegrityLevel {
    /// `SECURITY_MANDATORY_LOW_RID`.
    pub const LOW: Self = Self(0x1000);
    /// `SECURITY_MANDATORY_MEDIUM_RID`.
    pub const MEDIUM: Self = Self(0x2000);
    /// `SECURITY_MANDATORY_HIGH_RID`.
    pub const HIGH: Self = Self(0x3000);
    /// `SECURITY_MANDATORY_SYSTEM_RID`.
    pub const SYSTEM: Self = Self(0x4000);
}

/// Whether `target` is at a strictly higher integrity level than `current`, in
/// which case input injection is blocked by UIPI and must be skipped.
pub fn is_target_higher(current: IntegrityLevel, target: IntegrityLevel) -> bool {
    target > current
}

/// The integrity level of the current (LangCheck) process.
pub fn current() -> windows::core::Result<IntegrityLevel> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing.
    let process = unsafe { GetCurrentProcess() };
    let mut token = HANDLE::default();
    // SAFETY: opening the current process token for query; `token` is written on success.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token)? };
    let result = integrity_of_token(token);
    // SAFETY: closing the token handle we opened above.
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

/// The integrity level of the process owning `window`.
pub fn of_window(window: HWND) -> windows::core::Result<IntegrityLevel> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid writable u32; the call has no other preconditions.
    let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if thread_id == 0 || pid == 0 {
        return Err(windows::core::Error::from_win32());
    }
    // SAFETY: opening the target process with query-limited rights by pid.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? };
    let mut token = HANDLE::default();
    // SAFETY: opening the target process token for query.
    let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    let result = match opened {
        Ok(()) => integrity_of_token(token),
        Err(e) => Err(e),
    };
    // SAFETY: closing the handles we opened (token only if it was set).
    unsafe {
        if !token.is_invalid() {
            let _ = CloseHandle(token);
        }
        let _ = CloseHandle(process);
    }
    result
}

/// Read the integrity-level RID out of an access token's mandatory label.
fn integrity_of_token(token: HANDLE) -> windows::core::Result<IntegrityLevel> {
    let mut needed = 0u32;
    // First call queries the required buffer size (returns ERROR_INSUFFICIENT_BUFFER).
    // SAFETY: passing a null buffer with length 0 to learn the size; `needed` is written.
    unsafe {
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
    }
    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: `buffer` is `needed` bytes; the class matches the TOKEN_MANDATORY_LABEL output.
    unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )?;
    }
    // SAFETY: on success `buffer` holds a TOKEN_MANDATORY_LABEL whose `Label.Sid`
    // is a valid SID for the lifetime of `buffer`; the integrity RID is its last
    // sub-authority. The pointers from GetSidSubAuthority* index within that SID.
    let rid = unsafe {
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        *GetSidSubAuthority(sid, u32::from(count.saturating_sub(1)))
    };
    Ok(IntegrityLevel(rid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_integrity_targets_are_detected() {
        assert!(is_target_higher(
            IntegrityLevel::MEDIUM,
            IntegrityLevel::HIGH
        ));
        assert!(is_target_higher(
            IntegrityLevel::MEDIUM,
            IntegrityLevel::SYSTEM
        ));
        assert!(!is_target_higher(
            IntegrityLevel::HIGH,
            IntegrityLevel::MEDIUM
        ));
        assert!(!is_target_higher(
            IntegrityLevel::MEDIUM,
            IntegrityLevel::MEDIUM
        ));
        assert!(!is_target_higher(
            IntegrityLevel::MEDIUM,
            IntegrityLevel::LOW
        ));
    }

    #[test]
    fn ordering_follows_rid() {
        assert!(IntegrityLevel::LOW < IntegrityLevel::MEDIUM);
        assert!(IntegrityLevel::MEDIUM < IntegrityLevel::HIGH);
        assert!(IntegrityLevel::HIGH < IntegrityLevel::SYSTEM);
    }

    #[test]
    fn current_process_is_at_least_low() {
        // Smoke test of the live token read on the test process itself.
        let level = current().expect("read current integrity");
        assert!(level >= IntegrityLevel::LOW, "got {level:?}");
    }
}
