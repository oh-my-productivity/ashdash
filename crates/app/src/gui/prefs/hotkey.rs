//! The hotkey recorder (pure, tested) and re-registration of the global launcher hotkey.

use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

use super::super::keynames::{display_hotkey, normalize_spec};
use crate::prefs_model::{system_hotkey_conflict, validate_hotkey};

// ------------------------------------------------------------------ which id means "the launcher"

/// Id of the currently registered launcher hotkey (0 = none). The process-wide event handler
/// (installed once, see `window::install_hotkey`) reads it, so the hotkey can change at run time.
static MAIN_ID: AtomicU32 = AtomicU32::new(0);

pub(in crate::gui) fn main_id() -> u32 {
    MAIN_ID.load(Ordering::Relaxed)
}

pub(in crate::gui) fn set_main_id(id: u32) {
    MAIN_ID.store(id, Ordering::Relaxed);
}

/// Register `new` as the launcher hotkey and release `old`. On failure nothing changes: the
/// previous hotkey stays registered and in force. Needs the main thread, like all hotkey calls.
pub(in crate::gui) fn rebind(manager: &mut Option<GlobalHotKeyManager>, old: &str, new: &str) -> Result<(), String> {
    let new_key = HotKey::from_str(new).map_err(|e| format!("invalid hotkey '{new}': {e}"))?;
    let old_key = HotKey::from_str(old).ok();
    if manager.is_some() && main_id() == new_key.id() {
        return Ok(());
    }
    if manager.is_none() {
        *manager = Some(GlobalHotKeyManager::new().map_err(|e| format!("hotkeys unavailable: {e}"))?);
    }
    let Some(mgr) = manager.as_ref() else {
        return Err("hotkeys unavailable".into());
    };
    mgr.register(new_key)
        .map_err(|e| format!("could not register '{new}': {e}"))?;
    set_main_id(new_key.id());
    if let Some(old_key) = old_key.filter(|k| k.id() != new_key.id()) {
        let _ = mgr.unregister(old_key);
    }
    Ok(())
}

/// Can this process register `spec` right now? Registers and immediately releases it, so a
/// shortcut that macOS or another app already owns is refused *before* it is saved.
pub fn probe(spec: &str) -> Result<(), String> {
    let key = HotKey::from_str(spec).map_err(|e| format!("invalid shortcut: {e}"))?;
    let mgr = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
    mgr.register(key).map_err(|e| e.to_string())?;
    let _ = mgr.unregister(key);
    Ok(())
}

/// A shortcut in words (`Option+Shift+Space`): the default UI font has no ⌥ or ⌃ glyph.
pub fn label(spec: &str) -> String {
    let Some(display) = display_hotkey(spec) else {
        return spec.to_string();
    };
    display
        .replace('⌃', "Ctrl+")
        .replace('⌥', "Option+")
        .replace('⇧', "Shift+")
        .replace('⌘', "Cmd+")
}

// ------------------------------------------------------------------ the recorder

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

/// One key press seen while recording: modifiers held plus the key's canonical name
/// (`KeyA`, `Space`, `F5`, ... as in `keynames`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Press {
    pub mods: Mods,
    pub code: String,
}

/// A recorded shortcut, ready to be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub spec: String,
    pub display: String,
    /// A macOS shortcut this one collides with, if known.
    pub conflict: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RecorderState {
    #[default]
    Idle,
    /// Waiting for a key combination.
    Listening,
    /// The last combination was refused (still listening).
    Rejected(String),
}

#[derive(Debug, Default)]
pub struct Recorder {
    pub state: RecorderState,
}

impl Recorder {
    pub fn start(&mut self) {
        self.state = RecorderState::Listening;
    }

    pub fn cancel(&mut self) {
        self.state = RecorderState::Idle;
    }

    pub fn is_listening(&self) -> bool {
        !matches!(self.state, RecorderState::Idle)
    }

