//! Result items shown in the launcher and the actions attached to them.

use std::path::PathBuf;

use crate::system::SystemCommand;
use crate::workflow::WorkflowAction;

/// Keyboard modifiers held while activating a result.
///
/// The numeric flag values match `NSEventModifierFlags`, which is what the reference launcher
/// workflow connections store in their `modifiers` field (observed in the
/// bundled `.alfredworkflow` templates: `"modifiers": 0` for the plain action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Mods {
    pub cmd: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub fn_key: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        cmd: false,
        alt: false,
        ctrl: false,
        shift: false,
        fn_key: false,
    };

    pub fn flags(&self) -> u64 {
        let mut f = 0;
        if self.shift {
            f |= 1 << 17;
        }
        if self.ctrl {
            f |= 1 << 18;
        }
        if self.alt {
            f |= 1 << 19;
        }
        if self.cmd {
            f |= 1 << 20;
        }
        if self.fn_key {
            f |= 1 << 23;
        }
        f
    }

    /// Parse the `mods` keys used by Script Filter JSON: `cmd`, `alt`,
    /// `ctrl`, `shift`, `fn`, combined with `+` (e.g. `cmd+alt`).
    pub fn parse(s: &str) -> Option<Mods> {
        let mut m = Mods::NONE;
        for part in s.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "cmd" => m.cmd = true,
                "alt" => m.alt = true,
                "ctrl" => m.ctrl = true,
                "shift" => m.shift = true,
                "fn" => m.fn_key = true,
                _ => return None,
            }
        }
        Some(m)
    }

    pub fn is_none(&self) -> bool {
        *self == Mods::NONE
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Icon {
    None,
    /// An `.app` bundle whose icon should be extracted.
    App(PathBuf),
    /// A plain image file (png/jpg/...).
    Image(PathBuf),
    /// A short text glyph used when no real icon exists.
    Glyph(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Open a file, folder or application with its default handler.
    Open(PathBuf),
    /// Reveal in Finder.
    Reveal(PathBuf),
    OpenUrl(String),
    Copy(String),
    System(SystemCommand),
    /// Run a command line through `/bin/sh -c`.
    Shell(String),
    Workflow(Box<WorkflowAction>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable identifier used for usage learning.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Icon,
    pub action: Action,
    /// Text to put in the query field when Tab is pressed.
    pub autocomplete: Option<String>,
    pub valid: bool,
    pub score: f64,
    /// Text shown by Large Type (cmd+L) for this item.
    pub large_type: Option<String>,
    /// Text copied by cmd+C for this item.
    pub copy_text: Option<String>,
    /// Action used when Cmd is held (e.g. reveal instead of open).
    pub cmd_action: Option<Action>,
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Item {
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            icon: Icon::None,
            action: Action::None,
            autocomplete: None,
            valid: true,
            score: 0.0,
            large_type: None,
            copy_text: None,
            cmd_action: None,
        }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }

    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = i;
        self
    }

    pub fn action(mut self, a: Action) -> Self {
        self.action = a;
        self
    }

    pub fn autocomplete(mut self, s: impl Into<String>) -> Self {
        self.autocomplete = Some(s.into());
        self
    }

    pub fn valid(mut self, v: bool) -> Self {
        self.valid = v;
        self
    }

    pub fn score(mut self, s: f64) -> Self {
        self.score = s;
        self
    }
}
