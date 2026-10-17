//! Preferences.
//!
//! The window is a **separate process** (`ashdash prefs-window`, started by `ashdash prefs`, the
//! status-item menu or `ashdash://prefs`) with the normal "regular" activation policy: its own
//! Dock entry, menu bar and Cmd+Tab slot, and nothing it does can touch the launcher's focus,
//! level or auto-hide behaviour (see docs/spikes/ui-focus.md, "Preferences window").
//!
//! The two processes only talk through files in the data folder:
//!
//! * the window edits `config.json` (through [`crate::prefs_model::Store`], which writes only
//!   valid changes), installs/removes workflows and edits snippets;
//! * the launcher's [`reload::HotReload`] watches those files, applies them live and reports
//!   problems back in `run/reload-status.json` ([`status`]).
//!
//! Module map: [`hotkey`] (recorder + re-registration), [`reload`] (launcher side),
//! [`app`] (the window), [`general`], [`features`], [`workflows`], [`permissions`] (pages), [`single`],
//! [`dialogs`], [`status`]; the look: [`style`] (all tokens), [`fonts`], [`kit`] (shared
//! components), [`nav`] (sidebar entries), [`summary`] (status strip and cards).

pub mod app;
mod dialogs;
mod features;
mod fonts;
mod general;
pub mod hotkey;
mod kit;
mod nav;
mod permissions;
pub mod reload;
mod single;
pub mod status;
mod style;
mod summary;
mod widgets;
mod workflows;

use std::process::{Command, Stdio};

use ashdash_core::Paths;
use anyhow::{Context, Result};

/// Start the preferences window as its own process (or bring the open one forward). Returns at
/// once; used by the CLI, the status-item menu and the URL scheme.
pub fn open_window() -> Result<()> {
    use std::os::unix::process::CommandExt;
    Command::new(std::env::current_exe().context("cannot find the ashdash executable")?)
        .arg("prefs-window")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("could not start the preferences window")?;
    Ok(())
}

/// [`open_window`] for callers that have nobody to tell: failures go to stderr.
pub fn open_or_report() {
    if let Err(e) = open_window() {
        eprintln!("ashdash: {e:#}");
    }
}

/// Entry point of the window process: take the single-instance lock, then run the window.
pub fn run(paths: Paths) -> Result<()> {
    match single::acquire(&single::lock_file(&paths))? {
        single::Acquired::Taken(pid) => {
            if let Some(pid) = pid {
                single::activate(pid);
            }
            eprintln!("ashdash: the preferences window is already open");
            Ok(())
        }
        single::Acquired::First(_lock) => app::run(paths),
    }
}
