//! Keyboard observation via a low-level keyboard hook (`WH_KEYBOARD_LL`).
//!
//! A dedicated thread installs the hook and runs a Windows message loop (the hook
//! callback only runs while that thread pumps messages). The callback does the
//! minimum possible work: it drops everything while `capture_allowed` is false,
//! ignores LangCheck's own injected events, stamps a monotonic generation, and
//! pushes a compact [`InputEvent`] into a bounded channel — never allocating,
//! logging, locking on a contended path, or calling COM/UIA/the dictionary (see
//! `blueprint.md` Sections 8.1, 11.1). A full queue drops the event and bumps a
//! counter rather than blocking.
//!
//! `WH_KEYBOARD_LL` is the MVP observer selected in ADR-0002; Raw Input remains a
//! documented alternative pending the on-hardware measurements in that ADR.
//!
//! Implemented in delivery Step 01 (Windows Input and Focus Feasibility Spike).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::LANGCHECK_INJECTED_MARKER;

pub mod translate;

/// Whether a physical key event was a press or a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEventKind {
    KeyDown,
    KeyUp,
}

/// A compact, copied keyboard event handed off by the observer (`blueprint.md`
/// Section 8.1). Carries no heap data so the callback never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    /// Monotonic physical-input generation assigned by the observer.
    pub generation: u64,
    /// Event time in milliseconds (system tick count from the hook struct).
    pub timestamp_ms: u64,
    /// Press or release.
    pub kind: InputEventKind,
    /// Virtual-key code.
    pub virtual_key: u16,
    /// Hardware scan code.
    pub scan_code: u16,
    /// Raw low-level hook flags (includes the injected bit).
    pub flags: u32,
}

impl InputEvent {
    /// Whether the OS marked this event as injected (synthetic) rather than typed.
    pub fn is_injected(&self) -> bool {
        self.flags & LLKHF_INJECTED.0 != 0
    }
}

/// Errors starting the observer.
#[derive(Debug)]
pub enum InputError {
    /// An observer is already installed for this process.
    AlreadyRunning,
    /// `SetWindowsHookExW` failed.
    HookInstallFailed(windows::core::Error),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::AlreadyRunning => f.write_str("keyboard observer already running"),
            InputError::HookInstallFailed(e) => write!(f, "failed to install keyboard hook: {e}"),
        }
    }
}

impl std::error::Error for InputError {}

// Shared, callback-visible state. The hook callback is an `extern "system"` fn
// with no captured environment, so its state must live in statics. All reads in
// the callback are lock-free (atomics / `OnceLock::get` / `SyncSender::try_send`).
static SINK: OnceLock<SyncSender<InputEvent>> = OnceLock::new();
static CAPTURE_ALLOWED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Enable or disable capture. While disabled (the default), the callback discards
/// every event without translating, queuing, or otherwise retaining it. Capture
/// must only be enabled for a field positively classified as safe prose
/// (`blueprint.md` Section 12.2).
pub fn set_capture_allowed(allowed: bool) {
    CAPTURE_ALLOWED.store(allowed, Ordering::SeqCst);
}

/// Whether capture is currently enabled.
pub fn capture_allowed() -> bool {
    CAPTURE_ALLOWED.load(Ordering::SeqCst)
}

/// The current physical-input generation (number of queued physical events so far).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// How many events have been dropped because the queue was full (redacted counter;
/// `blueprint.md` Sections 8.1, 18).
pub fn dropped_count() -> u64 {
    DROPPED.load(Ordering::SeqCst)
}

/// A running low-level keyboard observer. The hook is installed for the process
/// lifetime; enable/disable is done via [`set_capture_allowed`] rather than by
/// repeatedly hooking and unhooking.
pub struct LowLevelKeyboardObserver {
    thread: Option<JoinHandle<()>>,
}

