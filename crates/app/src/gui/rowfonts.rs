//! The fonts of the result rows: Inter for titles and plain subtitles, Geist Mono for paths, URLs and the `⌘N` hints. Both are already
//! embedded for the preferences window (`prefs::fonts`), so this adds no bytes to the binary.
//!
//! They are two named families registered once when the launcher starts. `Proportional` and `Monospace` are not touched, so the query
//! field keeps its font. After Inter / Geist Mono each family continues with egui's default proportional chain (Ubuntu-Light, the emoji
//! fonts), which is where glyphs they lack come from. The system CJK fonts that `crate::fonts::Fallbacks` loads on demand are appended
//! to these families too (see [`families`]).

use std::sync::Arc;

use ashdash_core::theme::Theme;
use eframe::egui::text::LayoutJob;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

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

/// The printable ASCII range, `⌘` and the digits of the hints, and the ellipsis: what typing and the first results draw.
const WARM_TEXT: &str =
    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~⌘…";

/// What the glyph atlas was warmed for. The same key means the warm-up would add nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmKey {
    ppp_bits: u32,
    sizes: [u32; 3],
    epoch: u32,
}

impl WarmKey {
    /// The pixels-per-point, the three text sizes of the theme and the font-set epoch decide which glyphs exist in the atlas.
    pub fn of(t: &Theme, pixels_per_point: f32, font_epoch: u32) -> WarmKey {
        WarmKey {
            ppp_bits: pixels_per_point.to_bits(),
            sizes: [t.search_size, t.result_text_size, t.result_subtext_size].map(f32::to_bits),
            epoch: font_epoch,
        }
    }
}

/// The (family, size) pairs the launcher draws text with: the query field, the row titles, the plain and the monospace subtitles
/// and the `⌘N` hints.
fn drawn_fonts(t: &Theme) -> [FontId; 5] {
    [
        FontId::proportional(t.search_size),
        FontId::new(t.result_text_size, text_family()),
        FontId::new(t.result_subtext_size, text_family()),
        FontId::new(t.result_subtext_size - super::rowtext::MONO_SIZE_DELTA, mono_family()),
        FontId::new(t.result_subtext_size, mono_family()),
    ]
}

/// Lay out `text` in `font` four times, each starting a quarter of a physical pixel further right. egui keeps up to four bitmaps per
/// glyph, one for each quarter-pixel position it can land on, so this puts every one of them in the atlas.
fn layout_at_quarter_pixels(f: &mut egui::epaint::text::FontsView<'_>, text: &str, font: &FontId, ppp: f32) {
    for quarter in 0..4 {
        let mut job = LayoutJob::simple_singleline(text.to_owned(), font.clone(), egui::Color32::WHITE);
        if let Some(section) = job.sections.first_mut() {
            section.leading_space = quarter as f32 * 0.25 / ppp;
        }
        f.layout_job(job);
    }
}

/// Lay out [`WARM_TEXT`] in every font the launcher draws with, which makes egui rasterise the glyphs into its atlas now.
pub fn prewarm(ctx: &egui::Context, t: &Theme) {
    let ppp = ctx.pixels_per_point();
    ctx.fonts_mut(|f| {
        for font in drawn_fonts(t) {
            layout_at_quarter_pixels(f, WARM_TEXT, &font, ppp);
        }
    });
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

    /// Pixels the finished pass uploads to the GPU (every texture, in this app only the glyph atlas).
    fn uploaded_by(mut out: egui::FullOutput) -> usize {
        let pixels = out
            .textures_delta
            .set
            .values()
            .flat_map(|deltas| deltas.iter())
            .map(|d| d.image.width() * d.image.height())
            .sum();
        out.textures_delta.clear();
        pixels
    }

    /// Pixels of the glyph atlas that a pass uploaded to the GPU: what laying out new glyphs costs.
    fn uploaded_pixels(ctx: &egui::Context, t: &Theme, texts: &[&str]) -> usize {
        ctx.begin_pass(RawInput::default());
        let ppp = ctx.pixels_per_point();
        ctx.fonts_mut(|f| {
            for text in texts {
                for font in drawn_fonts(t) {
                    // every position a glyph can land on, as typing and row layout reach them
                    layout_at_quarter_pixels(f, text, &font, ppp);
                    f.layout_no_wrap((*text).to_string(), font, egui::Color32::WHITE);
                }
            }
        });
        uploaded_by(ctx.end_pass())
    }

    /// Letters, digits and punctuation that are drawn one after the other. (Glyphs the font only picks in a context, such as the
    /// form of `~` before `/` in Geist Mono, are not part of the warm-up: they are rare and cost one small glyph each.)
    #[test]
    fn after_the_warm_up_typing_pup_adds_no_glyph_to_the_atlas() {
        let t = Theme::default();
        let texts = [
            "pup",
            "Safari",
            "Visual Studio Code",
            "x.app",
            "/x",
            "⌘1 ⌘9 …",
            "Quick brown fox",
        ];
        for ppp in [1.0, 2.0] {
            // cold: laying out "pup" uploads glyphs
            let cold = egui::Context::default();
            cold.set_pixels_per_point(ppp);
            cold.set_fonts(definitions());
            assert!(
                uploaded_pixels(&cold, &t, &["pup"]) > 0,
                "without a warm-up the first characters upload glyphs"
            );

            // warm: the pass that warms uploads them, the passes after it have nothing left to upload
            let warm = egui::Context::default();
            warm.set_pixels_per_point(ppp);
            warm.set_fonts(definitions());
            warm.begin_pass(RawInput::default());
            prewarm(&warm, &t);
            let warmed = uploaded_by(warm.end_pass());
            assert!(warmed > 0, "the warm-up rasterised something");
            for text in texts {
                assert_eq!(
                    uploaded_pixels(&warm, &t, &[text]),
                    0,
                    "{text:?} at scale {ppp} needed a new glyph after the warm-up"
                );
            }
        }
    }

    #[test]
    fn the_warm_key_changes_with_scale_sizes_and_fonts_only() {
        let t = Theme::default();
        let base = WarmKey::of(&t, 2.0, 0);
        assert_eq!(base, WarmKey::of(&t, 2.0, 0));
        assert_ne!(base, WarmKey::of(&t, 1.0, 0), "another screen scale");
        assert_ne!(base, WarmKey::of(&t, 2.0, 1), "a font was added");
        let bigger = Theme {
            result_text_size: t.result_text_size + 1.0,
            ..t.clone()
        };
        assert_ne!(base, WarmKey::of(&bigger, 2.0, 0), "another text size");
        let recoloured = Theme {
            result_text: [1, 2, 3, 255],
            ..t
        };
        assert_eq!(base, WarmKey::of(&recoloured, 2.0, 0), "colours do not change glyphs");
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
