//! Pure decisions about the native window look (vibrancy, corner radius).
//! The AppKit calls that apply them live in `mac`.

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
}

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

pub fn window_style(t: &Theme) -> WindowStyle {
    WindowStyle {
        vibrancy: vibrancy_for(t.window_color),
        corner_radius: f64::from(t.window_roundness.max(0.0)),
    }
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
    fn style_follows_theme_roundness() {
        let mut t = Theme {
            window_roundness: 22.0,
            ..Theme::default()
        };
        t.window_color[3] = 180;
        let s = window_style(&t);
        assert_eq!(s.corner_radius, 22.0);
        assert!(s.vibrancy.is_some());
        t.window_roundness = -3.0;
        assert_eq!(window_style(&t).corner_radius, 0.0);
    }
}