    /// Feed one key press. Returns the shortcut once a valid combination was typed; Escape
    /// alone cancels; Backspace alone answers with the default shortcut (the keyboard route to
    /// "reset to default"); any other key without a modifier is refused and recording continues.
    pub fn feed(&mut self, press: &Press) -> Option<Captured> {
        if !self.is_listening() {
            return None;
        }
        let m = press.mods;
        let any_mod = m.ctrl || m.alt || m.shift || m.cmd;
        if press.code == "Escape" && !any_mod {
            self.cancel();
            return None;
        }
        if press.code == "Backspace" && !any_mod {
            self.state = RecorderState::Idle;
            let spec = ashdash_core::brand::DEFAULT_HOTKEY.to_string();
            return Some(Captured {
                display: display_hotkey(&spec).unwrap_or_else(|| spec.clone()),
                conflict: None,
                spec,
            });
        }
        if !(m.ctrl || m.alt || m.cmd) {
            // Shift alone would swallow capital letters everywhere.
            self.state = RecorderState::Rejected("Hold Cmd, Option or Ctrl together with the key.".into());
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        for (on, n) in [(m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift"), (m.cmd, "cmd")] {
            if on {
                parts.push(n);
            }
        }
        parts.push(&press.code);
        let candidate = parts.join("+");
        let spec = normalize_spec(&candidate).and_then(|s| validate_hotkey(&s).ok());
        let Some(spec) = spec else {
            self.state = RecorderState::Rejected("That key cannot be used in a shortcut.".into());
            return None;
        };
        let display = display_hotkey(&spec).unwrap_or_else(|| spec.clone());
        self.state = RecorderState::Idle;
        Some(Captured {
            conflict: system_hotkey_conflict(&spec),
            spec,
            display,
        })
    }
}

/// The canonical name `keynames` knows for an egui key.
pub fn code_for(key: egui::Key) -> Option<String> {
    use egui::Key::*;
    let fixed = match key {
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        ArrowUp => "ArrowUp",
        Escape => "Escape",
        Tab => "Tab",
        Backspace => "Backspace",
        Enter => "Enter",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Space => "Space",
        Minus => "Minus",
        Equals => "Equal",
        Comma => "Comma",
        Period => "Period",
        Slash => "Slash",
        Semicolon => "Semicolon",
        Backslash => "Backslash",
        OpenBracket => "BracketLeft",
        CloseBracket => "BracketRight",
        Backtick => "Backquote",
        Quote => "Quote",
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(fixed.to_string());
    }
    // letters, digits and function keys are named by their label ("A", "5", "F3")
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().to_string()),
        (Some('F'), Some(d)) if d.is_ascii_digit() => Some(name.to_string()),
        _ => None,
    }
}

