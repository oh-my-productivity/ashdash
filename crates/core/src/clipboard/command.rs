//! Commands that act on the clipboard history or the pasteboard, in one
//! serialisable-by-value enum so the engine needs a single `Action` variant
//! to expose all of them (see the integration notes in `clipboard/mod.rs`).

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use super::history::{ClipEntry, ClipboardHistory};
use super::paste::{self, KeySender, SystemKeys};
use super::restore;
use crate::engine::Outcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipCommand {
    /// Put an entry back on the pasteboard (text, image or files).
    Restore {
        id: String,
    },
    /// Restore an entry, then paste it into the frontmost app (Accessibility).
    Paste {
        id: String,
    },
    /// Copy `text` and paste it; `cursor_back` characters are skipped back
    /// afterwards (snippets with a `{cursor}` marker).
    PasteText {
        text: String,
        cursor_back: Option<usize>,
    },
    Pin {
        id: String,
        pinned: bool,
    },
    Delete {
        id: String,
    },
    /// `clip clear`: remove everything except pinned entries.
    Clear,
    /// Remove everything including pinned entries.
    ClearAll,
}

/// The OS-facing side of the commands, replaceable in tests.
pub trait ClipEnv {
    fn restore(&self, history: &ClipboardHistory, entry: &ClipEntry) -> Result<()>;
    fn copy_text(&self, text: &str) -> Result<()>;
    fn keys(&self) -> &dyn KeySender;
    fn settle(&self) -> Duration;
}

pub struct SystemEnv {
    pub settle: Duration,
}

impl Default for SystemEnv {
    fn default() -> Self {
        SystemEnv {
            settle: paste::DEFAULT_SETTLE,
        }
    }
}

impl ClipEnv for SystemEnv {
    fn restore(&self, history: &ClipboardHistory, entry: &ClipEntry) -> Result<()> {
        restore::restore(history, entry)
    }
    fn copy_text(&self, text: &str) -> Result<()> {
        restore::copy_text(text)
    }
    fn keys(&self) -> &dyn KeySender {
        &SystemKeys
    }
    fn settle(&self) -> Duration {
        self.settle
    }
}

/// Run a command against the real system.
pub fn run(history: &Mutex<ClipboardHistory>, cmd: &ClipCommand) -> Outcome {
    run_with(history, cmd, &SystemEnv::default())
}

