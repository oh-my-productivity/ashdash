//! State and plumbing for the launcher's secondary features (actions panel,
//! buffer, import prompt, theme hot-apply), kept out of `LauncherApp` itself so
//! the shared `gui/mod.rs` stays small.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use ashdash_core::actions::Buffer;
use ashdash_core::workflow::StagedPackage;
use ashdash_core::{Item, Outcome};
use eframe::egui;

use super::importprompt::ImportPrompt;
use super::panel::{PanelStack, UiAction, is_workflow_package, ui_action};
use super::schedule::SelectionTrack;
use super::{LauncherApp, mac, trace};

/// Results of work done off the UI thread.
pub(super) enum AsyncMsg {
    /// Applications for an "Open With..." level.
    List { token: u64, entries: Vec<Item> },
    /// A workflow package was opened for inspection (or could not be). The staged copy is
    /// what the prompt asks about and what gets installed.
    Preview(Result<Box<StagedPackage>, String>),
    /// An installation finished (name or error).
    Installed(Result<String, String>),
}

pub(super) struct Glue {
    pub panels: PanelStack,
    pub buffer: Buffer,
    pub import: Option<ImportPrompt>,
    /// Select this result when the next answer arrives (after going back from a panel).
    pub keep_selection: Option<String>,
    /// An "Open With..." list is still being fetched.
    pub loading: bool,
    pub token: u64,
    pub tx: Sender<AsyncMsg>,
    pub rx: Receiver<AsyncMsg>,
    /// Last value of `opendocs::theme_epoch()` the window applied.
    pub theme_epoch: u64,
    /// Points the Large Type text is to be scrolled by on the next frame.
    pub large_scroll: f32,
    /// The query was replaced by code: put the text cursor at its end before the next draw.
    pub cursor_to_end: bool,
    /// Moves of the selection and replaced lists, for "the highlighted result stays" and
    /// "Return runs what you saw".
    pub track: SelectionTrack,
    /// The start-up crash-report check has run (once, at the first showing).
    pub crash_checked: bool,
}

impl Glue {
    pub fn new() -> Glue {
        let (tx, rx) = channel();
        Glue {
            panels: PanelStack::default(),
            buffer: Buffer::new(),
            import: None,
            keep_selection: None,
            loading: false,
            token: 0,
            tx,
            rx,
            theme_epoch: crate::mac::opendocs::theme_epoch(),
            large_scroll: 0.0,
            cursor_to_end: false,
            track: SelectionTrack::default(),
            crash_checked: false,
        }
    }

    /// Forget everything tied to one showing of the launcher.
    pub fn reset(&mut self) {
        self.panels.clear();
        self.buffer.clear();
        self.import = None;
        self.keep_selection = None;
        self.loading = false;
        self.token += 1; // late answers for the old showing are ignored
        self.large_scroll = 0.0;
        self.track.reset();
    }
}

impl LauncherApp {
    /// What the actions panel / Quick Look / import prompt run themselves.
    pub(super) fn run_ui_action(&mut self, ctx: &egui::Context, action: UiAction) {
        match action {
            UiAction::OpenWith(p) => self.open_with(ctx, p),
            UiAction::QuickLook(ps) => {
                let id = self.items.get(self.selected).map(|i| i.id.clone());
                self.open_quicklook(&ps, id);
            }
            UiAction::InstallWorkflow(p) => self.begin_import(ctx, p),
        }
    }

    /// Return on a row: `true` when the launcher handled it (nothing to execute).
    pub(super) fn handle_ui_row(&mut self, ctx: &egui::Context, index: usize) -> bool {
        match self.items.get(index).and_then(ui_action) {
            Some(a) => {
                self.selected = index;
                self.run_ui_action(ctx, a);
                true
            }
            None => false,
        }
    }

