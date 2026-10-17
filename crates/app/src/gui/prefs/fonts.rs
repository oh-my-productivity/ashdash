//! The fonts of the preferences window: Inter (Regular, Medium) for text and Geist Mono for keys
//! and numbers, embedded as static TrueType files (sources and checksums in
//! `resources/fonts/SOURCES.md`, licences next to the files). The default egui fonts stay behind
//! them as fallbacks for any glyph these do not have. The launcher keeps its own fonts.

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

use super::style::medium_family;

pub const INTER_REGULAR: &[u8] = include_bytes!("../../../resources/fonts/Inter-Regular.ttf");
pub const INTER_MEDIUM: &[u8] = include_bytes!("../../../resources/fonts/Inter-Medium.ttf");
pub const GEIST_MONO: &[u8] = include_bytes!("../../../resources/fonts/GeistMono-Regular.ttf");

const INTER: &str = "inter-regular";
const INTER_MED: &str = "inter-medium";
const GEIST: &str = "geist-mono";

/// The font set: ours first in every family, egui's defaults after them.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(INTER.into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    fonts
        .font_data
        .insert(INTER_MED.into(), Arc::new(FontData::from_static(INTER_MEDIUM)));
    fonts
        .font_data
        .insert(GEIST.into(), Arc::new(FontData::from_static(GEIST_MONO)));
    let fallbacks =
        |family: &FontFamily, fonts: &FontDefinitions| fonts.families.get(family).cloned().unwrap_or_default();

    let proportional = fallbacks(&FontFamily::Proportional, &fonts);
    let monospace = fallbacks(&FontFamily::Monospace, &fonts);
    // Geist Mono has no ⌘ ⌥ ⌃: Inter fills those in, then the defaults.
    fonts.families.insert(
        FontFamily::Proportional,
        [vec![INTER.to_string()], proportional.clone()].concat(),
    );
    fonts.families.insert(
        FontFamily::Monospace,
        [vec![GEIST.to_string(), INTER.to_string()], monospace].concat(),
    );
    fonts
        .families
        .insert(medium_family(), [vec![INTER_MED.to_string()], proportional].concat());
    fonts
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_files_are_truetype_and_the_families_lead_with_them() {
        for (name, bytes) in [
            ("Inter-Regular", INTER_REGULAR),
            ("Inter-Medium", INTER_MEDIUM),
            ("Geist", GEIST_MONO),
        ] {
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "{name} is not a TrueType file");
            assert!(bytes.len() > 100_000, "{name} looks truncated");
        }
        let d = definitions();
        assert_eq!(d.families[&FontFamily::Proportional][0], INTER);
        assert_eq!(d.families[&FontFamily::Monospace][0], GEIST);
        assert_eq!(d.families[&medium_family()][0], INTER_MED);
        // egui's own fonts remain as fallbacks
        assert!(d.families[&FontFamily::Proportional].len() > 1);
    }

    #[test]
    fn the_budget_for_the_three_files_is_respected() {
        let total = INTER_REGULAR.len() + INTER_MEDIUM.len() + GEIST_MONO.len();
        assert!(total < 1_200_000, "fonts add {total} bytes");
    }
}
