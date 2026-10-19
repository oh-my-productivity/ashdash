//! Reader for appearance (theme) files.
//!
//! The file is a plist with a top-level theme dictionary (key `format_ids::THEME_KEY`) containing
//! `window`, `search`, `result`, `separator` and `scrollbar` sections. Colours
//! are `#RRGGBBAA` strings. Themes are read from disk at runtime; none are
//! bundled with this project.

use crate::format_ids::THEME_KEY;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub type Rgba = [u8; 4];

/// Themes are small property lists; a file offered for **installation** (Finder, `ashdash theme`,
/// the preferences window) that is bigger is refused before it is parsed.
pub const MAX_THEME_BYTES: u64 = 2 * 1024 * 1024;
/// What [`Theme::load`] accepts: a theme already installed (or chosen in the settings) may be
/// larger than a new one is allowed to be - versions up to 0.1.0 read these with a bigger
/// allowance - because the parse is bounded in nesting and in values anyway ([`parse_plist`]).
pub const MAX_INSTALLED_THEME_BYTES: u64 = 8 * 1024 * 1024;
/// Deepest array/dictionary nesting accepted in a property list (the workflow parser's cap).
pub const MAX_PLIST_DEPTH: usize = 64;
/// Most values (keys included) a property list may expand to. A binary plist can refer to one
/// object from many places, so a few hundred bytes can describe 10^40 values; the probe stops
/// counting here. Real files stay far below it (a 2 MiB XML plist cannot hold more than ~200 000).
pub const MAX_PLIST_VALUES: usize = 500_000;

/// Parse a property list (XML, binary or ASCII) after checking how deeply it nests.
///
/// `plist::Value::from_reader` builds and drops its tree recursively, so a ~2 MB file of nested
/// `<array>` elements overflows the stack and aborts the whole process (confirmed for plist
/// 1.10.1; the same shape as the calculator's SEC-08). The probe below reads the same bytes
/// through plist's event-driven serde deserializer and fails at [`MAX_PLIST_DEPTH`], so its own
/// recursion never gets deeper than the cap; the tree is only built for files that passed.
/// Callers must also bound the input size ([`read_limited`]).
pub fn parse_plist(bytes: &[u8]) -> Result<plist::Value> {
    PROBE_VALUES.with(|n| n.set(0));
    plist::from_reader::<_, DepthProbe>(std::io::Cursor::new(bytes))?;
    Ok(plist::Value::from_reader(std::io::Cursor::new(bytes))?)
}

