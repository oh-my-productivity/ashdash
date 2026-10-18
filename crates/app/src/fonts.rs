//! Lazy system fonts for Chinese, Japanese and Korean text.
//!
//! egui's built-in fonts have no CJK glyphs, so such text is drawn as empty boxes. Embedding a CJK
//! font would add 10 MB or more to the binary, so instead the launcher looks at the strings it is
//! about to draw ([`Scripts::of`]) and, the first time one contains a CJK character, reads a font
//! that ships with macOS and appends it to egui's font families as a fallback ([`Fallbacks`]).
//! A user who never sees CJK text never pays for it: no file is opened, nothing is mapped.
//!
//! The decision logic is pure and the font files come in through [`FontSource`], so both are
//! tested with a fake source; [`SystemFonts`] is the real one.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use eframe::egui::{self, FontData, FontFamily};

/// A group of scripts that one system font is loaded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Han ideographs (Chinese, Japanese kanji, Korean hanja), kana, bopomofo, CJK punctuation and
    /// the fullwidth / halfwidth forms.
    Cjk,
    /// Hangul syllables and jamo.
    Hangul,
}

impl Group {
    pub const ALL: [Group; 2] = [Group::Cjk, Group::Hangul];

    fn bit(self) -> u8 {
        match self {
            Group::Cjk => 1,
            Group::Hangul => 2,
        }
    }

    /// The name of the font in egui's `font_data`.
    fn font_name(self) -> &'static str {
        match self {
            Group::Cjk => "system-cjk",
            Group::Hangul => "system-hangul",
        }
    }

    /// System font files to try, in order of preference; the first that can be read wins.
    /// Hiragino Sans has kana and the Japanese shapes of the ideographs, PingFang the Chinese
    /// ones; the older Hiragino Sans GB and STHeiti are what a system without either still ships.
    fn files(self) -> &'static [&'static str] {
        match self {
            Group::Cjk => &[
                "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
            ],
            Group::Hangul => &[
                "/System/Library/Fonts/AppleSDGothicNeo.ttc",
                "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            ],
        }
    }

    /// File names to look for inside the downloadable font assets (macOS 13 and newer keep
    /// PingFang there instead of in `/System/Library/Fonts`).
    fn asset_names(self) -> &'static [&'static str] {
        match self {
            Group::Cjk => &["PingFang.ttc", "Hiragino Sans GB.ttc"],
            Group::Hangul => &[],
        }
    }
}

/// Folders that hold `<asset>.asset/AssetData/<font file>`.
const ASSET_ROOTS: &[&str] = &[
    "/System/Library/AssetsV2/com_apple_MobileAsset_Font8",
    "/System/Library/AssetsV2/com_apple_MobileAsset_Font7",
    "/System/Library/AssetsV2/com_apple_MobileAsset_Font6",
];

/// Which groups a text needs. A tiny bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scripts(u8);

impl Scripts {
    pub const NONE: Scripts = Scripts(0);

    pub fn contains(self, g: Group) -> bool {
        self.0 & g.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, g: Group) {
        self.0 |= g.bit();
    }

    pub fn minus(self, other: Scripts) -> Scripts {
        Scripts(self.0 & !other.0)
    }

    /// The groups needed by `text`.
    pub fn of(text: &str) -> Scripts {
        let mut s = Scripts::NONE;
        // ASCII-only text (nearly everything) is decided without decoding.
        if text.is_ascii() {
            return s;
        }
        for ch in text.chars() {
            if let Some(g) = group_of(ch) {
                s.insert(g);
                if s.contains(Group::Cjk) && s.contains(Group::Hangul) {
                    break;
                }
            }
        }
        s
    }

    pub fn of_all<'a>(texts: impl IntoIterator<Item = &'a str>) -> Scripts {
        let mut s = Scripts::NONE;
        for t in texts {
            let t = Scripts::of(t);
            for g in Group::ALL {
                if t.contains(g) {
                    s.insert(g);
                }
            }
        }
        s
    }
}

