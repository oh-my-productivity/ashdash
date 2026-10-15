//! Auto-paste: copy, then send Cmd+V to the frontmost application.
//!
//! Posting keyboard events needs the **Accessibility** permission. We check
//! `AXIsProcessTrusted()` first and report a clear error instead of silently
//! doing nothing. The sequencing lives in [`paste_via`] on top of the small
//! [`KeySender`] trait, so it is unit-tested with a fake; [`SystemKeys`] is the
//! CoreGraphics implementation.
//!
//! Known limits:
//! * The key code for "V" is the physical ANSI position (9). On layouts that
//!   put V elsewhere (e.g. Dvorak) Cmd+V then triggers a different shortcut;
//!   looking the code up via the active layout is a follow-up.
//! * The caller must hide the launcher window and let focus return to the
//!   previous app *before* pasting; `settle` is the extra delay to wait for it.
//! * `{cursor}` is applied by pressing Left `cursor_back` times, which counts
//!   `char`s, so text with multi-scalar grapheme clusters (some emoji) can
//!   leave the caret slightly off.

use std::time::Duration;

use super::restore;
use crate::brand;
use crate::engine::Outcome;

pub const KEY_V: u16 = 9;
pub const KEY_LEFT: u16 = 123;
/// Beyond this many characters the caret is not repositioned.
pub const MAX_CURSOR_PRESSES: usize = 4096;
/// Default time to wait for the launcher to hide and focus to return.
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError {
    /// Accessibility permission is missing.
    NotTrusted,
    Failed(String),
}

impl std::fmt::Display for PasteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasteError::NotTrusted => write!(
                f,
                "Auto-paste needs the Accessibility permission: open System Settings > Privacy & Security > \
                 Accessibility, enable {}, then try again.",
                brand::NAME
            ),
            PasteError::Failed(m) => write!(f, "Auto-paste failed: {m}"),
        }
    }
}

impl std::error::Error for PasteError {}

/// Sends synthetic key presses.
pub trait KeySender {
    /// Whether this process may post keyboard events.
    fn trusted(&self) -> bool;
    /// Press and release `keycode`, with Command held when `command` is true.
    fn tap(&self, keycode: u16, command: bool) -> Result<(), String>;
    fn pause(&self, d: Duration);
}

/// Send Cmd+V and then, if `cursor_back` is set, Left that many times.
pub fn paste_via(keys: &dyn KeySender, cursor_back: Option<usize>, settle: Duration) -> Result<(), PasteError> {
    if !keys.trusted() {
        return Err(PasteError::NotTrusted);
    }
    keys.pause(settle);
    keys.tap(KEY_V, true).map_err(PasteError::Failed)?;
    if let Some(n) = cursor_back.filter(|n| (1..=MAX_CURSOR_PRESSES).contains(n)) {
        // Let the target app finish handling the paste before moving the caret.
        keys.pause(Duration::from_millis(80));
        for _ in 0..n {
            keys.tap(KEY_LEFT, false).map_err(PasteError::Failed)?;
        }
    }
    Ok(())
}

/// Send Cmd+V to the frontmost application using CoreGraphics.
pub fn send_paste(cursor_back: Option<usize>, settle: Duration) -> Result<(), PasteError> {
    paste_via(&SystemKeys, cursor_back, settle)
}

/// Whether this process has the Accessibility permission (no prompt).
pub fn is_accessibility_trusted() -> bool {
    SystemKeys.trusted()
}

/// Copy `text` and paste it into the frontmost app. The text is copied even
/// when pasting is not permitted, and the error then says so.
pub fn paste_text(text: &str, cursor_back: Option<usize>, settle: Duration) -> Result<(), PasteError> {
    restore::copy_text(text).map_err(|e| PasteError::Failed(format!("{e:#}")))?;
    send_paste(cursor_back, settle)
}

/// Turn a paste result into an [`Outcome`] with a user-facing message.
pub fn outcome(r: Result<(), PasteError>) -> Outcome {
    match r {
        Ok(()) => Outcome::Done,
        Err(PasteError::NotTrusted) => Outcome::Error(format!("Copied. {}", PasteError::NotTrusted)),
        Err(e) => Outcome::Error(e.to_string()),
    }
}

/// CoreGraphics-backed key sender (macOS only; elsewhere never trusted).
pub struct SystemKeys;

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub fn AXIsProcessTrusted() -> u8;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, down: bool) -> *mut c_void;
        pub fn CGEventSetFlags(event: *mut c_void, flags: u64);
        pub fn CGEventPost(tap: u32, event: *mut c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFRelease(cf: *const c_void);
    }

    pub const COMMAND_FLAG: u64 = 0x0010_0000; // kCGEventFlagMaskCommand
    pub const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap

    /// Create a key event; the caller owns the (non-null) result.
    pub fn key_event(keycode: u16, down: bool, command: bool) -> Option<*mut c_void> {
        // SAFETY: a null source is allowed; the returned CF object is
        // released by the caller or `post`.
        unsafe {
            let e = CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, down);
            if e.is_null() {
                return None;
            }
            // Always set the flags so a physically held modifier does not leak in.
            CGEventSetFlags(e, if command { COMMAND_FLAG } else { 0 });
            Some(e)
        }
    }

    /// Post and release an event created by [`key_event`].
    pub fn post(e: *mut c_void) {
        // SAFETY: `e` is a live CGEvent from `key_event`, consumed here.
        unsafe {
            CGEventPost(HID_EVENT_TAP, e);
            CFRelease(e);
        }
    }

    pub fn release(e: *mut c_void) {
        // SAFETY: `e` is a live CGEvent from `key_event`, consumed here.
        unsafe { CFRelease(e) }
    }
}

