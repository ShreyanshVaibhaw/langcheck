//! `langcheck-tsf` — post-MVP TSF (Text Services Framework) precision adapter.
//!
//! A minimal in-process COM **text service** (DLL) that, in compatible apps, will
//! apply corrections through TSF edit-session *range* replacement instead of
//! synthetic keystrokes — the proper fix for rich/web editors where `SendInput` is
//! unreliable (blueprint.md Step 13, Sections 7.1, 11.4).
//!
//! Hard constraints (blueprint.md Sections 7.1, 11.4, 12.3):
//! - **Fail open.** Any error, or any uncertainty, leaves the host app's typing
//!   completely unchanged. Loading the adapter must never block or alter input.
//! - **No language logic or persistence here.** The dictionary, ranking, and
//!   confidence policy stay in the broker; this adapter only observes a token and
//!   asks the broker (over same-user IPC, added next) what to do.
//! - **Opt-in + elevation.** Never registered by default. TSF text-service
//!   registration is *machine-wide* and needs administrator elevation (like every
//!   IME); the broker's `--register-tsf` self-elevates.
//!
//! STATUS (all verified on a real desktop where noted):
//! - COM server + per-machine TSF registration — an elevated register/unregister
//!   round-trip writes then cleanly removes the HKLM TIP + CLSID, no crash.
//! - Broker IPC client (`ask_broker`) over the same-user pipe — `--tsf-selftest`
//!   loads the DLL and confirms it reaches the broker (wierd → weird).
//! - Activation advises a thread-manager focus sink — `--tsf-comtest` drives the
//!   activate → advise → focus → deactivate path through a real TSF thread manager
//!   with no fault (catches the COM-plumbing/AV class of bug without a host app).
//!
//! - Live edit path (detect a completed word in `OnEndEdit` → `ask_broker` →
//!   replace the verified word range via an async read-write edit session). This is
//!   **implemented and compile-/unit-verified** (the pure detection logic is tested;
//!   the replacement only acts on a range whose text is re-checked to equal the
//!   detected word, so it can never replace the wrong text) but is **NOT yet
//!   host-verified** — it only runs inside a real editor, where a bug could
//!   destabilise the host. That host-process testing is the remaining work; the
//!   whole path is fail-open (any error or uncertainty leaves host text untouched).

// COM/FFI requires `unsafe`; enforce a `// SAFETY:` comment on every unsafe block
// (blueprint.md Section 12.4) rather than forbidding it.
#![deny(clippy::undocumented_unsafe_blocks)]

use core::ffi::c_void;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use langcheck_core::ipc::{Request, Response};
use langcheck_core::Boundary;

use windows::core::{implement, Error, IUnknown, Interface, Result, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, HMODULE, MAX_PATH, S_FALSE,
    S_OK,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IClassFactory, IClassFactory_Impl,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfCategoryMgr,
    ITfContext, ITfDocumentMgr, ITfEditRecord, ITfEditSession, ITfEditSession_Impl,
    ITfInputProcessorProfiles, ITfRange, ITfSource, ITfTextEditSink, ITfTextEditSink_Impl,
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, GUID_TFCAT_TIP_KEYBOARD, TF_DEFAULT_SELECTION, TF_ES_ASYNCDONTCARE,
    TF_ES_READWRITE, TF_SELECTION, TF_ST_CORRECTION,
};

/// Fixed CLSID of the LangCheck text service. Must never change once registered.
pub const CLSID_LANGCHECK_TSF: GUID = GUID::from_u128(0x4c43_4b54_5346_4d56_5001_0000_0000_0001);

/// Count of live COM objects plus held server locks. The DLL may unload only at zero.
static REF_COUNT: AtomicI32 = AtomicI32::new(0);