impl LowLevelKeyboardObserver {
    /// Install the hook on a dedicated thread and deliver events to `sink`.
    ///
    /// Capture starts disabled; call [`set_capture_allowed`] once a field is known
    /// safe. Only one observer may run per process.
    pub fn start(sink: SyncSender<InputEvent>) -> Result<Self, InputError> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err(InputError::AlreadyRunning);
        }
        // The sink is set once for the process lifetime (single-instance broker).
        let _ = SINK.set(sink);

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), InputError>>();
        let thread = std::thread::Builder::new()
            .name("langcheck-input".to_owned())
            .spawn(move || run_hook_thread(&ready_tx))
            .expect("spawn input thread");

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                RUNNING.store(false, Ordering::SeqCst);
                Err(e)
            }
            Err(_) => {
                RUNNING.store(false, Ordering::SeqCst);
                Err(InputError::AlreadyRunning)
            }
        }
    }

    /// Stop the observer: ask the hook thread to quit and join it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let thread_id = HOOK_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            // SAFETY: posting WM_QUIT to the hook thread is sound; the thread id is
            // the one we recorded after the loop started, and PostThreadMessageW
            // tolerates a stale id by failing harmlessly.
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        RUNNING.store(false, Ordering::SeqCst);
    }
}

impl Drop for LowLevelKeyboardObserver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Install the hook, signal readiness, and pump messages until `WM_QUIT`.
fn run_hook_thread(ready: &std::sync::mpsc::Sender<Result<(), InputError>>) {
    HOOK_THREAD_ID.store(
        // SAFETY: GetCurrentThreadId has no preconditions and cannot fail.
        unsafe { GetCurrentThreadId() },
        Ordering::SeqCst,
    );

    // SAFETY: GetModuleHandleW(None) returns this process's module handle, which is
    // a valid HINSTANCE for the lifetime of the process and is the module that
    // contains `keyboard_hook_proc`.
    let hmodule = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(InputError::HookInstallFailed(e)));
            return;
        }
    };

    // SAFETY: a WH_KEYBOARD_LL hook with a valid callback and the current module's
    // handle is the documented usage; `keyboard_hook_proc` outlives the hook.
    let hook =
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), hmodule, 0) } {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(InputError::HookInstallFailed(e)));
                return;
            }
        };

    let _ = ready.send(Ok(()));

    let mut msg = MSG::default();
    // SAFETY: standard message loop; `&mut msg` is a valid writable MSG and the
    // window-handle filter is null (all messages for this thread).
    unsafe {
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // SAFETY: unhooking the handle we installed above; safe to call once.
    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
}

/// The low-level keyboard hook callback. Must return quickly and must not block.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION on a WH_KEYBOARD_LL hook, `lparam` points to a
        // KBDLLHOOKSTRUCT owned by the OS for the duration of this call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };

        let is_ours = info.dwExtraInfo == LANGCHECK_INJECTED_MARKER;
        if !is_ours && CAPTURE_ALLOWED.load(Ordering::SeqCst) {
            if let Some(kind) = classify_message(wparam.0 as u32) {
                let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                let event = InputEvent {
                    generation,
                    timestamp_ms: u64::from(info.time),
                    kind,
                    virtual_key: info.vkCode as u16,
                    scan_code: info.scanCode as u16,
                    flags: info.flags.0,
                };
                if let Some(sink) = SINK.get() {
                    match sink.try_send(event) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                            DROPPED.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }

    // SAFETY: passing the event down the hook chain is required; the handle
    // argument may be null per the documented contract.
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

/// Map a low-level keyboard message to a key-up/down kind, or `None` for messages
/// the observer ignores.
fn classify_message(message: u32) -> Option<InputEventKind> {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(InputEventKind::KeyDown),
        WM_KEYUP | WM_SYSKEYUP => Some(InputEventKind::KeyUp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_classification() {
        assert_eq!(classify_message(WM_KEYDOWN), Some(InputEventKind::KeyDown));
        assert_eq!(
            classify_message(WM_SYSKEYDOWN),
            Some(InputEventKind::KeyDown)
        );
        assert_eq!(classify_message(WM_KEYUP), Some(InputEventKind::KeyUp));
        assert_eq!(classify_message(WM_SYSKEYUP), Some(InputEventKind::KeyUp));
        assert_eq!(classify_message(0xFFFF), None);
    }

    #[test]
    fn injected_flag_is_read_from_flags() {
        let event = InputEvent {
            generation: 1,
            timestamp_ms: 0,
            kind: InputEventKind::KeyDown,
            virtual_key: 0x41,
            scan_code: 0,
            flags: LLKHF_INJECTED.0,
        };
        assert!(event.is_injected());
        let typed = InputEvent { flags: 0, ..event };
        assert!(!typed.is_injected());
    }
}
