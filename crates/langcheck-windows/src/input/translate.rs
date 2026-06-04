//! Character translation: turn observed virtual-key events into the abstract
//! [`Translated`] actions the core session consumes.
//!
//! Translation happens on the coordinator thread, never in the hook callback
//! (`blueprint.md` Section 11.2). The MVP supports ordinary ASCII Latin input via
//! an explicit virtual-key map (English layout); anything uncertain — a key with
//! an active Ctrl/Alt/Win modifier, navigation/editing keys, or an unrecognised
//! key — yields a session reset rather than a guessed character. Dead keys and IME
//! composition are out of scope and reset the session. The per-key map is pure and
//! unit-tested.
//!
//! Implemented in delivery Step 06 (End-to-End Conservative Autocorrect).

use langcheck_core::session::{Boundary, ResetReason};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1,
    VK_OEM_2, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_PRIOR, VK_RCONTROL, VK_RIGHT,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

use super::{InputEvent, InputEventKind};

/// The result of translating one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Translated {
    /// A literal character was produced.
    Char(char),
    /// A safe boundary character was produced.
    Boundary(Boundary),
    /// Backspace.
    Backspace,
    /// The session must reset for this reason.
    Reset(ResetReason),
    /// A modifier or otherwise-ignored key with no token effect.
    Ignore,
}

/// Tracks modifier/lock state across events and translates non-modifier keys.
#[derive(Debug, Clone, Copy)]
pub struct KeyTranslator {
    shift: bool,
    ctrl: bool,
    alt: bool,
    win: bool,
    caps: bool,
}

impl Default for KeyTranslator {
    fn default() -> Self {
        // Best-effort initial Caps Lock toggle; tracked precisely thereafter.
        // SAFETY: GetKeyState has no preconditions.
        let caps = (unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1) != 0;
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            win: false,
            caps,
        }
    }
}

impl KeyTranslator {
    /// Translate one event, updating modifier state.
    pub fn translate(&mut self, event: &InputEvent) -> Translated {
        let down = event.kind == InputEventKind::KeyDown;
        let vk = event.virtual_key;

        if vk == VK_SHIFT.0 || vk == VK_LSHIFT.0 || vk == VK_RSHIFT.0 {
            self.shift = down;
            return Translated::Ignore;
        }
        if vk == VK_CONTROL.0 || vk == VK_LCONTROL.0 || vk == VK_RCONTROL.0 {
            self.ctrl = down;
            return Translated::Ignore;
        }
        if vk == VK_MENU.0 || vk == VK_LMENU.0 || vk == VK_RMENU.0 {
            self.alt = down;
            return Translated::Ignore;
        }
        if vk == VK_LWIN.0 || vk == VK_RWIN.0 {
            self.win = down;
            return Translated::Ignore;
        }
        if vk == VK_CAPITAL.0 {
            if down {
                self.caps = !self.caps;
            }
            return Translated::Ignore;
        }

        // Only key-down events produce characters or actions.
        if !down {
            return Translated::Ignore;
        }
        // A character typed with Ctrl/Alt/Win held is a shortcut, not text.
        if self.ctrl || self.alt || self.win {
            return Translated::Reset(ResetReason::ModifierActive);
        }
        translate_key(vk, self.shift, self.caps)
    }
}

/// Pure virtual-key → [`Translated`] map for the English MVP layout.
pub fn translate_key(vk: u16, shift: bool, caps: bool) -> Translated {
    // ASCII letters: 'A'..='Z' share their virtual-key codes (0x41..=0x5A).
    if (0x41..=0x5A).contains(&vk) {
        let base = b'a' + (vk - 0x41) as u8;
        let ch = if shift ^ caps {
            base.to_ascii_uppercase()
        } else {
            base
        };
        return Translated::Char(ch as char);
    }
    // Top-row digits: 0x30..=0x39. Shifted symbols are mostly non-prose.
    if (0x30..=0x39).contains(&vk) {
        if !shift {
            return Translated::Char((b'0' + (vk - 0x30) as u8) as char);
        }
        // Shift+1 is '!', a safe boundary; other shifted digits are non-prose.
        return if vk == 0x31 {
            Translated::Boundary(Boundary::Exclamation)
        } else {
            Translated::Reset(ResetReason::UnknownTranslation)
        };
    }

    match vk {
        v if v == VK_SPACE.0 => Translated::Boundary(Boundary::Space),
        v if v == VK_BACK.0 => Translated::Backspace,
        v if v == VK_OEM_PERIOD.0 => {
            boundary_or_reset(shift, Boundary::Period) // '.' / '>'
        }
        v if v == VK_OEM_COMMA.0 => boundary_or_reset(shift, Boundary::Comma), // ',' / '<'
        v if v == VK_OEM_1.0 => {
            // ';' unshifted, ':' shifted — both safe boundaries.
            Translated::Boundary(if shift {
                Boundary::Colon
            } else {
                Boundary::Semicolon
            })
        }
        v if v == VK_OEM_2.0 => {
            // '/' unshifted (kept in token -> classified as a path), '?' shifted.
            if shift {
                Translated::Boundary(Boundary::Question)
            } else {
                Translated::Char('/')
            }
        }
        v if v == VK_OEM_7.0 => {
            // apostrophe unshifted (word-internal), '"' shifted -> reset.
            if shift {
                Translated::Reset(ResetReason::UnknownTranslation)
            } else {
                Translated::Char('\'')
            }
        }
        v if v == VK_OEM_MINUS.0 => Translated::Char(if shift { '_' } else { '-' }),
        v if v == VK_RETURN_VK => Translated::Reset(ResetReason::Newline),
        v if v == VK_TAB.0 => Translated::Reset(ResetReason::Tab),
        v if v == VK_DELETE.0 => Translated::Reset(ResetReason::Deletion),
        v if v == VK_LEFT.0
            || v == VK_RIGHT.0
            || v == VK_UP.0
            || v == VK_DOWN.0
            || v == VK_HOME.0
            || v == VK_END.0
            || v == VK_PRIOR.0
            || v == VK_NEXT.0
            || v == VK_INSERT.0 =>
        {
            Translated::Reset(ResetReason::Navigation)
        }
        v if v == VK_ESCAPE.0 => Translated::Reset(ResetReason::Navigation),
        _ => Translated::Reset(ResetReason::UnknownTranslation),
    }
}