/// The group a character needs a system font for, if any.
pub fn group_of(ch: char) -> Option<Group> {
    let c = ch as u32;
    if c < 0x1100 {
        return None;
    }
    match c {
        // Hangul jamo, compatibility jamo, jamo extended-A/B, syllables
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Some(Group::Hangul),
        // halfwidth Hangul
        0xFFA0..=0xFFDC => Some(Group::Hangul),
        // CJK radicals, Kangxi radicals, ideographic description characters
        0x2E80..=0x2FFF
        // CJK symbols and punctuation, hiragana, katakana
        | 0x3000..=0x30FF
        // bopomofo, kanbun, bopomofo extended, CJK strokes, katakana extension
        | 0x3100..=0x312F | 0x3190..=0x31FF
        // enclosed CJK letters and months, CJK compatibility
        | 0x3200..=0x33FF
        // CJK extension A, Yijing hexagrams (drawn with the CJK font), unified ideographs
        | 0x3400..=0x4DBF | 0x4DC0..=0x4DFF | 0x4E00..=0x9FFF
        // CJK compatibility ideographs
        | 0xF900..=0xFAFF
        // vertical forms, CJK compatibility forms, small form variants
        | 0xFE10..=0xFE1F | 0xFE30..=0xFE6F
        // fullwidth ASCII variants, halfwidth katakana and punctuation, fullwidth signs
        | 0xFF00..=0xFF9F | 0xFFE0..=0xFFEE
        // kana supplement and extensions, small kana extension
        | 0x1B000..=0x1B2FF
        // extensions B to J and compatibility supplement
        | 0x20000..=0x323AF => Some(Group::Cjk),
        _ => None,
    }
}

/// Where font files come from.
pub trait FontSource {
    /// The bytes of the font file at `path`, or `None` if it is missing or unreadable.
    fn load(&self, path: &Path) -> Option<Cow<'static, [u8]>>;
    /// The entries of a folder (full paths), empty if it cannot be listed.
    fn list(&self, dir: &Path) -> Vec<PathBuf>;
}

/// A font that was found: its egui name and data.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: &'static str,
    pub path: PathBuf,
    pub data: FontData,
}

/// Find the system font for `group`: the fixed locations first, then the font assets.
pub fn find(source: &dyn FontSource, group: Group) -> Option<Found> {
    let from_assets = || {
        ASSET_ROOTS.iter().flat_map(|root| {
            let mut dirs = source.list(Path::new(root));
            dirs.sort();
            dirs.into_iter().flat_map(|d| {
                group
                    .asset_names()
                    .iter()
                    .map(move |n| d.join("AssetData").join(n))
                    .collect::<Vec<_>>()
            })
        })
    };
    let mut candidates: Vec<PathBuf> = group.files().iter().map(PathBuf::from).collect();
    // The font assets come right after the fixed PingFang path: PingFang in the assets is preferred
    // over the legacy Chinese fonts, but not over Hiragino Sans.
    let at = candidates
        .iter()
        .position(|p| p.ends_with("PingFang.ttc"))
        .map_or(candidates.len(), |i| i + 1);
    let assets: Vec<PathBuf> = from_assets().collect();
    candidates.splice(at..at, assets);
    candidates.into_iter().find_map(|path| {
        let bytes = source.load(&path)?;
        Some(Found {
            name: group.font_name(),
            data: FontData {
                font: bytes,
                index: 0,
                tweak: Default::default(),
            },
            path,
        })
    })
}

/// Which groups have been looked for already (found or not): each is tried once per process.
#[derive(Debug, Default)]
pub struct Fallbacks {
    tried: Scripts,
}

