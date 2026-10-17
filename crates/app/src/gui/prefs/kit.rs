//! The shared components of the preferences window (V4 "Instrument"): sidebar item, page header,
//! setting row (label + mono key | control), toggle, stepper, buttons, shortcut recorder, card,
//! status strip with square lights, focus ring and the row error state. Colours and sizes come
//! from `style`; pages never hard-code either.

use std::ops::RangeInclusive;

use eframe::egui::{
    self, Align, Align2, Button, Color32, CornerRadius, Frame, Layout, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};

use super::nav::Nav;
use super::style::{self, Palette, m};

fn radius() -> CornerRadius {
    CornerRadius::same(m::RADIUS)
}

/// The keyboard focus ring: a 2 px outline just outside `rect`, only while `resp` has focus.
pub fn focus_ring(ui: &Ui, resp: &Response, rect: Rect) {
    if resp.has_focus() {
        let p = Palette::of(ui);
        ui.painter().rect_stroke(
            rect.expand(2.0),
            radius(),
            Stroke::new(m::STROKE_STRONG, p.accent_text),
            StrokeKind::Outside,
        );
    }
}

// ------------------------------------------------------------------ sidebar

/// One sidebar entry: small caps name, `⌘n` in mono on the right, and for the active entry a
/// 3 px orange bar on the left edge plus brighter, medium-weight text (so the state is not
/// carried by colour alone).
pub fn nav_item(ui: &mut Ui, nav: Nav, active: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), m::NAV_ITEM_H), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), active, nav.title()));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, 0.0, p.nav_hover);
            painter.rect_filled(
                Rect::from_min_size(rect.min, vec2(m::NAV_BAR_W, rect.height())),
                0.0,
                p.accent,
            );
        } else if resp.hovered() {
            painter.rect_filled(rect, 0.0, p.nav_hover);
        }
        let name_color = if active { p.text } else { p.dim };
        let mut name = style::caps(nav.title(), name_color);
        if !active {
            name = name.font(egui::FontId::proportional(m::LABEL));
        }
        let galley = egui::WidgetText::from(name).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(pos2(rect.min.x + 18.0, y), galley, name_color);
        painter.text(
            pos2(rect.max.x - 14.0, rect.center().y),
            Align2::RIGHT_CENTER,
            nav.shortcut_label(),
            style::mono_font(m::KEY),
            p.dim,
        );
        focus_ring(ui, &resp, rect.shrink(2.0));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The "Go to a setting…  ⌘K" field. Returns its response; `⌘K` is drawn inside while empty.
pub fn search_field(ui: &mut Ui, text: &mut String, id: egui::Id) -> Response {
    let p = Palette::of(ui);
    let width = ui.available_width();
    let resp = ui.add_sized(
        [width, m::SEARCH_H],
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(RichText::new("Go to a setting…").size(m::SMALL).color(p.dim))
            .margin(egui::Margin {
                left: 8,
                right: 30,
                top: 4,
                bottom: 4,
            })
            .font(egui::FontId::proportional(m::SMALL)),
    );
    if text.is_empty() {
        ui.painter().text(
            pos2(resp.rect.max.x - 8.0, resp.rect.center().y),
            Align2::RIGHT_CENTER,
            "⌘K",
            style::mono_font(m::KEY),
            p.dim,
        );
    }
    resp
}

// ------------------------------------------------------------------ page header

/// Page title (large, letter-spaced caps), subtitle under it in dim small caps, and optional dim
/// monospace text on the right (e.g. the settings file path).
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, right: Option<&str>) {
    let p = Palette::of(ui);
    ui.add_space(m::HEADER_PAD_Y);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(style::title(&p, title));
            ui.label(style::caps_dim(&p, subtitle, m::SUBTITLE, m::SUBTITLE_SPACING));
        });
        if let Some(right) = right {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                ui.label(style::key(&p, right));
            });
        }
    });
    ui.add_space(m::HEADER_PAD_Y);
}

// ------------------------------------------------------------------ rows

