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
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};

use windows::core::{implement, IUnknown, Interface, Result, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, HINSTANCE, HMODULE, MAX_PATH,
    S_FALSE, S_OK,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IClassFactory, IClassFactory_Impl,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
    ITfInputProcessorProfiles, ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
    GUID_TFCAT_TIP_KEYBOARD,
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

// ---------------------------------------------------------------------------
// Per-user registration (HKCU only; reversible).
//
// Installing writes (a) the COM in-process server CLSID under
// HKCU\Software\Classes\CLSID and (b) a TSF keyboard-TIP profile via
// ITfInputProcessorProfiles + ITfCategoryMgr. Removal undoes both. This only
// installs/removes the registration; enabling or disabling the adapter at
// runtime without uninstalling is the broker's kill switch, added later.
//
// Nothing here runs by default — the broker invokes DllRegisterServer /
// DllUnregisterServer (or regsvr32) explicitly, and only after the user opts in.
// ---------------------------------------------------------------------------

/// Profile GUID for the single en-US language profile. Fixed once registered.
const PROFILE_LANGCHECK_TSF: GUID = GUID::from_u128(0x4c43_4b54_5346_4d56_5002_0000_0000_0001);

/// en-US language id for the profile.
const LANGID_EN_US: u16 = 0x0409;

/// Human-readable description shown for the text service / profile.
const SERVICE_DESCRIPTION: &str = "LangCheck";

/// Module handle of this DLL, captured in `DllMain` (stored as an `isize`).
static MODULE: AtomicIsize = AtomicIsize::new(0);

/// DLL entry point: record our own module handle so we can resolve our path.
#[no_mangle]
extern "system" fn DllMain(instance: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(instance.0 as isize, Ordering::SeqCst);
    }
    BOOL(1)
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
fn module_path() -> Option<Vec<u16>> {
    let module = HMODULE(MODULE.load(Ordering::SeqCst) as *mut c_void);
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

/// Write the COM in-process-server CLSID key (per-user) pointing at this DLL.
fn write_com_registration(dll_path: &[u16]) -> Result<()> {
    let subkey = wide_z(&format!(
        "Software\\Classes\\CLSID\\{}\\InprocServer32",
        guid_braced(&CLSID_LANGCHECK_TSF)
    ));
    let mut hkey = HKEY::default();
    // SAFETY: valid HKCU root, NUL-terminated subkey, and `hkey` out-param.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
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

/// Delete the per-user COM CLSID subtree for this service.
fn delete_com_registration() -> Result<()> {
    let subkey = wide_z(&format!(
        "Software\\Classes\\CLSID\\{}",
        guid_braced(&CLSID_LANGCHECK_TSF)
    ));
    // SAFETY: valid HKCU root and NUL-terminated subkey.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr())).ok() }
}

/// Register the text service as an en-US keyboard TIP with TSF. COM must be
/// initialised by the caller.
fn register_tsf_profile() -> Result<()> {
    // SAFETY: COM is initialised; the CLSIDs are valid TSF singletons.
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: as above.
    let category: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }?;
    let description: Vec<u16> = SERVICE_DESCRIPTION.encode_utf16().collect();
    // SAFETY: every pointer references a const or a local valid for the call.
    unsafe {
        profiles.Register(&CLSID_LANGCHECK_TSF)?;
        profiles.AddLanguageProfile(
            &CLSID_LANGCHECK_TSF,
            LANGID_EN_US,
            &PROFILE_LANGCHECK_TSF,
            description.as_slice(),
            &[],
            0,
        )?;
        category.RegisterCategory(
            &CLSID_LANGCHECK_TSF,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_LANGCHECK_TSF,
        )?;
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
        let dll_path = module_path().ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        write_com_registration(&dll_path)?;
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