#[cfg(target_os = "macos")]
impl KeySender for SystemKeys {
    fn trusted(&self) -> bool {
        // SAFETY: plain query, no arguments.
        unsafe { sys::AXIsProcessTrusted() != 0 }
    }

    fn tap(&self, keycode: u16, command: bool) -> Result<(), String> {
        let down = sys::key_event(keycode, true, command).ok_or("could not create a key event")?;
        let Some(up) = sys::key_event(keycode, false, command) else {
            sys::release(down);
            return Err("could not create a key event".into());
        };
        sys::post(down);
        std::thread::sleep(Duration::from_millis(8));
        sys::post(up);
        std::thread::sleep(Duration::from_millis(8));
        Ok(())
    }

    fn pause(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(not(target_os = "macos"))]
impl KeySender for SystemKeys {
    fn trusted(&self) -> bool {
        false
    }
    fn tap(&self, _: u16, _: bool) -> Result<(), String> {
        Err("auto-paste is only available on macOS".into())
    }
    fn pause(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Fake {
        trusted: bool,
        log: RefCell<Vec<String>>,
        fail_on: Option<u16>,
    }

    impl KeySender for Fake {
        fn trusted(&self) -> bool {
            self.trusted
        }
        fn tap(&self, keycode: u16, command: bool) -> Result<(), String> {
            if self.fail_on == Some(keycode) {
                return Err("boom".into());
            }
            self.log
                .borrow_mut()
                .push(format!("{}{keycode}", if command { "cmd+" } else { "" }));
            Ok(())
        }
        fn pause(&self, d: Duration) {
            self.log.borrow_mut().push(format!("pause{}", d.as_millis()));
        }
    }

    #[test]
    fn untrusted_sends_nothing_and_reports_accessibility() {
        let f = Fake::default();
        assert_eq!(paste_via(&f, Some(3), DEFAULT_SETTLE), Err(PasteError::NotTrusted));
        assert!(f.log.borrow().is_empty());
        let msg = PasteError::NotTrusted.to_string();
        assert!(msg.contains("Accessibility") && msg.contains(brand::NAME), "{msg}");
        let Outcome::Error(m) = outcome(Err(PasteError::NotTrusted)) else {
            panic!("expected an error outcome")
        };
        assert!(m.starts_with("Copied.") && m.contains("Accessibility"), "{m}");
    }

    #[test]
    fn plain_paste_is_settle_then_cmd_v() {
        let f = Fake {
            trusted: true,
            ..Default::default()
        };
        paste_via(&f, None, Duration::from_millis(150)).unwrap();
        assert_eq!(*f.log.borrow(), vec!["pause150", "cmd+9"]);
    }

    #[test]
    fn cursor_moves_left_after_the_paste() {
        let f = Fake {
            trusted: true,
            ..Default::default()
        };
        paste_via(&f, Some(3), Duration::ZERO).unwrap();
        assert_eq!(*f.log.borrow(), vec!["pause0", "cmd+9", "pause80", "123", "123", "123"]);
    }

    #[test]
    fn zero_or_huge_cursor_offsets_do_not_move_the_caret() {
        for n in [Some(0), Some(MAX_CURSOR_PRESSES + 1), None] {
            let f = Fake {
                trusted: true,
                ..Default::default()
            };
            paste_via(&f, n, Duration::ZERO).unwrap();
            assert_eq!(f.log.borrow().len(), 2, "{n:?}");
        }
    }

    #[test]
    fn key_failures_are_reported() {
        let f = Fake {
            trusted: true,
            fail_on: Some(KEY_V),
            ..Default::default()
        };
        assert_eq!(
            paste_via(&f, None, Duration::ZERO),
            Err(PasteError::Failed("boom".into()))
        );
        assert_eq!(outcome(Ok(())), Outcome::Done);
    }

    /// Creating (but never posting) events proves the CoreGraphics bindings
    /// link and behave; posting would type into whatever app is focused.
    #[cfg(target_os = "macos")]
    #[test]
    fn coregraphics_events_can_be_created_without_posting() {
        let e = sys::key_event(KEY_V, true, true).expect("event");
        sys::release(e);
        // Querying trust must not prompt or panic, whatever the answer is.
        let _ = is_accessibility_trusted();
    }
}