/// Draw a grid of setting rows: a hairline on top, and each [`row`] adds its own sides and
/// bottom so neighbours share one line.
pub fn rows<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::of(ui);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let y = ui.cursor().min.y;
        let x = ui.available_rect_before_wrap().x_range();
        ui.painter().hline(x, y + 0.5, Stroke::new(m::STROKE, p.hairline));
        ui.add_space(1.0);
        add(ui)
    })
    .inner
}

/// One setting: small-caps label with the config key in grey mono under it, then the control.
/// `key` is `None` for settings that are not in `config.json` (say so in `note` inside the
/// control). `error` outlines the row in the error colour.
pub fn row<R>(ui: &mut Ui, label: &str, key: Option<&str>, error: bool, control: impl FnOnce(&mut Ui) -> R) -> R {
    row_with_note(ui, label, key, None, error, control)
}

/// [`row`] with one more dim mono line under the key in the label cell (e.g. the keys that work
/// while the shortcut recorder is listening). Keep it to one short line.
pub fn row_with_note<R>(
    ui: &mut Ui,
    label: &str,
    key: Option<&str>,
    note: Option<&str>,
    error: bool,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    let p = Palette::of(ui);
    let width = ui.available_width();
    let pad = m::ROW_PAD;
    let top_left = ui.cursor().min;
    let (left_h, right_h, out) = {
        let mut left_h = 0.0;
        let mut right_h = 0.0;
        let mut out = None;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let left = ui.allocate_ui_with_layout(vec2(m::LABEL_COL_W, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(m::LABEL_COL_W);
                Frame::new().inner_margin(style::margin(pad, pad)).show(ui, |ui| {
                    ui.set_width(m::LABEL_COL_W - 2.0 * pad);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add(egui::Label::new(style::caps(label, p.text)));
                    if let Some(key) = key {
                        ui.add(egui::Label::new(style::key(&p, key)).wrap());
                    }
                    if let Some(note) = note {
                        ui.add(egui::Label::new(style::mono_dim(&p, note, m::NOTE)).wrap());
                    }
                });
            });
            left_h = left.response.rect.height();
            let right_w = (width - m::LABEL_COL_W).max(120.0);
            let right = ui.allocate_ui_with_layout(vec2(right_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(right_w);
                Frame::new().inner_margin(style::margin(pad, pad)).show(ui, |ui| {
                    ui.set_width(right_w - 2.0 * pad);
                    ui.spacing_mut().item_spacing = vec2(m::GAP, m::GAP);
                    // single-line controls sit level with the label, not centred in a taller line
                    ui.spacing_mut().interact_size.y = m::TOGGLE_H;
                    out = Some(control(ui));
                });
            });
            right_h = right.response.rect.height();
        });
        (left_h, right_h, out)
    };
    let h = left_h.max(right_h).max(m::ROW_MIN_H);
    let used = left_h.max(right_h);
    if used < h {
        ui.add_space(h - used);
    }
    let rect = Rect::from_min_size(top_left, vec2(width, h));
    let painter = ui.painter();
    let line = Stroke::new(m::STROKE, p.hairline);
    painter.vline(rect.min.x + 0.5, rect.y_range(), line);
    painter.vline(rect.max.x - 0.5, rect.y_range(), line);
    painter.vline(rect.min.x + m::LABEL_COL_W, rect.y_range(), line);
    painter.hline(rect.x_range(), rect.max.y - 0.5, line);
    if error {
        painter.rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(m::STROKE_MID, p.error),
            StrokeKind::Inside,
        );
    }
    out.expect("row control ran")
}

// ------------------------------------------------------------------ section header + card

/// A titled block for free-form content (lists, tables, forms): small-caps title with a
/// hairline, content inside a card.
pub fn section<R>(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::of(ui);
    ui.add_space(m::GAP_L);
    ui.label(style::caps(title, p.text));
    ui.add_space(4.0);
    card(ui, body)
}