    /// Start a Quick Look through the engine's process handle.
    pub(super) fn open_quicklook(&mut self, paths: &[PathBuf], for_id: Option<String>) {
        trace::trace(|| format!("quicklook {paths:?}"));
        self.quicklook_for = for_id;
        let engine = std::sync::Arc::clone(&self.engine);
        let paths = paths.to_vec();
        self.quicklook.open_with(move || match engine.spawn_quick_look(&paths) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("ashdash: quick look: {e:#}");
                None
            }
        });
        self.raise_launcher(false);
    }

    /// Put the launcher above (`true`) or below (`false`) other floating windows.
    /// A Quick Look preview must be able to cover it.
    pub(super) fn raise_launcher(&self, raised: bool) {
        mac::set_launcher_raised(super::WINDOW_TITLE, raised);
    }

    // ------------------------------------------------------------ workflow import

    /// Open a package for inspection and, when it is readable, ask. Nothing is
    /// installed until the user confirms.
    pub(super) fn begin_import(&mut self, ctx: &egui::Context, path: PathBuf) {
        if !is_workflow_package(&path) {
            self.status = Some("Only .alfredworkflow files can be installed here".into());
            return;
        }
        self.status = Some("Reading the workflow…".into());
        let (engine, tx, ctx) = (std::sync::Arc::clone(&self.engine), self.glue.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = engine.stage_workflow(&path).map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = tx.send(AsyncMsg::Preview(r));
            ctx.request_repaint();
        });
    }

    /// Return on the prompt (the caller has checked `importprompt::prompt_key`: a fresh press,
    /// after the delay). Installs the staged copy that was shown, not the file again.
    pub(super) fn confirm_import(&mut self, ctx: &egui::Context) {
        let Some(p) = self.glue.import.take() else {
            return;
        };
        self.status = Some("Installing…".into());
        let (engine, tx, ctx) = (std::sync::Arc::clone(&self.engine), self.glue.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = super::importprompt::install_confirmed(&engine, p);
            let _ = tx.send(AsyncMsg::Installed(r));
            ctx.request_repaint();
        });
    }

    pub(super) fn cancel_import(&mut self) {
        self.glue.import = None;
        self.status = None;
    }

    /// Dropped `.alfredworkflow` files start the same confirmation.
    pub(super) fn poll_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        let Some(first) = dropped.iter().find(|p| is_workflow_package(p)).cloned() else {
            if !dropped.is_empty() {
                self.status = Some("Only .alfredworkflow files can be installed here".into());
            }
            return;
        };
        if dropped.len() > 1 {
            self.status = Some("Installing one workflow at a time: the first dropped file".into());
        }
        self.begin_import(ctx, first);
    }

    pub(super) fn apply_async(&mut self, ctx: &egui::Context, msg: AsyncMsg) {
        match msg {
            list @ AsyncMsg::List { .. } => self.panel_apply(&list),
            AsyncMsg::Preview(Ok(staged)) => {
                self.status = None;
                self.glue.import = Some(ImportPrompt::new(*staged));
                self.want_focus = true;
            }
            AsyncMsg::Preview(Err(e)) => self.status = Some(format!("Cannot install: {e}")),
            AsyncMsg::Installed(Ok(name)) => {
                self.status = Some(format!("Installed {name}"));
                // new workflows may bring hotkeys
                self.automation.reload_hotkeys(self._hotkey_manager.as_ref());
                self.engine.reload();
                ctx.request_repaint();
            }
            AsyncMsg::Installed(Err(e)) => self.status = Some(format!("Install failed: {e}")),
        }
    }

    // ------------------------------------------------------------ misc per-frame

    /// A theme installed from Finder replaces the running one at once.
    pub(super) fn poll_theme_epoch(&mut self) {
        let now = crate::mac::opendocs::theme_epoch();
        if now == self.glue.theme_epoch {
            return;
        }
        self.glue.theme_epoch = now;
        let config = ashdash_core::Config::load(&self.engine.paths);
        if let Some(file) = config.theme {
            match ashdash_core::theme::Theme::load(&file) {
                Ok(t) => {
                    self.theme = t;
                    if self.visible {
                        mac::style_launcher_window(super::WINDOW_TITLE, &super::style::window_style(&self.theme));
                        self.last_height = 0.0;
                    }
                }
                Err(e) => eprintln!("ashdash: theme {}: {e:#}", file.display()),
            }
        }
    }

    /// Show the launcher with `query` typed in, like the hotkey followed by typing.
    pub(super) fn show_with_query(&mut self, ctx: &egui::Context, query: String) {
        self.show(ctx);
        self.query = query;
        self.glue.cursor_to_end = true;
        self.request_query();
    }

    /// Show the launcher with `text` in the field and **no query running**: the text came from a
    /// URL, so no workflow keyword or Script Filter may see it before the user has touched the
    /// field (an edit sets `resp.changed()`, which sends the query as usual).
    pub(super) fn show_with_prefill(&mut self, ctx: &egui::Context, text: String) {
        self.show(ctx);
        self.query = text;
        self.glue.cursor_to_end = true;
        self.status = Some("Text from a link: edit it to search".into());
    }

    /// Move the query field's text cursor to the end of the (programmatically replaced) text.
    pub(super) fn apply_cursor_flag(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.glue.cursor_to_end) {
            return;
        }
        if let Some(mut state) = egui::TextEdit::load_state(ctx, super::query_id()) {
            let end = egui::text::CCursor::new(self.query.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ctx, super::query_id());
        }
    }

    /// An outcome that arrived without the window (hotkey, URL, workflow).
    pub(super) fn present_outcome(&mut self, ctx: &egui::Context, outcome: Outcome) {
        match outcome {
            Outcome::Done => {}
            Outcome::LargeType(t) => {
                self.show(ctx);
                self.large_type = Some(t);
            }
            Outcome::ShowLauncher(q) => self.show_with_query(ctx, q),
            Outcome::Confirm(m) | Outcome::Error(m) => {
                self.show(ctx);
                self.status = Some(m);
            }
        }
    }
}