impl Fallbacks {
    /// The fonts to add now for `needed`: every group that is needed and not tried yet.
    /// Marks them tried, so a missing font is not searched for again on every frame.
    pub fn take(&mut self, needed: Scripts, source: &dyn FontSource) -> Vec<Found> {
        let fresh = needed.minus(self.tried);
        let mut out = Vec::new();
        for g in Group::ALL {
            if fresh.contains(g) {
                self.tried.insert(g);
                if let Some(f) = find(source, g) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// True when nothing is left to load (so callers can skip scanning text).
    pub fn done(&self) -> bool {
        Group::ALL.iter().all(|g| self.tried.contains(*g))
    }

    /// Scan `texts`; when they need a font that is not loaded yet, load it and add it to egui as
    /// the last fallback of the proportional and monospace families. Returns true if a font was
    /// added (the glyphs appear from the next pass, so the caller should repaint).
    pub fn ensure<'a>(
        &mut self,
        ctx: &egui::Context,
        source: &dyn FontSource,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        if self.done() {
            return false;
        }
        let needed = Scripts::of_all(texts);
        if needed.minus(self.tried).is_empty() {
            return false;
        }
        let found = self.take(needed, source);
        let added = !found.is_empty();
        for f in found {
            ctx.add_font(egui::epaint::text::FontInsert::new(
                f.name,
                f.data,
                vec![
                    egui::epaint::text::InsertFontFamily {
                        family: FontFamily::Proportional,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                    egui::epaint::text::InsertFontFamily {
                        family: FontFamily::Monospace,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                ],
            ));
        }
        if added {
            ctx.request_repaint();
        }
        added
    }
}

/// The fonts of the running macOS. Files are mapped read-only instead of read: the CJK collections
/// are 20 to 55 MB, and only the pages of the glyphs actually drawn ever get paged in.
pub struct SystemFonts;

impl FontSource for SystemFonts {
    fn load(&self, path: &Path) -> Option<Cow<'static, [u8]>> {
        map_file(path)
            .map(Cow::Borrowed)
            .or_else(|| std::fs::read(path).ok().map(Cow::Owned))
    }

    fn list(&self, dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default()
    }
}

/// Map `path` read-only for the rest of the process (the mapping is leaked on purpose: egui keeps the
/// slice for as long as the font is in use, and each group is loaded at most once).
fn map_file(path: &Path) -> Option<&'static [u8]> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if len == 0 {
        return None;
    }
    // SAFETY: a private read-only mapping of a regular file; the system font files are on the sealed
    // system volume and are never modified while the process runs. The pointer is checked against
    // MAP_FAILED and the slice is never written through.
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        );
        if p == libc::MAP_FAILED {
            return None;
        }
        Some(std::slice::from_raw_parts(p.cast::<u8>(), len))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeFonts {
        files: HashMap<PathBuf, &'static [u8]>,
        dirs: HashMap<PathBuf, Vec<PathBuf>>,
        asked: RefCell<Vec<PathBuf>>,
    }

    impl FakeFonts {
        fn with(mut self, path: &str, bytes: &'static [u8]) -> Self {
            self.files.insert(PathBuf::from(path), bytes);
            self
        }
    }