/// Title of a block of rows: small caps on the left, the config key it edits (grey mono) on
/// the right.
pub fn section_title(ui: &mut Ui, title: &str, key: Option<&str>) {
    let p = Palette::of(ui);
    ui.add_space(m::GAP_L);
    ui.horizontal(|ui| {
        ui.label(style::caps(title, p.text));
        if let Some(key) = key {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(style::key(&p, key));
            });
        }
    });
    ui.add_space(6.0);
}

/// A panel-coloured block with a hairline border.
pub fn card<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.bg)
        .stroke(Stroke::new(m::STROKE, p.hairline))
        .corner_radius(radius())
        .inner_margin(style::margin(m::ROW_PAD, m::ROW_PAD))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        })
        .inner
}

/// A card with an optional small-caps title, lines of dim text, and room for a button on the right
/// (the RELOAD and attention cards of the General page). The text wraps; the button keeps its size.
pub fn info_card(ui: &mut Ui, title: Option<&str>, tone: Tone, lines: &[String], right: impl FnOnce(&mut Ui)) {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.bg)
        .stroke(Stroke::new(m::STROKE, tone.color(&p).unwrap_or(p.hairline)))
        .corner_radius(radius())
        .inner_margin(style::margin(m::ROW_PAD, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::containers::Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.horizontal_top(|ui| {
                        light(ui, tone);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            if let Some(title) = title {
                                ui.label(style::caps(title, p.text));
                            }
                            for l in lines {
                                let t = if title.is_some() {
                                    style::hint_text(&p, l)
                                } else {
                                    RichText::new(l).size(m::SMALL).color(p.text)
                                };
                                ui.add(egui::Label::new(t).wrap());
                            }
                        });
                    });
                },
                right,
            );
        });
}

// ------------------------------------------------------------------ status

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Error,
    Off,
}

impl Tone {
    pub fn color(self, p: &Palette) -> Option<Color32> {
        match self {
            Tone::Ok => Some(p.ok),
            Tone::Warn => Some(p.warn),
            Tone::Error => Some(p.error),
            Tone::Off => None,
        }
    }
}

/// A painted warning triangle with an exclamation mark (not a font glyph), 12 pt.
pub fn warn_triangle(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let c = rect.center();
    let tri = vec![
        pos2(c.x, rect.min.y + 1.0),
        pos2(rect.max.x - 0.5, rect.max.y - 1.5),
        pos2(rect.min.x + 0.5, rect.max.y - 1.5),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        tri,
        Color32::TRANSPARENT,
        Stroke::new(1.5, color),
    ));
    ui.painter()
        .line_segment([pos2(c.x, c.y - 1.5), pos2(c.x, c.y + 1.5)], Stroke::new(1.5, color));
    ui.painter().circle_filled(pos2(c.x, c.y + 3.8), 0.9, color);
}

/// A square status light (never colour alone: the text beside it says the same).
pub fn light(ui: &mut Ui, tone: Tone) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(m::STATUS_LIGHT, m::STATUS_LIGHT + 6.0), Sense::hover());
    let sq = Rect::from_min_size(
        pos2(rect.min.x, rect.min.y + 5.0),
        vec2(m::STATUS_LIGHT, m::STATUS_LIGHT),
    );
    match tone.color(&p) {
        Some(c) => ui.painter().rect_filled(sq, 1.0, c),
        None => ui
            .painter()
            .rect_stroke(sq, 1.0, Stroke::new(m::STROKE, p.dim), StrokeKind::Inside),
    };
}

/// One cell of the bottom strip.
pub struct StatusItem {
    pub label: &'static str,
    pub value: String,
    pub tone: Tone,
}

