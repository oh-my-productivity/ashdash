//! Design tokens of the preferences window ("V4 Instrument"): every colour, size, spacing,
//! radius and stroke width lives in this file, for dark and light, and nothing else in
//! `prefs/` hard-codes one. The window follows the system appearance; `ASHDASH_PREFS_THEME`
//! (`dark` | `light`) forces one for screenshots.
//!
//! The launcher window has its own look (`gui/style.rs`) and is not touched by this file.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, Theme, vec2};

// ------------------------------------------------------------------ colours

/// One complete colour set. All text/background pairs the window uses are checked against
/// WCAG AA by the tests at the bottom of this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    /// Content background.
    pub bg: Color32,
    /// Sidebar, cards and the status strip.
    pub panel: Color32,
    /// Fill of buttons, inputs and the stepper.
    pub control: Color32,
    /// Fill of a control under the pointer.
    pub control_hover: Color32,
    /// Grid lines and separators.
    pub hairline: Color32,
    /// Outline of buttons and inputs.
    pub border: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text: config keys, hints, units.
    pub dim: Color32,
    /// The signal colour as a fill (toggle on, active-item bar, recorder outline).
    pub accent: Color32,
    /// The signal colour as text on `bg`/`panel`.
    pub accent_text: Color32,
    /// Toggle thumb.
    pub thumb: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    bg: rgb(0x14, 0x14, 0x14),
    panel: rgb(0x1b, 0x1b, 0x1b),
    control: rgb(0x22, 0x22, 0x22),
    control_hover: rgb(0x2b, 0x2b, 0x2b),
    hairline: rgb(0x2c, 0x2c, 0x2c),
    border: rgb(0x44, 0x44, 0x44),
    text: rgb(0xed, 0xed, 0xed),
    dim: rgb(0x9a, 0x9a, 0x9a),
    accent: rgb(0xff, 0x5f, 0x1f),
    accent_text: rgb(0xff, 0x7a, 0x45),
    thumb: rgb(0xff, 0xff, 0xff),
    ok: rgb(0x4c, 0xc3, 0x8a),
    warn: rgb(0xf5, 0xa6, 0x23),
    error: rgb(0xff, 0x6b, 0x6b),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    bg: rgb(0xf6, 0xf6, 0xf4),
    panel: rgb(0xec, 0xec, 0xe8),
    control: rgb(0xff, 0xff, 0xff),
    control_hover: rgb(0xf0, 0xf0, 0xec),
    hairline: rgb(0xd6, 0xd6, 0xd0),
    border: rgb(0xa8, 0xa8, 0xa0),
    text: rgb(0x17, 0x17, 0x17),
    dim: rgb(0x5a, 0x5a, 0x5a),
    accent: rgb(0xe0, 0x4a, 0x0c),
    accent_text: rgb(0xb3, 0x3c, 0x08),
    thumb: rgb(0xff, 0xff, 0xff),
    ok: rgb(0x1d, 0x7a, 0x4b),
    warn: rgb(0x8a, 0x58, 0x00),
    error: rgb(0xb3, 0x26, 0x1e),
};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Palette {
    pub fn of(ui: &egui::Ui) -> Palette {
        Palette::for_dark(ui.visuals().dark_mode)
    }

    pub fn for_dark(dark: bool) -> Palette {
        if dark { DARK } else { LIGHT }
    }
}

// ------------------------------------------------------------------ metrics

/// Sizes in points. The window is 760 x 560.
pub mod m {
    pub const WINDOW: [f32; 2] = [760.0, 560.0];
    pub const WINDOW_MIN: [f32; 2] = [640.0, 480.0];

    pub const SIDEBAR_W: f32 = 188.0;
    pub const NAV_ITEM_H: f32 = 32.0;
    /// Width of the orange bar on the active nav item.
    pub const NAV_BAR_W: f32 = 3.0;
    pub const SEARCH_H: f32 = 28.0;

    pub const PAGE_PAD_X: f32 = 24.0;
    pub const HEADER_PAD_Y: f32 = 16.0;

    /// Width of the label cell of a setting row; the control cell takes the rest.
    pub const LABEL_COL_W: f32 = 200.0;
    pub const ROW_PAD: f32 = 14.0;
    pub const ROW_MIN_H: f32 = 56.0;

