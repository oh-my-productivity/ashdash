//! General page: launcher shortcut, launch at login, theme, result count, learning.

use std::path::Path;
use std::time::Duration;

use ashdash_core::Paths;
use ashdash_core::settings_view::{self, ThemeInfo};
use ashdash_core::usage::Usage;
use eframe::egui::{self, RichText};

use super::super::keynames::display_hotkey;
use super::app::{PrefsApp, Purpose};
use super::hotkey::{self, Captured, Recorder};
use super::widgets::{Msg, WARN, confirm_button, hint, section, show_msg};
use crate::ipc;
use crate::mac::loginitem::{self, Command, Outcome, Status, SystemBackend};
use crate::prefs_model as model;

#[derive(Default)]
pub struct State {
    recorder: Recorder,
    hotkey_msg: Option<Msg>,
    themes: Vec<ThemeInfo>,
    theme_input: String,
    theme_msg: Option<Msg>,
    login: Option<Result<Outcome, String>>,
    login_msg: Option<Msg>,
    results_draft: Option<usize>,
    results_msg: Option<Msg>,
    confirm_forget: bool,
    usage_msg: Option<Msg>,
}

impl State {
    pub fn refresh(&mut self, paths: &Paths) {
        self.themes = settings_view::list_themes(paths);
        self.login = Some(loginitem::run(Command::Status, &SystemBackend).map_err(|e| e.to_string()));
    }
}

impl PrefsApp {
    pub(super) fn general_page(&mut self, ui: &mut egui::Ui) {
        self.hotkey_section(ui);
        self.login_section(ui);
        self.theme_section(ui);
        self.results_section(ui);
        self.usage_section(ui);
    }

    // ---------------------------------------------------------------- shortcut