/// The bottom STATUS strip: "STATUS" then `■ LABEL value` cells separated by hairlines.
pub fn status_strip(ui: &mut Ui, items: &[StatusItem]) {
    let p = Palette::of(ui);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add_space(m::GAP_L);
        ui.label(style::caps_dim(&p, "Status", m::STATUS, m::STATUS_SPACING));
        for it in items {
            ui.add_space(6.0);
            let x = ui.cursor().min.x;
            let r = ui.max_rect();
            ui.painter()
                .vline(x, (r.min.y + 6.0)..=(r.max.y - 6.0), Stroke::new(m::STROKE, p.hairline));
            ui.add_space(8.0);
            light(ui, it.tone);
            ui.label(
                RichText::new(it.label.to_uppercase())
                    .font(style::medium_font(m::STATUS))
                    .extra_letter_spacing(m::STATUS_SPACING)
                    .color(p.text),
            );
            ui.label(
                RichText::new(&it.value)
                    .font(style::mono_font(m::STATUS + 1.0))
                    .color(p.dim),
            );
            // the light is a colour: the triangle says "look at this" without it
            if matches!(it.tone, Tone::Warn | Tone::Error) {
                warn_triangle(ui, it.tone.color(&p).unwrap_or(p.warn));
            }
        }
    });
}

// ------------------------------------------------------------------ controls

/// A plain toggle: orange pill with a white thumb when on, outlined when off; no "ON" label.
/// `name` is what a screen reader says.
pub fn toggle(ui: &mut Ui, on: &mut bool, name: &str) -> Response {
    let p = Palette::of(ui);
    let enabled = ui.is_enabled();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(m::TOGGLE_W, m::TOGGLE_H), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, value, name));
    if ui.is_rect_visible(rect) {
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
        let pill = CornerRadius::same((m::TOGGLE_H / 2.0) as u8);
        let (fill, stroke) = if value {
            (p.accent, p.accent)
        } else {
            (p.field, if resp.hovered() { p.dim } else { p.border })
        };
        ui.painter().rect(
            rect,
            pill,
            fade(fill),
            Stroke::new(m::STROKE, fade(stroke)),
            StrokeKind::Inside,
        );
        let r = m::TOGGLE_H / 2.0 - 3.5;
        let cx = if value {
            rect.max.x - r - 3.5
        } else {
            rect.min.x + r + 3.5
        };
        let thumb = if value { p.thumb } else { p.dim };
        ui.painter().circle_filled(pos2(cx, rect.center().y), r, fade(thumb));
        focus_ring(ui, &resp, rect);
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// `− value +` in one bordered group. The value is a drag/typed number in the mono font. The
/// returned response is `changed()` when the value changed; for the buttons that is a finished
/// edit, for dragging use `drag_stopped()` as usual.
pub fn stepper(ui: &mut Ui, value: &mut usize, range: RangeInclusive<usize>) -> Response {
    let p = Palette::of(ui);
    let (lo, hi) = (*range.start(), *range.end());
    let size = vec2(2.0 * m::STEPPER_BTN_W + m::STEPPER_VALUE_W, m::STEPPER_H);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        radius(),
        p.field,
        Stroke::new(m::STROKE, p.border),
        StrokeKind::Inside,
    );
    let minus = Rect::from_min_size(rect.min, vec2(m::STEPPER_BTN_W, rect.height()));
    let plus = Rect::from_min_size(
        pos2(rect.max.x - m::STEPPER_BTN_W, rect.min.y),
        vec2(m::STEPPER_BTN_W, rect.height()),
    );
    let mid = Rect::from_min_max(pos2(minus.max.x, rect.min.y), pos2(plus.min.x, rect.max.y));
    let mut changed = false;
    for (r, glyph, delta, name) in [(minus, "−", -1_i64, "Decrease"), (plus, "+", 1, "Increase")] {
        let resp = ui.interact(r, ui.id().with(glyph), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), name));
        let can = (delta < 0 && *value > lo) || (delta > 0 && *value < hi);
        if resp.hovered() && can {
            ui.painter().rect_filled(r.shrink(1.0), 0.0, p.hover);
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            style::mono_font(20.0),
            if can { p.text } else { p.dim.gamma_multiply(0.5) },
        );
        if resp.clicked() && can {
            let next = (*value as i64 + delta).clamp(lo as i64, hi as i64);
            *value = next as usize;
            changed = true;
        }
        focus_ring(ui, &resp, r.shrink(1.0));
    }
    ui.painter()
        .vline(mid.min.x, rect.y_range(), Stroke::new(m::STROKE, p.hairline));
    ui.painter()
        .vline(mid.max.x, rect.y_range(), Stroke::new(m::STROKE, p.hairline));
    let mut resp = ui
        .scope_builder(UiBuilder::new().max_rect(mid.shrink2(vec2(2.0, 0.0))), |ui| {
            let vis = ui.visuals_mut();
            for w in [
                &mut vis.widgets.inactive,
                &mut vis.widgets.hovered,
                &mut vis.widgets.active,
                &mut vis.widgets.noninteractive,
            ] {
                w.bg_fill = Color32::TRANSPARENT;
                w.weak_bg_fill = Color32::TRANSPARENT;
                w.bg_stroke = Stroke::NONE;
            }
            ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Monospace, style::mono_font(m::BIG_NUMBER));
            ui.centered_and_justified(|ui| ui.add(egui::DragValue::new(value).range(lo..=hi).speed(0.2)))
                .inner
        })
        .inner;
    if changed {
        resp.mark_changed();
    }
    resp
}

