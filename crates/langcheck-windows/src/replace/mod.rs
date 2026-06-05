//! Replacement executor.
//!
//! Applies a correction with a single `SendInput` batch — backspaces to erase the
//! original token and the boundary the user typed, the corrected word as Unicode
//! input, then the original boundary re-emitted. Every injected event carries
//! `dwExtraInfo = LANGCHECK_INJECTED_MARKER` so the observer ignores it and never
//! recurses. The clipboard is never used, and a target at a higher integrity level
//! is detected and skipped (UIPI is never bypassed). A partial/failed insertion
//! errors without blind retry (see `blueprint.md` Sections 8.10, 8.11, 12.2, 17).
//!
//! The plan → keystroke-sequence logic is pure and unit-tested; only the actual
//! `SendInput`/foreground/integrity calls touch Win32.
//!
//! Implemented in delivery Step 05 (Safe Replacement Executor).
//!
//! NOTE (manual verification): injection against real controls (Edit / Rich Edit /
//! password / read-only / elevated) is verified with `langcheck --replace-demo`;
//! it cannot be exercised from the build environment.

use langcheck_core::session::Boundary;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::integrity;
use crate::LANGCHECK_INJECTED_MARKER;

/// Upper bound on a single side of a replacement (token / replacement length), so
/// a batch can never grow unboundedly (`blueprint.md` Section 9).
pub const MAX_REPLACEMENT_CHARS: usize = 64;

/// A planned correction to apply (`blueprint.md` Section 8.10). Built by the
/// coordinator after its final safety checks pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacementPlan {
    /// The focus the plan was built for (revalidated by the coordinator).
    pub focus_id: u64,
    /// The physical-input generation expected at commit (the boundary event).
    pub expected_generation: u64,
    /// The original token, as typed.
    pub original: String,
    /// The replacement word, with case already restored.
    pub replacement: String,
    /// The boundary the user typed (erased and then re-emitted).
    pub boundary: Boundary,
}

/// A record of an applied correction, used to reverse it (delivery Step 10/11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoTransaction {
    pub focus_id: u64,
    pub original: String,
    pub replacement: String,
    pub boundary: Boundary,
}

/// A single synthetic keystroke in a replacement batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Press and release Backspace.
    Backspace,
    /// Type one (ASCII, BMP) character as Unicode input.
    Type(char),
}

/// Errors from planning or executing a replacement.
#[derive(Debug)]
pub enum ReplaceError {
    /// The replacement was empty.
    EmptyReplacement,
    /// The original or replacement is not ASCII (out of scope for the MVP).
    NotAscii,
    /// The plan exceeded the length bound.
    TooLong,
    /// There is no foreground window to target.
    NoForegroundWindow,
    /// The target is at a higher integrity level; injection is skipped (UIPI).
    TargetHigherIntegrity,
    /// An integrity-level read failed; fail closed (no replacement).
    IntegrityCheckFailed(windows::core::Error),
    /// `SendInput` inserted fewer events than planned; state must be cleared.
    PartialInsertion { sent: u32, expected: u32 },
    /// `SendInput` inserted nothing.
    SendInputFailed,
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplaceError::EmptyReplacement => f.write_str("empty replacement"),
            ReplaceError::NotAscii => f.write_str("non-ASCII replacement (unsupported in MVP)"),
            ReplaceError::TooLong => f.write_str("replacement plan exceeds length bound"),
            ReplaceError::NoForegroundWindow => f.write_str("no foreground window"),
            ReplaceError::TargetHigherIntegrity => {
                f.write_str("target is higher integrity; skipped (UIPI)")
            }
            ReplaceError::IntegrityCheckFailed(e) => write!(f, "integrity check failed: {e}"),
            ReplaceError::PartialInsertion { sent, expected } => {
                write!(f, "partial insertion: sent {sent} of {expected}")
            }
            ReplaceError::SendInputFailed => f.write_str("SendInput inserted nothing"),
        }
    }
}

impl std::error::Error for ReplaceError {}