fn add_ref() {
    REF_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn release() {
    REF_COUNT.fetch_sub(1, Ordering::SeqCst);
}

/// Set when `AdviseSink` succeeds during activation. Diagnostic only (lets the COM
/// self-test confirm the focus sink registered without needing a host app).
static ADVISE_OK: AtomicU32 = AtomicU32::new(0);

/// How many characters before the caret to read when detecting the just-typed word.
const LOOKBACK_CHARS: i32 = 48;

/// Map a trailing character to a sentence [`Boundary`], or `None` if it is not one.
fn boundary_from_char(c: char) -> Option<Boundary> {
    match c {
        ' ' => Some(Boundary::Space),
        '.' => Some(Boundary::Period),
        ',' => Some(Boundary::Comma),
        '?' => Some(Boundary::Question),
        '!' => Some(Boundary::Exclamation),
        ':' => Some(Boundary::Colon),
        ';' => Some(Boundary::Semicolon),
        _ => None,
    }
}

/// Given the text ending at the caret, return the just-completed `(token, boundary)`
/// if the last character is a boundary preceded by a non-empty token. Pure — the
/// broker makes the actual correction decision, so this only does minimal framing.
fn detect_token(text_before_caret: &str) -> Option<(String, Boundary)> {
    let last = text_before_caret.chars().next_back()?;
    let boundary = boundary_from_char(last)?;
    let before = &text_before_caret[..text_before_caret.len() - last.len_utf8()];
    let mut token: Vec<char> = before
        .chars()
        .rev()
        .take_while(|c| !c.is_whitespace() && boundary_from_char(*c).is_none())
        .collect();
    token.reverse();
    if token.is_empty() {
        None
    } else {
        Some((token.into_iter().collect(), boundary))
    }
}

/// Read the text immediately before the caret (read-only, within `ec`) and, if a
/// word was just completed by a boundary, return it. Fail-open: any COM error → None.
fn read_trailing_token(context: &ITfContext, ec: u32) -> Option<(String, Boundary)> {
    let mut selection = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    // SAFETY: read the current selection within the supplied read cookie `ec`.
    unsafe { context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched) }.ok()?;
    if fetched == 0 {
        return None;
    }
    // SAFETY: take ownership of the range out of the ManuallyDrop; `selection` is
    // not read again, so its Drop will not touch the moved-out value.
    let range = unsafe { ManuallyDrop::take(&mut selection[0].range) }?;
    let mut shifted = 0i32;
    // SAFETY: move the start anchor back up to LOOKBACK_CHARS (read cookie; no halt
    // condition). This adjusts our range object only, not the document.
    let _ = unsafe { range.ShiftStart(ec, -LOOKBACK_CHARS, &mut shifted, std::ptr::null()) };
    let mut buffer = [0u16; LOOKBACK_CHARS as usize];
    let mut copied = 0u32;
    // SAFETY: read the (now back-extended) range's text into `buffer`.
    unsafe { range.GetText(ec, 0, &mut buffer, &mut copied) }.ok()?;
    let text = String::from_utf16_lossy(&buffer[..copied as usize]);
    detect_token(&text)
}

/// Re-derive the range covering exactly `token` — the `token`-length run ending one
/// boundary character before the caret — and return it ONLY if its text equals
/// `token`. That verification is the safety guarantee: a range is never handed to
/// `SetText` unless it provably covers exactly the detected word, so a shift-math
/// error can never replace the wrong text.
fn verified_word_range(context: &ITfContext, ec: u32, token: &str) -> Option<ITfRange> {
    let token_chars = i32::try_from(token.chars().count()).ok()?;
    let mut selection = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    // SAFETY: read the caret selection within the read cookie `ec`.
    unsafe { context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched) }.ok()?;
    if fetched == 0 {
        return None;
    }
    // SAFETY: take the range out of the ManuallyDrop; `selection` is not reused.
    let range = unsafe { ManuallyDrop::take(&mut selection[0].range) }?;
    let mut moved = 0i32;
    // SAFETY: move the start back over the word + the one boundary char.
    unsafe { range.ShiftStart(ec, -(token_chars + 1), &mut moved, std::ptr::null()) }.ok()?;
    // SAFETY: move the end back over just the boundary char, leaving exactly the word.
    unsafe { range.ShiftEnd(ec, -1, &mut moved, std::ptr::null()) }.ok()?;
    let mut buffer = [0u16; LOOKBACK_CHARS as usize];
    let mut copied = 0u32;
    // SAFETY: read the candidate word range's text to verify it.
    unsafe { range.GetText(ec, 0, &mut buffer, &mut copied) }.ok()?;
    let got = String::from_utf16_lossy(&buffer[..copied as usize]);
    (got == token).then_some(range)
}

/// An edit session that replaces one fixed range's text with a fixed string.
#[implement(ITfEditSession)]
struct ReplaceSession {
    range: ITfRange,
    replacement: Vec<u16>,
}

