//! The preferences window (its own process, see the module docs of `prefs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ashdash_core::workflow::StagedPackage;
use ashdash_core::{Config, Paths};
use anyhow::{Result, anyhow};
use eframe::egui::{self, Frame, Key, Modifiers};

use super::kit::{self, Tone};
use super::nav::{self, Nav};
use super::status::{self, LauncherStatus};
use super::style::{self, Palette, m};
use super::{features, fonts, general, summary, workflows};
use crate::mac::permissions::{Pane, Probes, Snapshot, SystemProbes};
use crate::prefs_model::{EditResult, Store};

pub const WINDOW_TITLE: &str = "Ashdash Preferences";
const POLL_EVERY: Duration = Duration::from_millis(900);

/// Which file dialog was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Theme,
    Workflow,
    Snippets,
}

/// What a file dropped on the window is for, by extension (case-insensitive). Workflow packages
/// use the extension list shared with the launcher and Finder opens.
pub fn drop_target(path: &std::path::Path) -> Option<Purpose> {
    if ashdash_core::workflow::is_workflow_package(path) {
        return Some(Purpose::Workflow);
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "alfredappearance" => Some(Purpose::Theme),
        "alfredsnippets" => Some(Purpose::Snippets),
        _ => None,
    }
}

/// Results of work done off the UI thread.
pub enum Task {
    Picked(Purpose, Option<PathBuf>),
    Preview(Box<Result<StagedPackage, String>>),
    /// A round of permission probes finished.
    Permissions(Box<Snapshot>),
}

pub struct PrefsApp {
    pub(super) paths: Paths,
    pub(super) store: Store,
    /// The settings on disk as last read.
    pub(super) cfg: Config,
    /// Set while `config.json` cannot be read; editing is refused then.
    pub(super) cfg_error: Option<String>,
    nav: Nav,
    /// Text of the "Go to a setting…" field (filters the sidebar).
    nav_query: String,
    launcher: Option<LauncherStatus>,
    last_poll: Instant,
    pub(super) general: general::State,
    pub(super) features: features::State,
    pub(super) workflows: workflows::State,
    pub(super) tx: Sender<Task>,
    rx: Receiver<Task>,
    pub(super) ctx: egui::Context,
    focused_once: bool,
    /// Was the window the key window in the previous frame? (Coming back from System Settings re-probes.)
    was_focused: bool,
    probes: Arc<dyn Probes>,
    /// The last permission probe, `None` until the first one finishes.
    pub(super) perms: Option<Snapshot>,
    pub(super) perms_checking: bool,
    /// Seconds since the epoch of the last finished probe (0 = none yet).
    pub(super) perms_at: u64,
    /// (content height, visible height) of the page's scroll area in the last frame.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) last_scroll: (f32, f32),
}

pub fn run(paths: Paths) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size(m::WINDOW)
        .with_min_inner_size(m::WINDOW_MIN)
        .with_drag_and_drop(true)
        // our own title bar: content under a transparent native one, title text hidden,
        // traffic lights kept (the drawn bar is 28 pt high and leaves them alone)
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false);
    let options = eframe::NativeOptions {
        viewport,
        event_loop_builder: Some(Box::new(|builder| {
            // An ordinary app while it is open: Dock icon, Cmd+Tab, menu bar.
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Regular);
        })),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            Ok(Box::new(PrefsApp::new(
                cc.egui_ctx.clone(),
                paths,
                Arc::new(SystemProbes),
            )))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

impl PrefsApp {
    fn new(ctx: egui::Context, paths: Paths, probes: Arc<dyn Probes>) -> PrefsApp {
        fonts::install(&ctx);
        style::install(&ctx);
        let store = Store::new(paths.clone());
        let (cfg, cfg_error) = match store.load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        let (tx, rx) = channel();
        let mut app = PrefsApp {
            paths,
            store,
            cfg,
            cfg_error,
            nav: Nav::General,
            nav_query: String::new(),
            launcher: None,
            last_poll: Instant::now() - POLL_EVERY,
            general: general::State::default(),
            features: features::State::default(),
            workflows: workflows::State::default(),
            tx,
            rx,
            ctx,
            focused_once: false,
            was_focused: false,
            probes,
            perms: None,
            perms_checking: false,
            perms_at: 0,
            last_scroll: (0.0, 0.0),
        };
        app.check_permissions();
        app.enter_page();
        app
    }

