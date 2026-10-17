//! Permissions page: what macOS has granted Ashdash, what each grant is for, and a button that
//! opens the right pane of System Settings. The probes are silent (see `mac::permissions`):
//! looking at this page never makes macOS ask for anything.

use eframe::egui::{self, Layout, Align, vec2};

use super::app::PrefsApp;
use super::kit::{self, Tone};
use super::style::{self, Palette, m};
use super::summary;
use crate::mac::permissions::{AUTOMATION_TARGETS, Grant, Pane};

/// Colour of the light for each answer: only a refusal is a warning; "never asked" and "cannot
/// tell" are neutral (an outlined light), because nothing is wrong yet.
pub fn tone_of(grant: Option<Grant>) -> Tone {
    match grant {
        Some(Grant::Granted) => Tone::Ok,
        Some(Grant::NotGranted) => Tone::Warn,
        Some(Grant::AskedOnFirstUse) | Some(Grant::Unknown) | None => Tone::Off,
    }
}

/// The word beside the light; `None` = the first check has not finished.
pub fn word_of(grant: Option<Grant>) -> &'static str {
    grant.map_or("Checking…", Grant::word)
}

/// Width reserved for the word so the descriptions line up.
const WORD_W: f32 = 124.0;

impl PrefsApp {
    pub(super) fn permissions_page(&mut self, ui: &mut egui::Ui) {
        let snap = self.perms.clone();
        let p = Palette::of(ui);
        let mut open: Option<Pane> = None;
        let mut recheck = false;
        kit::rows(ui, |ui| {
            kit::row(ui, "Accessibility", None, false, |ui| {
                let g = snap.as_ref().map(|s| s.accessibility);
                top_line(ui, g, None, |ui| open_button(ui, Pane::Accessibility, &mut open));
                ui.label(style::hint_text(&p, "Needed for auto-paste and keystrokes."));
            });
            kit::row(ui, "Automation", None, false, |ui| {
                for (i, t) in AUTOMATION_TARGETS.iter().enumerate() {
                    let g = snap.as_ref().and_then(|s| s.automation.get(i).copied());
                    let what = if g == Some(Grant::Unknown) {
                        format!("{} · {} is not running, so it cannot be checked", t.used_for, t.name)
                    } else {
                        t.used_for.to_string()
                    };
                    let name = t.name;
                    let text = format!("{name} · {what}");
                    top_line(ui, g, Some(&text), |ui| {
                        if i == 0 {
                            open_button(ui, Pane::Automation, &mut open);
                        }
                    });
                }
            });
            kit::row(ui, "Full Disk Access", None, false, |ui| {
                let g = snap.as_ref().map(|s| s.full_disk_access);
                top_line(ui, g, None, |ui| open_button(ui, Pane::FullDiskAccess, &mut open));
                ui.label(style::hint_text(&p, "Safari bookmarks (bm)."));
                if g == Some(Grant::NotGranted) {
                    ui.label(style::hint_text(
                        &p,
                        "After granting it, quit and reopen Ashdash: macOS applies it to new processes.",
                    ));
                }
            });
        });
        ui.add_space(m::GAP_L);
        ui.horizontal(|ui| {
            if ui.button("Re-check  ⌘R").clicked() {
                recheck = true;
            }
            let when = if self.perms_checking {
                "checking…".to_string()
            } else if self.perms_at > 0 {
                format!("checked {}", summary::local_hms(self.perms_at))
            } else {
                String::new()
            };
            ui.label(style::mono_dim(&p, &when, m::KEY));
        });
        ui.add_space(m::GAP);
        ui.label(style::hint_text(
            &p,
            "Looking at this page never makes macOS show a prompt. It asks the first time a feature needs a permission.",
        ));
        if let Some(pane) = open {
            self.open_pane(pane);
        }
        if recheck {
            self.check_permissions();
        }
    }
}

/// `■ Word  [text]` on the left and `right` (a button) at the right edge.
fn top_line(ui: &mut egui::Ui, grant: Option<Grant>, text: Option<&str>, right: impl FnOnce(&mut egui::Ui)) {
    let p = Palette::of(ui);
    egui::containers::Sides::new().shrink_left().show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                kit::light(ui, tone_of(grant));
                ui.allocate_ui_with_layout(vec2(WORD_W, m::CONTROL_H - 8.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.label(style::mono(&p, word_of(grant), m::KEY));
                });
                if let Some(text) = text {
                    ui.add(egui::Label::new(egui::RichText::new(text).size(m::SMALL).color(p.text)).wrap());
                }
            });
        },
        right,
    );
}

fn open_button(ui: &mut egui::Ui, pane: Pane, open: &mut Option<Pane>) {
    if ui.button("Open System Settings").clicked() {
        *open = Some(pane);
    }
}