/// Build the keystroke sequence that turns `original<boundary>` into
/// `replacement<boundary>`: erase the token and the boundary, type the
/// replacement, re-emit the boundary. Pure and fully unit-tested.
pub fn build_key_actions(
    original: &str,
    replacement: &str,
    boundary: Boundary,
) -> Result<Vec<KeyAction>, ReplaceError> {
    if replacement.is_empty() {
        return Err(ReplaceError::EmptyReplacement);
    }
    if !original.is_ascii() || !replacement.is_ascii() {
        return Err(ReplaceError::NotAscii);
    }
    let original_len = original.chars().count();
    let replacement_len = replacement.chars().count();
    if original_len > MAX_REPLACEMENT_CHARS || replacement_len > MAX_REPLACEMENT_CHARS {
        return Err(ReplaceError::TooLong);
    }

    let mut actions = Vec::with_capacity(original_len + replacement_len + 2);
    // Erase the original token plus the boundary the user already typed.
    for _ in 0..=original_len {
        actions.push(KeyAction::Backspace);
    }
    // Type the corrected word, then re-emit the original boundary.
    for ch in replacement.chars() {
        actions.push(KeyAction::Type(ch));
    }
    actions.push(KeyAction::Type(boundary.as_char()));
    Ok(actions)
}

/// Applies a [`ReplacementPlan`] to the focused control.
pub trait ReplacementExecutor {
    /// Execute the plan, returning an [`UndoTransaction`] on success. Never uses
    /// the clipboard, never targets a higher-integrity window, and never retries a
    /// partial insertion.
    fn execute(&mut self, plan: &ReplacementPlan) -> Result<UndoTransaction, ReplaceError>;

    /// Reverse a correction immediately after the user's rejecting Backspace: the
    /// boundary has already been deleted by that Backspace, so erase the remaining
    /// `replacement` and restore `original` followed by the `boundary`.
    fn execute_undo(
        &mut self,
        original: &str,
        replacement: &str,
        boundary: Boundary,
    ) -> Result<(), ReplaceError>;
}

/// The MVP executor: a single `SendInput` batch with injected-event marking.
#[derive(Debug, Default)]
pub struct SendInputExecutor;

impl ReplacementExecutor for SendInputExecutor {
    fn execute(&mut self, plan: &ReplacementPlan) -> Result<UndoTransaction, ReplaceError> {
        let actions = build_key_actions(&plan.original, &plan.replacement, plan.boundary)?;
        check_foreground_target()?;
        send_actions(&actions)?;
        Ok(UndoTransaction {
            focus_id: plan.focus_id,
            original: plan.original.clone(),
            replacement: plan.replacement.clone(),
            boundary: plan.boundary,
        })
    }

    fn execute_undo(
        &mut self,
        original: &str,
        replacement: &str,
        boundary: Boundary,
    ) -> Result<(), ReplaceError> {
        let actions = build_undo_key_actions(original, replacement, boundary)?;
        check_foreground_target()?;
        send_actions(&actions)
    }
}

/// Build the keystroke sequence to undo a correction: erase the `replacement`
/// (its boundary was already removed by the user's Backspace) and type the
/// `original` followed by the `boundary`. Pure and unit-tested.
pub fn build_undo_key_actions(
    original: &str,
    replacement: &str,
    boundary: Boundary,
) -> Result<Vec<KeyAction>, ReplaceError> {
    if original.is_empty() {
        return Err(ReplaceError::EmptyReplacement);
    }
    if !original.is_ascii() || !replacement.is_ascii() {
        return Err(ReplaceError::NotAscii);
    }
    let original_len = original.chars().count();
    let replacement_len = replacement.chars().count();
    if original_len > MAX_REPLACEMENT_CHARS || replacement_len > MAX_REPLACEMENT_CHARS {
        return Err(ReplaceError::TooLong);
    }

    let mut actions = Vec::with_capacity(replacement_len + original_len + 1);
    for _ in 0..replacement_len {
        actions.push(KeyAction::Backspace);
    }
    for ch in original.chars() {
        actions.push(KeyAction::Type(ch));
    }
    actions.push(KeyAction::Type(boundary.as_char()));
    Ok(actions)
}

/// Inject `text` as marked Unicode keystrokes (no backspaces). Intended for the
/// manual-verification demo; validates ASCII and the length bound.
pub fn inject_text(text: &str) -> Result<(), ReplaceError> {
    if !text.is_ascii() {
        return Err(ReplaceError::NotAscii);
    }
    if text.chars().count() > MAX_REPLACEMENT_CHARS {
        return Err(ReplaceError::TooLong);
    }
    let actions: Vec<KeyAction> = text.chars().map(KeyAction::Type).collect();
    send_actions(&actions)
}