    /// Apply one edit to `config.json`; the window's copy follows on success.
    pub(super) fn edit(&mut self, f: impl FnOnce(&mut Config) -> EditResult) -> EditResult {
        if let Some(e) = &self.cfg_error {
            return Err(format!("{e}. Nothing was changed."));
        }
        let c = self.store.edit(f)?;
        self.cfg = c;
        Ok(())
    }

    /// Reload what a page shows from disk (on page switch and when files changed).
    fn enter_page(&mut self) {
        use crate::prefs_model::Page;
        match self.nav.page() {
            Page::General => self.general.refresh(&self.paths),
            Page::Features => self.features.refresh(&self.paths, &self.cfg),
            Page::Workflows => self.workflows.refresh(&self.paths, &self.cfg),
            Page::Permissions => self.check_permissions(),
            Page::About => {}
        }
    }

    /// Probe the permissions in a thread (silently, see `mac::permissions`); the answer arrives as a [`Task`].
    pub(super) fn check_permissions(&mut self) {
        if self.perms_checking {
            return;
        }
        self.perms_checking = true;
        let (probes, tx, ctx) = (self.probes.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let snap = crate::mac::permissions::check(&*probes);
            let _ = tx.send(Task::Permissions(Box::new(snap)));
            ctx.request_repaint();
        });
    }

    pub(super) fn open_pane(&self, pane: Pane) {
        if let Err(e) = pane.open() {
            eprintln!("ashdash: could not open System Settings: {e}");
        }
    }

    /// Safari bookmarks are on and macOS refuses Full Disk Access.
    pub(super) fn disk_access_missing(&self) -> bool {
        summary::disk_access_missing(&self.cfg, self.perms.as_ref())
    }

    fn poll_disk(&mut self) {
        if self.last_poll.elapsed() < POLL_EVERY {
            return;
        }
        self.last_poll = Instant::now();
        match self.store.load() {
            Ok(c) => {
                self.cfg_error = None;
                if serde_json::to_value(&c).ok() != serde_json::to_value(&self.cfg).ok() {
                    self.cfg = c;
                    self.features.config_changed();
                }
            }
            Err(e) => self.cfg_error = Some(e),
        }
        self.launcher = status::read(&self.paths);
    }

    fn poll_tasks(&mut self) {
        while let Ok(task) = self.rx.try_recv() {
            match task {
                Task::Picked(purpose, Some(path)) => match purpose {
                    Purpose::Theme => self.import_theme(&path),
                    Purpose::Workflow => self.start_preview(path),
                    Purpose::Snippets => self.import_snippets(&path),
                },
                Task::Picked(_, None) => {}
                Task::Preview(r) => self.workflows.preview_done(*r),
                Task::Permissions(snap) => {
                    self.perms = Some(*snap);
                    self.perms_checking = false;
                    self.perms_at = status::now_secs();
                }
            }
        }
    }

    /// Files dropped on the window go to the page they belong to.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let path = f.path().to_path_buf();
            match drop_target(&path) {
                Some(Purpose::Workflow) => {
                    self.go(Nav::Workflows);
                    self.start_preview(path);
                }
                Some(Purpose::Theme) => {
                    self.go(Nav::General);
                    self.import_theme(&path);
                }
                Some(Purpose::Snippets) => {
                    self.go(Nav::Snippets);
                    self.import_snippets(&path);
                }
                None => {}
            }
        }
    }

    /// Ask for a file in a thread; the answer arrives as a [`Task`].
    pub(super) fn choose_file(&self, purpose: Purpose, prompt: &'static str, extensions: &'static [&'static str]) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let path = super::dialogs::choose_file(prompt, extensions);
            let _ = tx.send(Task::Picked(purpose, path));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for PrefsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl PrefsApp {
    pub(super) fn launcher_status(&self) -> Option<&LauncherStatus> {
        self.launcher.as_ref()
    }

    /// Show a sidebar entry (reloading what its page needs).
    pub(super) fn go(&mut self, nav: Nav) {
        if self.nav != nav {
            self.nav = nav;
            self.enter_page();
        }
    }

    /// Cmd+1..7 move between pages, Cmd+K focuses the "Go to a setting…" field, Cmd+R re-checks
    /// the permissions, Return (with nothing focused) follows the "Fix…" button of the General
    /// page. Nothing of this runs while the shortcut recorder is listening: the combination
    /// being recorded must not also switch pages.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.general.recorder.is_listening() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in &events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = ev
            {
                if let Some(n) = Nav::from_shortcut(*modifiers, *key) {
                    self.go(n);
                } else if *key == Key::K && modifiers.matches_exact(Modifiers::COMMAND) {
                    ctx.memory_mut(|mem| mem.request_focus(search_id()));
                } else if *key == Key::R && modifiers.matches_exact(Modifiers::COMMAND) {
                    self.check_permissions();
                } else if *key == Key::Enter
                    && modifiers.is_none()
                    && self.nav == Nav::General
                    && self.disk_access_missing()
                    && ctx.memory(|mem| mem.focused().is_none())
                {
                    self.go(Nav::Permissions);
                }
            }
        }
    }

    /// One frame of the window.
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.focused_once {
            self.focused_once = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.poll_disk();
        self.poll_tasks();
        self.handle_drops(&ctx);
        self.handle_shortcuts(&ctx);
        ctx.request_repaint_after(POLL_EVERY);
        let p = Palette::of(ui);
        let alive = self.launcher.as_ref().is_some_and(|l| status::process_alive(l.pid));
        // back from System Settings (or any other app): the grants may have changed
        let focused = ctx.input(|i| i.viewport().focused == Some(true));
        if focused && !self.was_focused && self.perms_at > 0 {
            self.check_permissions();
        }
        self.was_focused = focused;
        let disk_missing = self.disk_access_missing();

        egui::Panel::top("prefs-titlebar")
            .exact_size(m::TITLEBAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.titlebar))
            .show(ui, |ui| kit::title_bar(ui, "Ashdash"));

        egui::Panel::bottom("prefs-status")
            .exact_size(m::STATUS_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.status_bg))
            .show(ui, |ui| {
                let y = ui.max_rect().min.y;
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    y + 0.5,
                    egui::Stroke::new(m::STROKE, p.hairline),
                );
                kit::status_strip(
                    ui,
                    &summary::strip_items(self.cfg_error.as_deref(), self.launcher.as_ref(), alive, disk_missing),
                );
            });

        egui::Panel::left("prefs-sidebar")
            .exact_size(m::SIDEBAR_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.sidebar))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg))
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(style::margin(m::PAGE_PAD_X, 0.0))
                    .show(ui, |ui| {
                        let right =
                            matches!(self.nav, Nav::General | Nav::Search | Nav::Clipboard).then_some("config.json");
                        kit::page_header(ui, self.nav.title(), self.nav.subtitle(), right);
                        let line_y = ui.cursor().min.y;
                        let full = ui.clip_rect().x_range();
                        ui.painter()
                            .hline(full, line_y, egui::Stroke::new(m::STROKE, p.hairline));
                        if self.nav != Nav::General {
                            // a damaged settings file must be seen on every page
                            for a in summary::attention(self.cfg_error.as_deref(), None, false, false)
                                .into_iter()
                                .filter(|a| a.tone == Tone::Error)
                            {
                                ui.add_space(m::GAP);
                                kit::info_card(ui, a.title, a.tone, &a.lines, |_| {});
                            }
                        }
                        let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.add_space(m::GAP);
                            match self.nav {
                                Nav::General => self.general_page(ui, alive),
                                Nav::Search | Nav::Clipboard | Nav::Snippets => {
                                    if let Some(part) = self.nav.features_part() {
                                        self.features_page(ui, part);
                                    }
                                }
                                Nav::Workflows => self.workflows_page(ui),
                                Nav::Permissions => self.permissions_page(ui),
                                Nav::About => self.about_page(ui),
                            }
                            ui.add_space(m::GAP);
                        });
                        self.last_scroll = (scroll.content_size.y, scroll.inner_rect.height());
                    });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        let rect = ui.max_rect();
        ui.painter().vline(
            rect.max.x - 0.5,
            rect.y_range(),
            egui::Stroke::new(m::STROKE, p.hairline),
        );
        ui.add_space(m::GAP_L);
        let id = search_id();
        Frame::new().inner_margin(style::margin(12.0, 0.0)).show(ui, |ui| {
            let resp = kit::search_field(ui, &mut self.nav_query, id);
            // Return and Esc make a single-line edit lose focus in the same frame
            if resp.has_focus() || resp.lost_focus() {
                let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if enter && resp.lost_focus() {
                    if let Some(first) = nav::filter(&self.nav_query).first().copied() {
                        self.nav_query.clear();
                        self.go(first);
                    }
                } else if esc {
                    self.nav_query.clear();
                    resp.surrender_focus();
                }
            }
        });
        ui.add_space(m::GAP_L);
        ui.spacing_mut().item_spacing.y = 0.0;
        let shown = nav::filter(&self.nav_query);
        for n in &shown {
            if kit::nav_item(ui, *n, *n == self.nav).clicked() {
                self.go(*n);
            }
        }
        if shown.is_empty() {
            ui.add_space(m::GAP);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(style::hint_text(&p, "No page with that name"));
            });
        }
    }
}