pub fn run_with(history: &Mutex<ClipboardHistory>, cmd: &ClipCommand, env: &dyn ClipEnv) -> Outcome {
    let Ok(mut h) = history.lock() else {
        return Outcome::Error("clipboard history is unavailable".into());
    };
    let unknown = || Outcome::Error("that clipboard entry no longer exists".into());
    match cmd {
        ClipCommand::Restore { id } | ClipCommand::Paste { id } => {
            let Some(entry) = h.find(id).cloned() else {
                return unknown();
            };
            if let Err(e) = env.restore(&h, &entry) {
                return Outcome::Error(format!("{e:#}"));
            }
            if matches!(cmd, ClipCommand::Paste { .. }) {
                // Do not hold the history lock while sleeping/pasting.
                drop(h);
                return paste::outcome(paste::paste_via(env.keys(), None, env.settle()));
            }
            Outcome::Done
        }
        ClipCommand::PasteText { text, cursor_back } => {
            drop(h);
            if let Err(e) = env.copy_text(text) {
                return Outcome::Error(format!("{e:#}"));
            }
            paste::outcome(paste::paste_via(env.keys(), *cursor_back, env.settle()))
        }
        ClipCommand::Pin { id, pinned } => {
            if h.set_pinned(id, *pinned) {
                Outcome::Done
            } else {
                unknown()
            }
        }
        ClipCommand::Delete { id } => {
            if h.remove(id) {
                Outcome::Done
            } else {
                unknown()
            }
        }
        ClipCommand::Clear => {
            h.clear();
            Outcome::Done
        }
        ClipCommand::ClearAll => {
            h.clear_all();
            Outcome::Done
        }
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
        fail_restore: bool,
    }

    impl KeySender for Fake {
        fn trusted(&self) -> bool {
            self.trusted
        }
        fn tap(&self, keycode: u16, command: bool) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("key{keycode}{}", if command { "+cmd" } else { "" }));
            Ok(())
        }
        fn pause(&self, _: Duration) {}
    }

    impl ClipEnv for Fake {
        fn restore(&self, _: &ClipboardHistory, e: &ClipEntry) -> Result<()> {
            if self.fail_restore {
                anyhow::bail!("file gone");
            }
            self.log.borrow_mut().push(format!("restore:{:?}", e.kind));
            Ok(())
        }
        fn copy_text(&self, t: &str) -> Result<()> {
            self.log.borrow_mut().push(format!("copy:{t}"));
            Ok(())
        }
        fn keys(&self) -> &dyn KeySender {
            self
        }
        fn settle(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn setup() -> (Mutex<ClipboardHistory>, String) {
        let mut h = ClipboardHistory::in_memory(10);
        h.push("hello");
        let id = h.entries()[0].id();
        (Mutex::new(h), id)
    }

    #[test]
    fn restore_and_paste_sequences() {
        let (h, id) = setup();
        let f = Fake {
            trusted: true,
            ..Default::default()
        };
        assert_eq!(
            run_with(&h, &ClipCommand::Restore { id: id.clone() }, &f),
            Outcome::Done
        );
        assert_eq!(*f.log.borrow(), vec!["restore:Text"]);
        f.log.borrow_mut().clear();
        assert_eq!(run_with(&h, &ClipCommand::Paste { id }, &f), Outcome::Done);
        assert_eq!(*f.log.borrow(), vec!["restore:Text", "key9+cmd"]);
    }

    #[test]
    fn paste_without_accessibility_keeps_the_copy_and_explains() {
        let (h, id) = setup();
        let f = Fake::default();
        let Outcome::Error(m) = run_with(&h, &ClipCommand::Paste { id }, &f) else {
            panic!("expected an error")
        };
        assert!(m.contains("Accessibility"), "{m}");
        assert_eq!(*f.log.borrow(), vec!["restore:Text"], "restored but no keys sent");
    }

    #[test]
    fn paste_text_with_cursor() {
        let (h, _) = setup();
        let f = Fake {
            trusted: true,
            ..Default::default()
        };
        let cmd = ClipCommand::PasteText {
            text: "Dear ,".into(),
            cursor_back: Some(1),
        };
        assert_eq!(run_with(&h, &cmd, &f), Outcome::Done);
        assert_eq!(*f.log.borrow(), vec!["copy:Dear ,", "key9+cmd", "key123"]);
    }

    #[test]
    fn restore_failure_and_unknown_ids_are_errors() {
        let (h, id) = setup();
        let f = Fake {
            fail_restore: true,
            ..Default::default()
        };
        assert_eq!(
            run_with(&h, &ClipCommand::Restore { id }, &f),
            Outcome::Error("file gone".into())
        );
        for cmd in [
            ClipCommand::Restore { id: "x".into() },
            ClipCommand::Paste { id: "x".into() },
            ClipCommand::Pin {
                id: "x".into(),
                pinned: true,
            },
            ClipCommand::Delete { id: "x".into() },
        ] {
            assert!(matches!(run_with(&h, &cmd, &f), Outcome::Error(_)), "{cmd:?}");
        }
    }

    #[test]
    fn pin_delete_clear() {
        let (h, id) = setup();
        h.lock().unwrap().push("second");
        let f = Fake::default();
        assert_eq!(
            run_with(
                &h,
                &ClipCommand::Pin {
                    id: id.clone(),
                    pinned: true
                },
                &f
            ),
            Outcome::Done
        );
        assert_eq!(run_with(&h, &ClipCommand::Clear, &f), Outcome::Done);
        assert_eq!(h.lock().unwrap().len(), 1, "pinned survives clear");
        assert_eq!(run_with(&h, &ClipCommand::Delete { id }, &f), Outcome::Done);
        assert!(h.lock().unwrap().is_empty());
        h.lock().unwrap().push("again");
        assert_eq!(run_with(&h, &ClipCommand::ClearAll, &f), Outcome::Done);
        assert!(h.lock().unwrap().is_empty());
    }
}