    fn hotkey_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Launcher shortcut", |ui| {
            let spec = self.cfg.hotkey.clone();
            let shown = display_hotkey(&spec).unwrap_or_else(|| spec.clone());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&shown).size(22.0).monospace());
                if self.general.recorder.is_listening() {
                    if ui.button("Cancel").clicked() {
                        self.general.recorder.cancel();
                    }
                } else if ui.button("Record new shortcut…").clicked() {
                    self.general.recorder.start();
                    self.general.hotkey_msg = None;
                }
                if !self.general.recorder.is_listening()
                    && spec != ashdash_core::brand::DEFAULT_HOTKEY
                    && ui.button("Reset").clicked()
                {
                    self.apply_hotkey(&Captured {
                        spec: ashdash_core::brand::DEFAULT_HOTKEY.into(),
                        display: String::new(),
                        conflict: None,
                    });
                }
            });
            if self.general.recorder.is_listening() {
                let text = match &self.general.recorder.state {
                    hotkey::RecorderState::Rejected(why) => why.clone(),
                    _ => "Press the new shortcut now (Esc cancels)…".into(),
                };
                ui.label(RichText::new(text).color(WARN));
                let events = ui.ctx().input(|i| i.events.clone());
                for press in hotkey::presses(&events) {
                    if let Some(cap) = self.general.recorder.feed(&press) {
                        self.apply_hotkey(&cap);
                    }
                }
            }
            show_msg(ui, &self.general.hotkey_msg);
            hint(
                ui,
                "Use at least one of ⌘ ⌥ ⌃ with a key. The launcher switches to it within a second.",
            );
        });
    }

    /// Check that macOS lets us have the shortcut, then save it; the launcher re-registers it.
    fn apply_hotkey(&mut self, cap: &Captured) {
        if cap.spec.eq_ignore_ascii_case(&self.cfg.hotkey) {
            self.general.hotkey_msg = Some(Msg::ok("That is the current shortcut."));
            return;
        }
        if let Err(e) = hotkey::probe(&cap.spec) {
            self.general.hotkey_msg = Some(Msg::err(format!(
                "{} is not available: {e}. Another app or macOS may be using it.",
                if cap.display.is_empty() {
                    &cap.spec
                } else {
                    &cap.display
                }
            )));
            return;
        }
        let spec = cap.spec.clone();
        self.general.hotkey_msg = Some(match self.edit(|c| model::set_hotkey(c, &spec)) {
            Ok(()) => {
                let shown = display_hotkey(&spec).unwrap_or(spec);
                match cap.conflict {
                    Some(what) => Msg::ok(format!(
                        "Saved {shown}. Note: macOS uses this for {what}; one of them may win."
                    )),
                    None => Msg::ok(format!("Saved {shown}.")),
                }
            }
            Err(e) => Msg::err(e),
        });
    }

    // ---------------------------------------------------------------- login item

    fn login_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Startup", |ui| {
            match self.general.login.clone() {
                Some(Ok(Outcome::State { status, .. })) => {
                    let mut on = matches!(status, Status::Enabled | Status::RequiresApproval);
                    if ui.checkbox(&mut on, "Launch Ashdash at login").changed() {
                        let cmd = if on { Command::Enable } else { Command::Disable };
                        match loginitem::run(cmd, &SystemBackend) {
                            Ok(out) => {
                                self.general.login_msg = Some(Msg::ok(out.message()));
                                self.general.login = Some(Ok(out));
                            }
                            Err(e) => {
                                self.general.login_msg = Some(Msg::err(e.to_string()));
                                self.general.login =
                                    Some(loginitem::run(Command::Status, &SystemBackend).map_err(|e| e.to_string()));
                            }
                        }
                    }
                    if status == Status::RequiresApproval || status == Status::NotFound {
                        ui.label(RichText::new(status.describe()).color(WARN));
                    }
                }
                Some(Ok(Outcome::Unavailable(why))) => {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, "Launch Ashdash at login"));
                    hint(ui, &why.to_string());
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(WARN));
                }
                None => {}
            }
            show_msg(ui, &self.general.login_msg);
        });
    }

    // ---------------------------------------------------------------- theme

    fn theme_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Appearance", |ui| {
            let current = self.cfg.theme.clone();
            let label = match &current {
                None => "Default".to_string(),
                Some(p) => self
                    .general
                    .themes
                    .iter()
                    .find(|t| &t.path == p)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| p.display().to_string()),
            };
            let mut choice: Option<Option<std::path::PathBuf>> = None;
            ui.horizontal(|ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("prefs-theme")
                    .selected_text(label)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "Default").clicked() {
                            choice = Some(None);
                        }
                        for t in &self.general.themes {
                            if ui
                                .selectable_label(current.as_ref() == Some(&t.path), &t.name)
                                .clicked()
                            {
                                choice = Some(Some(t.path.clone()));
                            }
                        }
                    });
            });
            if let Some(theme) = choice {
                self.general.theme_msg = Some(match self.edit(|c| model::set_theme(c, theme)) {
                    Ok(()) => Msg::ok("Theme changed. It shows the next time you open the launcher."),
                    Err(e) => Msg::err(e),
                });
            }
            ui.horizontal(|ui| {
                if ui.button("Import theme…").clicked() {
                    self.choose_file(
                        Purpose::Theme,
                        "Choose a reference-compatible theme (.alfredappearance)",
                        "alfredappearance",
                    );
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.general.theme_input)
                        .hint_text("or paste the path of an .alfredappearance file")
                        .desired_width(320.0),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Import").clicked() || enter) && !self.general.theme_input.trim().is_empty() {
                    let path = Path::new(self.general.theme_input.trim()).to_path_buf();
                    self.import_theme(&path);
                }
            });
            hint(ui, "You can also drop an .alfredappearance file onto this window.");
            show_msg(ui, &self.general.theme_msg);
        });
    }

    /// Validate, copy into the themes folder and select it.
    pub(super) fn import_theme(&mut self, file: &Path) {
        let result = settings_view::import_theme(&self.paths, file)
            .map_err(|e| format!("{e:#}"))
            .and_then(|dest| {
                self.edit(|c| model::set_theme(c, Some(dest)))?;
                Ok(())
            });
        self.general.theme_msg = Some(match result {
            Ok(()) => {
                self.general.theme_input.clear();
                self.general.themes = settings_view::list_themes(&self.paths);
                Msg::ok("Theme imported and selected. It shows the next time you open the launcher.")
            }
            Err(e) => Msg::err(format!("Could not import the theme: {e}")),
        });
    }

    // ---------------------------------------------------------------- results

    fn results_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Results", |ui| {
            let mut n = self.general.results_draft.unwrap_or(self.cfg.max_results);
            ui.horizontal(|ui| {
                ui.label("Results per search");
                let resp = ui.add(egui::Slider::new(&mut n, model::MAX_RESULTS_RANGE));
                if resp.changed() {
                    self.general.results_draft = Some(n);
                }
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    self.general.results_draft = None;
                    self.general.results_msg = self.edit(|c| model::set_max_results(c, n)).err().map(Msg::err);
                }
            });
            show_msg(ui, &self.general.results_msg);
        });
    }

    // ---------------------------------------------------------------- learning

    fn usage_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Learning", |ui| {
            let mut on = self.cfg.learn_usage;
            if ui.checkbox(&mut on, "Rank results by what I pick").changed() {
                self.general.usage_msg = self.edit(|c| model::set_learn_usage(c, on)).err().map(Msg::err);
            }
            hint(
                ui,
                "When off, nothing new is recorded and earlier picks stop affecting the order.",
            );
            if confirm_button(
                ui,
                &mut self.general.confirm_forget,
                "Forget what I picked…",
                "Yes, forget everything learned",
            ) {
                self.general.usage_msg = Some(match self.forget_usage() {
                    Ok(()) => Msg::ok("Forgotten."),
                    Err(e) => Msg::err(e),
                });
            }
            show_msg(ui, &self.general.usage_msg);
        });
    }

    /// The running launcher holds the learned data in memory, so it must do the clearing; with
    /// no launcher the file is cleared directly.
    fn forget_usage(&self) -> Result<(), String> {
        let sock = ipc::socket_path(&self.paths, ipc::current_uid());
        match ipc::send(&sock, &ipc::Request::ClearUsage, Duration::from_secs(5)) {
            Ok(r) if r.ok => Ok(()),
            Ok(r) => Err(r.error),
            Err(ipc::ClientError::NotRunning) => {
                Usage::load(self.paths.usage_file()).clear();
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
