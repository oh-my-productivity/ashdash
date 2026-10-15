//! Workflows page (list, enable/disable, import with the trust prompt, per-workflow settings,
//! remove) and the About page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ashdash_core::engine::WorkflowSetting;
use ashdash_core::settings_view::{self, PackagePreview, WorkflowInfo};
use ashdash_core::workflow::Workflow;
use ashdash_core::{Config, Paths};
use eframe::egui::{self, RichText};

use super::app::{PrefsApp, Purpose, Task};
use super::widgets::{ERROR, Msg, WARN, confirm_button, hint, section, show_msg};
use crate::prefs_model::{self as model, SettingControl};

#[derive(Default)]
pub struct State {
    list: Vec<WorkflowInfo>,
    import_input: String,
    preview: Option<(PathBuf, PackagePreview)>,
    previewing: bool,
    msg: Option<Msg>,
    /// Bundle id of the workflow whose settings are expanded.
    open_settings: Option<String>,
    settings: Vec<WorkflowSetting>,
    /// Text being edited, by variable.
    edits: HashMap<String, String>,
    setting_msg: Option<Msg>,
    details: Option<String>,
    confirm_remove: Option<String>,
}

impl State {
    pub fn refresh(&mut self, paths: &Paths, cfg: &Config) {
        self.list = settings_view::list_workflows(paths, &cfg.workflow_dirs);
    }

    #[cfg(test)]
    pub fn list_len(&self) -> usize {
        self.list.len()
    }

    pub fn preview_done(&mut self, r: Result<(PathBuf, PackagePreview), String>) {
        self.previewing = false;
        match r {
            Ok(p) => self.preview = Some(p),
            Err(e) => self.msg = Some(Msg::err(format!("This file cannot be installed: {e}"))),
        }
    }
}

impl PrefsApp {
    pub(super) fn workflows_page(&mut self, ui: &mut egui::Ui) {
        // Another process (the launcher, the CLI, Finder) may have changed the folder.
        if ui.ctx().input(|i| i.viewport().focused == Some(true)) && self.workflows.preview.is_none() {
            self.workflows.refresh(&self.paths, &self.cfg);
        }
        self.import_section(ui);
        section(ui, "Installed workflows", |ui| {
            if self.workflows.list.is_empty() {
                hint(
                    ui,
                    "No workflows installed. Import a .alfredworkflow file above, or point workflow_dirs at an existing folder of workflows.",
                );
            }
            let list = self.workflows.list.clone();
            for w in &list {
                self.workflow_row(ui, w);
                ui.separator();
            }
            show_msg(ui, &self.workflows.msg);
        });
    }

    // ---------------------------------------------------------------- import

