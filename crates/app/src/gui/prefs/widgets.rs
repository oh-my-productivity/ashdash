//! Small egui helpers shared by the preferences pages.

use eframe::egui::{self, Color32, RichText};

/// A one-line result shown next to the control that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub text: String,
    pub error: bool,
}

impl Msg {
    pub fn ok(text: impl Into<String>) -> Msg {
        Msg {
            text: text.into(),
            error: false,
        }
    }

    pub fn err(text: impl Into<String>) -> Msg {
        Msg {
            text: text.into(),
            error: true,
        }
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new(&self.text).color(if self.error { ERROR } else { GOOD }));
    }
}

pub const ERROR: Color32 = Color32::from_rgb(0xd0, 0x40, 0x40);
pub const WARN: Color32 = Color32::from_rgb(0xc8, 0x8a, 0x10);
pub const GOOD: Color32 = Color32::from_rgb(0x2e, 0x9b, 0x4e);

pub fn show_msg(ui: &mut egui::Ui, msg: &Option<Msg>) {
    if let Some(m) = msg {
        m.show(ui);
    }
}

/// A titled group with a little space above.
pub fn section<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(14.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.separator();
    body(ui)
}

pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak().small());
}

/// What the user did with a list row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    None,
    Remove(usize),
}

/// A list of strings with a ✕ button each, and an entry field to add one. Returns
/// `Some(text)` when the user pressed Add / Enter with `input`, and the row to remove, if any.
pub fn string_list(
    ui: &mut egui::Ui,
    id: &str,
    items: &[String],
    input: &mut String,
    hint_text: &str,
) -> (RowAction, Option<String>) {
    let mut action = RowAction::None;
    ui.push_id(id, |ui| {
        for (i, item) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                    action = RowAction::Remove(i);
                }
                ui.label(item);
            });
        }
    });
    let mut added = None;
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(input)
                .hint_text(hint_text)
                .desired_width(280.0),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || enter) && !input.trim().is_empty() {
            added = Some(input.clone());
        }
    });
    (action, added)
}

/// A button that needs a second click to count; returns true on the confirming click.
pub fn confirm_button(ui: &mut egui::Ui, armed: &mut bool, label: &str, confirm_label: &str) -> bool {
    if *armed {
        let mut fire = false;
        ui.horizontal(|ui| {
            if ui.button(RichText::new(confirm_label).color(ERROR)).clicked() {
                fire = true;
            }
            if ui.button("Cancel").clicked() {
                *armed = false;
            }
        });
        if fire {
            *armed = false;
        }
        fire
    } else {
        if ui.button(label).clicked() {
            *armed = true;
        }
        false
    }
}
