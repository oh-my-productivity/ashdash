//! Workflows page (list, enable/disable, import with the trust prompt, per-workflow settings,
//! remove) and the About page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ashdash_core::engine::WorkflowSetting;
use ashdash_core::settings_view::{self, WorkflowInfo};
use ashdash_core::workflow::{StagedPackage, Workflow};
use ashdash_core::{Config, Paths};
use eframe::egui::{self, RichText};

use super::app::{PrefsApp, Purpose, Task};
use super::kit;
use super::style::{self, Palette};
use super::widgets::{Msg, confirm_button, error_text, hint, section, show_msg, warn_text};
use crate::prefs_model::{self as model, SettingControl};

#[derive(Default)]
pub struct State {
    list: Vec<WorkflowInfo>,
    import_input: String,
    /// A package opened for review: the staged copy is what gets installed.
    preview: Option<StagedPackage>,
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

    /// The names the page shows (for the font fallback).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.list.iter().map(|w| w.name.as_str())
    }

    #[cfg(test)]
    pub fn list_len(&self) -> usize {
        self.list.len()
    }

    /// The wording of the open confirmation: the launcher's, cleaned and capped.
    fn prompt(&self) -> Option<crate::gui::importprompt::PromptText> {
        self.preview
            .as_ref()
            .map(|s| crate::gui::importprompt::prompt_text(s.preview()))
    }

    pub fn preview_done(&mut self, r: Result<StagedPackage, String>) {
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
        kit::section_title(ui, "Installed workflows", Some("workflow_dirs"));
        if self.workflows.list.is_empty() {
            hint(
                ui,
                "No workflows installed. Import a workflow package above, or point workflow_dirs at an existing folder of workflows.",
            );
        }
        let list = self.workflows.list.clone();
        show_msg(ui, &self.workflows.msg);
        ui.add_space(style::m::GAP);
        if !list.is_empty() {
            kit::rows(ui, |ui| {
                for w in &list {
                    self.workflow_row(ui, w);
                }
            });
        }
    }

    // ---------------------------------------------------------------- import

    fn import_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Import", |ui| {
            if self.workflows.preview.is_some() {
                self.import_prompt(ui);
                return;
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Choose workflow file…").clicked() {
                    self.choose_file(
                        Purpose::Workflow,
                        "Choose a workflow package",
                        &ashdash_core::workflow::WORKFLOW_PACKAGE_EXTENSIONS,
                    );
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.workflows.import_input)
                        .hint_text("or paste a file path")
                        .desired_width(160.0),
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
                "You can also drop a workflow package onto this window. Nothing is installed before you confirm.",
            );
        });
    }

    /// Unpack in a thread (size-limited) and show what the workflow can do.
    pub(super) fn start_preview(&mut self, path: PathBuf) {
        self.workflows.previewing = true;
        self.workflows.msg = None;
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = ashdash_core::workflow::stage_package(&paths, &path).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Task::Preview(Box::new(r)));
            ctx.request_repaint();
        });
    }

    /// The confirmation for the staged package: the same wording as the launcher's prompt
    /// (`importprompt::prompt_text`, which cleans everything the package wrote).
    fn import_prompt(&mut self, ui: &mut egui::Ui) {
        let Some(text) = self.workflows.prompt() else {
            return;
        };
        let (mut install, mut cancel) = (false, false);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(style::caps(&text.title, Palette::of(ui).text));
            for (i, l) in text.lines.iter().enumerate() {
                if l.is_empty() {
                    ui.add_space(4.0);
                } else if l.starts_with("Runs code") {
                    ui.label(error_text(ui, l).strong());
                } else if l.starts_with("This replaces") || l.starts_with("Settings saved") {
                    ui.label(warn_text(ui, l));
                } else if i + 1 == text.lines.len() {
                    hint(ui, l);
                } else {
                    ui.label(l);
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                install = kit::primary_button(ui, "Install").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if install {
            if let Some(staged) = self.workflows.preview.take() {
                self.workflows.msg = Some(match Self::install_workflow(staged) {
                    Ok(n) => Msg::ok(format!("Installed {n}.")),
                    Err(e) => Msg::err(format!("Install failed: {e}")),
                });
            }
            self.workflows.import_input.clear();
            self.workflows.refresh(&self.paths, &self.cfg);
        } else if cancel {
            self.workflows.preview = None; // dropping the staged copy deletes it
        }
    }

    /// Install the copy that was shown (the package file is not read again).
    fn install_workflow(staged: StagedPackage) -> Result<String, String> {
        let dest = staged.install().map_err(|e| format!("{e:#}"))?;
        Ok(dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default())
    }

    // ---------------------------------------------------------------- list

    fn workflow_row(&mut self, ui: &mut egui::Ui, w: &WorkflowInfo) {
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
        let key = sub.join(" · ");
        kit::row(ui, &w.name, Some(&key), false, |ui| {
            ui.horizontal(|ui| {
                if w.managed {
                    let mut on = !w.disabled;
                    if kit::toggle(ui, &mut on, &format!("{} enabled", w.name))
                        .on_hover_text("Enabled")
                        .changed()
                    {
                        self.workflows.msg = Some(match settings_view::set_workflow_enabled(&self.paths, &w.dir, on) {
                            Ok(()) => Msg::ok(format!("{} {}.", w.name, if on { "enabled" } else { "disabled" })),
                            Err(e) => Msg::err(format!("{e:#}")),
                        });
                        self.workflows.refresh(&self.paths, &self.cfg);
                    }
                } else {
                    ui.add_enabled_ui(false, |ui| kit::toggle(ui, &mut !w.disabled, &w.name));
                }
                ui.label(style::hint_text(
                    &Palette::of(ui),
                    if w.disabled { "off" } else { "on" },
                ));
            });
            ui.horizontal_wrapped(|ui| {
                if w.settings > 0 && ui.button("Configure…").clicked() {
                    self.open_workflow_settings(w);
                }
                if ui.button("What can it do?").clicked() {
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
                    ui.add(
                        egui::Label::new(if i == 0 {
                            RichText::new(l).strong()
                        } else {
                            RichText::new(format!("• {l}"))
                        })
                        .wrap(),
                    );
                }
            }
            if self.workflows.open_settings.as_deref() == Some(w.bundle_id.as_str()) && !w.bundle_id.is_empty() {
                self.settings_form(ui, w);
            }
        });
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
                ui.label(style::caps(label, Palette::of(ui).text));
                if !s.description.is_empty() {
                    hint(ui, &s.description);
                }
                let text = self.workflows.edits.entry(s.variable.clone()).or_default();
                match model::setting_control(&s.kind) {
                    SettingControl::Checkbox => {
                        let mut on = text.trim() == "1" || text.trim() == "true";
                        if kit::toggle(ui, &mut on, label).changed() {
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
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(280.0));
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
        let p = Palette::of(ui);
        kit::rows(ui, |ui| {
            for (k, v) in model::about_rows(&self.paths) {
                kit::row(ui, k, None, false, |ui| {
                    ui.add(egui::Label::new(style::mono(&p, &v, style::m::KEY + 1.0)).wrap());
                });
            }
        });
        ui.add_space(style::m::GAP_L);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Show data folder in Finder").clicked() {
                let _ = ashdash_core::engine::reveal(&self.paths.data);
            }
            if ui.button("Show settings file in Finder").clicked() {
                let _ = ashdash_core::engine::reveal(&self.paths.config_file());
            }
        });
        ui.add_space(style::m::GAP);
        hint(ui, "Ashdash is open source software.");
        hint(
            ui,
            "Settings are kept in config.json; `ashdash config keys` lists every setting.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::importprompt::is_format_char;
    use ashdash_core::format_ids::{kind, workflow_file};

    fn hostile_package(dir: &Path, name: &str, author: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(dir);
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>name</key><string>{name}</string><key>bundleid</key><string>t.prefs.hostile</string>
<key>createdby</key><string>{author}</string><key>objects</key><array>
<dict><key>uid</key><string>W</string><key>type</key><string>{ty}</string>
<key>config</key><dict><key>filename</key><string>~/.zshrc</string><key>type</key><integer>1</integer></dict></dict>
</array></dict></plist>"#,
            ty = kind::OUTPUT_WRITEFILE
        );
        let p = dir.join(workflow_file("h"));
        std::fs::write(
            &p,
            crate::gui::importprompt::test_zip::stored(&[("info.plist", plist.as_bytes())]),
        )
        .unwrap();
        p
    }

    #[test]
    fn the_prefs_prompt_is_cleaned_shows_the_author_and_never_calls_a_file_writer_harmless() {
        let root = std::env::temp_dir().join(format!("oa-prefs-wf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        let name = format!("Safe\u{202e}gnp.exe{}", "n".repeat(10_000));
        let pkg = hostile_package(&root, &name, "Ann\u{200b} Author\nBundle id: com.apple.x");
        let staged = ashdash_core::workflow::stage_package(&paths, &pkg).unwrap();

        let mut state = State::default();
        assert!(state.prompt().is_none());
        state.preview_done(Ok(staged));
        let text = state.prompt().expect("a package is under review");
        let all = text.plain();
        assert!(all.chars().all(|c| !is_format_char(c)), "{all:?}");
        assert!(text.title.chars().count() < 120, "{}", text.title.chars().count());
        assert!(all.chars().count() < 3000, "{}", all.chars().count());
        assert!(all.contains("Author: Ann Author Bundle id: com.apple.x"), "{all}");
        assert!(!all.contains("Runs no scripts and no terminal commands"), "{all}");
        assert!(all.contains("appends to ~/.zshrc"), "{all}");

        // cancelling drops the staged copy and removes it from disk
        assert_eq!(std::fs::read_dir(paths.data.join("import-staging")).unwrap().count(), 1);
        state.preview = None;
        assert_eq!(std::fs::read_dir(paths.data.join("import-staging")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installing_from_the_prefs_prompt_uses_the_staged_copy() {
        let root = std::env::temp_dir().join(format!("oa-prefs-wf2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        let pkg = hostile_package(&root, "Shown", "A");
        let staged = ashdash_core::workflow::stage_package(&paths, &pkg).unwrap();
        std::fs::remove_file(&pkg).unwrap(); // the file is gone; the staged copy is not
        let name = PrefsApp::install_workflow(staged).unwrap();
        assert_eq!(name, "user.workflow.t.prefs.hostile");
        assert!(paths.workflows_dir().join(name).join("info.plist").is_file());
        let _ = std::fs::remove_dir_all(root);
    }
}