    fn import_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Import", |ui| {
            if let Some((path, pv)) = self.workflows.preview.clone() {
                self.import_prompt(ui, &path, &pv);
                return;
            }
            ui.horizontal(|ui| {
                if ui.button("Choose workflow file…").clicked() {
                    self.choose_file(
                        Purpose::Workflow,
                        "Choose a workflow (.alfredworkflow)",
                        "alfredworkflow",
                    );
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.workflows.import_input)
                        .hint_text("or paste the path of a .alfredworkflow file")
                        .desired_width(320.0),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Review").clicked() || enter) && !self.workflows.import_input.trim().is_empty() {
                    let path = Path::new(self.workflows.import_input.trim()).to_path_buf();
                    self.start_preview(path);
                }
            });
            if self.workflows.previewing {
                ui.label("Reading the package…");
            }
            hint(
                ui,
                "You can also drop a .alfredworkflow file onto this window. Nothing is installed before you confirm.",
            );
        });
    }

    /// Unpack in a thread (size-limited) and show what the workflow can do.
    pub(super) fn start_preview(&mut self, path: PathBuf) {
        self.workflows.previewing = true;
        self.workflows.msg = None;
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = settings_view::preview_package(&paths, &path)
                .map(|pv| (path, pv))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Task::Preview(Box::new(r)));
            ctx.request_repaint();
        });
    }

    fn import_prompt(&mut self, ui: &mut egui::Ui, path: &Path, pv: &PackagePreview) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(format!("Install “{}”?", pv.name)).strong().size(15.0));
            let mut facts = vec![format!(
                "Identifier: {}",
                if pv.bundle_id.is_empty() {
                    "(none)"
                } else {
                    &pv.bundle_id
                }
            )];
            if !pv.version.is_empty() {
                facts.push(format!("Version {}", pv.version));
            }
            ui.label(facts.join("   ·   "));
            if pv.replaces_installed {
                ui.label(
                    RichText::new("A workflow with this identifier is installed and will be replaced.").color(WARN),
                );
            }
            ui.add_space(4.0);
            let lines = model::trust_lines(&pv.trust);
            let color = if pv.trust.runs_code() {
                ERROR
            } else {
                ui.visuals().text_color()
            };
            ui.label(RichText::new(&lines[0]).color(color).strong());
            for l in &lines[1..] {
                ui.label(format!("• {l}"));
            }
            hint(ui, "Only install workflows from people you trust.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Install").clicked() {
                    self.workflows.msg = Some(match self.install_workflow(path) {
                        Ok(n) => Msg::ok(format!("Installed {n}.")),
                        Err(e) => Msg::err(format!("Install failed: {e}")),
                    });
                    self.workflows.preview = None;
                    self.workflows.import_input.clear();
                    self.workflows.refresh(&self.paths, &self.cfg);
                }
                if ui.button("Cancel").clicked() {
                    self.workflows.preview = None;
                }
            });
        });
    }

    fn install_workflow(&self, path: &Path) -> Result<String, String> {
        let dest =
            ashdash_core::workflow::import_package(path, &self.paths.workflows_dir()).map_err(|e| format!("{e:#}"))?;
        Ok(dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default())
    }

    // ---------------------------------------------------------------- list

    fn workflow_row(&mut self, ui: &mut egui::Ui, w: &WorkflowInfo) {
        ui.horizontal(|ui| {
            if w.managed {
                let mut on = !w.disabled;
                if ui.checkbox(&mut on, "").on_hover_text("Enabled").changed() {
                    self.workflows.msg = Some(match settings_view::set_workflow_enabled(&self.paths, &w.dir, on) {
                        Ok(()) => Msg::ok(format!("{} {}.", w.name, if on { "enabled" } else { "disabled" })),
                        Err(e) => Msg::err(format!("{e:#}")),
                    });
                    self.workflows.refresh(&self.paths, &self.cfg);
                }
            } else {
                ui.add_enabled(false, egui::Checkbox::new(&mut !w.disabled, ""));
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(&w.name).strong());
                let mut sub = vec![w.bundle_id.clone()];
                if !w.version.is_empty() {
                    sub.push(format!("v{}", w.version));
                }
                if !w.managed {
                    sub.push(format!(
                        "from {} (managed there)",
                        w.dir.parent().map(|p| p.display().to_string()).unwrap_or_default()
                    ));
                }
                ui.label(RichText::new(sub.join("  ·  ")).weak().small());
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            if w.settings > 0 && ui.small_button("Configure…").clicked() {
                self.open_workflow_settings(w);
            }
            if ui.small_button("What can it do?").clicked() {
                let same = self.workflows.details.as_deref() == Some(w.id.as_str());
                self.workflows.details = (!same).then(|| w.id.clone());
            }
            if w.managed {
                let armed = self.workflows.confirm_remove.as_deref() == Some(w.id.as_str());
                let mut flag = armed;
                if confirm_button(ui, &mut flag, "Remove…", "Yes, remove it") {
                    self.workflows.msg = Some(match settings_view::remove_workflow(&self.paths, &w.dir) {
                        Ok(()) => Msg::ok(format!("Removed {}.", w.name)),
                        Err(e) => Msg::err(format!("{e:#}")),
                    });
                    self.workflows.confirm_remove = None;
                    self.workflows.refresh(&self.paths, &self.cfg);
                } else {
                    self.workflows.confirm_remove = flag.then(|| w.id.clone());
                }
            }
        });
        if self.workflows.details.as_deref() == Some(w.id.as_str()) {
            for (i, l) in model::trust_lines(&w.trust).iter().enumerate() {
                ui.label(if i == 0 {
                    RichText::new(l).strong()
                } else {
                    RichText::new(format!("• {l}"))
                });
            }
        }
        if self.workflows.open_settings.as_deref() == Some(w.bundle_id.as_str()) && !w.bundle_id.is_empty() {
            self.settings_form(ui, w);
        }
    }

    // ---------------------------------------------------------------- per-workflow settings

    fn open_workflow_settings(&mut self, w: &WorkflowInfo) {
        if self.workflows.open_settings.as_deref() == Some(w.bundle_id.as_str()) {
            self.workflows.open_settings = None;
            return;
        }
        self.workflows.setting_msg = None;
        match self.load_settings(&w.dir) {
            Ok(list) => {
                self.workflows.edits = list.iter().map(|s| (s.variable.clone(), s.value.clone())).collect();
                self.workflows.settings = list;
                self.workflows.open_settings = Some(w.bundle_id.clone());
            }
            Err(e) => self.workflows.msg = Some(Msg::err(e)),
        }
    }

    fn load_settings(&self, dir: &Path) -> Result<Vec<WorkflowSetting>, String> {
        let wf = Workflow::load(dir).map_err(|e| format!("{e:#}"))?;
        Ok(settings_view::workflow_settings_of(&wf, &self.paths))
    }

    fn settings_form(&mut self, ui: &mut egui::Ui, w: &WorkflowInfo) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let mut save: Option<(String, String)> = None;
            let mut reset: Option<String> = None;
            let rows = self.workflows.settings.clone();
            for s in &rows {
                let label = if s.label.is_empty() { &s.variable } else { &s.label };
                ui.label(RichText::new(label).strong());
                if !s.description.is_empty() {
                    ui.label(RichText::new(&s.description).weak().small());
                }
                let text = self.workflows.edits.entry(s.variable.clone()).or_default();
                match model::setting_control(&s.kind) {
                    SettingControl::Checkbox => {
                        let mut on = text.trim() == "1" || text.trim() == "true";
                        if ui.checkbox(&mut on, "").changed() {
                            *text = if on { "1".into() } else { "0".into() };
                            save = Some((s.variable.clone(), text.clone()));
                        }
                    }
                    SettingControl::MultilineText => {
                        let r = ui.add(
                            egui::TextEdit::multiline(text)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                        if r.lost_focus() {
                            save = Some((s.variable.clone(), text.clone()));
                        }
                    }
                    SettingControl::Number | SettingControl::Text => {
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(360.0));
                        if r.lost_focus() {
                            save = Some((s.variable.clone(), text.clone()));
                        }
                    }
                }
                if !s.is_default && ui.small_button("Use default").clicked() {
                    reset = Some(s.variable.clone());
                }
                ui.add_space(6.0);
            }
            show_msg(ui, &self.workflows.setting_msg);
            if let Some((var, value)) = save {
                let kind = rows
                    .iter()
                    .find(|s| s.variable == var)
                    .map(|s| s.kind.clone())
                    .unwrap_or_default();
                let current = rows.iter().find(|s| s.variable == var).map(|s| s.value.clone());
                if current.as_deref() != Some(value.as_str()) {
                    self.workflows.setting_msg = Some(match self.save_setting(w, &var, &kind, &value) {
                        Ok(()) => Msg::ok("Saved."),
                        Err(e) => Msg::err(e),
                    });
                    self.reload_settings(w);
                }
            }
            if let Some(var) = reset {
                self.workflows.setting_msg = Some(match self.reset_setting(w, &var) {
                    Ok(()) => Msg::ok("Back to the default."),
                    Err(e) => Msg::err(e),
                });
                self.reload_settings(w);
            }
        });
    }

    fn reload_settings(&mut self, w: &WorkflowInfo) {
        if let Ok(list) = self.load_settings(&w.dir) {
            self.workflows.edits = list.iter().map(|s| (s.variable.clone(), s.value.clone())).collect();
            self.workflows.settings = list;
        }
    }

    fn save_setting(&self, w: &WorkflowInfo, var: &str, kind: &str, raw: &str) -> Result<(), String> {
        let value = model::validate_setting_value(kind, raw)?;
        let wf = Workflow::load(&w.dir).map_err(|e| format!("{e:#}"))?;
        settings_view::write_workflow_value(&wf, &self.paths, var, Some(&value)).map_err(|e| format!("{e:#}"))
    }

    fn reset_setting(&self, w: &WorkflowInfo, var: &str) -> Result<(), String> {
        let wf = Workflow::load(&w.dir).map_err(|e| format!("{e:#}"))?;
        settings_view::write_workflow_value(&wf, &self.paths, var, None).map_err(|e| format!("{e:#}"))
    }

    // ---------------------------------------------------------------- about

    pub(super) fn about_page(&mut self, ui: &mut egui::Ui) {
        section(ui, "About Ashdash", |ui| {
            egui::Grid::new("about-grid")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    for (k, v) in model::about_rows(&self.paths) {
                        ui.label(RichText::new(k).strong());
                        ui.label(RichText::new(v).monospace());
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Show data folder in Finder").clicked() {
                    let _ = ashdash_core::engine::reveal(&self.paths.data);
                }
                if ui.button("Show settings file in Finder").clicked() {
                    let _ = ashdash_core::engine::reveal(&self.paths.config_file());
                }
            });
            ui.add_space(8.0);
            hint(
                ui,
                "Ashdash is open source software. It is not affiliated with the reference launcher or its publisher.",
            );
            hint(
                ui,
                "Settings are kept in config.json; `ashdash config keys` lists every setting.",
            );
        });
    }
}