    impl FontSource for FakeFonts {
        fn load(&self, path: &Path) -> Option<Cow<'static, [u8]>> {
            self.asked.borrow_mut().push(path.to_path_buf());
            self.files.get(path).map(|b| Cow::Borrowed(*b))
        }
        fn list(&self, dir: &Path) -> Vec<PathBuf> {
            self.dirs.get(dir).cloned().unwrap_or_default()
        }
    }

    #[test]
    fn scripts_are_detected_per_range() {
        let cjk = [
            '日', '本', '語', // ideographs
            '中', '文', '𠀀', // extension B
            'ひ', 'ら', 'が', 'な', // hiragana
            'カ', 'タ', 'ナ', // katakana
            'ｱ',  // halfwidth katakana
            'Ａ', '！', '，', // fullwidth forms
            '、', '。', '「', '」', // CJK punctuation
            '㈱', '㌔', // enclosed / compatibility
            'ㄅ', // bopomofo
            '豈', // compatibility ideograph
        ];
        for ch in cjk {
            assert_eq!(group_of(ch), Some(Group::Cjk), "{ch} U+{:04X}", ch as u32);
        }
        let hangul = ['한', '국', '어', '\u{1100}', '\u{3131}', '\u{D7B0}', '\u{FFA1}'];
        for ch in hangul {
            assert_eq!(group_of(ch), Some(Group::Hangul), "{ch} U+{:04X}", ch as u32);
        }
        let none = [
            'a', 'Z', '0', ' ', 'ü', 'é', 'ß', 'я', 'α', '€', '⌘', '→', '•', '▣', '😀', 'ا', 'א',
        ];
        for ch in none {
            assert_eq!(group_of(ch), None, "{ch} U+{:04X}", ch as u32);
        }
    }

    #[test]
    fn range_edges() {
        for (c, want) in [
            (0x2E7F, None),
            (0x2E80, Some(Group::Cjk)),
            (0x30FF, Some(Group::Cjk)),
            (0x3100, Some(Group::Cjk)),
            (0x312F, Some(Group::Cjk)),
            (0x3130, Some(Group::Hangul)),
            (0x318F, Some(Group::Hangul)),
            (0x3190, Some(Group::Cjk)),
            (0x33FF, Some(Group::Cjk)),
            (0x3400, Some(Group::Cjk)),
            (0x9FFF, Some(Group::Cjk)),
            (0xA000, None),
            (0xABFF, None),
            (0xAC00, Some(Group::Hangul)),
            (0xD7A3, Some(Group::Hangul)),
            (0xD7FF, Some(Group::Hangul)),
            (0xF8FF, None),
            (0xF900, Some(Group::Cjk)),
            (0xFAFF, Some(Group::Cjk)),
            (0xFEFF, None),
            (0xFF00, Some(Group::Cjk)),
            (0xFF9F, Some(Group::Cjk)),
            (0xFFA0, Some(Group::Hangul)),
            (0xFFDC, Some(Group::Hangul)),
            (0xFFE0, Some(Group::Cjk)),
            (0xFFFD, None),
            (0x1F600, None),
            (0x20000, Some(Group::Cjk)),
            (0x2A6DF, Some(Group::Cjk)),
        ] {
            let ch = char::from_u32(c).unwrap();
            assert_eq!(group_of(ch), want, "U+{c:04X}");
        }
    }

    #[test]
    fn text_scanning() {
        assert!(Scripts::of("").is_empty());
        assert!(Scripts::of("Safari — Café ⌘1 😀").is_empty());
        assert!(!Scripts::of("open 日本語").is_empty());
        let s = Scripts::of("日本語 and 한국어");
        assert!(s.contains(Group::Cjk) && s.contains(Group::Hangul));
        let s = Scripts::of("한국어");
        assert!(!s.contains(Group::Cjk) && s.contains(Group::Hangul));
        let s = Scripts::of_all(["plain", "ひらがな", ""]);
        assert!(s.contains(Group::Cjk) && !s.contains(Group::Hangul));
        assert!(Scripts::of_all(Vec::<&str>::new()).is_empty());
    }

    #[test]
    fn no_font_is_opened_while_no_text_needs_one() {
        let src = FakeFonts::default().with("/System/Library/Fonts/PingFang.ttc", b"x");
        let mut fb = Fallbacks::default();
        let needed = Scripts::of_all(["Safari", "Café", "", "⌘1"]);
        assert!(fb.take(needed, &src).is_empty());
        assert!(src.asked.borrow().is_empty(), "nothing may be read for Latin text");
        assert!(!fb.done());
    }

    #[test]
    fn the_first_readable_candidate_wins_and_loads_once() {
        let src = FakeFonts::default()
            .with("/System/Library/Fonts/PingFang.ttc", b"pingfang")
            .with("/System/Library/Fonts/STHeiti Light.ttc", b"heiti");
        let mut fb = Fallbacks::default();
        let got = fb.take(Scripts::of("日本語"), &src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "system-cjk");
        assert_eq!(got[0].data.font.as_ref(), b"pingfang");
        // Hiragino Sans is tried first, then PingFang; the legacy fonts are not touched
        let asked = src.asked.borrow().clone();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(asked[0].to_string_lossy().contains("ヒラギノ"));
        // asking again, with the same or other CJK text, loads nothing
        let before = asked.len();
        assert!(fb.take(Scripts::of("中文"), &src).is_empty());
        assert_eq!(src.asked.borrow().len(), before);
    }

    #[test]
    fn hiragino_sans_is_preferred_for_ideographs_and_kana() {
        let src = FakeFonts::default()
            .with("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", b"hiragino")
            .with("/System/Library/Fonts/PingFang.ttc", b"pingfang");
        let got = Fallbacks::default().take(Scripts::of("かな"), &src);
        assert_eq!(got[0].data.font.as_ref(), b"hiragino");
    }

    #[test]
    fn pingfang_is_found_in_the_font_assets() {
        let root = ASSET_ROOTS[0];
        let asset = format!("{root}/abc123.asset");
        let mut src = FakeFonts::default().with(&format!("{asset}/AssetData/PingFang.ttc"), b"asset-pingfang");
        src.dirs.insert(PathBuf::from(root), vec![PathBuf::from(&asset)]);
        let got = find(&src, Group::Cjk).expect("found in assets");
        assert_eq!(got.data.font.as_ref(), b"asset-pingfang");
        assert!(got.path.starts_with(root));
    }

    #[test]
    fn assets_beat_the_legacy_chinese_fonts() {
        let root = ASSET_ROOTS[0];
        let asset = format!("{root}/abc123.asset");
        let mut src = FakeFonts::default()
            .with(&format!("{asset}/AssetData/PingFang.ttc"), b"asset-pingfang")
            .with("/System/Library/Fonts/Hiragino Sans GB.ttc", b"gb");
        src.dirs.insert(PathBuf::from(root), vec![PathBuf::from(&asset)]);
        assert_eq!(find(&src, Group::Cjk).unwrap().data.font.as_ref(), b"asset-pingfang");
    }

    #[test]
    fn hangul_and_cjk_load_independently() {
        let src = FakeFonts::default()
            .with("/System/Library/Fonts/PingFang.ttc", b"pf")
            .with("/System/Library/Fonts/AppleSDGothicNeo.ttc", b"sd");
        let mut fb = Fallbacks::default();
        let first = fb.take(Scripts::of("한국어"), &src);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name, "system-hangul");
        assert!(!fb.done());
        let second = fb.take(Scripts::of("한국어 日本語"), &src);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].name, "system-cjk");
        assert!(fb.done());
    }

    #[test]
    fn a_missing_font_is_searched_for_once_and_text_still_works() {
        let src = FakeFonts::default();
        let mut fb = Fallbacks::default();
        assert!(fb.take(Scripts::of("日本語"), &src).is_empty());
        let asked = src.asked.borrow().len();
        assert!(asked > 0);
        assert!(fb.take(Scripts::of("日本語"), &src).is_empty());
        assert_eq!(
            src.asked.borrow().len(),
            asked,
            "a missing font must not be retried every frame"
        );
    }

    #[test]
    fn ensure_adds_the_font_to_the_context_only_when_needed() {
        let ctx = egui::Context::default();
        let src = FakeFonts::default().with(
            "/System/Library/Fonts/PingFang.ttc",
            include_bytes!("../resources/fonts/Inter-Regular.ttf"),
        );
        let mut fb = Fallbacks::default();
        assert!(!fb.ensure(&ctx, &src, ["hello", ""]));
        assert!(src.asked.borrow().is_empty());
        assert!(fb.ensure(&ctx, &src, ["hello", "日本語"]));
        assert!(!fb.ensure(&ctx, &src, ["日本語"]), "already loaded");
    }

    #[test]
    fn the_real_source_maps_a_file_and_rejects_a_missing_one() {
        let dir = std::env::temp_dir().join(format!("ashdash-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.ttc");
        std::fs::write(&file, b"font bytes").unwrap();
        assert_eq!(SystemFonts.load(&file).unwrap().as_ref(), b"font bytes");
        assert!(SystemFonts.load(&dir.join("missing.ttc")).is_none());
        assert!(SystemFonts.load(&dir.join("empty-never")).is_none());
        std::fs::write(dir.join("empty.ttc"), b"").unwrap();
        // an empty file cannot be mapped; the fallback read returns it, egui rejects it later
        assert!(SystemFonts.load(&dir.join("empty.ttc")).is_some());
        assert!(SystemFonts.list(&dir).contains(&file));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
