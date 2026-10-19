//! The fonts of the result rows: Inter for titles and plain subtitles, Geist Mono for paths, URLs and the `⌘N` hints. Both are already
//! embedded for the preferences window (`prefs::fonts`), so this adds no bytes to the binary.
//!
//! They are two named families registered once when the launcher starts. `Proportional` and `Monospace` are not touched, so the query
//! field keeps its font. After Inter / Geist Mono each family continues with egui's default proportional chain (Ubuntu-Light, the emoji
//! fonts), which is where glyphs they lack come from. The system CJK fonts that `crate::fonts::Fallbacks` loads on demand are appended
//! to these families too (see [`families`]).

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

use super::prefs::fonts::{GEIST_MONO, INTER_REGULAR};

const INTER: &str = "results-inter";
const GEIST: &str = "results-geist-mono";

/// Inter, then the default proportional chain.
pub fn text_family() -> FontFamily {
    FontFamily::Name("results".into())
}

/// Geist Mono, then Inter (which has `⌘`, `⌥`, `⌃` that Geist Mono lacks), then the default proportional chain.
pub fn mono_family() -> FontFamily {
    FontFamily::Name("results-mono".into())
}

/// Both families: what the CJK fallbacks are appended to besides `Proportional` and `Monospace`.
pub fn families() -> [FontFamily; 2] {
    [text_family(), mono_family()]
}

/// egui's default fonts plus the two result families.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(INTER.into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    fonts
        .font_data
        .insert(GEIST.into(), Arc::new(FontData::from_static(GEIST_MONO)));
    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts
        .families
        .insert(text_family(), [vec![INTER.to_string()], proportional.clone()].concat());
    fonts.families.insert(
        mono_family(),
        [vec![GEIST.to_string(), INTER.to_string()], proportional].concat(),
    );
    fonts
}

/// Register the families. Called once, when the egui context is created; never per show or per frame.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use eframe::egui::{FontId, RawInput};

    use super::*;

    #[test]
    fn the_families_lead_with_inter_and_geist_mono_and_keep_the_default_chain() {
        let d = definitions();
        let default = FontDefinitions::default();
        let prop = &default.families[&FontFamily::Proportional];
        assert_eq!(d.families[&text_family()][0], INTER);
        assert_eq!(&d.families[&text_family()][1..], prop.as_slice());
        assert_eq!(&d.families[&mono_family()][..2], [GEIST, INTER]);
        assert_eq!(&d.families[&mono_family()][2..], prop.as_slice());
    }

    #[test]
    fn the_query_field_keeps_its_fonts() {
        let d = definitions();
        let default = FontDefinitions::default();
        for f in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(d.families[&f], default.families[&f], "{f:?} must stay as egui ships it");
        }
    }

    #[test]
    fn the_embedded_fonts_are_the_preferences_ones() {
        let d = definitions();
        assert_eq!(d.font_data[INTER].font.as_ref(), INTER_REGULAR);
        assert_eq!(d.font_data[GEIST].font.as_ref(), GEIST_MONO);
    }

    /// Which of the two embedded fonts has a glyph, asked of a real egui font set (no window involved).
    fn has_glyph(only: &str, ch: char) -> bool {
        let mut d = definitions();
        let fam = FontFamily::Name("probe".into());
        d.families.insert(fam.clone(), vec![only.to_string()]);
        let ctx = egui::Context::default();
        ctx.set_fonts(d);
        ctx.begin_pass(RawInput::default());
        // a glyph the only font lacks has no advance (`has_glyph` cannot tell: its replacement face is that same font)
        let has = ctx.fonts_mut(|f| f.glyph_width(&FontId::new(12.0, fam), ch)) > 0.0;
        ctx.end_pass().textures_delta.clear();
        has
    }

    #[test]
    fn inter_draws_the_hint_glyphs_that_geist_mono_lacks() {
        // the hints are `⌘1`..`⌘9`: the digits come from Geist Mono, the ⌘ from Inter
        assert!(has_glyph(GEIST, '1'));
        assert!(!has_glyph(GEIST, '⌘'), "Geist Mono has no ⌘ (prefs::fonts says so)");
        assert!(has_glyph(INTER, '⌘'), "Inter must draw the ⌘ of the hint column");
        assert!(has_glyph(INTER, '…'), "and the ellipsis of elided text");
        assert!(has_glyph(GEIST, '…'));
        // `↩` (proposal 5, not drawn yet) is in both
        assert!(has_glyph(GEIST, '↩') && has_glyph(INTER, '↩'));
    }

    #[test]
    fn the_hint_and_the_ellipsis_resolve_in_the_chains() {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions());
        ctx.begin_pass(RawInput::default());
        let (mono, text) = ctx.fonts_mut(|f| {
            (
                f.has_glyphs(&FontId::new(12.0, mono_family()), "⌘1…~/"),
                f.has_glyphs(&FontId::new(16.0, text_family()), "Search “saf” …"),
            )
        });
        ctx.end_pass().textures_delta.clear();
        assert!(mono && text);
    }
}
