//! Native Win32 system-tray icon and context menu.
//!
//! A message-only window receives the tray callback and menu commands; the menu
//! (status, enable/disable, pause/resume, open settings, exit) drives a
//! [`TrayHandler`] supplied by the broker. No Electron/WebView is used
//! (`blueprint.md` Sections 13.1, 13.2).
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).
//!
//! NOTE (manual verification): the icon, menu, and click handling are compiled but
//! not runtime-verified from the build environment; confirm with `langcheck`
//! running in the background.

use std::cell::RefCell;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassExW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU, HWND_MESSAGE,
    IDI_APPLICATION, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SW_SHOWNORMAL, TPM_RIGHTBUTTON,
    WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP,
    WNDCLASSEXW, WS_OVERLAPPED,
};

/// Tray callback message (`WM_APP + 1`).
const WM_TRAYCALLBACK: u32 = WM_APP + 1;
const ID_STATUS: usize = 1;
const ID_TOGGLE_ENABLED: usize = 2;
const ID_TOGGLE_PAUSE: usize = 3;
const ID_OPEN_SETTINGS: usize = 4;
const ID_EXIT: usize = 5;
const TRAY_UID: u32 = 1;

/// Current state shown in the menu.
#[derive(Debug, Clone, Copy)]
pub struct TrayStatus {
    pub enabled: bool,
    pub paused: bool,
}

/// Actions the tray menu invokes on the broker. Methods run on the tray thread.
pub trait TrayHandler {
    /// Toggle the global enable/disable kill switch.
    fn toggle_enabled(&self);
    /// Toggle pause.
    fn toggle_pause(&self);
    /// Open the settings (the config file) for editing.
    fn open_settings(&self);
    /// Request shutdown (the tray then exits its loop).
    fn request_exit(&self);
    /// Current state for the menu labels.
    fn status(&self) -> TrayStatus;
}

thread_local! {
    static TRAY_HANDLER: RefCell<Option<Box<dyn TrayHandler>>> = const { RefCell::new(None) };
}

/// Run the tray: register the window class, create the message-only window, add
/// the icon, and pump messages until the menu's Exit (or `WM_QUIT`). Blocks the
/// calling thread; returns when the tray is dismissed.
pub fn run_tray(handler: Box<dyn TrayHandler>) -> windows::core::Result<()> {
    TRAY_HANDLER.with(|cell| *cell.borrow_mut() = Some(handler));
    let result = run_tray_inner();
    TRAY_HANDLER.with(|cell| *cell.borrow_mut() = None);
    result
}

fn run_tray_inner() -> windows::core::Result<()> {
    // SAFETY: GetModuleHandleW(None) yields this process's module handle.
    let hmodule = unsafe { GetModuleHandleW(None)? };
    let hinstance = HINSTANCE(hmodule.0);
    let class_name = w!("LangCheckTrayWindow");

    let wndclass = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap(),
        lpfnWndProc: Some(tray_wndproc),
        hInstance: hinstance,
        lpszClassName: class_name,
        ..Default::default()
    };
    // SAFETY: `wndclass` is fully initialized with a valid wndproc and class name.
    let atom = unsafe { RegisterClassExW(&wndclass) };
    if atom == 0 {
        return Err(windows::core::Error::from_win32());
    }

    // SAFETY: creating a message-only window of our registered class.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("LangCheck"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            hinstance,
            None,
        )?
    };

    add_icon(hwnd)?;

    // SAFETY: standard message loop driving `tray_wndproc` via DispatchMessageW.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// The tray icon: the executable's embedded application icon (resource id 1) if
/// present, otherwise the default Windows application icon.
fn load_tray_icon() -> HICON {
    // SAFETY: this process's module handle; a null handle is tolerated below.
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map(|m| HINSTANCE(m.0))
        .unwrap_or_default();
    // Resource id 1 as MAKEINTRESOURCE — a sentinel pointer value (address 1, never
    // dereferenced), not a real/dangling pointer; `without_provenance` says so.
    let resource_id = PCWSTR(std::ptr::without_provenance::<u16>(1));
    // SAFETY: try the embedded app icon (numeric resource id 1) from our module.
    if let Ok(icon) = unsafe { LoadIconW(hinstance, resource_id) } {
        return icon;
    }
    // SAFETY: fall back to the shared predefined application icon.
    unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
}

