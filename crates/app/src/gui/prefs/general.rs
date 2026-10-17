//! General page: launcher shortcut, launch at login, theme, result count, learning.

use std::path::Path;
use std::time::Duration;

use ashdash_core::Paths;
use ashdash_core::settings_view::{self, ThemeInfo};
use ashdash_core::usage::Usage;
use eframe::egui;

use super::app::{PrefsApp, Purpose};
use super::hotkey::{self, Captured, Recorder};
use super::kit;
use super::style::{self, Palette};
use super::summary;
use super::widgets::{Msg, confirm_button, hint, is_error, show_msg, warn_text};
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
    pub(super) fn general_page(&mut self, ui: &mut egui::Ui, launcher_alive: bool) {
        kit::rows(ui, |ui| {
            self.hotkey_row(ui);
            self.theme_row(ui);
            self.results_row(ui);
            self.login_row(ui);
            self.usage_row(ui);
        });
        ui.add_space(super::style::m::GAP_L);
        self.reload_cards(ui, launcher_alive);
    }

    /// RELOAD card (what the launcher did with the settings file) and the attention cards.
    fn reload_cards(&mut self, ui: &mut egui::Ui, alive: bool) {
        let cfg_error = self.cfg_error.clone();
        let (tone, line) = summary::reload_line(cfg_error.as_deref(), self.launcher_status(), alive);
        let file = self.paths.config_file();
        kit::info_card(ui, "Reload", tone, &[line], |ui| {
            if ui.button("Show config.json").clicked() {
                let _ = ashdash_core::engine::reveal(&file);
            }
        });
        for a in summary::attention(cfg_error.as_deref(), self.launcher_status(), alive) {
            ui.add_space(super::style::m::GAP);
            kit::info_card(ui, a.title, a.tone, &a.lines, |_| {});
        }
    }

    // ---------------------------------------------------------------- shortcut

    fn hotkey_row(&mut self, ui: &mut egui::Ui) {
        let err = is_error(&self.general.hotkey_msg);
        kit::row(ui, "Launcher shortcut", Some("hotkey"), err, |ui| {
            let p = Palette::of(ui);
            let spec = self.cfg.hotkey.clone();
            let shown = display(&spec);
            let listening = self.general.recorder.is_listening();
            let (main, sub) = if listening {
                match &self.general.recorder.state {
                    hotkey::RecorderState::Rejected(why) => {
                        (why.clone(), format!("{shown} stays until you press another"))
                    }
                    _ => ("Press the new shortcut…".to_string(), format!("{shown} · Esc cancels")),
                }
            } else {
                (shown, String::new())
            };
            if kit::recorder_box(ui, listening, &main, &sub).clicked() && !listening {
                self.general.recorder.start();
                self.general.hotkey_msg = None;
            }
            ui.horizontal_wrapped(|ui| {
                if listening {
                    if ui.button("Cancel").clicked() {
                        self.general.recorder.cancel();
                    }
                } else {
                    if ui.button("Record new shortcut…").clicked() {
                        self.general.recorder.start();
                        self.general.hotkey_msg = None;
                    }
                    if spec != ashdash_core::brand::DEFAULT_HOTKEY && ui.button("Reset").clicked() {
                        self.apply_hotkey(&Captured {
                            spec: ashdash_core::brand::DEFAULT_HOTKEY.into(),
                            display: String::new(),
                            conflict: None,
                        });
                    }
                }
            });
            if self.general.recorder.is_listening() {
                let events = ui.ctx().input(|i| i.events.clone());
                for press in hotkey::presses(&events) {
                    if let Some(cap) = self.general.recorder.feed(&press) {
                        self.apply_hotkey(&cap);
                    }
                }
            }
            show_msg(ui, &self.general.hotkey_msg);
            ui.add(
                egui::Label::new(style::hint_text(
                    &p,
                    "Use at least one of Cmd, Option or Ctrl with a key. The launcher switches to it within a second.",
                ))
                .wrap(),
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
                let shown = hotkey::label(&spec);
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

    fn login_row(&mut self, ui: &mut egui::Ui) {
        let err = is_error(&self.general.login_msg) || matches!(self.general.login, Some(Err(_)));
        // not a config.json setting: the macOS login item
        kit::row(ui, "Launch at login", None, err, |ui| {
            let p = Palette::of(ui);
            let note = "macOS login item · not in config.json";
            match self.general.login.clone() {
                Some(Ok(Outcome::State { status, .. })) => {
                    let mut on = matches!(status, Status::Enabled | Status::RequiresApproval);
                    if kit::toggle(ui, &mut on, "Launch Ashdash at login").changed() {
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
                    ui.label(style::hint_text(&p, note));
                    if status == Status::RequiresApproval || status == Status::NotFound {
                        ui.add(egui::Label::new(warn_text(ui, status.describe())).wrap());
                    }
                }
                Some(Ok(Outcome::Unavailable(why))) => {
                    ui.add_enabled_ui(false, |ui| kit::toggle(ui, &mut false, "Launch Ashdash at login"));
                    ui.label(style::hint_text(&p, note));
                    hint(ui, &why.to_string());
                }
                Some(Err(e)) => {
                    ui.add(egui::Label::new(warn_text(ui, &e)).wrap());
                }
                None => {}
            }
            show_msg(ui, &self.general.login_msg);
        });
    }

    // ---------------------------------------------------------------- theme

    fn theme_row(&mut self, ui: &mut egui::Ui) {
        let err = is_error(&self.general.theme_msg);
        kit::row(ui, "Theme", Some("theme"), err, |ui| {
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
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("prefs-theme")
                    .selected_text(label)
                    .width(200.0)
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
                if ui.button("Import theme…").clicked() {
                    self.choose_file(
                        Purpose::Theme,
                        "Choose a reference-compatible theme (.alfredappearance)",
                        &["alfredappearance"],
                    );
                }
            });
            if let Some(theme) = choice {
                self.general.theme_msg = Some(match self.edit(|c| model::set_theme(c, theme)) {
                    Ok(()) => Msg::ok("Theme changed. It shows the next time you open the launcher."),
                    Err(e) => Msg::err(e),
                });
            }
            ui.horizontal_wrapped(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.general.theme_input)
                        .hint_text("or paste the path of an .alfredappearance file")
                        .desired_width(200.0),
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

    fn results_row(&mut self, ui: &mut egui::Ui) {
        let err = is_error(&self.general.results_msg);
        kit::row(ui, "Results per search", Some("max_results"), err, |ui| {
            let mut n = self.general.results_draft.unwrap_or(self.cfg.max_results);
            let resp = kit::stepper(ui, &mut n, model::MAX_RESULTS_RANGE);
            if resp.changed() {
                self.general.results_draft = Some(n);
            }
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                self.general.results_draft = None;
                self.general.results_msg = self.edit(|c| model::set_max_results(c, n)).err().map(Msg::err);
            }
            show_msg(ui, &self.general.results_msg);
        });
    }

    // ---------------------------------------------------------------- learning

    fn usage_row(&mut self, ui: &mut egui::Ui) {
        let err = is_error(&self.general.usage_msg);
        kit::row(ui, "Rank results by what I pick", Some("learn_usage"), err, |ui| {
            let mut on = self.cfg.learn_usage;
            if kit::toggle(ui, &mut on, "Rank results by what I pick").changed() {
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

/// The shortcut as drawn in the recorder: macOS symbols (Inter has them), or the raw spec.
fn display(spec: &str) -> String {
    crate::gui::keynames::display_hotkey(spec).unwrap_or_else(|| spec.to_string())
}
