//! Reader for the reference launcher `.alfredappearance` theme files.
//!
//! The file is a plist with a top-level `alfredtheme` dictionary containing
//! `window`, `search`, `result`, `separator` and `scrollbar` sections. Colours
//! are `#RRGGBBAA` strings. Themes are read from disk at runtime; none are
//! bundled with this project.

use std::path::Path;

use anyhow::{Context, Result};

pub type Rgba = [u8; 4];

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub window_color: Rgba,
    pub window_border: Rgba,
    pub window_roundness: f32,
    pub window_padding: f32,
    pub window_width: f32,
    pub search_background: Rgba,
    pub search_text: Rgba,
    pub search_size: f32,
    pub search_roundness: f32,
    pub result_text: Rgba,
    pub result_text_selected: Rgba,
    pub result_text_size: f32,
    pub result_subtext: Rgba,
    pub result_subtext_selected: Rgba,
    pub result_subtext_size: f32,
    pub result_selected_background: Rgba,
    pub result_roundness: f32,
    pub icon_size: f32,
    pub separator: Rgba,
}

impl Default for Theme {
    /// A dark theme with values in the same ballpark as the reference launcher's stock dark look.
    fn default() -> Self {
        Theme {
            name: "Ashdash Dark".into(),
            window_color: [30, 30, 34, 255],
            window_border: [0, 0, 0, 127],
            window_roundness: 15.0,
            window_padding: 9.0,
            window_width: 700.0,
            search_background: [0, 0, 0, 62],
            search_text: [255, 255, 255, 255],
            search_size: 30.0,
            search_roundness: 8.0,
            result_text: [255, 255, 255, 203],
            result_text_selected: [255, 255, 255, 255],
            result_text_size: 17.0,
            result_subtext: [255, 255, 255, 126],
            result_subtext_selected: [255, 255, 255, 203],
            result_subtext_size: 12.0,
            result_selected_background: [53, 129, 136, 255],
            result_roundness: 8.0,
            icon_size: 36.0,
            separator: [203, 203, 203, 40],
        }
    }
}

pub fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

fn dict<'a>(v: &'a plist::Value, key: &str) -> Option<&'a plist::Dictionary> {
    v.as_dictionary()?.get(key)?.as_dictionary()
}

fn num(d: Option<&plist::Dictionary>, key: &str) -> Option<f32> {
    let v = d?.get(key)?;
    v.as_real()
        .map(|f| f as f32)
        .or_else(|| v.as_signed_integer().map(|i| i as f32))
        .or_else(|| v.as_unsigned_integer().map(|i| i as f32))
}

fn col(d: Option<&plist::Dictionary>, key: &str) -> Option<Rgba> {
    parse_hex(d?.get(key)?.as_string()?)
}

impl Theme {
    pub fn load(path: &Path) -> Result<Theme> {
        let root = plist::Value::from_file(path).with_context(|| format!("reading {}", path.display()))?;
        let t = root
            .as_dictionary()
            .and_then(|d| d.get("alfredtheme"))
            .context("missing alfredtheme dictionary")?;
        let mut out = Theme::default();
        if let Some(n) = t.as_dictionary().and_then(|d| d.get("name")).and_then(|v| v.as_string()) {
            out.name = n.to_string();
        }
        let wrap = plist::Value::Dictionary(t.as_dictionary().cloned().unwrap_or_default());
        let window = dict(&wrap, "window");
        let search = dict(&wrap, "search");
        let result = dict(&wrap, "result");
        let rtext = result.and_then(|r| r.get("text")).and_then(|v| v.as_dictionary());
        let rsub = result.and_then(|r| r.get("subtext")).and_then(|v| v.as_dictionary());
        let stext = search.and_then(|r| r.get("text")).and_then(|v| v.as_dictionary());

        macro_rules! set {
            ($field:expr, $val:expr) => {
                if let Some(v) = $val {
                    $field = v;
                }
            };
        }
        set!(out.window_color, col(window, "color"));
        set!(out.window_border, col(window, "borderColor"));
        set!(out.window_roundness, num(window, "roundness"));
        set!(out.window_padding, num(window, "paddingVertical"));
        set!(out.window_width, num(window, "width"));
        set!(out.search_background, col(search, "background"));
        set!(out.search_roundness, num(search, "roundness"));
        set!(out.search_text, col(stext, "color"));
        set!(out.search_size, num(stext, "size"));
        set!(out.result_text, col(rtext, "color"));
        set!(out.result_text_selected, col(rtext, "colorSelected"));
        set!(out.result_text_size, num(rtext, "size"));
        set!(out.result_subtext, col(rsub, "color"));
        set!(out.result_subtext_selected, col(rsub, "colorSelected"));
        set!(out.result_subtext_size, num(rsub, "size"));
        set!(out.result_selected_background, col(result, "backgroundSelected"));
        set!(out.result_roundness, num(result, "roundness"));
        set!(out.icon_size, num(result, "iconSize"));
        set!(out.separator, col(dict(&wrap, "separator"), "color"));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#358188FF"), Some([0x35, 0x81, 0x88, 0xFF]));
        assert_eq!(parse_hex("#112233"), Some([0x11, 0x22, 0x33, 0xFF]));
        assert_eq!(parse_hex("358188FF"), None);
        assert_eq!(parse_hex("#12"), None);
    }

    #[test]
    fn loads_theme_file() {
        let path = std::env::temp_dir().join(format!("oa-theme-{}.alfredappearance", std::process::id()));
        std::fs::write(
            &path,
            r##"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>alfredtheme</key><dict>
<key>name</key><string>T</string>
<key>window</key><dict><key>color</key><string>#00000066</string><key>roundness</key><integer>20</integer><key>width</key><integer>640</integer></dict>
<key>result</key><dict><key>backgroundSelected</key><string>#480069E4</string>
<key>text</key><dict><key>size</key><integer>22</integer></dict></dict>
</dict></dict></plist>"##,
        )
        .unwrap();
        let t = Theme::load(&path).unwrap();
        assert_eq!(t.name, "T");
        assert_eq!(t.window_color, [0, 0, 0, 0x66]);
        assert_eq!(t.window_roundness, 20.0);
        assert_eq!(t.window_width, 640.0);
        assert_eq!(t.result_selected_background, [0x48, 0, 0x69, 0xE4]);
        assert_eq!(t.result_text_size, 22.0);
        let _ = std::fs::remove_file(path);
    }
}