/// A filled call-to-action button (Install, Save).
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let p = Palette::of(ui);
    ui.add(
        Button::new(
            RichText::new(text)
                .color(if p.dark { p.bg } else { Color32::WHITE })
                .strong(),
        )
        .fill(if p.dark { p.accent } else { p.accent_text })
        .stroke(Stroke::NONE),
    )
}

/// What the recorder box shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderView<'a> {
    /// The current shortcut, large and mono.
    Idle { shortcut: &'a str },
    /// Listening: an orange outline, the prompt, and the shortcut it replaces (`was`).
    Recording { was: &'a str },
}

/// The shortcut recorder: **one box**. Idle it shows the current shortcut; Click / Return / Space
/// on the focused box starts recording (the caller decides, from the response). While recording
/// it has a 1.5 pt orange outline, "Press the new shortcut…" and "was ⌥⇧Space" right-aligned
/// (on the second line when the box is too narrow for both).
pub fn recorder_box(ui: &mut Ui, id: egui::Id, view: RecorderView<'_>) -> Response {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), m::RECORDER_H), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let listening = matches!(view, RecorderView::Recording { .. });
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Launcher shortcut"));
    let (stroke, fill) = if listening {
        (Stroke::new(m::STROKE_MID, p.accent), p.bg)
    } else {
        (
            Stroke::new(m::STROKE, if resp.hovered() { p.dim } else { p.border }),
            p.field,
        )
    };
    ui.painter().rect(rect, radius(), fill, stroke, StrokeKind::Inside);
    match view {
        RecorderView::Idle { shortcut } => {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                shortcut,
                style::mono_font(m::RECORDER_TEXT),
                p.text,
            );
        }
        RecorderView::Recording { was } => {
            let inner = rect.shrink2(vec2(m::GAP + 2.0, 0.0));
            let painter = ui.painter();
            let prompt = painter.layout_no_wrap("Press the new shortcut…".into(), style::mono_font(m::BODY), p.text);
            let was_galley = painter.layout_no_wrap(format!("was {was}"), style::mono_font(m::KEY), p.dim);
            let fits = prompt.size().x + was_galley.size().x + 2.0 * m::GAP <= inner.width();
            let (y1, y2) = if fits {
                (rect.center().y, rect.center().y)
            } else {
                (rect.center().y - 9.0, rect.center().y + 10.0)
            };
            painter.galley(pos2(inner.min.x, y1 - prompt.size().y / 2.0), prompt, p.text);
            painter.galley(
                pos2(inner.max.x - was_galley.size().x, y2 - was_galley.size().y / 2.0),
                was_galley,
                p.dim,
            );
        }
    }
    focus_ring(ui, &resp, rect);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ own title bar

/// The window's own title bar: the native title is hidden (the traffic lights stay), so this
/// strip is what sits next to them. "Ashdash" centred, and the whole strip drags the window;
/// a double click zooms it like a native title bar.
pub fn title_bar(ui: &mut Ui, title: &str) {
    let p = Palette::of(ui);
    let rect = ui.max_rect();
    let resp = ui.interact(rect, ui.id().with("titlebar-drag"), Sense::click_and_drag());
    ui.painter()
        .hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(m::STROKE, p.hairline));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        title,
        style::medium_font(m::BODY),
        p.text,
    );
    let ctx = ui.ctx();
    if resp.double_clicked() {
        let zoomed = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!zoomed));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: impl FnMut(&mut Ui)) {
        let ctx = egui::Context::default();
        super::super::style::install(&ctx);
        super::super::fonts::install(&ctx);
        let mut f = f;
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| f(ui));
            out.textures_delta.clear();
        }
    }

    #[test]
    fn components_draw_in_both_themes_without_panicking() {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let mut on = true;
            let mut n = 30usize;
            run(|ui| {
                ui.ctx().set_theme(theme);
                page_header(ui, "General", "Shortcut", Some("/tmp/config.json"));
                rows(ui, |ui| {
                    row(ui, "Theme", Some("theme"), false, |ui| {
                        toggle(ui, &mut on, "x");
                    });
                    row(ui, "Results per search", Some("max_results"), true, |ui| {
                        stepper(ui, &mut n, 1..=50);
                    });
                });
                recorder_box(ui, egui::Id::new("rec"), RecorderView::Recording { was: "⌥⇧Space" });
                recorder_box(
                    ui,
                    egui::Id::new("rec2"),
                    RecorderView::Idle {
                        shortcut: "⌥⇧Space"
                    },
                );
                status_strip(
                    ui,
                    &[StatusItem {
                        label: "Config",
                        value: "applied".into(),
                        tone: Tone::Ok,
                    }],
                );
                info_card(ui, Some("Reload"), Tone::Warn, &["a".into()], |ui| {
                    primary_button(ui, "Fix…");
                });
                for nav in Nav::ALL {
                    nav_item(ui, nav, nav == Nav::General);
                }
            });
        }
    }

    #[test]
    fn the_single_line_row_labels_fit_the_label_column() {
        let mut widths = Vec::new();
        run(|ui| {
            widths.clear();
            for label in ["Launcher shortcut", "Results per search", "Launch at login", "Theme"] {
                let g = egui::WidgetText::from(style::caps(label, Color32::WHITE)).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                );
                widths.push((label, g.size().x));
            }
        });
        let room = m::LABEL_COL_W - 2.0 * m::ROW_PAD;
        for (label, w) in widths {
            assert!(w <= room, "{label} needs {w} pt but the cell has {room}");
        }
    }

    #[test]
    fn a_row_is_at_least_the_minimum_height_and_grows_with_its_control() {
        let mut heights = Vec::new();
        run(|ui| {
            heights.clear();
            rows(ui, |ui| {
                for tall in [false, true] {
                    let before = ui.cursor().min.y;
                    row(ui, "A", None, false, |ui| {
                        ui.add_space(if tall { 100.0 } else { 1.0 });
                    });
                    heights.push(ui.cursor().min.y - before);
                }
            });
        });
        assert!(heights[0] >= m::ROW_MIN_H - 0.5, "{heights:?}");
        assert!(heights[1] > heights[0] + 40.0, "{heights:?}");
    }

    #[test]
    fn the_toggle_flips_on_click_and_reports_a_change() {
        let ctx = egui::Context::default();
        let mut on = false;
        let mut changed = false;
        let mut center = None;
        for step in 0..4 {
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0))),
                ..Default::default()
            };
            if let Some(c) = center {
                match step {
                    1 => raw.events.push(egui::Event::PointerMoved(c)),
                    2 => raw.events.push(egui::Event::PointerButton {
                        pos: c,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    }),
                    3 => raw.events.push(egui::Event::PointerButton {
                        pos: c,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    }),
                    _ => {}
                }
            }
            let mut out = ctx.run_ui(raw, |ui| {
                let r = toggle(ui, &mut on, "t");
                center = Some(r.rect.center());
                changed |= r.changed();
            });
            out.textures_delta.clear();
        }
        assert!(on && changed);
    }
}
