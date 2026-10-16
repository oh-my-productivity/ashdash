//! The preferences window (its own process, see the module docs of `prefs`).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ashdash_core::workflow::StagedPackage;
use ashdash_core::{Config, Paths};
use anyhow::{Result, anyhow};
use eframe::egui::{self, RichText};

use super::status::{self, LauncherStatus};
use super::widgets::{ERROR, WARN};
use super::{features, general, workflows};
use crate::prefs_model::{EditResult, Page, Store};

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
}

pub struct PrefsApp {
    pub(super) paths: Paths,
    pub(super) store: Store,
    /// The settings on disk as last read.
    pub(super) cfg: Config,
    /// Set while `config.json` cannot be read; editing is refused then.
    pub(super) cfg_error: Option<String>,
    page: Page,
    launcher: Option<LauncherStatus>,
    last_poll: Instant,
    pub(super) general: general::State,
    pub(super) features: features::State,
    pub(super) workflows: workflows::State,
    pub(super) tx: Sender<Task>,
    rx: Receiver<Task>,
    pub(super) ctx: egui::Context,
    focused_once: bool,
}

pub fn run(paths: Paths) -> Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size([820.0, 640.0])
        .with_min_inner_size([640.0, 480.0])
        .with_drag_and_drop(true);
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
        Box::new(move |cc| Ok(Box::new(PrefsApp::new(cc.egui_ctx.clone(), paths)))),
    )
    .map_err(|e| anyhow!("{e}"))
}

impl PrefsApp {
    fn new(ctx: egui::Context, paths: Paths) -> PrefsApp {
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
            page: Page::General,
            launcher: None,
            last_poll: Instant::now() - POLL_EVERY,
            general: general::State::default(),
            features: features::State::default(),
            workflows: workflows::State::default(),
            tx,
            rx,
            ctx,
            focused_once: false,
        };
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
        match self.page {
            Page::General => self.general.refresh(&self.paths),
            Page::Features => self.features.refresh(&self.paths, &self.cfg),
            Page::Workflows => self.workflows.refresh(&self.paths, &self.cfg),
            Page::About => {}
        }
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
                    self.page = Page::Workflows;
                    self.enter_page();
                    self.start_preview(path);
                }
                Some(Purpose::Theme) => {
                    self.page = Page::General;
                    self.enter_page();
                    self.import_theme(&path);
                }
                Some(Purpose::Snippets) => {
                    self.page = Page::Features;
                    self.enter_page();
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

    fn banners(&self, ui: &mut egui::Ui) {
        if let Some(e) = &self.cfg_error {
            ui.label(
                RichText::new(format!(
                    "Warning: {e}\nThe window shows defaults and will not change the file."
                ))
                .color(ERROR),
            );
        }
        if let Some(st) = &self.launcher {
            if !status::process_alive(st.pid) {
                ui.label(
                    RichText::new("Ashdash's launcher is not running: changes apply the next time it starts.")
                        .color(WARN),
                );
            } else {
                for p in &st.problems {
                    ui.label(RichText::new(format!("Warning: {p}")).color(ERROR));
                }
                if !st.restart_required.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "Restart Ashdash for these to take effect: {}.",
                            st.restart_required.join(", ")
                        ))
                        .color(WARN),
                    );
                }
            }
        } else {
            ui.label(RichText::new("The launcher has not reported yet: changes apply when it is running.").color(WARN));
        }
    }
}

impl eframe::App for PrefsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl PrefsApp {
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
        ctx.request_repaint_after(POLL_EVERY);

        egui::Panel::top("prefs-tabs").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for p in Page::ALL {
                    if ui
                        .selectable_label(self.page == p, RichText::new(p.title()).size(15.0))
                        .clicked()
                        && self.page != p
                    {
                        self.page = p;
                        self.enter_page();
                    }
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            self.banners(ui);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(760.0);
                match self.page {
                    Page::General => self.general_page(ui),
                    Page::Features => self.features_page(ui),
                    Page::Workflows => self.workflows_page(ui),
                    Page::About => self.about_page(ui),
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-prefs-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (Paths::under(&root), root)
    }

    fn frame(ctx: &egui::Context, app: &mut PrefsApp) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(820.0, 640.0))),
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
        let mut app = PrefsApp::new(ctx.clone(), paths);
        for page in Page::ALL {
            app.page = page;
            app.enter_page();
            for _ in 0..3 {
                frame(&ctx, &mut app);
            }
        }
        assert_eq!(app.workflows.list_len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_config_is_shown_and_never_rewritten() {
        let (paths, root) = temp("broken");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), b"{ oops").unwrap();
        let ctx = egui::Context::default();
        let mut app = PrefsApp::new(ctx.clone(), paths.clone());
        assert!(app.cfg_error.is_some());
        assert!(app.edit(|c| crate::prefs_model::set_max_results(c, 10)).is_err());
        for page in Page::ALL {
            app.page = page;
            app.enter_page();
            frame(&ctx, &mut app);
        }
        assert_eq!(std::fs::read(paths.config_file()).unwrap(), b"{ oops");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_edit_reaches_the_file_and_the_window_copy() {
        let (paths, root) = temp("edit");
        let ctx = egui::Context::default();
        let mut app = PrefsApp::new(ctx, paths.clone());
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
