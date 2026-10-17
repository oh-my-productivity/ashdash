//! Small helpers shared by the preferences pages, built on `kit` and `style`.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

use super::style::{self, Palette};

/// How long a "Saved." style confirmation stays: it confirms the click and then makes room
/// again (the page is meant to fit the window). Errors stay until the cause is gone.
pub const OK_VISIBLE: Duration = Duration::from_secs(6);

/// A one-line result shown next to the control that caused it. Errors carry a glyph and text,
/// never colour alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

impl Msg {
    pub fn ok(text: impl Into<String>) -> Msg {
        Msg {
            text: text.into(),
            error: false,
            at: Instant::now(),
        }
    }

    pub fn err(text: impl Into<String>) -> Msg {
        Msg {
            text: text.into(),
            error: true,
            at: Instant::now(),
        }
    }

    /// Is the message still to be shown? Errors always; confirmations for [`OK_VISIBLE`].
    pub fn is_live(&self) -> bool {
        self.error || self.at.elapsed() < OK_VISIBLE
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        if !self.is_live() {
            return;
        }
        if !self.error {
            ui.ctx()
                .request_repaint_after(OK_VISIBLE.saturating_sub(self.at.elapsed()) + Duration::from_millis(50));
        }
        let p = Palette::of(ui);
        ui.horizontal_wrapped(|ui| {
            let (color, text) = if self.error {
                super::kit::warn_triangle(ui, p.error);
                (p.error, self.text.clone())
            } else {
                (p.ok_text, format!("✓ {}", self.text))
            };
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        });
    }
}

/// Is there an error message to show? (Outlines the row.)
pub fn is_error(msg: &Option<Msg>) -> bool {
    msg.as_ref().is_some_and(|m| m.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmations_expire_and_errors_do_not() {
        let mut ok = Msg::ok("Saved.");
        let mut err = Msg::err("no");
        assert!(ok.is_live() && err.is_live());
        let old = Instant::now() - OK_VISIBLE - Duration::from_secs(1);
        ok.at = old;
        err.at = old;
        assert!(!ok.is_live(), "a confirmation makes room again");
        assert!(err.is_live(), "an error stays until its cause is fixed");
    }
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
    RichText::new(text).color(Palette::of(ui).warn_text)
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
