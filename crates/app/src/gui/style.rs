//! Pure decisions about the window look: vibrancy, corner radius, border, and the heights that make the
//! window exactly as tall as the panel drawn in it. The AppKit calls that apply them live in `mac`.

use ashdash_core::theme::{Rgba, Theme};

/// How the blur behind the launcher should look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vibrancy {
    /// Dark vibrant appearance (light text on dark blur) vs. light.
    pub dark: bool,
}

/// What the theme asks of the native window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowStyle {
    /// `None` = the theme colour is opaque, so no blur is needed.
    pub vibrancy: Option<Vibrancy>,
    /// Corner radius in points; the blur view and the shadow follow it.
    pub corner_radius: f64,
    /// Whether the window has the system shadow. Off from macOS 26 on: see [`system_shadow`].
    pub system_shadow: bool,
    /// Whether the blur view is clipped to rounded corners. With square corners there is nothing to clip,
    /// so no mask is set at all (a stray mask would only cost a layer pass and risk a hairline at the edges).
    pub clip_to_corners: bool,
}

/// The first macOS release (26, "Tahoe") whose window server paints a light "Liquid Glass" rim over the outer one or two device pixels of
/// every window that has a system shadow, brighter the lighter the desktop behind it. It lies on top of the window's content, so it
/// covers our own 1 pixel border (`#242628` came out as `#3a3c3d` on a dark desktop and as a white line on a light one) and cannot be
/// switched off other than by switching the shadow off: measured on CI with the blur view hidden, with an opaque window and with the
/// level changed, only `hasShadow = NO` removed it (docs/design/launcher-look.md, "The white border").
pub const RIM_MACOS_MAJOR: u32 = 26;

/// Whether the launcher window gets the system shadow on this macOS release (`os_major`, 0 = unknown): not where the system adds its
/// rim to windows that have one. The window then has exactly our border and nothing else.
pub fn system_shadow(os_major: u32) -> bool {
    os_major < RIM_MACOS_MAJOR
}

/// Horizontal space between the panel's padding and the query text, in points. Together with the border and
/// `window_padding` this puts the caret 17 pt from the panel's left edge in the default look.
pub const FIELD_PAD_X: f32 = 8.0;
/// Height of the query row as a multiple of the query font size. egui's text row is about 1.12 times the font size,
/// so the row is always at least as tall as the text and the panel height is exact (the caret is centred in it).
pub const QUERY_ROW_FACTOR: f32 = 1.25;
/// Space between the query row and the results (or a notice row below it).
pub const BODY_GAP: f32 = 9.0;
/// Height of the footer line (status, loading, buffer) under the results, including the space above it.
pub const FOOTER_HEIGHT: f32 = 26.0;

/// Perceived brightness 0..=255 (Rec. 601 weights).
pub fn luminance(c: Rgba) -> f32 {
    0.299 * f32::from(c[0]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[2])
}

/// Blur only matters when the theme's window colour lets something through.
pub fn vibrancy_for(window_color: Rgba) -> Option<Vibrancy> {
    if window_color[3] == 255 {
        return None;
    }
    Some(Vibrancy {
        dark: luminance(window_color) < 128.0,
    })
}

/// The window style for the macOS release this process runs on.
pub fn window_style(t: &Theme) -> WindowStyle {
    window_style_on(t, super::mac::macos_major())
}

/// [`window_style`] for a given macOS major version.
pub fn window_style_on(t: &Theme, os_major: u32) -> WindowStyle {
    let corner_radius = corner_radius(t);
    WindowStyle {
        vibrancy: vibrancy_for(t.window_color),
        corner_radius: f64::from(corner_radius),
        system_shadow: system_shadow(os_major),
        clip_to_corners: corner_radius > 0.0,
    }
}

/// The window's corner radius in points: never negative, 0 for a nonsensical value.
pub fn corner_radius(t: &Theme) -> f32 {
    if t.window_roundness.is_finite() {
        t.window_roundness.max(0.0)
    } else {
        0.0
    }
}

/// Border width in points. A theme that gives no width gets one *device* pixel (0.5 pt on a 2x screen), a width it
/// gives is used as written (0 = no border).
pub fn border_width(theme_width: Option<f32>, pixels_per_point: f32) -> f32 {
    match theme_width {
        Some(w) if w.is_finite() => w.max(0.0),
        _ if pixels_per_point.is_finite() && pixels_per_point >= 1.0 => 1.0 / pixels_per_point,
        _ => 1.0,
    }
}

/// Height of the query row (the text line) in points.
pub fn query_row_height(t: &Theme) -> f32 {
    t.search_size.max(1.0) * QUERY_ROW_FACTOR
}

/// Height of the panel with nothing but the query row: border, padding, row.
pub fn header_height(t: &Theme, pixels_per_point: f32) -> f32 {
    query_row_height(t) + 2.0 * t.window_padding + 2.0 * border_width(t.window_border_width, pixels_per_point)
}