    pub const CONTROL_H: f32 = 30.0;
    pub const STEPPER_BTN_W: f32 = 32.0;
    pub const STEPPER_VALUE_W: f32 = 64.0;
    pub const TOGGLE_W: f32 = 36.0;
    pub const TOGGLE_H: f32 = 20.0;
    pub const STATUS_H: f32 = 30.0;
    pub const STATUS_LIGHT: f32 = 8.0;

    /// Corner radius of controls, cards and rows.
    pub const RADIUS: u8 = 3;
    /// Hairlines, control outlines.
    pub const STROKE: f32 = 1.0;
    /// Recorder outline and focus ring.
    pub const STROKE_STRONG: f32 = 2.0;
    pub const GAP: f32 = 8.0;
    pub const GAP_L: f32 = 16.0;

    // type scale
    pub const TITLE: f32 = 22.0;
    pub const TITLE_SPACING: f32 = 3.0;
    pub const LABEL: f32 = 11.5;
    pub const LABEL_SPACING: f32 = 1.4;
    pub const KEY: f32 = 11.5;
    pub const BODY: f32 = 13.5;
    pub const SMALL: f32 = 11.5;
    pub const STATUS: f32 = 10.5;
    pub const STATUS_SPACING: f32 = 1.4;
    pub const BIG_NUMBER: f32 = 17.0;
}

// ------------------------------------------------------------------ fonts

/// Inter Medium: the family used for caps labels, titles and the active nav item.
pub fn medium_family() -> FontFamily {
    FontFamily::Name("inter-medium".into())
}

pub fn mono_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn medium_font(size: f32) -> FontId {
    FontId::new(size, medium_family())
}

// ------------------------------------------------------------------ text helpers

/// Page title: large, upper case, letter-spaced.
pub fn title(p: &Palette, text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(medium_font(m::TITLE))
        .extra_letter_spacing(m::TITLE_SPACING)
        .color(p.text)
}

/// Small-caps label (row label, section header, nav item): upper case + letter spacing.
pub fn caps(text: &str, color: Color32) -> RichText {
    RichText::new(text.to_uppercase())
        .font(medium_font(m::LABEL))
        .extra_letter_spacing(m::LABEL_SPACING)
        .color(color)
}

/// Dim small caps, regular weight (subtitles, status labels).
pub fn caps_dim(p: &Palette, text: &str, size: f32, spacing: f32) -> RichText {
    RichText::new(text.to_uppercase())
        .size(size)
        .extra_letter_spacing(spacing)
        .color(p.dim)
}

/// A config key or other machine text: grey monospace.
pub fn key(p: &Palette, text: &str) -> RichText {
    RichText::new(text).font(mono_font(m::KEY)).color(p.dim)
}

/// Monospace in the primary colour (numbers, shortcuts).
pub fn mono(p: &Palette, text: &str, size: f32) -> RichText {
    RichText::new(text).font(mono_font(size)).color(p.text)
}

/// Secondary prose (hints).
pub fn hint_text(p: &Palette, text: &str) -> RichText {
    RichText::new(text).size(m::SMALL).color(p.dim)
}

// ------------------------------------------------------------------ egui style

/// Install the dark and light styles; the active one follows the system appearance unless
/// `ASHDASH_PREFS_THEME` says otherwise.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT));
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(m::GAP, m::GAP - 2.0);
        style.spacing.button_padding = vec2(14.0, 6.0);
        style.spacing.interact_size = vec2(32.0, m::CONTROL_H);
        style.spacing.combo_width = 220.0;
        style.spacing.icon_width = 18.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.text_edit_width = 220.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.text_styles = text_styles();
    });
    match std::env::var("ASHDASH_PREFS_THEME").as_deref() {
        Ok("dark") => ctx.set_theme(Theme::Dark),
        Ok("light") => ctx.set_theme(Theme::Light),
        _ => {}
    }
}

fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, FontId> {
    use egui::TextStyle::*;
    [
        (Heading, medium_font(m::TITLE)),
        (Body, FontId::proportional(m::BODY)),
        (Button, FontId::proportional(m::BODY)),
        (Small, FontId::proportional(m::SMALL)),
        (Monospace, mono_font(12.5)),
    ]
    .into_iter()
    .collect()
}