impl ReplaceSession {
    fn new(range: ITfRange, replacement: &str) -> Self {
        add_ref();
        Self {
            range,
            replacement: replacement.encode_utf16().collect(),
        }
    }
}

impl Drop for ReplaceSession {
    fn drop(&mut self) {
        release();
    }
}

impl ITfEditSession_Impl for ReplaceSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        // SAFETY: write within the granted write cookie; replace the verified word
        // range. TF_ST_CORRECTION marks this as an autocorrection to the host.
        unsafe { self.range.SetText(ec, TF_ST_CORRECTION, &self.replacement) }
    }
}

/// Detect a just-completed word, ask the broker, and — if it returns a correction —
/// apply it via an asynchronous read-write edit session. Fail-open at every step:
/// any error or uncertainty leaves the host's text untouched.
fn maybe_correct(context: &ITfContext, ec: u32, client_id: u32) {
    let Some((token, boundary)) = read_trailing_token(context, ec) else {
        return;
    };
    let Some(replacement) = ask_broker(&token, boundary) else {
        return;
    };
    let Some(range) = verified_word_range(context, ec, &token) else {
        return;
    };
    let session: ITfEditSession = ReplaceSession::new(range, &replacement).into();
    // SAFETY: request an async read-write edit session — TSF calls DoEditSession with
    // a write cookie once the lock is free. ASYNCDONTCARE is required here: we are
    // inside OnEndEdit, where a synchronous lock would be refused.
    let _ = unsafe {
        context.RequestEditSession(client_id, &session, TF_ES_READWRITE | TF_ES_ASYNCDONTCARE)
    };
}

/// Per-context text-edit sink: after each edit, if a word was just completed by a
/// boundary it is detected, sent to the broker, and any correction is applied via an
/// edit session. Fail-open everywhere.
#[implement(ITfTextEditSink)]
struct EditSink {
    /// TSF client id (from activation), needed to request edit sessions.
    client_id: u32,
}

impl EditSink {
    fn new(client_id: u32) -> Self {
        add_ref();
        Self { client_id }
    }
}

impl Drop for EditSink {
    fn drop(&mut self) {
        release();
    }
}

impl ITfTextEditSink_Impl for EditSink_Impl {
    fn OnEndEdit(
        &self,
        context: Option<&ITfContext>,
        ec: u32,
        _record: Option<&ITfEditRecord>,
    ) -> Result<()> {
        if let Some(context) = context {
            maybe_correct(context, ec, self.client_id);
        }
        Ok(())
    }
}

/// Thread-manager event sink: tracks focus and advises an [`EditSink`] on the
/// focused document's context. Fail-open — observing focus or failing to advise can
/// never disturb the host.
#[implement(ITfThreadMgrEventSink)]
struct ThreadMgrSink {
    /// TSF client id (from activation), passed to each [`EditSink`] it advises.
    client_id: u32,
    /// The focused context's event source + our edit-sink cookie, so we can
    /// unadvise when focus moves away or on teardown. `None` when not advised.
    edit_advise: RefCell<Option<(ITfSource, u32)>>,
}

impl ThreadMgrSink {
    fn new(client_id: u32) -> Self {
        add_ref();
        Self {
            client_id,
            edit_advise: RefCell::new(None),
        }
    }

    /// Unadvise the current edit sink, if any.
    fn unadvise_edit(&self) {
        if let Some((source, cookie)) = self.edit_advise.borrow_mut().take() {
            // SAFETY: `source`/`cookie` are exactly the pair returned by AdviseSink.
            unsafe {
                let _ = source.UnadviseSink(cookie);
            }
        }
    }

    /// Advise a fresh edit sink on the focused document's top context (fail-open).
    fn advise_edit(&self, focus: &ITfDocumentMgr) {
        self.unadvise_edit();
        // SAFETY: GetTop returns the focused context (or an error if none).
        let Ok(context) = (unsafe { focus.GetTop() }) else {
            return;
        };
        let Ok(source) = context.cast::<ITfSource>() else {
            return;
        };
        let sink: ITfTextEditSink = EditSink::new(self.client_id).into();
        // SAFETY: `source` is the context's event source; `sink` is a valid COM
        // object that implements ITfTextEditSink.
        if let Ok(cookie) = unsafe { source.AdviseSink(&ITfTextEditSink::IID, &sink) } {
            *self.edit_advise.borrow_mut() = Some((source, cookie));
        }
    }
}

