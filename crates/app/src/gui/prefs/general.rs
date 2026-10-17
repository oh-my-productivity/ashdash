//! General page: launcher shortcut, launch at login, theme, result count, learning.

use std::path::Path;
use std::time::Duration;

use ashdash_core::Paths;
use ashdash_core::settings_view::{self, ThemeInfo};
use ashdash_core::usage::Usage;
use eframe::egui;

use super::app::{PrefsApp, Purpose};
use super::hotkey::{self, Captured, Recorder, RecorderState};
use super::kit::{self, RecorderView};
use super::style::{self, Palette};
use super::summary;
use super::widgets::{Msg, confirm_button, is_error, show_msg, warn_text};
use crate::ipc;
use crate::mac::loginitem::{self, Command, Outcome, Status, SystemBackend, Unavailable};
use crate::prefs_model as model;

#[derive(Default)]
pub struct State {
    pub(super) recorder: Recorder,
    hotkey_msg: Option<Msg>,
    themes: Vec<ThemeInfo>,
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

/// Id of the shortcut recorder box (tests focus it by id).
pub(super) fn recorder_id() -> egui::Id {
    egui::Id::new("prefs-hotkey-recorder")
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
        ui.add_space(super::style::m::GAP);
        self.reload_cards(ui, launcher_alive);
    }

    /// The RELOAD card (what the launcher did with the settings file) and, beside it, the
    /// attention cards (problems, restart needed, Full Disk Access). Alone it takes the full width.
    fn reload_cards(&mut self, ui: &mut egui::Ui, alive: bool) {
        let cfg_error = self.cfg_error.clone();
        let (tone, line) = summary::reload_line(cfg_error.as_deref(), self.launcher_status(), alive);
        let file = self.paths.config_file();
        let disk_missing = self.disk_access_missing();
        let cards = summary::attention(cfg_error.as_deref(), self.launcher_status(), alive, disk_missing);
        let reload = |ui: &mut egui::Ui| {
            kit::info_card(ui, Some("Reload"), tone, &[line], |ui| {
                if ui.button("Show config.json").clicked() {
                    let _ = ashdash_core::engine::reveal(&file);
                }
            });
        };
        let mut fix = None;
        let mut show_cards = |ui: &mut egui::Ui| {
            for (i, a) in cards.iter().enumerate() {
                if i > 0 {
                    ui.add_space(super::style::m::GAP);
                }
                kit::info_card(ui, a.title, a.tone, &a.lines, |ui| {
                    if let Some(page) = a.fix
                        && ui.button("Fix…  ↵").clicked()
                    {
                        fix = Some(page);
                    }
                });
            }
        };
        if cards.is_empty() {
            reload(ui);
        } else {
            ui.columns(2, |cols| {
                reload(&mut cols[0]);
                show_cards(&mut cols[1]);
            });
        }
        if let Some(page) = fix {
            self.go(page);
        }
    }

    // ---------------------------------------------------------------- shortcut

    fn hotkey_row(&mut self, ui: &mut egui::Ui) {
        let listening = self.general.recorder.is_listening();
        let refused = match &self.general.recorder.state {
            RecorderState::Rejected(why) => Some(why.clone()),
            _ => None,
        };
        let err = is_error(&self.general.hotkey_msg) || refused.is_some();
        // the keys that work while recording are said in the label cell: the box itself is one box
        let note = listening.then_some("Esc cancels · ⌫ default");
        kit::row_with_note(ui, "Launcher shortcut", Some("hotkey"), note, err, |ui| {
            let shown = display(&self.cfg.hotkey);
            let id = recorder_id();
            let view = if listening {
                RecorderView::Recording { was: &shown }
            } else {
                RecorderView::Idle { shortcut: &shown }
            };
            let resp = kit::recorder_box(ui, id, view);
            // Click, Return and Space on the focused box all arrive as `clicked()`
            let mut just_started = false;
            if !listening && resp.clicked() {
                self.general.recorder.start();
                self.general.hotkey_msg = None;
                resp.request_focus();
                just_started = true;
            }
            if self.general.recorder.is_listening() {
                // Tab, Esc and the arrows belong to the recorder (⌥← is a shortcut, Esc cancels)
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });
                if !just_started {
                    // the key that started recording (Return / Space) is this frame's event: skip it
                    if !resp.has_focus() {
                        self.general.recorder.cancel();
                    }
                    let events = ui.ctx().input(|i| i.events.clone());
                    for press in hotkey::presses(&events) {
                        if let Some(cap) = self.general.recorder.feed(&press) {
                            self.apply_hotkey(&cap);
                        }
                    }
                }
            }
            // shown only when a recorded combination was refused, or saving it failed
            if let RecorderState::Rejected(why) = &self.general.recorder.state {
                Msg::err(why.clone()).show(ui);
            }
            show_msg(ui, &self.general.hotkey_msg);
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
                    ui.horizontal_wrapped(|ui| {
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
                                    self.general.login = Some(
                                        loginitem::run(Command::Status, &SystemBackend).map_err(|e| e.to_string()),
                                    );
                                }
                            }
                        }
                        ui.label(style::mono_dim(&p, note, style::m::KEY));
                    });
                    if status == Status::RequiresApproval || status == Status::NotFound {
                        ui.add(egui::Label::new(warn_text(ui, status.describe())).wrap());
                    }
                }
                Some(Ok(Outcome::Unavailable(why))) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(false, |ui| kit::toggle(ui, &mut false, "Launch Ashdash at login"));
                        ui.label(style::mono_dim(&p, note, style::m::KEY));
                    });
                    // one line: why the switch is off
                    ui.add(egui::Label::new(warn_text(ui, unavailable_reason(&why))).wrap());
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
            // a dropdown and "Import theme…": a file chooser, dropping a file on the window
            // and `ashdash theme <file>` do the importing
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("prefs-theme")
                    .selected_text(label)
                    .width(180.0)
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
            ui.horizontal_wrapped(|ui| {
                let mut on = self.cfg.learn_usage;
                if kit::toggle(ui, &mut on, "Rank results by what I pick").changed() {
                    self.general.usage_msg = self.edit(|c| model::set_learn_usage(c, on)).err().map(Msg::err);
                }
                if confirm_button(
                    ui,
                    &mut self.general.confirm_forget,
                    "Forget usage…",
                    "Yes, forget everything learned",
                ) {
                    self.general.usage_msg = Some(match self.forget_usage() {
                        Ok(()) => Msg::ok("Forgotten."),
                        Err(e) => Msg::err(e),
                    });
                }
            });
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

/// Why the login switch is off, in one line.
fn unavailable_reason(why: &Unavailable) -> &'static str {
    match why {
        Unavailable::NotInBundle => "Needs Ashdash.app; this is not running from a bundle.",
        Unavailable::OsTooOld => "Needs macOS 13 or newer.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_switch_gives_its_reason_in_one_short_line() {
        for why in [Unavailable::NotInBundle, Unavailable::OsTooOld] {
            let line = unavailable_reason(&why);
            assert!(!line.contains('\n') && line.len() < 60, "{line}");
        }
    }
}
