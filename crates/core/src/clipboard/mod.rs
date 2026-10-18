//! Clipboard history, pasteboard watching, paste, and snippets.
//!
//! * [`filter`] - pure `should_record(types, frontmost_app, ignore_list)`.
//! * [`history`] - the store: text/image/file entries, dedupe, retention, pins.
//! * [`watch`] - change detection on a [`watch::PasteboardSource`] (the app
//!   crate implements it with `NSPasteboard`).
//! * [`restore`], [`paste`], [`command`] - put entries back, auto-paste (Cmd+V
//!   via CGEvent, Accessibility), and the [`ClipCommand`] enum tying them together.
//! * [`snippets`], [`placeholders`] - collections, snippet package import and
//!   dynamic placeholders.
//!
//! # Integration notes for the engine owner
//! Everything below works today through plain `Action::Copy` / `Action::Open`
//! fallbacks, but three features need an `Action` the clipboard module cannot
//! add on its own (`item.rs` / `engine.rs` are not owned by this module):
//!
//! ```text
//! // item.rs:   Action::Clipboard(crate::clipboard::ClipCommand),
//! // engine.rs, in `perform`:
//! //   Action::Clipboard(c) => return match crate::clipboard::run_command(&self.clipboard, c) {
//! //       Outcome::Error(m) => Ok(Some(format!("{TAG_ERR}{m}"))),
//! //       _ => Ok(None),
//! //   },
//! // providers/clipboard.rs: `clip_action()` returns Action::Clipboard(cmd) instead of the fallback.
//! ```
//! Once wired, image/file entries restore on Enter, `clip clear` appears, and
//! snippets can paste with cursor placement. Settings the engine should feed
//! into the history after `ClipboardHistory::load`: `set_retention(max,
//! max_age)`, `set_ignored_apps(list)` and, optionally, `set_max_image_bytes`.

pub mod command;
pub mod filter;
pub mod history;
pub mod paste;
pub mod placeholders;
pub mod restore;
pub mod snippets;
pub mod watch;

pub use command::{ClipCommand, run as run_command};
pub use filter::{DEFAULT_IGNORED_APPS, SkipReason, should_record};
pub use history::{ClipEntry, ClipKind, ClipboardHistory, ImageRef};
pub use placeholders::{ExpandCtx, Expanded, expand};
pub use snippets::{ImportReport, Snippet, import_snippet_package, load_snippets};

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Expand snippet placeholders with `clipboard` as the only clipboard entry.
/// Kept for callers of the original API; prefer [`expand`].
pub fn expand_snippet(text: &str, clipboard: &str) -> String {
    let clips = [clipboard.to_string()];
    expand(text, &ExpandCtx::system(&clips)).text
}
