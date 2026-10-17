//! Small helpers shared by the preferences pages, built on `kit` and `style`.

use eframe::egui::{self, RichText};

use super::style::{self, Palette};

/// A one-line result shown next to the control that caused it. Errors carry a glyph and text,
/// never colour alone.
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
        let p = Palette::of(ui);
        let (glyph, color) = if self.error { ("⚠", p.error) } else { ("✓", p.ok) };
        ui.add(egui::Label::new(RichText::new(format!("{glyph} {}", self.text)).color(color)).wrap());
    }
}

/// Is there an error message to show? (Outlines the row.)
pub fn is_error(msg: &Option<Msg>) -> bool {
    msg.as_ref().is_some_and(|m| m.error)
}

pub fn show_msg(ui: &mut egui::Ui, msg: &Option<Msg>) {
    if let Some(m) = msg {
        m.show(ui);
    }
}

/// Error-coloured text without a glyph (for inline emphasis inside a block).
pub fn error_text(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text).color(Palette::of(ui).error)
}

pub fn warn_text(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text).color(Palette::of(ui).warn)
}

/// A titled group of free-form content (a card under a small-caps title).
pub fn section<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    super::kit::section(ui, title, body)
}

pub fn hint(ui: &mut egui::Ui, text: &str) {
    let p = Palette::of(ui);
    ui.add(egui::Label::new(style::hint_text(&p, text)).wrap());
}

/// What the user did with a list row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    None,
    Remove(usize),
}

/// A list of strings with a × button each, and an entry field to add one. Returns
/// `Some(text)` when the user pressed Add / Enter with `input`, and the row to remove, if any.
pub fn string_list(
    ui: &mut egui::Ui,
    id: &str,
    items: &[String],
    input: &mut String,
    hint_text: &str,
) -> (RowAction, Option<String>) {
    let p = Palette::of(ui);
    let mut action = RowAction::None;
    ui.push_id(id, |ui| {
        for (i, item) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new("×").color(p.dim)).frame(false))
                    .on_hover_text("Remove")
                    .clicked()
                {
                    action = RowAction::Remove(i);
                }
                ui.label(style::mono(&p, item, style::m::KEY + 1.0));
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
        let p = Palette::of(ui);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(confirm_label).color(p.error)).clicked() {
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
