//! Design tokens of the preferences window ("V4 Instrument"): every colour, size, spacing,
//! radius and stroke width lives in this file, for dark and light, and nothing else in
//! `prefs/` hard-codes one. The window follows the system appearance; `ASHDASH_PREFS_THEME`
//! (`dark` | `light`) forces one for screenshots.
//!
//! The launcher window has its own look (`gui/style.rs`) and is not touched by this file.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, Theme, vec2};

// ------------------------------------------------------------------ colours

/// One complete colour set. Values follow `docs/design/settings-v4/SPEC.md` (sampled from the
/// V4 images by script, then rounded; `adj` entries were moved to pass AA). All text/background
/// pairs the window uses are checked against WCAG AA by the tests at the bottom of this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    /// Content area and cards.
    pub bg: Color32,
    pub sidebar: Color32,
    pub status_bg: Color32,
    /// Fill of inputs (search, text edits, recorder idle, stepper value, toggle off).
    pub field: Color32,
    /// Fill of buttons and dropdowns.
    pub button: Color32,
    /// Fill of a control under the pointer.
    pub hover: Color32,
    /// Sidebar row under the pointer, and the active one.
    pub nav_hover: Color32,
    /// Grid lines and separators (decorative).
    pub hairline: Color32,
    /// Outline of controls (non-text contrast, at least 3:1).
    pub border: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text: hints, units, inactive nav, status values.
    pub dim: Color32,
    /// The grey monospace config key under a label.
    pub key: Color32,
    /// The signal colour as a fill (toggle on, active-item bar, recorder outline, focus ring).
    pub accent: Color32,
    /// The signal colour as text.
    pub accent_text: Color32,
    /// Toggle thumb.
    pub thumb: Color32,
    /// Status lights and outlines.
    pub ok: Color32,
    pub warn: Color32,
    /// Same hues as text (darker in the light theme to reach 4.5:1; SPEC.md lists only the light colours).
    pub ok_text: Color32,
    pub warn_text: Color32,
    pub error: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    bg: rgb(0x1c, 0x1c, 0x1c),
    sidebar: rgb(0x1a, 0x1a, 0x1a),
    status_bg: rgb(0x1c, 0x1c, 0x1c),
    field: rgb(0x28, 0x28, 0x28),
    button: rgb(0x30, 0x30, 0x30),
    hover: rgb(0x38, 0x38, 0x38),
    nav_hover: rgb(0x23, 0x23, 0x23),
    hairline: rgb(0x2a, 0x2a, 0x2a),
    border: rgb(0x6a, 0x6a, 0x6a),
    text: rgb(0xf0, 0xf0, 0xf0),
    dim: rgb(0x9a, 0x9a, 0x9a),
    key: rgb(0x8a, 0x8a, 0x8a),
    accent: rgb(0xff, 0x5f, 0x1f),
    accent_text: rgb(0xff, 0x5f, 0x1f),
    thumb: rgb(0xff, 0xff, 0xff),
    ok: rgb(0x1f, 0xbf, 0x63),
    warn: rgb(0xe8, 0x96, 0x2e),
    ok_text: rgb(0x1f, 0xbf, 0x63),
    warn_text: rgb(0xe8, 0x96, 0x2e),
    error: rgb(0xff, 0x6b, 0x6b),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    bg: rgb(0xf7, 0xf6, 0xf4),
    sidebar: rgb(0xea, 0xe8, 0xe4),
    status_bg: rgb(0xf2, 0xf1, 0xed),
    field: rgb(0xff, 0xff, 0xff),
    button: rgb(0xff, 0xff, 0xff),
    hover: rgb(0xf0, 0xee, 0xea),
    nav_hover: rgb(0xe3, 0xe1, 0xdc),
    hairline: rgb(0xde, 0xdd, 0xd9),
    border: rgb(0x8c, 0x8c, 0x8c),
    text: rgb(0x11, 0x11, 0x11),
    dim: rgb(0x5f, 0x5f, 0x5f),
    key: rgb(0x6b, 0x6b, 0x6b),
    accent: rgb(0xe5, 0x47, 0x0a),
    accent_text: rgb(0xb5, 0x3a, 0x04),
    thumb: rgb(0xff, 0xff, 0xff),
    ok: rgb(0x1a, 0x94, 0x55),
    warn: rgb(0xb8, 0x6a, 0x00),
    ok_text: rgb(0x15, 0x7a, 0x47),
    warn_text: rgb(0x8a, 0x52, 0x00),
    error: rgb(0xc6, 0x28, 0x28),
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

    pub const SIDEBAR_W: f32 = 160.0;
    pub const NAV_ITEM_H: f32 = 34.0;
    /// Width of the orange bar on the active nav item.
    pub const NAV_BAR_W: f32 = 3.0;
    pub const SEARCH_H: f32 = 28.0;

    pub const PAGE_PAD_X: f32 = 20.0;
    pub const HEADER_PAD_Y: f32 = 14.0;

    /// Width of the label cell of a setting row; the control cell takes the rest.
    pub const LABEL_COL_W: f32 = 172.0;
    pub const ROW_PAD: f32 = 14.0;
    pub const ROW_MIN_H: f32 = 60.0;

    pub const CONTROL_H: f32 = 28.0;
    pub const STEPPER_H: f32 = 32.0;
    pub const STEPPER_BTN_W: f32 = 36.0;
    pub const STEPPER_VALUE_W: f32 = 100.0;
    pub const RECORDER_H: f32 = 48.0;
    pub const TOGGLE_W: f32 = 38.0;
    pub const TOGGLE_H: f32 = 20.0;
    /// SPEC.md says 48; 36 keeps more of the 560 pt window for content.
    pub const STATUS_H: f32 = 36.0;
    pub const STATUS_LIGHT: f32 = 8.0;

    /// Corner radius of controls, cards and rows.
    pub const RADIUS: u8 = 4;
    /// Hairlines, control outlines.
    pub const STROKE: f32 = 1.0;
    /// Recorder outline while recording, and the row error outline.
    pub const STROKE_MID: f32 = 1.5;
    /// Focus ring.
    pub const STROKE_STRONG: f32 = 2.0;
    pub const GAP: f32 = 8.0;
    pub const GAP_L: f32 = 16.0;

    // type scale
    pub const TITLE: f32 = 17.0;
    pub const TITLE_SPACING: f32 = 3.0;
    pub const LABEL: f32 = 11.0;
    pub const LABEL_SPACING: f32 = 1.5;
    pub const SUBTITLE: f32 = 10.0;
    pub const SUBTITLE_SPACING: f32 = 1.2;
    pub const KEY: f32 = 11.0;
    pub const BODY: f32 = 13.0;
    pub const SMALL: f32 = 11.5;
    pub const STATUS: f32 = 10.0;
    pub const STATUS_SPACING: f32 = 1.4;
    pub const BIG_NUMBER: f32 = 20.0;
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
    RichText::new(text).font(mono_font(m::KEY)).color(p.key)
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
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.interact_size = vec2(32.0, m::CONTROL_H);
        style.spacing.combo_width = 190.0;
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
        (Monospace, mono_font(12.0)),
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
    v.window_fill = p.bg;
    v.window_stroke = Stroke::new(m::STROKE, p.border);
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.extreme_bg_color = p.field; // text edit background
    v.faint_bg_color = p.sidebar;
    v.code_bg_color = p.sidebar;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.warn_text;
    v.error_fg_color = p.error;
    v.weak_text_color = Some(p.dim);
    v.selection.bg_fill = with_alpha(p.accent, if p.dark { 90 } else { 70 });
    v.selection.stroke = Stroke::new(m::STROKE, p.text);
    v.text_cursor.stroke = Stroke::new(m::STROKE_STRONG, p.accent);
    v.striped = false;
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.bg;
    w.noninteractive.bg_stroke = Stroke::new(m::STROKE, p.hairline);
    w.noninteractive.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.inactive.bg_fill = p.button;
    w.inactive.weak_bg_fill = p.button;
    w.inactive.bg_stroke = Stroke::new(m::STROKE, p.border);
    w.inactive.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(m::STROKE, p.dim);
    w.hovered.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.active.bg_fill = p.hover;
    w.active.weak_bg_fill = p.hover;
    w.active.bg_stroke = Stroke::new(m::STROKE, p.accent);
    w.active.fg_stroke = Stroke::new(m::STROKE, p.text);
    w.open.bg_fill = p.button;
    w.open.weak_bg_fill = p.button;
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
        for bg in [p.bg, p.sidebar, p.status_bg, p.field, p.button] {
            v.push(("text", p.text, bg));
            v.push(("dim", p.dim, bg));
        }
        // config keys are only drawn on the content area, never on the sidebar
        v.push(("key", p.key, p.bg));
        for bg in [p.bg, p.sidebar] {
            v.push(("accent_text", p.accent_text, bg));
            v.push(("error", p.error, bg));
        }
        v.push(("error on field", p.error, p.field));
        for bg in [p.bg, p.field, p.status_bg] {
            v.push(("ok_text", p.ok_text, bg));
            v.push(("warn_text", p.warn_text, bg));
        }
        v.push(("text on hover", p.text, p.hover));
        v.push(("dim on nav hover", p.dim, p.nav_hover));
        v.push(("text on nav hover", p.text, p.nav_hover));
        v
    }

    /// Pairs that are not text: need 3:1 (WCAG 1.4.11).
    fn graphic_pairs(p: &Palette) -> Vec<(&'static str, Color32, Color32)> {
        vec![
            ("accent on bg", p.accent, p.bg),
            ("accent on sidebar", p.accent, p.sidebar),
            ("control outline on bg", p.border, p.bg),
            ("ok light on status", p.ok, p.status_bg),
            ("warn light on status", p.warn, p.status_bg),
        ]
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
    fn graphic_elements_meet_three_to_one_in_both_themes() {
        for p in [DARK, LIGHT] {
            for (name, fg, bg) in graphic_pairs(&p) {
                let c = contrast(fg, bg);
                assert!(c >= 3.0, "{} {name}: {c:.2}", if p.dark { "dark" } else { "light" });
            }
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