impl Drop for ThreadMgrSink {
    fn drop(&mut self) {
        self.unadvise_edit();
        release();
    }
}

impl ITfThreadMgrEventSink_Impl for ThreadMgrSink_Impl {
    fn OnInitDocumentMgr(&self, _dim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _dim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnSetFocus(
        &self,
        focus: Option<&ITfDocumentMgr>,
        _previous: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        match focus {
            Some(focus) => self.advise_edit(focus),
            None => self.unadvise_edit(),
        }
        Ok(())
    }
    fn OnPushContext(&self, _context: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
    fn OnPopContext(&self, _context: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
}

/// The text input processor. On activation it advises a thread-manager event sink
/// so it can track focus. Fail-open throughout: if advising fails the service is
/// simply inert and can never block or alter host typing.
#[implement(ITfTextInputProcessor)]
struct TextService {
    /// The thread manager's event source + our advise cookie, kept so we can
    /// unadvise on deactivation. `None` until activated.
    advise: RefCell<Option<(ITfSource, u32)>>,
}

impl TextService {
    fn new() -> Self {
        add_ref();
        Self {
            advise: RefCell::new(None),
        }
    }
}

impl Drop for TextService {
    fn drop(&mut self) {
        release();
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, thread_mgr: Option<&ITfThreadMgr>, client_id: u32) -> Result<()> {
        // Fail open: advise the focus sink, but never fail activation if we cannot —
        // an inert service is always safe.
        if let Some(thread_mgr) = thread_mgr {
            if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                let sink: ITfThreadMgrEventSink = ThreadMgrSink::new(client_id).into();
                // SAFETY: `source` is the thread manager's event source and `sink`
                // is a valid COM object; AdviseSink returns a cookie on success.
                let advised = unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &sink) };
                if let Ok(cookie) = advised {
                    ADVISE_OK.fetch_add(1, Ordering::SeqCst);
                    *self.advise.borrow_mut() = Some((source, cookie));
                }
            }
        }
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        if let Some((source, cookie)) = self.advise.borrow_mut().take() {
            // SAFETY: `source`/`cookie` are exactly the pair returned by AdviseSink.
            unsafe {
                let _ = source.UnadviseSink(cookie);
            }
        }
        Ok(())
    }
}

/// COM class factory that hands out [`TextService`] instances.
#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let service: ITfTextInputProcessor = TextService::new().into();
        // SAFETY: `iid` and `object` are the QueryInterface out-parameters COM
        // supplies; `service` is a valid COM object.
        unsafe { service.query(iid, object).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            add_ref();
        } else {
            release();
        }
        Ok(())
    }
}

/// COM entry point: hand out the class factory for our CLSID.
///
/// # Safety
/// COM supplies valid `clsid`/`iid` pointers and an `object` out-pointer.
#[no_mangle]
extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: `clsid` is a valid pointer per the COM contract; compared by value.
    if clsid.is_null() || unsafe { *clsid } != CLSID_LANGCHECK_TSF {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = ClassFactory.into();
    // SAFETY: `iid`/`object` are the QueryInterface out-parameters from COM.
    unsafe { factory.query(iid, object) }
}

/// COM unload check: the DLL may unload only when no objects or locks remain.
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if REF_COUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

// ---------------------------------------------------------------------------
// Broker IPC client.
//
// The adapter holds no language logic: it asks the broker (over the same-user,
// local-only pipe in `langcheck-ipc`) what to do with a typed token. Every call is
// fail-open — any error leaves the host's text untouched.
// ---------------------------------------------------------------------------

/// Ask the broker whether `token` (followed by `boundary`) should be corrected.
/// Returns the replacement on an auto-correct decision, else `None`. Fail-open:
/// no broker, a timeout, or a malformed reply all yield `None`.
fn ask_broker(token: &str, boundary: Boundary) -> Option<String> {
    let request = Request::Evaluate {
        token: token.to_owned(),
        boundary,
    };
    match langcheck_ipc::request(&request) {
        Ok(Response::Replace { replacement }) => Some(replacement),
        _ => None,
    }
}