/// Height of the whole window: the query row plus, when there is something below it, the gap and `body_h`
/// (results, a notice or the import prompt), plus the footer line when one is shown.
pub fn window_height(t: &Theme, pixels_per_point: f32, body_h: f32, footer: bool) -> f32 {
    let gap = if body_h > 0.0 { BODY_GAP } else { 0.0 };
    header_height(t, pixels_per_point) + gap + body_h + if footer { FOOTER_HEIGHT } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_colour_needs_no_blur() {
        assert_eq!(vibrancy_for([10, 10, 10, 255]), None);
    }

    #[test]
    fn translucent_dark_and_light() {
        assert_eq!(vibrancy_for([20, 20, 30, 200]), Some(Vibrancy { dark: true }));
        assert_eq!(vibrancy_for([240, 240, 240, 120]), Some(Vibrancy { dark: false }));
        assert_eq!(vibrancy_for([0, 0, 0, 0]), Some(Vibrancy { dark: true }));
    }

    #[test]
    fn the_system_shadow_is_off_where_the_system_draws_a_rim_on_it() {
        assert!(system_shadow(15), "macOS 15: shadow, no rim (CI screenshots)");
        assert!(system_shadow(14));
        assert!(!system_shadow(26), "macOS 26: rim on shadowed windows");
        assert!(!system_shadow(27), "and later releases");
        assert!(system_shadow(0), "unknown version: the plain look");
        let t = Theme::default();
        assert!(window_style_on(&t, 15).system_shadow);
        assert!(!window_style_on(&t, 26).system_shadow);
        // nothing else about the look depends on the release
        let (a, b) = (window_style_on(&t, 15), window_style_on(&t, 26));
        assert_eq!(
            (a.vibrancy, a.corner_radius, a.clip_to_corners),
            (b.vibrancy, b.corner_radius, b.clip_to_corners)
        );
    }

    #[test]
    fn style_follows_theme_roundness() {
        let mut t = Theme {
            window_roundness: 22.0,
            ..Theme::default()
        };
        t.window_color[3] = 180;
        let s = window_style_on(&t, 15);
        assert_eq!(s.corner_radius, 22.0);
        assert!(s.vibrancy.is_some());
        assert!(s.clip_to_corners);
        t.window_roundness = -3.0;
        assert_eq!(window_style_on(&t, 15).corner_radius, 0.0);
        assert!(!window_style_on(&t, 15).clip_to_corners);
        t.window_roundness = f32::NAN;
        assert_eq!(corner_radius(&t), 0.0);
    }

    #[test]
    fn the_default_panel_is_square_and_has_no_mask() {
        let s = window_style_on(&Theme::default(), 15);
        assert_eq!(s.corner_radius, 0.0);
        assert!(!s.clip_to_corners);
        // nearly opaque, so the blur is kept (it shows faintly) and it is the dark one
        assert_eq!(s.vibrancy, Some(Vibrancy { dark: true }));
    }

    #[test]
    fn a_theme_without_a_width_gets_one_device_pixel() {
        assert_eq!(border_width(None, 2.0), 0.5);
        assert_eq!(border_width(None, 1.0), 1.0);
        assert_eq!(border_width(None, 3.0), 1.0 / 3.0);
        // a nonsensical scale falls back to a point
        assert_eq!(border_width(None, 0.0), 1.0);
        assert_eq!(border_width(None, f32::NAN), 1.0);
    }

    #[test]
    fn a_width_from_the_theme_is_used_as_written() {
        assert_eq!(border_width(Some(2.0), 2.0), 2.0);
        assert_eq!(border_width(Some(0.0), 2.0), 0.0, "0 = no border");
        assert_eq!(border_width(Some(-1.0), 2.0), 0.0);
        assert_eq!(
            border_width(Some(f32::INFINITY), 2.0),
            0.5,
            "not a number of points: treated as unset"
        );
    }

    #[test]
    fn the_empty_default_panel_is_about_575_by_62_points() {
        let t = Theme::default();
        assert_eq!(t.window_width, 575.0);
        let h = window_height(&t, 2.0, 0.0, false);
        assert!((h - 62.0).abs() <= 1.0, "{h}");
        assert_eq!(h, header_height(&t, 2.0));
        // the caret sits border + padding + field padding from the left edge: 17.5 pt (reference: about 17)
        let caret_x = border_width(t.window_border_width, 2.0) + t.window_padding + FIELD_PAD_X;
        assert!((caret_x - 17.0).abs() <= 1.0, "{caret_x}");
        // the row is taller than the caret (~39 pt for a 34 pt font) so the caret is centred in it
        assert!(query_row_height(&t) > 39.0 && query_row_height(&t) < 46.0);
    }

    #[test]
    fn results_extend_the_panel_downward() {
        let t = Theme::default();
        let empty = window_height(&t, 2.0, 0.0, false);
        let three_rows = window_height(&t, 2.0, 3.0 * 48.0, false);
        assert_eq!(three_rows, empty + BODY_GAP + 144.0);
        // a footer line adds its own height, with or without rows
        assert_eq!(window_height(&t, 2.0, 0.0, true), empty + FOOTER_HEIGHT);
        assert_eq!(window_height(&t, 2.0, 144.0, true), three_rows + FOOTER_HEIGHT);
        // the width never changes with the content: only the height
        assert!(window_height(&t, 2.0, 432.0, true) > three_rows);
    }

    #[test]
    fn a_thicker_border_or_bigger_font_makes_the_header_taller() {
        let mut t = Theme::default();
        let base = header_height(&t, 2.0);
        t.window_border_width = Some(3.0);
        assert_eq!(header_height(&t, 2.0), base - 1.0 + 6.0);
        t.window_border_width = None;
        t.search_size = 40.0;
        assert_eq!(header_height(&t, 2.0), base + 6.0 * QUERY_ROW_FACTOR);
        t.search_size = 0.0;
        assert!(
            header_height(&t, 2.0) > 0.0,
            "a zero font size does not collapse the row"
        );
    }
}
