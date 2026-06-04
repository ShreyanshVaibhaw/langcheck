//! Focus safety inspection.
//!
//! Decides, for the currently focused control, whether autocorrection is safe. The
//! decision is **fail-closed**: a field is treated as capturable only when it is
//! *positively* classified as enabled, editable, non-password prose (an Edit or
//! Document control). Anything else — a password field, a disabled/unknown
//! control, or any failure to read a property — yields a non-capturable state
//! (`blueprint.md` Sections 8.2, 12.2).
//!
//! The classification is a pure, unit-tested function; only the property reads use
//! UI Automation, which must run on a dedicated COM thread (`blueprint.md`
//! Section 8.2). Password field *values* are never read.
//!
//! Implemented in delivery Step 01 (Windows Input and Focus Feasibility Spike).
//!
//! NOTE (manual verification): the UIA property reads below are compiled but have
//! not been validated against real applications; ADR-0002 lists the on-hardware
//! checks required before this is trusted.

use windows::core::Result;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// The current foreground window as an opaque id (`0` if none). The coordinator
/// uses this as a coarse focus identity to detect focus changes; a change resets
/// the typing session.
pub fn foreground_window_id() -> u64 {
    // SAFETY: GetForegroundWindow has no preconditions and may return a null HWND.
    let hwnd = unsafe { GetForegroundWindow() };
    hwnd.0 as u64
}

/// How a focused field is classified for capture (`blueprint.md` Section 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldClass {
    /// Enabled, editable, non-password prose: capture is allowed.
    NormalProse,
    /// A password or otherwise sensitive field: never capture.
    Sensitive,
    /// A control that is not prose (disabled, read-only, non-text).
    NonProse,
    /// The field could not be classified; treated as unsafe.
    Unknown,
}

impl FieldClass {
    /// Capture is allowed only for a positively-classified prose field.
    pub fn capture_allowed(self) -> bool {
        matches!(self, FieldClass::NormalProse)
    }
}

/// The focus-relevant properties read from UI Automation. `is_password` defaults to
/// `true` (fail-closed) whenever the real value cannot be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldProperties {
    /// Raw UIA control-type id.
    pub control_type: i32,
    /// Whether the control is a password field (fail-closed: unknown -> true).
    pub is_password: bool,
    /// Whether the control is enabled.
    pub is_enabled: bool,
    /// Whether the control is editable (not read-only).
    pub is_editable: bool,
}

/// Classify a field from its properties. Pure and fail-closed.
pub fn classify_field(props: &FieldProperties) -> FieldClass {
    if props.is_password {
        return FieldClass::Sensitive;
    }
    if !props.is_enabled || !props.is_editable {
        return FieldClass::NonProse;
    }
    if props.control_type == UIA_EditControlTypeId.0
        || props.control_type == UIA_DocumentControlTypeId.0
    {
        FieldClass::NormalProse
    } else {
        // An enabled, editable control of an unrecognised type: fail closed.
        FieldClass::Unknown
    }
}

/// A UI Automation focus inspector. Must be constructed and used on a single
/// COM-initialized thread (the dedicated focus thread; `blueprint.md` Section 9).
pub struct FocusInspector {
    automation: IUIAutomation,
}

impl FocusInspector {
    /// Initialize COM (multithreaded) for this thread and create the automation
    /// object. Call once per focus thread.
    pub fn new() -> Result<Self> {
        // SAFETY: initializing COM for the calling thread; a matching CoUninitialize
        // is intentionally omitted because the focus thread lives for the process.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        // SAFETY: creating the standard CUIAutomation in-process server; the CLSID
        // and interface are the documented pair for UI Automation.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self { automation })
    }

    /// Read and classify the currently focused field. Any read failure classifies
    /// the field as [`FieldClass::Unknown`] (fail-closed) rather than erroring.
    pub fn classify_focused(&self) -> FieldClass {
        match self.read_focused() {
            Ok(props) => classify_field(&props),
            Err(_) => FieldClass::Unknown,
        }
    }

    /// Read the focused element's safety-relevant properties. Never reads a field's
    /// value/text.
    fn read_focused(&self) -> Result<FieldProperties> {
        // SAFETY: GetFocusedElement returns an owned element or an error; subsequent
        // Current* reads borrow it for the duration of each call.
        let element = unsafe { self.automation.GetFocusedElement()? };
        // SAFETY: reading scalar properties of a live element.
        let control_type = unsafe { element.CurrentControlType()?.0 };
        // SAFETY: as above.
        let is_enabled = unsafe { element.CurrentIsEnabled()?.as_bool() };
        // SAFETY: as above; unknown password status is treated as a password below.
        let is_password = unsafe { element.CurrentIsPassword() }
            .map(|b| b.as_bool())
            .unwrap_or(true);
        Ok(FieldProperties {
            control_type,
            is_password,
            is_enabled,
            // Read-only detection via the Value/Text patterns is refined in Step 06;
            // for now an enabled control is treated as editable.
            is_editable: is_enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(
        control_type: i32,
        is_password: bool,
        is_enabled: bool,
        is_editable: bool,
    ) -> FieldProperties {
        FieldProperties {
            control_type,
            is_password,
            is_enabled,
            is_editable,
        }
    }

    #[test]
    fn normal_edit_field_is_capturable() {
        let c = classify_field(&props(UIA_EditControlTypeId.0, false, true, true));
        assert_eq!(c, FieldClass::NormalProse);
        assert!(c.capture_allowed());
    }

    #[test]
    fn document_field_is_capturable() {
        let c = classify_field(&props(UIA_DocumentControlTypeId.0, false, true, true));
        assert_eq!(c, FieldClass::NormalProse);
    }

    #[test]
    fn password_field_is_never_capturable() {
        let c = classify_field(&props(UIA_EditControlTypeId.0, true, true, true));
        assert_eq!(c, FieldClass::Sensitive);
        assert!(!c.capture_allowed());
    }

    #[test]
    fn disabled_readonly_and_unknown_fail_closed() {
        assert_eq!(
            classify_field(&props(UIA_EditControlTypeId.0, false, false, true)),
            FieldClass::NonProse
        );
        assert_eq!(
            classify_field(&props(UIA_EditControlTypeId.0, false, true, false)),
            FieldClass::NonProse
        );
        // An unrecognised control type (e.g. a button: 50000) fails closed.
        let unknown = classify_field(&props(50000, false, true, true));
        assert_eq!(unknown, FieldClass::Unknown);
        assert!(!unknown.capture_allowed());
    }
}