fn search_id() -> egui::Id {
    egui::Id::new("prefs-go-to-a-setting")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::permissions::Grant;
    use crate::mac::permissions::fake::Fake;

    fn test_app(ctx: &egui::Context, paths: Paths) -> PrefsApp {
        app_with(ctx, paths, Arc::new(Fake::all(Grant::Granted)))
    }

    fn app_with(ctx: &egui::Context, paths: Paths, fake: Arc<Fake>) -> PrefsApp {
        PrefsApp::new(ctx.clone(), paths, fake)
    }

    /// Run frames until the permission probe thread has answered.
    fn wait_for_probe(ctx: &egui::Context, app: &mut PrefsApp) {
        for _ in 0..200 {
            frame(ctx, app);
            if app.perms.is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the permission probe never answered");
    }

    fn temp(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-prefs-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (Paths::under(&root), root)
    }

    fn frame(ctx: &egui::Context, app: &mut PrefsApp) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(m::WINDOW[0], m::WINDOW[1]),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| app.draw(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn every_page_draws_with_and_without_data() {
        let (paths, root) = temp("pages");
        std::fs::create_dir_all(paths.workflows_dir().join("user.workflow.x")).unwrap();
        std::fs::write(
            paths.workflows_dir().join("user.workflow.x/info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
            <key>name</key><string>X</string><key>bundleid</key><string>x.b</string>
            <key>objects</key><array/><key>connections</key><dict/></dict></plist>"#,
        )
        .unwrap();
        std::fs::write(paths.snippets_file(), r#"[{"name":"Sig","keyword":"sig","text":"hi"}]"#).unwrap();
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        for nav in Nav::ALL {
            app.nav = nav;
            app.enter_page();
            for _ in 0..3 {
                frame(&ctx, &mut app);
            }
        }
        assert_eq!(app.workflows.list_len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    fn key_event(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn frame_with(ctx: &egui::Context, app: &mut PrefsApp, events: Vec<egui::Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(m::WINDOW[0], m::WINDOW[1]),
            )),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| app.draw(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn cmd_digits_move_between_pages_and_cmd_k_focuses_the_search_field() {
        let (paths, root) = temp("keys");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        frame(&ctx, &mut app);
        assert_eq!(app.nav, Nav::General);
        frame_with(&ctx, &mut app, vec![key_event(Key::Num5, Modifiers::COMMAND)]);
        assert_eq!(app.nav, Nav::Workflows);
        frame_with(&ctx, &mut app, vec![key_event(Key::Num2, Modifiers::COMMAND)]);
        assert_eq!(app.nav, Nav::Search);
        // a bare digit or Cmd+Shift+digit does nothing
        frame_with(&ctx, &mut app, vec![key_event(Key::Num3, Modifiers::NONE)]);
        frame_with(
            &ctx,
            &mut app,
            vec![key_event(Key::Num3, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        assert_eq!(app.nav, Nav::Search);
        frame_with(&ctx, &mut app, vec![key_event(Key::K, Modifiers::COMMAND)]);
        frame(&ctx, &mut app);
        assert!(ctx.memory(|mem| mem.has_focus(search_id())));
        frame_with(&ctx, &mut app, vec![key_event(Key::Escape, Modifiers::NONE)]);
        frame_with(&ctx, &mut app, vec![key_event(Key::Num6, Modifiers::COMMAND)]);
        assert_eq!(app.nav, Nav::Permissions);
        frame_with(&ctx, &mut app, vec![key_event(Key::Num7, Modifiers::COMMAND)]);
        assert_eq!(app.nav, Nav::About);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_search_field_filters_the_sidebar_and_return_opens_the_first_match() {
        let (paths, root) = temp("filter");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        frame(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![key_event(Key::K, Modifiers::COMMAND)]);
        frame(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![egui::Event::Text("snip".into())]);
        assert_eq!(app.nav_query, "snip");
        assert_eq!(nav::filter(&app.nav_query), vec![Nav::Snippets]);
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.nav, Nav::Snippets);
        assert!(app.nav_query.is_empty(), "the field is cleared after opening");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_config_is_shown_and_never_rewritten() {
        let (paths, root) = temp("broken");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), b"{ oops").unwrap();
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths.clone());
        assert!(app.cfg_error.is_some());
        assert!(app.edit(|c| crate::prefs_model::set_max_results(c, 10)).is_err());
        for nav in Nav::ALL {
            app.nav = nav;
            app.enter_page();
            frame(&ctx, &mut app);
        }
        assert_eq!(std::fs::read(paths.config_file()).unwrap(), b"{ oops");
        let _ = std::fs::remove_dir_all(root);
    }

    fn viewport_commands(
        ctx: &egui::Context,
        app: &mut PrefsApp,
        events: Vec<egui::Event>,
    ) -> Vec<egui::ViewportCommand> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(m::WINDOW[0], m::WINDOW[1]),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| app.draw(ui));
        out.viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        let b = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), b(true), b(false)]
    }

    fn focus_recorder(ctx: &egui::Context, app: &mut PrefsApp) {
        frame(ctx, app);
        ctx.memory_mut(|m| m.request_focus(general::recorder_id()));
        frame(ctx, app);
    }

    #[test]
    fn return_on_the_focused_recorder_starts_recording_without_being_recorded() {
        let (paths, root) = temp("rec-return");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        focus_recorder(&ctx, &mut app);
        assert!(!app.general.recorder.is_listening());
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert_eq!(
            app.general.recorder.state,
            crate::gui::prefs::hotkey::RecorderState::Listening,
            "the Return that started it is not a refused shortcut"
        );
        // the same for Space
        app.general.recorder.cancel();
        frame(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![key_event(Key::Space, Modifiers::NONE)]);
        assert_eq!(
            app.general.recorder.state,
            crate::gui::prefs::hotkey::RecorderState::Listening
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_click_starts_recording_esc_cancels_and_the_refusal_shows_only_for_a_refused_combination() {
        use crate::gui::prefs::hotkey::RecorderState::*;
        let (paths, root) = temp("rec-click");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        frame(&ctx, &mut app);
        // find the box: draw once, then click where the recorder is
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(m::WINDOW[0], m::WINDOW[1]),
                )),
                ..Default::default()
            },
            |ui| {
                app.draw(ui);
                rect = ui.ctx().read_response(general::recorder_id()).map_or(rect, |r| r.rect);
            },
        );
        assert!(rect.is_positive(), "the recorder box was laid out");
        frame_with(&ctx, &mut app, click_at(rect.center()));
        frame(&ctx, &mut app);
        assert_eq!(app.general.recorder.state, Listening);
        // a bare key is refused, with the message, and recording continues
        frame_with(&ctx, &mut app, vec![key_event(Key::A, Modifiers::NONE)]);
        assert!(matches!(app.general.recorder.state, Rejected(_)));
        // Esc cancels
        frame_with(&ctx, &mut app, vec![key_event(Key::Escape, Modifiers::NONE)]);
        assert_eq!(app.general.recorder.state, Idle);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn backspace_while_recording_resets_to_the_default_shortcut() {
        let (paths, root) = temp("rec-reset");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), br#"{"hotkey":"cmd+shift+KeyJ"}"#).unwrap();
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths.clone());
        assert_eq!(app.cfg.hotkey, "cmd+shift+KeyJ");
        focus_recorder(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        frame_with(&ctx, &mut app, vec![key_event(Key::Backspace, Modifiers::NONE)]);
        assert_eq!(app.cfg.hotkey, ashdash_core::brand::DEFAULT_HOTKEY);
        assert_eq!(
            Config::load_strict(&paths).unwrap().hotkey,
            ashdash_core::brand::DEFAULT_HOTKEY
        );
        assert!(!app.general.recorder.is_listening());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_combination_being_recorded_does_not_switch_pages() {
        let (paths, root) = temp("rec-nav");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        focus_recorder(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        frame_with(&ctx, &mut app, vec![key_event(Key::Num2, Modifiers::COMMAND)]);
        assert_eq!(app.nav, Nav::General, "Cmd+2 was meant for the recorder");
        let _ = std::fs::remove_dir_all(root);
    }

    fn seeded_data(paths: &Paths) {
        std::fs::create_dir_all(paths.workflows_dir().join("user.workflow.x")).unwrap();
        std::fs::write(
            paths.workflows_dir().join("user.workflow.x/info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
            <key>name</key><string>X</string><key>bundleid</key><string>x.b</string>
            <key>objects</key><array/><key>connections</key><dict/></dict></plist>"#,
        )
        .unwrap();
    }

    fn frame_sized(ctx: &egui::Context, app: &mut PrefsApp, size: [f32; 2]) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| app.draw(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn general_with_the_attention_card_fits_the_default_window_without_scrolling() {
        let (paths, root) = temp("fit");
        seeded_data(&paths);
        let ctx = egui::Context::default();
        let fake = Arc::new(Fake {
            fda: Grant::NotGranted,
            ..Fake::all(Grant::Granted)
        });
        let mut app = app_with(&ctx, paths, fake);
        wait_for_probe(&ctx, &mut app);
        assert!(app.disk_access_missing(), "Safari bookmarks are on and FDA is refused");
        for _ in 0..3 {
            frame_sized(&ctx, &mut app, m::WINDOW);
        }
        let (content, visible) = app.last_scroll;
        assert!(visible > 100.0, "the page area was measured: {visible}");
        assert!(
            content <= visible + 0.5,
            "General needs {content} pt but the window shows {visible} pt: it scrolls at {:?}",
            m::WINDOW
        );
        // the minimum size may scroll but must lay out
        for _ in 0..3 {
            frame_sized(&ctx, &mut app, m::WINDOW_MIN);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn return_follows_the_fix_button_to_permissions_only_when_nothing_is_focused() {
        let (paths, root) = temp("fix");
        let ctx = egui::Context::default();
        let fake = Arc::new(Fake {
            fda: Grant::NotGranted,
            ..Fake::all(Grant::Granted)
        });
        let mut app = app_with(&ctx, paths, fake);
        wait_for_probe(&ctx, &mut app);
        frame(&ctx, &mut app);
        // focus is somewhere: Return belongs to it
        ctx.memory_mut(|m| m.request_focus(search_id()));
        frame(&ctx, &mut app);
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.nav, Nav::General);
        ctx.memory_mut(|m| m.surrender_focus(search_id()));
        frame(&ctx, &mut app);
        // a real keyboard releases the key in between (otherwise egui sees a repeat)
        frame_with(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.nav, Nav::Permissions);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn without_a_refusal_return_does_nothing_on_general() {
        let (paths, root) = temp("nofix");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        wait_for_probe(&ctx, &mut app);
        assert!(!app.disk_access_missing());
        frame_with(&ctx, &mut app, vec![key_event(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.nav, Nav::General);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cmd_r_probes_again_and_entering_the_permissions_page_probes() {
        let (paths, root) = temp("recheck");
        let ctx = egui::Context::default();
        let fake = Arc::new(Fake::all(Grant::Granted));
        let mut app = app_with(&ctx, paths, fake.clone());
        wait_for_probe(&ctx, &mut app);
        let rounds = || fake.rounds.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(rounds(), 1, "one probe at start-up");
        frame_with(&ctx, &mut app, vec![key_event(Key::R, Modifiers::COMMAND)]);
        for _ in 0..200 {
            frame(&ctx, &mut app);
            if rounds() == 2 && !app.perms_checking {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(rounds(), 2, "Cmd+R re-checks");
        frame_with(&ctx, &mut app, vec![key_event(Key::Num6, Modifiers::COMMAND)]);
        for _ in 0..200 {
            frame(&ctx, &mut app);
            if rounds() == 3 && !app.perms_checking {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(rounds(), 3, "the page checks when it opens");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dragging_the_title_bar_starts_a_window_drag() {
        let (paths, root) = temp("drag");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        frame(&ctx, &mut app);
        let from = egui::pos2(400.0, m::TITLEBAR_H / 2.0);
        let b = |pressed| egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        viewport_commands(&ctx, &mut app, vec![egui::Event::PointerMoved(from)]);
        viewport_commands(&ctx, &mut app, vec![b(true)]);
        let cmds = viewport_commands(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(from + egui::vec2(12.0, 3.0))],
        );
        assert!(
            cmds.iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag)),
            "{cmds:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_title_bar_is_the_top_strip_and_content_starts_below_it() {
        let (paths, root) = temp("titlebar");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths);
        frame(&ctx, &mut app);
        let mut sidebar_top = f32::MAX;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(m::WINDOW[0], m::WINDOW[1]),
                )),
                ..Default::default()
            },
            |ui| {
                app.draw(ui);
                sidebar_top = ui
                    .ctx()
                    .read_response(egui::Id::new("prefs-go-to-a-setting"))
                    .map_or(sidebar_top, |r| r.rect.min.y);
            },
        );
        assert!(
            sidebar_top >= m::TITLEBAR_H,
            "the first widget of the sidebar starts at {sidebar_top}, under the traffic lights"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_edit_reaches_the_file_and_the_window_copy() {
        let (paths, root) = temp("edit");
        let ctx = egui::Context::default();
        let mut app = test_app(&ctx, paths.clone());
        app.edit(|c| crate::prefs_model::set_max_results(c, 12)).unwrap();
        assert_eq!(app.cfg.max_results, 12);
        assert_eq!(Config::load_strict(&paths).unwrap().max_results, 12);
        // another process edits the file: the window follows on its next poll
        let mut c = Config::load_strict(&paths).unwrap();
        c.max_results = 20;
        c.save(&paths).unwrap();
        app.last_poll = Instant::now() - POLL_EVERY * 2;
        app.poll_disk();
        assert_eq!(app.cfg.max_results, 20);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod drop_tests {
    use std::path::Path;

    use super::{Purpose, drop_target};

    #[test]
    fn the_drop_zone_accepts_every_workflow_extension_the_launcher_does() {
        for ext in ashdash_core::workflow::WORKFLOW_PACKAGE_EXTENSIONS {
            for name in [format!("a.{ext}"), format!("A.{}", ext.to_uppercase())] {
                assert_eq!(drop_target(Path::new(&name)), Some(Purpose::Workflow), "{name}");
            }
        }
        assert_eq!(drop_target(Path::new("x.alfredappearance")), Some(Purpose::Theme));
        assert_eq!(drop_target(Path::new("x.ALFREDSNIPPETS")), Some(Purpose::Snippets));
        for no in ["x.zip", "x", "x.alfred2workflow", "x.alfredworkflow.bak"] {
            assert_eq!(drop_target(Path::new(no)), None, "{no}");
        }
    }
}
