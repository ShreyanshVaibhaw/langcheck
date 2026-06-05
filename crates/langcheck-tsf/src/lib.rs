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
//! - **Kill switch + opt-in.** It is never registered or enabled by default.
//!
//! STATUS: foundation only. The text input processor is currently a **no-op** — it
//! loads and activates without touching input — so it is safe to register for
//! testing. Focus tracking, edit-session replacement, and broker IPC are added in
//! subsequent commits, each behind the fail-open contract. None of this is
//! runtime-verified yet; a buggy in-process COM server can destabilise host apps,
//! so it requires dedicated host-process testing before real use.

// COM/FFI requires `unsafe`; enforce a `// SAFETY:` comment on every unsafe block
// (blueprint.md Section 12.4) rather than forbidding it.
#![deny(clippy::undocumented_unsafe_blocks)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};

use windows::core::{implement, IUnknown, Interface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::{
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
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

/// The minimal text input processor.
///
/// Fail-open: activation is a no-op, so loading the service can never alter or
/// block host-app typing. Real focus/edit handling is wired in later, behind a
/// kill switch and the same fail-open contract.
#[implement(ITfTextInputProcessor)]
struct TextService;

impl TextService {
    fn new() -> Self {
        add_ref();
        Self
    }
}

impl Drop for TextService {
    fn drop(&mut self) {
        release();
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, _thread_mgr: Option<&ITfThreadMgr>, _client_id: u32) -> Result<()> {
        // Fail open: do nothing until exact-range editing is verified against a
        // real host app. A no-op activation cannot disturb the host.
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
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