thread_local! {
    static PROBE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PROBE_VALUES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one more value; fails once a document has expanded to [`MAX_PLIST_VALUES`].
fn count_value<E: serde::de::Error>() -> Result<(), E> {
    PROBE_VALUES.with(|n| {
        if n.get() >= MAX_PLIST_VALUES {
            return Err(E::custom(format!(
                "the property list holds more than {MAX_PLIST_VALUES} values"
            )));
        }
        n.set(n.get() + 1);
        Ok(())
    })
}

/// Holds one level of nesting; gives it back when dropped (also on an error return).
struct DepthGuard;

impl DepthGuard {
    fn enter<E: serde::de::Error>() -> Result<DepthGuard, E> {
        PROBE_DEPTH.with(|d| {
            if d.get() >= MAX_PLIST_DEPTH {
                return Err(E::custom(format!(
                    "the property list is nested more than {MAX_PLIST_DEPTH} levels deep"
                )));
            }
            d.set(d.get() + 1);
            Ok(DepthGuard)
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        PROBE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Accepts any property list and keeps nothing but the nesting depth check.
struct DepthProbe;

impl<'de> serde::Deserialize<'de> for DepthProbe {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<DepthProbe, D::Error> {
        count_value()?;
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = DepthProbe;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a property list")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<DepthProbe, E> {
                Ok(DepthProbe)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<DepthProbe, A::Error> {
                let _level = DepthGuard::enter()?;
                while seq.next_element::<DepthProbe>()?.is_some() {}
                Ok(DepthProbe)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<DepthProbe, A::Error> {
                let _level = DepthGuard::enter()?;
                while map.next_key::<DepthProbe>()?.is_some() {
                    map.next_value::<DepthProbe>()?;
                }
                Ok(DepthProbe)
            }
        }
        d.deserialize_any(V)
    }
}

/// Read a whole file, refusing one larger than `max` bytes (without reading past `max + 1`).
pub fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        bail!("the file is unusually large (over {max} bytes)");
    }
    Ok(buf)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub window_color: Rgba,
    pub window_border: Rgba,
    /// Border width in points. `None` (the default; no `borderWidth` key) = one device pixel, whatever the screen's scale.
    pub window_border_width: Option<f32>,
    pub window_roundness: f32,
    pub window_padding: f32,
    pub window_width: f32,
    pub search_background: Rgba,
    pub search_text: Rgba,
    pub search_size: f32,
    pub search_roundness: f32,
    /// Colour of the text caret in the query field.
    pub search_caret: Rgba,
    /// Width of the caret in points.
    pub search_caret_width: f32,
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
    /// The look a user gets with no theme chosen: a square, near-black panel with a one-pixel dark grey border and a large
    /// query line with a thin white caret (docs/design/launcher-look.md). A theme file overrides every value.
    fn default() -> Self {
        Theme {
            name: "Ashdash Dark".into(),
            window_color: [5, 8, 10, 245],
            window_border: [0x24, 0x26, 0x28, 255],
            window_border_width: None,
            window_roundness: 0.0,
            window_padding: 9.0,
            window_width: 575.0,
            search_background: [0, 0, 0, 0],
            search_text: [255, 255, 255, 255],
            search_size: 34.0,
            search_roundness: 0.0,
            search_caret: [255, 255, 255, 255],
            search_caret_width: 1.0,
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
    pub fn from_bytes(bytes: &[u8]) -> Result<Theme> {
        let root = parse_plist(bytes)?;
        let t = root
            .as_dictionary()
            .and_then(|d| d.get(THEME_KEY))
            .context("missing theme dictionary")?;
        let mut out = Theme::default();
        if let Some(n) = t
            .as_dictionary()
            .and_then(|d| d.get("name"))
            .and_then(|v| v.as_string())
        {
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
        set!(
            out.window_border_width,
            num(window, "borderWidth").map(|w| Some(w.max(0.0)))
        );
        set!(out.window_roundness, num(window, "roundness"));
        set!(out.window_padding, num(window, "paddingVertical"));
        set!(out.window_width, num(window, "width"));
        set!(out.search_background, col(search, "background"));
        set!(out.search_roundness, num(search, "roundness"));
        set!(out.search_text, col(stext, "color"));
        set!(out.search_size, num(stext, "size"));
        set!(out.search_caret, col(search, "caretColor"));
        set!(out.search_caret_width, num(search, "caretWidth").map(|w| w.max(0.0)));
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

    /// Load a theme that is already on this machine (the `theme` setting, `<data>/themes/`):
    /// up to [`MAX_INSTALLED_THEME_BYTES`].
    pub fn load(path: &Path) -> Result<Theme> {
        Theme::load_limited(path, MAX_INSTALLED_THEME_BYTES)
    }

    /// Load a theme someone offers for installation: up to [`MAX_THEME_BYTES`].
    pub fn load_for_install(path: &Path) -> Result<Theme> {
        Theme::load_limited(path, MAX_THEME_BYTES)
    }

    fn load_limited(path: &Path, max: u64) -> Result<Theme> {
        let bytes = read_limited(path, max).with_context(|| format!("reading {}", path.display()))?;
        Theme::from_bytes(&bytes).with_context(|| format!("reading {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format_ids::{THEME_KEY, appearance_file};

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#358188FF"), Some([0x35, 0x81, 0x88, 0xFF]));
        assert_eq!(parse_hex("#112233"), Some([0x11, 0x22, 0x33, 0xFF]));
        assert_eq!(parse_hex("358188FF"), None);
        assert_eq!(parse_hex("#12"), None);
    }

    #[test]
    fn loads_theme_file() {
        let path = std::env::temp_dir().join(appearance_file(&format!("oa-theme-{}", std::process::id())));
        std::fs::write(
            &path,
            format!(
                r##"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>{THEME_KEY}</key><dict>
<key>name</key><string>T</string>
<key>window</key><dict><key>color</key><string>#00000066</string><key>roundness</key><integer>20</integer><key>width</key><integer>640</integer></dict>
<key>result</key><dict><key>backgroundSelected</key><string>#480069E4</string>
<key>text</key><dict><key>size</key><integer>22</integer></dict></dict>
</dict></dict></plist>"##
            ),
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

    #[test]
    fn the_default_look_is_the_square_near_black_panel() {
        let t = Theme::default();
        assert_eq!(
            t.window_color,
            [0x05, 0x08, 0x0A, 245],
            "near-black, slightly cool, ~0.96 opaque"
        );
        assert_eq!(t.window_border, [0x24, 0x26, 0x28, 255]);
        assert_eq!(t.window_border_width, None, "one device pixel");
        assert_eq!(t.window_roundness, 0.0, "square corners");
        assert_eq!(t.window_width, 575.0);
        assert_eq!(t.search_size, 34.0);
        assert_eq!(t.search_background[3], 0, "no box around the query");
        assert_eq!(t.search_text, [255, 255, 255, 255]);
        assert_eq!(t.search_caret, [255, 255, 255, 255]);
        assert_eq!(t.search_caret_width, 1.0);
    }

    fn theme_with(window: &str, search: &str) -> Theme {
        Theme::from_bytes(
            format!(
                r##"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>{THEME_KEY}</key><dict>
<key>window</key><dict>{window}</dict><key>search</key><dict>{search}</dict>
</dict></dict></plist>"##
            )
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn a_theme_without_the_new_keys_keeps_the_default_border_and_caret() {
        let t = theme_with("<key>color</key><string>#112233FF</string>", "");
        assert_eq!(t.window_color, [0x11, 0x22, 0x33, 0xFF]);
        let d = Theme::default();
        assert_eq!(t.window_border, d.window_border);
        assert_eq!(t.window_border_width, None);
        assert_eq!(
            (t.search_caret, t.search_caret_width),
            (d.search_caret, d.search_caret_width)
        );
    }

    #[test]
    fn border_and_caret_keys_are_read() {
        let t = theme_with(
            "<key>borderColor</key><string>#FF000080</string><key>borderWidth</key><real>2.5</real>",
            "<key>caretColor</key><string>#00FF00</string><key>caretWidth</key><integer>3</integer>",
        );
        assert_eq!(t.window_border, [255, 0, 0, 0x80]);
        assert_eq!(t.window_border_width, Some(2.5));
        assert_eq!(t.search_caret, [0, 255, 0, 255]);
        assert_eq!(t.search_caret_width, 3.0);
    }

    #[test]
    fn negative_widths_become_zero_and_bad_colours_are_ignored() {
        let t = theme_with(
            "<key>borderColor</key><string>nonsense</string><key>borderWidth</key><integer>-4</integer>",
            "<key>caretColor</key><string>#12</string><key>caretWidth</key><real>-1</real>",
        );
        assert_eq!(
            t.window_border_width,
            Some(0.0),
            "an explicit 0 means no border, not a hairline"
        );
        assert_eq!(t.window_border, Theme::default().window_border);
        assert_eq!(t.search_caret_width, 0.0);
        assert_eq!(t.search_caret, Theme::default().search_caret);
    }

    /// A plist of `depth` nested arrays inside the theme dictionary.
    fn nested_theme(depth: usize) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>{THEME_KEY}</key><dict><key>x</key>{}{}</dict></dict></plist>"#,
            "<array>".repeat(depth),
            "</array>".repeat(depth)
        )
    }

    fn temp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(appearance_file(&format!("oa-theme-{tag}-{}", std::process::id())));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_deeply_nested_theme_is_refused_not_a_stack_overflow() {
        // 1.9 MB, under the Finder size cap: `plist::Value::from_file` overflowed the stack on this
        // (SIGABRT, "fatal runtime error: stack overflow"). Test threads have a 2 MiB stack.
        let text = nested_theme(125_000);
        assert!(text.len() > 1_800_000 && text.len() < 2 * 1024 * 1024, "{}", text.len());
        let path = temp_file("deep", text.as_bytes());
        let err = Theme::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("nested"), "{err:#}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn nesting_up_to_the_cap_parses_and_one_more_level_does_not() {
        let at_cap = nested_theme(MAX_PLIST_DEPTH - 2); // root dict + theme dict + arrays
        assert!(parse_plist(at_cap.as_bytes()).is_ok());
        let over = nested_theme(MAX_PLIST_DEPTH - 1);
        let err = parse_plist(over.as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("nested"), "{err:#}");
        // ordinary files are untouched
        assert!(parse_plist(b"<plist version=\"1.0\"><dict/></plist>").is_ok());
        assert!(parse_plist(b"not a plist").is_err());
    }

    #[test]
    fn every_kind_of_value_passes_the_probe_in_both_encodings() {
        let mut d = plist::Dictionary::new();
        d.insert("bool".into(), true.into());
        d.insert("int".into(), 5i64.into());
        d.insert("big".into(), u64::MAX.into());
        d.insert("real".into(), 1.5f64.into());
        d.insert("str".into(), "é".into());
        d.insert("data".into(), plist::Value::Data(vec![1, 2, 3]));
        d.insert(
            "date".into(),
            plist::Value::Date(plist::Date::from_xml_format("2024-01-02T03:04:05Z").unwrap()),
        );
        d.insert("uid".into(), plist::Value::Uid(plist::Uid::new(7)));
        d.insert("arr".into(), plist::Value::Array(vec![1i64.into(), "x".into()]));
        let v = plist::Value::Dictionary(d.clone());
        let mut bin = Vec::new();
        v.to_writer_binary(&mut bin).unwrap();
        assert_eq!(parse_plist(&bin).unwrap(), v);
        // XML cannot hold a Uid
        d.remove("uid");
        let v = plist::Value::Dictionary(d);
        let mut xml = Vec::new();
        v.to_writer_xml(&mut xml).unwrap();
        assert_eq!(parse_plist(&xml).unwrap(), v);
    }

    #[test]
    fn binary_plists_are_checked_too() {
        let mut shallow = plist::Dictionary::new();
        shallow.insert("a".into(), plist::Value::Array(vec![plist::Value::Array(vec![])]));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(shallow).to_writer_binary(&mut bytes).unwrap();
        assert!(bytes.starts_with(b"bplist"));
        assert!(parse_plist(&bytes).is_ok());
        // 70 levels, written by the library (deep enough to trip the cap, shallow enough to write)
        let mut v = plist::Value::Array(vec![]);
        for _ in 0..70 {
            v = plist::Value::Array(vec![v]);
        }
        let mut deep = Vec::new();
        v.to_writer_binary(&mut deep).unwrap();
        assert!(parse_plist(&deep).is_err());
    }

    /// A binary plist in which every array holds `fan` references to the one object below it:
    /// `fan^levels` values when read as a tree, in `levels * (fan + 1) + 10 + trailer` bytes.
    fn shared_reference_bomb(levels: usize, fan: usize) -> Vec<u8> {
        assert!(fan < 15 && levels + 1 < 256);
        let mut out = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        for i in 0..levels {
            offsets.push(out.len() as u8);
            out.push(0xA0 | fan as u8);
            out.extend(std::iter::repeat_n((i + 1) as u8, fan));
        }
        offsets.push(out.len() as u8);
        out.extend([0x10, 0x01]); // the leaf: the integer 1
        let table = out.len() as u64;
        out.extend(&offsets);
        out.extend([0u8; 6]);
        out.extend([1u8, 1u8]); // offset size, reference size
        out.extend((offsets.len() as u64).to_be_bytes());
        out.extend(0u64.to_be_bytes()); // the top object
        out.extend(table.to_be_bytes());
        out
    }

    /// Run `f` on its own thread; fail (instead of hanging the suite) if it takes over `limit`.
    fn finishes_within<T: Send + 'static>(
        limit: std::time::Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).ok()
    }

    #[test]
    fn shared_references_cannot_make_the_parse_do_exponential_work() {
        // 10 references per level, 8 levels: 147 bytes, 10^8 values as a tree, 9 levels deep.
        let bomb = shared_reference_bomb(8, 10);
        assert!(bomb.len() < 200, "{}", bomb.len());
        // Unbounded, the parse would run for hours; bounded, it ends in well under a second. The limit is a hang guard only.
        let got = finishes_within(std::time::Duration::from_secs(30), move || parse_plist(&bomb));
        let got = got.expect("the parse did not return within 30 s");
        let err = got.unwrap_err();
        assert!(format!("{err:#}").contains("values"), "{err:#}");
        // the same shape with a small fan-out is ordinary and still parses
        assert!(parse_plist(&shared_reference_bomb(4, 4)).is_ok());
    }

    #[test]
    fn a_plist_with_ordinary_values_still_parses() {
        plist_with_values_parses(900);
    }

    #[test]
    #[ignore = "ci: 100,000 values (the 900-value variant above runs locally)"]
    fn a_plist_with_a_lot_of_ordinary_values_still_parses() {
        plist_with_values_parses(100_000);
    }

    fn plist_with_values_parses(n: i64) {
        // n small values, flat: well inside the bound, in both encodings
        let v = plist::Value::Array((0..n).map(plist::Value::from).collect());
        let mut xml = Vec::new();
        v.to_writer_xml(&mut xml).unwrap();
        assert_eq!(parse_plist(&xml).unwrap(), v);
        let mut bin = Vec::new();
        v.to_writer_binary(&mut bin).unwrap();
        assert_eq!(parse_plist(&bin).unwrap(), v);
        // and the count starts again for every call (a thread parses many files)
        assert_eq!(parse_plist(&xml).unwrap(), v);
    }

    #[test]
    fn real_system_plists_still_parse() {
        let mut seen = 0;
        for app in [
            "/System/Applications/Calculator.app",
            "/System/Applications/Safari.app",
            "/System/Applications/Notes.app",
        ] {
            let Ok(bytes) = read_limited(&Path::new(app).join("Contents/Info.plist"), 4 * 1024 * 1024) else {
                continue; // not on this machine
            };
            let v = parse_plist(&bytes).unwrap_or_else(|e| panic!("{app}: {e:#}"));
            assert!(v.as_dictionary().is_some());
            seen += 1;
        }
        if seen == 0 {
            eprintln!("no system app Info.plist found; skipped");
        }
    }

    /// A valid theme padded with a long string to `size` bytes.
    fn padded_theme(size: usize) -> Vec<u8> {
        let head = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>{THEME_KEY}</key><dict><key>name</key><string>Big</string><key>pad</key><string>"#
        );
        let tail = "</string></dict></dict></plist>";
        let mut v = head.as_bytes().to_vec();
        v.resize(size - tail.len(), b'x');
        v.extend_from_slice(tail.as_bytes());
        v
    }

    #[test]
    fn an_installed_theme_may_be_bigger_than_one_offered_for_installation() {
        // Review wave4b item 8: the 2 MiB install cap also applied to themes already installed,
        // so a larger one stopped loading after the upgrade.
        let three_mib = temp_file("3mib", &padded_theme(3 * 1024 * 1024));
        assert_eq!(Theme::load(&three_mib).unwrap().name, "Big");
        let err = Theme::load_for_install(&three_mib).unwrap_err();
        assert!(format!("{err:#}").contains("large"), "{err:#}");
        let at_cap = temp_file("8mib", &padded_theme(MAX_INSTALLED_THEME_BYTES as usize));
        assert_eq!(Theme::load(&at_cap).unwrap().name, "Big");
        let over = temp_file("9mib", &padded_theme(MAX_INSTALLED_THEME_BYTES as usize + 1));
        let err = Theme::load(&over).unwrap_err();
        assert!(format!("{err:#}").contains("large"), "{err:#}");
        // small themes load either way
        let small = temp_file("small2", &padded_theme(1000));
        assert!(Theme::load_for_install(&small).is_ok() && Theme::load(&small).is_ok());
        for f in [three_mib, at_cap, over, small] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn a_theme_is_read_once_from_bytes_and_files_over_the_cap_are_refused() {
        let ok = nested_theme(2);
        assert!(Theme::from_bytes(ok.as_bytes()).is_ok());
        let big = temp_file("big", &vec![b' '; (MAX_THEME_BYTES + 1) as usize]);
        let err = Theme::load_for_install(&big).unwrap_err();
        assert!(format!("{err:#}").contains("large"), "{err:#}");
        assert!(read_limited(&big, MAX_THEME_BYTES).is_err());
        let _ = std::fs::remove_file(big);
        let small = temp_file("small", ok.as_bytes());
        assert_eq!(read_limited(&small, MAX_THEME_BYTES).unwrap(), ok.as_bytes());
        let _ = std::fs::remove_file(small);
    }
}