fn visuals(p: &Palette) -> egui::Visuals {
    let r = CornerRadius::same(m::RADIUS);
    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.window_stroke = Stroke::new(m::STROKE, p.border);
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.extreme_bg_color = p.control; // text edit background
    v.faint_bg_color = p.panel;
    v.code_bg_color = p.panel;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    v.weak_text_color = Some(p.dim);
    v.selection.bg_fill = with_alpha(p.accent, if p.dark { 90 } else { 70 });
    v.selection.stroke = Stroke::new(m::STROKE, p.text);
    v.text_cursor.stroke = Stroke::new(m::STROKE_STRONG, p.accent);
    v.striped = false;
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(m::STROKE, p.hairline);
    w.noninteractive.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.inactive.bg_fill = p.control;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::new(m::STROKE, p.border);
    w.inactive.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.hovered.bg_fill = p.control_hover;
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_stroke = Stroke::new(m::STROKE, p.dim);
    w.hovered.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.active.bg_fill = p.control_hover;
    w.active.weak_bg_fill = p.control_hover;
    w.active.bg_stroke = Stroke::new(m::STROKE, p.accent);
    w.active.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.open.bg_fill = p.control;
    w.open.weak_bg_fill = p.control;
    w.open.bg_stroke = Stroke::new(m::STROKE, p.border);
    w.open.fg_stroke = Stroke::new(m::STROKE, p.text);
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }
    v
}

pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Frame margin helper: symmetric margin in whole points.
pub fn margin(x: f32, y: f32) -> Margin {
    Margin::symmetric(x as i8, y as i8)
}

// ------------------------------------------------------------------ contrast

#[cfg(test)]
/// WCAG 2.x relative luminance of an opaque sRGB colour.
pub fn luminance(c: Color32) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

#[cfg(test)]
/// WCAG contrast ratio, 1.0 ..= 21.0.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (name, foreground, background) of every text pair the window draws.
    fn text_pairs(p: &Palette) -> Vec<(&'static str, Color32, Color32)> {
        let mut v = Vec::new();
        for (bn, bg) in [("bg", p.bg), ("panel", p.panel), ("control", p.control)] {
            let _ = bn;
            v.push(("text", p.text, bg));
            v.push(("dim", p.dim, bg));
            v.push(("accent_text", p.accent_text, bg));
            v.push(("ok", p.ok, bg));
            v.push(("warn", p.warn, bg));
            v.push(("error", p.error, bg));
        }
        v.push(("text on hover", p.text, p.control_hover));
        v.push(("dim on hover", p.dim, p.control_hover));
        v
    }

    #[test]
    fn every_text_pair_meets_wcag_aa_in_both_themes() {
        for p in [DARK, LIGHT] {
            for (name, fg, bg) in text_pairs(&p) {
                let c = contrast(fg, bg);
                assert!(
                    c >= 4.5,
                    "{} {name}: {c:.2} on {bg:?}",
                    if p.dark { "dark" } else { "light" }
                );
            }
        }
    }

    #[test]
    fn the_accent_fill_is_visible_against_both_backgrounds() {
        for p in [DARK, LIGHT] {
            assert!(contrast(p.accent, p.bg) >= 3.0, "accent on bg");
            assert!(contrast(p.accent, p.panel) >= 3.0, "accent on panel");
        }
    }

    #[test]
    fn contrast_math_matches_the_wcag_reference_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-9);
        // #767676 on white is the classic 4.54:1 AA boundary
        let c = contrast(rgb(0x76, 0x76, 0x76), Color32::WHITE);
        assert!((c - 4.54).abs() < 0.01, "{c}");
    }

    #[test]
    fn dark_and_light_cover_the_same_tokens_in_the_right_direction() {
        assert!(luminance(DARK.bg) < luminance(DARK.text));
        assert!(luminance(LIGHT.bg) > luminance(LIGHT.text));
    }

    #[test]
    fn install_gives_the_text_styles_and_both_themes() {
        let ctx = egui::Context::default();
        install(&ctx);
        let dark = ctx.style_of(Theme::Dark);
        let light = ctx.style_of(Theme::Light);
        assert_eq!(dark.visuals.panel_fill, DARK.bg);
        assert_eq!(light.visuals.panel_fill, LIGHT.bg);
        assert_eq!(dark.text_styles[&egui::TextStyle::Body].size, m::BODY);
    }
}