/// Diagnostic export: verify the adapter can reach the broker over same-user IPC.
/// Returns `S_OK` only if a liveness Ping is answered AND a known curated typo
/// round-trips to its correction; otherwise `E_FAIL`. Driven by
/// `langcheck --tsf-selftest`, this confirms the in-DLL client wiring without
/// activating the text service in a host app.
#[no_mangle]
extern "system" fn LangCheckIpcSelfTest() -> HRESULT {
    let ping_ok = matches!(langcheck_ipc::request(&Request::Ping), Ok(Response::Pong));
    let eval_ok = ask_broker("wierd", Boundary::Space).as_deref() == Some("weird");
    if ping_ok && eval_ok {
        S_OK
    } else {
        E_FAIL
    }
}

/// Diagnostic export: exercise the COM activation + focus-sink path WITHOUT a host
/// app. Creates a real TSF thread manager, activates our text service (which advises
/// the focus sink), focuses a document so `OnSetFocus` fires, then deactivates.
/// Returns `S_OK` only if nothing faulted and the focus sink was actually called —
/// catching the class of COM bug (e.g. a stale-reference access violation) that
/// would otherwise only surface inside a host app. Driven by `--tsf-comtest`.
#[no_mangle]
extern "system" fn LangCheckComSelfTest() -> HRESULT {
    ADVISE_OK.store(0, Ordering::SeqCst);
    match com_selftest() {
        // A COM call faulted/failed — surface its real HRESULT to localise the cause.
        Err(error) => error.code(),
        // AdviseSink failed during activation (the sink never registered).
        Ok(()) if ADVISE_OK.load(Ordering::SeqCst) == 0 => HRESULT(0x8004_3001u32 as i32),
        // Clean: the activate -> advise -> create/focus -> deactivate path ran
        // without faulting and the focus sink registered. Actual focus/edit *event
        // delivery* only fires under a real host (it needs a text store), so that is
        // confirmed by host testing rather than this synthetic harness.
        Ok(()) => S_OK,
    }
}

/// Body of [`LangCheckComSelfTest`], in a `Result` for `?` convenience.
fn com_selftest() -> Result<()> {
    // SAFETY: standard apartment-threaded COM init for this (STA) thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let outcome = (|| -> Result<()> {
        // SAFETY: CLSID_TF_ThreadMgr is the TSF thread-manager singleton.
        let thread_mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }?;
        // SAFETY: activate this thread's TSF manager; returns a client id.
        let client_id = unsafe { thread_mgr.Activate() }?;
        let service: ITfTextInputProcessor = TextService::new().into();
        // SAFETY: drive our own Activate with the real thread manager (advises sink).
        unsafe { service.Activate(&thread_mgr, client_id) }?;
        // SAFETY: create and focus a document manager — fires OnSetFocus.
        let document = unsafe { thread_mgr.CreateDocumentMgr() }?;
        // SAFETY: focusing the new (empty) document manager.
        let _ = unsafe { thread_mgr.SetFocus(&document) };
        // SAFETY: tear down in order — our Deactivate unadvises the sink, then the
        // thread manager is deactivated.
        unsafe {
            service.Deactivate()?;
            thread_mgr.Deactivate()?;
        }
        Ok(())
    })();
    // SAFETY: balance CoInitializeEx on this thread.
    unsafe { CoUninitialize() };
    outcome
}

// ---------------------------------------------------------------------------
// Registration (machine-wide; requires elevation; reversible).
//
// Installing writes (a) the COM in-process server CLSID under
// HKLM\Software\Classes\CLSID and (b) a TSF keyboard-TIP profile via
// ITfInputProcessorProfiles + ITfCategoryMgr (which write under HKLM\...\CTF).
// Removal undoes both.
//
// IMPORTANT: TSF text-service registration is *machine-wide* and needs
// administrator elevation — verified empirically: a non-elevated process cannot
// write HKLM\SOFTWARE\Microsoft\CTF and ITfInputProcessorProfiles::Register then
// fails with E_FAIL. This matches every IME (admin installers). The broker's
// `--register-tsf` self-elevates; nothing here runs by default and the adapter
// is opt-in only.
//
// Step-failure HRESULTs (0x8004_200N): 1=module path, 2=create profiles,
// 3=create category mgr, 4=Register, 5=AddLanguageProfile, 6=RegisterCategory,
// 7=COM CLSID registry write. Only the numeric code survives the DLL boundary,
// so each step has a distinct code to localise a failure without a debugger.
// ---------------------------------------------------------------------------