/// Verify the foreground window exists and is not at a higher integrity level than
/// us (so injection is permitted by UIPI). Public so callers can pre-check before
/// building a plan.
pub fn check_foreground_target() -> Result<(), ReplaceError> {
    // SAFETY: GetForegroundWindow has no preconditions; it may return a null HWND.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Err(ReplaceError::NoForegroundWindow);
    }
    let current = integrity::current().map_err(ReplaceError::IntegrityCheckFailed)?;
    let target = integrity::of_window(foreground).map_err(ReplaceError::IntegrityCheckFailed)?;
    if integrity::is_target_higher(current, target) {
        return Err(ReplaceError::TargetHigherIntegrity);
    }
    Ok(())
}

/// Convert actions to a single `SendInput` batch and verify the inserted count.
fn send_actions(actions: &[KeyAction]) -> Result<(), ReplaceError> {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(actions.len() * 2);
    for action in actions {
        match action {
            KeyAction::Backspace => {
                inputs.push(key_input(VK_BACK, 0, KEYBD_EVENT_FLAGS(0)));
                inputs.push(key_input(VK_BACK, 0, KEYEVENTF_KEYUP));
            }
            KeyAction::Type(ch) => {
                let scan = *ch as u16;
                inputs.push(key_input(VIRTUAL_KEY(0), scan, KEYEVENTF_UNICODE));
                inputs.push(key_input(
                    VIRTUAL_KEY(0),
                    scan,
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                ));
            }
        }
    }
    let expected = inputs.len() as u32;
    // SAFETY: `inputs` is a valid, non-empty slice of INPUT and `cbsize` is the
    // exact struct size, as SendInput requires.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent == 0 {
        return Err(ReplaceError::SendInputFailed);
    }
    if sent != expected {
        // Partial insertion: do not retry; the coordinator clears session/undo state.
        return Err(ReplaceError::PartialInsertion { sent, expected });
    }
    Ok(())
}

/// Build one keyboard `INPUT`, always carrying the LangCheck injected marker.
fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: LANGCHECK_INJECTED_MARKER,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_erase_token_and_boundary_then_type_replacement() {
        let actions = build_key_actions("teh", "the", Boundary::Space).unwrap();
        // 4 backspaces ("teh" + space), then "the", then space.
        let backspaces = actions
            .iter()
            .filter(|a| **a == KeyAction::Backspace)
            .count();
        assert_eq!(backspaces, 4);
        let typed: String = actions
            .iter()
            .filter_map(|a| match a {
                KeyAction::Type(c) => Some(*c),
                KeyAction::Backspace => None,
            })
            .collect();
        assert_eq!(typed, "the ");
    }

    #[test]
    fn punctuation_boundary_is_reemitted() {
        let actions = build_key_actions("wrd", "word", Boundary::Period).unwrap();
        assert_eq!(
            actions
                .iter()
                .filter(|a| **a == KeyAction::Backspace)
                .count(),
            4
        );
        assert_eq!(actions.last(), Some(&KeyAction::Type('.')));
    }

    #[test]
    fn rejects_empty_non_ascii_and_overlong() {
        assert!(matches!(
            build_key_actions("teh", "", Boundary::Space),
            Err(ReplaceError::EmptyReplacement)
        ));
        assert!(matches!(
            build_key_actions("teh", "thé", Boundary::Space),
            Err(ReplaceError::NotAscii)
        ));
        let long = "a".repeat(MAX_REPLACEMENT_CHARS + 1);
        assert!(matches!(
            build_key_actions(&long, "the", Boundary::Space),
            Err(ReplaceError::TooLong)
        ));
    }

    #[test]
    fn event_count_is_bounded_and_even() {
        // Each action is a down+up pair, so SendInput's expected count is 2 * actions.
        let actions = build_key_actions("teh", "the", Boundary::Space).unwrap();
        assert_eq!(actions.len(), 4 + 3 + 1); // 4 backspaces + "the" + space
    }

    #[test]
    fn undo_erases_replacement_and_restores_original() {
        // Boundary already removed by the user's Backspace: erase "the" (3), then
        // type "teh" + space.
        let actions = build_undo_key_actions("teh", "the", Boundary::Space).unwrap();
        assert_eq!(
            actions
                .iter()
                .filter(|a| **a == KeyAction::Backspace)
                .count(),
            3
        );
        let typed: String = actions
            .iter()
            .filter_map(|a| match a {
                KeyAction::Type(c) => Some(*c),
                KeyAction::Backspace => None,
            })
            .collect();
        assert_eq!(typed, "teh ");
    }
}