fn notify_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap(),
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAYCALLBACK,
        ..Default::default()
    };
    data.hIcon = load_tray_icon();
    let tip = "LangCheck — spelling autocorrect";
    for (slot, ch) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
        *slot = ch;
    }
    data
}

/// Show a tray balloon/toast (NIF_INFO) reflecting a state change.
fn show_balloon(hwnd: HWND, title: &str, message: &str) {
    let mut data = NOTIFYICONDATAW {
        cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap(),
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    for (slot, ch) in data.szInfoTitle.iter_mut().zip(title.encode_utf16()) {
        *slot = ch;
    }
    for (slot, ch) in data.szInfo.iter_mut().zip(message.encode_utf16()) {
        *slot = ch;
    }
    // SAFETY: `data` targets the icon we added; NIM_MODIFY+NIF_INFO shows a balloon.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn add_icon(hwnd: HWND) -> windows::core::Result<()> {
    let data = notify_icon_data(hwnd);
    // SAFETY: `data` is a fully-initialized NOTIFYICONDATAW for our window.
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(windows::core::Error::from_win32())
    }
}

fn remove_icon(hwnd: HWND) {
    let data = notify_icon_data(hwnd);
    // SAFETY: removing the icon we added for `hwnd`.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYCALLBACK => {
            let mouse = (lparam.0 as u32) & 0xFFFF;
            if mouse == WM_RBUTTONUP || mouse == WM_CONTEXTMENU || mouse == WM_LBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(hwnd, wparam.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_icon(hwnd);
            // SAFETY: ends the message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling for every other message.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND) {
    // Read status WITHOUT holding the handler borrow across TrackPopupMenu (which
    // re-enters the wndproc for WM_COMMAND).
    let status = TRAY_HANDLER.with(|cell| cell.borrow().as_ref().map(|h| h.status()));
    let Some(status) = status else {
        return;
    };

    // SAFETY: building and showing a popup menu owned for the duration of the call.
    unsafe {
        let menu: HMENU = match CreatePopupMenu() {
            Ok(menu) => menu,
            Err(_) => return,
        };
        let status_text = if status.enabled {
            if status.paused {
                "LangCheck: paused"
            } else {
                "LangCheck: correcting"
            }
        } else {
            "LangCheck: off"
        };
        let _ = AppendMenuW(menu, MF_GRAYED, ID_STATUS, &HSTRING::from(status_text));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let enable_label = if status.enabled {
            "Turn correction off"
        } else {
            "Turn correction on"
        };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_TOGGLE_ENABLED,
            &HSTRING::from(enable_label),
        );
        let pause_label = if status.paused {
            "Resume corrections"
        } else {
            "Pause corrections"
        };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_TOGGLE_PAUSE,
            &HSTRING::from(pause_label),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_SETTINGS, w!("Open settings file"));
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit LangCheck"));

        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Required so the menu dismisses when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

fn handle_command(hwnd: HWND, id: usize) {
    if id == ID_EXIT {
        // Request shutdown, then destroy the window (NIM_DELETE + PostQuitMessage).
        TRAY_HANDLER.with(|cell| {
            if let Some(handler) = cell.borrow().as_ref() {
                handler.request_exit();
            }
        });
        // SAFETY: destroying our own window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }
    TRAY_HANDLER.with(|cell| {
        if let Some(handler) = cell.borrow().as_ref() {
            match id {
                ID_TOGGLE_ENABLED => {
                    handler.toggle_enabled();
                    let message = if handler.status().enabled {
                        "Correction turned on."
                    } else {
                        "Correction turned off."
                    };
                    show_balloon(hwnd, "LangCheck", message);
                }
                ID_TOGGLE_PAUSE => {
                    handler.toggle_pause();
                    let message = if handler.status().paused {
                        "Corrections paused."
                    } else {
                        "Corrections resumed."
                    };
                    show_balloon(hwnd, "LangCheck", message);
                }
                ID_OPEN_SETTINGS => handler.open_settings(),
                _ => {}
            }
        }
    });
}

/// Open `path` in its default handler (e.g. the config file in the default editor).
pub fn open_path(path: &str) {
    let path = HSTRING::from(path);
    // SAFETY: ShellExecuteW with a valid verb and path; null window/params are valid.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