/// VK_RETURN (0x0D); named here because it is not in the imported set.
const VK_RETURN_VK: u16 = 0x0D;

/// A safe boundary when unshifted, or a reset when shifted (the shifted glyph,
/// e.g. '>' or '<', is non-prose).
fn boundary_or_reset(shift: bool, boundary: Boundary) -> Translated {
    if shift {
        Translated::Reset(ResetReason::UnknownTranslation)
    } else {
        Translated::Boundary(boundary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_respect_shift_and_caps() {
        assert_eq!(translate_key(0x41, false, false), Translated::Char('a'));
        assert_eq!(translate_key(0x41, true, false), Translated::Char('A'));
        assert_eq!(translate_key(0x41, false, true), Translated::Char('A'));
        // shift XOR caps: both set -> lowercase.
        assert_eq!(translate_key(0x41, true, true), Translated::Char('a'));
    }

    #[test]
    fn boundaries_are_mapped() {
        assert_eq!(
            translate_key(VK_SPACE.0, false, false),
            Translated::Boundary(Boundary::Space)
        );
        assert_eq!(
            translate_key(VK_OEM_PERIOD.0, false, false),
            Translated::Boundary(Boundary::Period)
        );
        assert_eq!(
            translate_key(VK_OEM_1.0, false, false),
            Translated::Boundary(Boundary::Semicolon)
        );
        assert_eq!(
            translate_key(VK_OEM_1.0, true, false),
            Translated::Boundary(Boundary::Colon)
        );
        assert_eq!(
            translate_key(0x31, true, false),
            Translated::Boundary(Boundary::Exclamation)
        );
    }

    #[test]
    fn apostrophe_is_word_internal_quote_resets() {
        assert_eq!(
            translate_key(VK_OEM_7.0, false, false),
            Translated::Char('\'')
        );
        assert_eq!(
            translate_key(VK_OEM_7.0, true, false),
            Translated::Reset(ResetReason::UnknownTranslation)
        );
    }

    #[test]
    fn navigation_and_editing_reset() {
        assert_eq!(
            translate_key(VK_LEFT.0, false, false),
            Translated::Reset(ResetReason::Navigation)
        );
        assert_eq!(
            translate_key(VK_DELETE.0, false, false),
            Translated::Reset(ResetReason::Deletion)
        );
        assert_eq!(
            translate_key(VK_TAB.0, false, false),
            Translated::Reset(ResetReason::Tab)
        );
        assert_eq!(
            translate_key(VK_RETURN_VK, false, false),
            Translated::Reset(ResetReason::Newline)
        );
    }

    #[test]
    fn backspace_and_digits() {
        assert_eq!(
            translate_key(VK_BACK.0, false, false),
            Translated::Backspace
        );
        assert_eq!(translate_key(0x30, false, false), Translated::Char('0'));
    }

    #[test]
    fn modifier_held_character_resets() {
        let mut t = KeyTranslator {
            shift: false,
            ctrl: true,
            alt: false,
            win: false,
            caps: false,
        };
        let event = InputEvent {
            generation: 1,
            timestamp_ms: 0,
            kind: InputEventKind::KeyDown,
            virtual_key: 0x41, // 'a' with Ctrl held = Ctrl+A
            scan_code: 0,
            flags: 0,
        };
        assert_eq!(
            t.translate(&event),
            Translated::Reset(ResetReason::ModifierActive)
        );
    }

    #[test]
    fn shift_press_is_tracked_then_capitalizes() {
        let mut t = KeyTranslator {
            shift: false,
            ctrl: false,
            alt: false,
            win: false,
            caps: false,
        };
        let shift_down = InputEvent {
            generation: 1,
            timestamp_ms: 0,
            kind: InputEventKind::KeyDown,
            virtual_key: VK_SHIFT.0,
            scan_code: 0,
            flags: 0,
        };
        assert_eq!(t.translate(&shift_down), Translated::Ignore);
        let a_down = InputEvent {
            virtual_key: 0x41,
            ..shift_down
        };
        assert_eq!(t.translate(&a_down), Translated::Char('A'));
    }
}