/// Key presses in this frame's input events.
pub fn presses(events: &[egui::Event]) -> Vec<Press> {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(Press {
                mods: Mods {
                    ctrl: modifiers.ctrl,
                    alt: modifiers.alt,
                    shift: modifiers.shift,
                    cmd: modifiers.mac_cmd,
                },
                code: code_for(*key)?,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(ctrl: bool, alt: bool, shift: bool, cmd: bool, code: &str) -> Press {
        Press {
            mods: Mods { ctrl, alt, shift, cmd },
            code: code.into(),
        }
    }

    #[test]
    fn labels_are_plain_words() {
        assert_eq!(label("alt+shift+space"), "Option+Shift+Space");
        assert_eq!(label("ctrl+cmd+KeyJ"), "Ctrl+Cmd+J");
        assert_eq!(label("bogus"), "bogus");
    }

    #[test]
    fn a_recorder_that_was_not_started_ignores_keys() {
        let mut r = Recorder::default();
        assert_eq!(r.feed(&press(false, true, true, false, "Space")), None);
        assert!(!r.is_listening());
    }

    #[test]
    fn option_shift_space_is_captured_in_spec_and_display_form() {
        let mut r = Recorder::default();
        r.start();
        let got = r.feed(&press(false, true, true, false, "Space")).unwrap();
        assert_eq!(got.spec, "alt+shift+Space");
        assert_eq!(got.display, "⌥⇧Space");
        assert_eq!(got.conflict, None);
        assert_eq!(r.state, RecorderState::Idle, "recording ends after a capture");
        assert!(got.spec.parse::<HotKey>().is_ok());
    }

    #[test]
    fn letters_digits_and_function_keys_work() {
        for (code, want) in [
            ("A", "cmd+KeyA"),
            ("5", "cmd+Digit5"),
            ("F5", "cmd+F5"),
            ("Enter", "cmd+Enter"),
        ] {
            let mut r = Recorder::default();
            r.start();
            let got = r.feed(&press(false, false, false, true, code)).unwrap();
            assert_eq!(got.spec, want, "{code}");
            assert!(got.spec.parse::<HotKey>().is_ok(), "{}", got.spec);
        }
    }

    #[test]
    fn modifier_order_is_canonical() {
        let mut r = Recorder::default();
        r.start();
        let got = r.feed(&press(true, true, true, true, "J")).unwrap();
        assert_eq!(got.spec, "ctrl+alt+shift+cmd+KeyJ");
        assert_eq!(got.display, "⌃⌥⇧⌘J");
    }

    #[test]
    fn a_bare_key_or_shift_alone_is_refused_and_recording_continues() {
        let mut r = Recorder::default();
        r.start();
        assert_eq!(r.feed(&press(false, false, false, false, "A")), None);
        assert!(matches!(r.state, RecorderState::Rejected(_)));
        assert!(r.is_listening());
        assert_eq!(r.feed(&press(false, false, true, false, "A")), None);
        assert!(r.is_listening());
        assert!(r.feed(&press(false, true, false, false, "A")).is_some());
    }

    #[test]
    fn escape_cancels_but_cmd_escape_is_a_shortcut() {
        let mut r = Recorder::default();
        r.start();
        assert_eq!(r.feed(&press(false, false, false, false, "Escape")), None);
        assert_eq!(r.state, RecorderState::Idle);
        r.start();
        assert_eq!(
            r.feed(&press(false, false, false, true, "Escape")).unwrap().spec,
            "cmd+Escape"
        );
    }

    #[test]
    fn a_bare_backspace_while_recording_answers_with_the_default_and_cmd_backspace_is_a_shortcut() {
        let mut r = Recorder::default();
        r.start();
        let got = r.feed(&press(false, false, false, false, "Backspace")).unwrap();
        assert_eq!(got.spec, ashdash_core::brand::DEFAULT_HOTKEY);
        assert_eq!(got.display, "⌥⇧Space");
        assert_eq!(r.state, RecorderState::Idle);
        r.start();
        assert_eq!(
            r.feed(&press(false, false, false, true, "Backspace")).unwrap().spec,
            "cmd+Backspace"
        );
        // not listening: nothing happens
        assert_eq!(r.feed(&press(false, false, false, false, "Backspace")), None);
    }

    #[test]
    fn known_system_shortcuts_are_flagged() {
        let mut r = Recorder::default();
        r.start();
        let got = r.feed(&press(false, false, false, true, "Space")).unwrap();
        assert_eq!(got.spec, "cmd+Space");
        assert_eq!(got.conflict, Some("Spotlight"));
    }

    #[test]
    fn unknown_keys_are_refused() {
        let mut r = Recorder::default();
        r.start();
        assert_eq!(r.feed(&press(false, true, false, false, "Mystery")), None);
        assert!(matches!(r.state, RecorderState::Rejected(_)));
    }

    #[test]
    fn egui_keys_map_to_names_the_spec_parser_knows() {
        use egui::Key;
        assert_eq!(code_for(Key::A).as_deref(), Some("A"));
        assert_eq!(code_for(Key::Num7).as_deref(), Some("7"));
        assert_eq!(code_for(Key::F12).as_deref(), Some("F12"));
        assert_eq!(code_for(Key::Space).as_deref(), Some("Space"));
        assert_eq!(code_for(Key::OpenBracket).as_deref(), Some("BracketLeft"));
        // every mapped key yields a spec the hotkey crate accepts
        for key in [
            Key::A,
            Key::Z,
            Key::Num0,
            Key::F1,
            Key::F20,
            Key::Enter,
            Key::Tab,
            Key::Space,
            Key::Minus,
            Key::Equals,
            Key::Comma,
            Key::Period,
            Key::Slash,
            Key::Semicolon,
            Key::Backslash,
            Key::OpenBracket,
            Key::CloseBracket,
            Key::Backtick,
            Key::Quote,
            Key::ArrowUp,
            Key::Home,
            Key::PageDown,
            Key::Delete,
            Key::Backspace,
        ] {
            let code = code_for(key).unwrap_or_else(|| panic!("{key:?} unmapped"));
            let spec = normalize_spec(&format!("alt+{code}")).unwrap_or_else(|| panic!("{key:?} -> {code} not a spec"));
            assert!(spec.parse::<HotKey>().is_ok(), "{key:?}: {spec}");
        }
    }

    #[test]
    fn presses_take_only_fresh_key_down_events() {
        let m = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        };
        let key = |pressed, repeat| egui::Event::Key {
            key: egui::Key::J,
            physical_key: None,
            pressed,
            repeat,
            modifiers: m,
        };
        let got = presses(&[
            key(true, false),
            key(true, true),
            key(false, false),
            egui::Event::Text("j".into()),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].code, "J");
        assert!(got[0].mods.alt && !got[0].mods.cmd);
    }
}