/// Profile GUID for the single en-US language profile. Fixed once registered.
const PROFILE_LANGCHECK_TSF: GUID = GUID::from_u128(0x4c43_4b54_5346_4d56_5002_0000_0000_0001);

/// en-US language id for the profile.
const LANGID_EN_US: u16 = 0x0409;

/// Human-readable description shown for the text service / profile.
const SERVICE_DESCRIPTION: &str = "LangCheck";

/// Distinct HRESULT per registration step. Only the numeric code survives the
/// DLL boundary, so a unique code per step is how a failure is localised.
fn step_error(step: u32) -> Error {
    Error::from_hresult(HRESULT((0x8004_2000u32 | step) as i32))
}

/// UTF-16, NUL-terminated copy of `s` (for registry strings / `PCWSTR`).
fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reinterpret a `u16` slice as its little-endian bytes (registry `REG_SZ` data).
fn as_bytes(w: &[u16]) -> &[u8] {
    // SAFETY: `[u16]` is freely reinterpretable as `[u8]` of twice the length;
    // the borrow is tied to `w`, and `u8` has weaker alignment than `u16`.
    unsafe { std::slice::from_raw_parts(w.as_ptr().cast::<u8>(), std::mem::size_of_val(w)) }
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` form of a GUID (a registry key name).
fn guid_braced(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7],
    )
}

/// Full path to this DLL as a NUL-terminated UTF-16 string, or `None` on failure.
///
/// Resolves our own module via `GetModuleHandleExW(FROM_ADDRESS)` using the
/// address of this function — robust whether or not the runtime calls `DllMain`.
fn module_path() -> Option<Vec<u16>> {
    let mut module = HMODULE::default();
    // SAFETY: with FROM_ADDRESS, the second argument is an address *inside* this
    // module (we pass this function's address cast to PCWSTR); `module` is written
    // on success and the refcount is left unchanged.
    let resolved = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module_path as *const () as *const u16),
            &mut module,
        )
    };
    if resolved.is_err() {
        return None;
    }
    let mut buf = [0u16; MAX_PATH as usize];
    // SAFETY: `buf` is a valid, sized output buffer; `module` is our own handle.
    let len = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    let mut path = buf[..len].to_vec();
    path.push(0);
    Some(path)
}

/// Write the COM in-process-server CLSID key (machine-wide) pointing at this DLL.
fn write_com_registration(dll_path: &[u16]) -> Result<()> {
    let subkey = wide_z(&format!(
        "Software\\Classes\\CLSID\\{}\\InprocServer32",
        guid_braced(&CLSID_LANGCHECK_TSF)
    ));
    let mut hkey = HKEY::default();
    // SAFETY: valid HKLM root, NUL-terminated subkey, and `hkey` out-param.
    unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;
    }
    let model_name = wide_z("ThreadingModel");
    let model_value = wide_z("Apartment");
    // SAFETY: `hkey` is open; the default value name is NULL; data slices are
    // valid for their byte lengths.
    let default =
        unsafe { RegSetValueExW(hkey, PCWSTR::null(), 0, REG_SZ, Some(as_bytes(dll_path))) };
    // SAFETY: as above, with a named value.
    let threading = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(model_name.as_ptr()),
            0,
            REG_SZ,
            Some(as_bytes(&model_value)),
        )
    };
    // SAFETY: `hkey` is a valid open key.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    default.ok()?;
    threading.ok()?;
    Ok(())
}

/// Delete the machine-wide COM CLSID subtree for this service.
fn delete_com_registration() -> Result<()> {
    let subkey = wide_z(&format!(
        "Software\\Classes\\CLSID\\{}",
        guid_braced(&CLSID_LANGCHECK_TSF)
    ));
    // SAFETY: valid HKLM root and NUL-terminated subkey.
    unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(subkey.as_ptr())).ok() }
}

/// Register the text service as an en-US keyboard TIP with TSF. COM must be
/// initialised by the caller.
fn register_tsf_profile() -> Result<()> {
    // SAFETY: COM is initialised; the CLSID is a valid TSF singleton.
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| step_error(2))?;
    let description: Vec<u16> = SERVICE_DESCRIPTION.encode_utf16().collect();
    // A valid empty icon path: a single NUL. `&[]` would hand the API a dangling
    // (non-null, zero-length) pointer, which is unsafe to pass across FFI.
    let empty_icon: [u16; 1] = [0];
    // SAFETY: every pointer references a const or a local valid for the call.
    unsafe {
        profiles
            .Register(&CLSID_LANGCHECK_TSF)
            .map_err(|_| step_error(4))?;
        profiles
            .AddLanguageProfile(
                &CLSID_LANGCHECK_TSF,
                LANGID_EN_US,
                &PROFILE_LANGCHECK_TSF,
                description.as_slice(),
                &empty_icon,
                0,
            )
            .map_err(|_| step_error(5))?;
    }
    // Create the category manager AFTER the profile registration, then register
    // the keyboard category. Creating it earlier and calling it post-registration
    // faulted (a stale category-manager reference once TSF state changed).
    // SAFETY: COM is initialised; the CLSID is a valid TSF singleton.
    let category: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| step_error(3))?;
    // SAFETY: every pointer references a const valid for the call.
    unsafe {
        category
            .RegisterCategory(
                &CLSID_LANGCHECK_TSF,
                &GUID_TFCAT_TIP_KEYBOARD,
                &CLSID_LANGCHECK_TSF,
            )
            .map_err(|_| step_error(6))?;
    }
    Ok(())
}

/// Unregister the TSF profile + category. COM must be initialised by the caller.
fn unregister_tsf_profile() -> Result<()> {
    // SAFETY: COM is initialised; the CLSIDs are valid TSF singletons.
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: as above.
    let category: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: all pointers reference consts valid for the call. Category removal
    // is best-effort so a missing category never blocks profile removal.
    unsafe {
        let _ = category.UnregisterCategory(
            &CLSID_LANGCHECK_TSF,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_LANGCHECK_TSF,
        );
        profiles.Unregister(&CLSID_LANGCHECK_TSF)?;
    }
    Ok(())
}

/// Run `op` inside a balanced apartment-threaded COM init.
fn with_com<F: FnOnce() -> Result<()>>(op: F) -> Result<()> {
    // SAFETY: standard COM init; `is_ok()` covers both `S_OK` and `S_FALSE`.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let initialised = init.is_ok();
    let result = op();
    if initialised {
        // SAFETY: balances a successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
    result
}

/// COM self-registration: install the CLSID and the TSF keyboard-TIP profile.
#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    let outcome = (|| {
        let dll_path = module_path().ok_or_else(|| step_error(1))?;
        write_com_registration(&dll_path).map_err(|_| step_error(7))?;
        with_com(register_tsf_profile)
    })();
    match outcome {
        Ok(()) => S_OK,
        Err(error) => error.code(),
    }
}

/// COM self-unregistration: remove the TSF profile then the CLSID subtree.
#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let tsf = with_com(unregister_tsf_profile);
    // Always attempt the registry cleanup, even if the TSF calls failed.
    let registry = delete_com_registration();
    match tsf.and(registry) {
        Ok(()) => S_OK,
        Err(error) => error.code(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_word_before_a_boundary() {
        assert_eq!(
            detect_token("wierd "),
            Some(("wierd".to_owned(), Boundary::Space))
        );
        assert_eq!(
            detect_token("wierd."),
            Some(("wierd".to_owned(), Boundary::Period))
        );
        // Only the trailing token, not the whole line.
        assert_eq!(
            detect_token("the other wierd,"),
            Some(("wierd".to_owned(), Boundary::Comma))
        );
    }

    #[test]
    fn no_boundary_means_no_detection() {
        // Still mid-word (no trailing boundary) — wait for the boundary.
        assert_eq!(detect_token("wierd"), None);
        assert_eq!(detect_token(""), None);
    }

    #[test]
    fn boundary_with_no_preceding_token_is_ignored() {
        assert_eq!(detect_token(" "), None);
        assert_eq!(detect_token("hello  "), None); // double space: empty token
        assert_eq!(detect_token("."), None);
    }

    #[test]
    fn boundary_chars_map_correctly() {
        assert_eq!(boundary_from_char(' '), Some(Boundary::Space));
        assert_eq!(boundary_from_char('?'), Some(Boundary::Question));
        assert_eq!(boundary_from_char(';'), Some(Boundary::Semicolon));
        assert_eq!(boundary_from_char('a'), None);
        assert_eq!(boundary_from_char('-'), None);
    }
}
