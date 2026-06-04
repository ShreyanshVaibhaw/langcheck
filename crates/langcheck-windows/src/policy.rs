//! Default application-exclusion policy and foreground-process identification.
//!
//! Some application categories are sensitive or destructive and must have
//! autocorrection disabled by default — password managers, terminals/shells,
//! remote-desktop and VM clients, and code editors/IDEs (`blueprint.md`
//! Sections 12.2, 22). The match is pure and unit-tested on the executable base
//! name; only reading the foreground process name touches Win32. Callers fail
//! closed when the process name cannot be determined.
//!
//! Implemented in delivery Step 07 (Privacy and Safety Hardening).

use windows::core::PWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Executable base names excluded from autocorrection by default. Compared
/// case-insensitively. Users can add their own exclusions (delivery Step 08).
const DEFAULT_EXCLUDED: &[&str] = &[
    // Terminals and shells.
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "windowsterminal.exe",
    "wt.exe",
    "conhost.exe",
    "alacritty.exe",
    "mintty.exe",
    "wezterm-gui.exe",
    // Remote desktop / VM consoles.
    "mstsc.exe",
    "vmconnect.exe",
    "vncviewer.exe",
    "teamviewer.exe",
    "anydesk.exe",
    // Password managers.
    "1password.exe",
    "keepass.exe",
    "keepassxc.exe",
    "bitwarden.exe",
    "lastpass.exe",
    "dashlane.exe",
    // Code editors / IDEs (correction off by default; blueprint 3.3).
    "code.exe",
    "devenv.exe",
    "idea64.exe",
    "pycharm64.exe",
    "clion64.exe",
    "sublime_text.exe",
    "rider64.exe",
];

/// Whether a process (given any path or base name) is excluded by default.
pub fn is_default_excluded(process_name: &str) -> bool {
    let base = process_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(process_name)
        .to_ascii_lowercase();
    DEFAULT_EXCLUDED.iter().any(|excluded| *excluded == base)
}

/// The full image path of the foreground window's process, or `None` if it cannot
/// be determined (callers treat `None` as excluded — fail closed).
pub fn foreground_process_name() -> Option<String> {
    // SAFETY: GetForegroundWindow may return a null HWND.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid writable u32.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // SAFETY: opening the process with query-limited rights by pid.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    let mut buffer = [0u16; 260];
    let mut size = buffer.len() as u32;
    // SAFETY: `buffer`/`size` describe a valid writable wide buffer; `handle` is live.
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    // SAFETY: closing the handle we opened.
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.ok()?;
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_categories_match_case_insensitively_on_base_name() {
        assert!(is_default_excluded(r"C:\Windows\System32\cmd.exe"));
        assert!(is_default_excluded("PowerShell.exe"));
        assert!(is_default_excluded(
            r"C:\Program Files\KeePassXC\KeePassXC.exe"
        ));
        assert!(is_default_excluded("/mnt/c/.../Code.exe"));
        assert!(is_default_excluded("mstsc.exe"));
    }

    #[test]
    fn ordinary_apps_are_not_excluded() {
        assert!(!is_default_excluded(r"C:\Windows\System32\notepad.exe"));
        assert!(!is_default_excluded("chrome.exe"));
        assert!(!is_default_excluded("WINWORD.EXE"));
    }
}
