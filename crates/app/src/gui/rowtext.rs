//! Pure decisions about the text of a result row: what a subtitle is (plain text, a path or a URL), how it is shortened, and where
//! the title and subtitle sit in the row. Painting is in `results.rs`; widths are measured there and passed in as a closure.

/// Width reserved at the right edge for the `⌘N` hint column, in points.
pub const HINT_COLUMN_W: f32 = 40.0;
/// Space between the hint column's right edge and the row's right edge.
pub const HINT_MARGIN_R: f32 = 12.0;
/// Space between the text and the hint column.
pub const TEXT_GAP_R: f32 = 8.0;
/// Space between the icon and the text.
pub const ICON_GAP: f32 = 10.0;
/// Space between the title and the subtitle.
pub const TITLE_SUB_GAP: f32 = 2.0;
/// The mono subtitle is smaller than the plain one by this much (Geist Mono is wider than Inter).
pub const MONO_SIZE_DELTA: f32 = 0.5;
/// A fallback row that is not selected shows its title at this share of the normal alpha.
pub const FALLBACK_TITLE_ALPHA: f32 = 0.75;
/// Cmd+1..9 select the first nine rows; only they show a hint.
pub const HINT_ROWS: usize = 9;

/// How a subtitle is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubKind {
    /// Inter, end elision.
    Plain,
    /// Geist Mono, `~` for the home folder, middle elision.
    Path,
    /// Geist Mono, without scheme and `www.`, end elision.
    Url,
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// Whether `subtitle` is a path or a URL: after trimming it starts with `/`, `~/`, `http://`, `https://` or `file://`.
pub fn sub_kind(subtitle: &str) -> SubKind {
    let s = subtitle.trim();
    if s.starts_with('/') || s.starts_with("~/") || strip_prefix_ci(s, "file://").is_some() {
        SubKind::Path
    } else if strip_prefix_ci(s, "http://").is_some() || strip_prefix_ci(s, "https://").is_some() {
        SubKind::Url
    } else {
        SubKind::Plain
    }
}

/// `path` with a leading `home` folder written as `~` (`/Users/u/x` becomes `~/x`; `/Users/uu/x` is left alone).
pub fn tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// A URL without its scheme and a leading `www.`: `https://www.example.com/a` becomes `example.com/a`.
pub fn url_display(url: &str) -> String {
    let s = url.trim();
    let s = strip_prefix_ci(s, "https://")
        .or_else(|| strip_prefix_ci(s, "http://"))
        .unwrap_or(s);
    strip_prefix_ci(s, "www.").unwrap_or(s).to_string()
}

/// The text of a subtitle before it is shortened to fit, and how to set it.
pub fn subtitle_text(subtitle: &str, home: &str) -> (SubKind, String) {
    let kind = sub_kind(subtitle);
    let s = subtitle.trim();
    let text = match kind {
        SubKind::Plain => subtitle.to_string(),
        SubKind::Url => url_display(s),
        SubKind::Path => tilde(strip_prefix_ci(s, "file://").unwrap_or(s), home),
    };
    (kind, text)
}

/// The longest prefix of `s` (in characters) that still fits with a closing `…`, with the `…`. `fits` must be monotone
/// (a shorter text never fits worse); the result is `…` alone when not even one character fits.
pub fn end_elide(s: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(s) {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let with_ellipsis = |n: usize| {
        let mut t: String = chars[..n].iter().collect();
        t.push('…');
        t
    };
    let (mut lo, mut hi) = (0usize, chars.len()); // lo fits (or is 0), hi does not as a prefix of the original
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(&with_ellipsis(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    with_ellipsis(lo)
}

/// Shorten a path (home already written as `~`) until `fits`:
/// 1. as it is;
/// 2. the first component, `/…/`, and as many trailing components as fit, always keeping the last
///    (`~/very/long/dir/file.txt` becomes `~/…/dir/file.txt`);
/// 3. when even `first/…/last` does not fit, that text cut at its end with `…`.
pub fn elide_path(s: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(s) {
        return s.to_string();
    }
    let comps: Vec<&str> = s.split('/').filter(|c| !c.is_empty()).collect();
    if comps.len() < 3 {
        return end_elide(s, fits);
    }
    let head = if s.starts_with('/') {
        format!("/{}", comps[0])
    } else {
        comps[0].to_string()
    };
    let tail = &comps[1..];
    let mut shortest = String::new();
    for keep in (1..tail.len()).rev() {
        let candidate = format!("{head}/…/{}", tail[tail.len() - keep..].join("/"));
        if fits(&candidate) {
            return candidate;
        }
        shortest = candidate;
    }
    end_elide(&shortest, fits)
}

/// The width the title and subtitle may use: from `text_x` to the hint column (or the right padding when the row has none).
pub fn text_max_width(text_x: f32, row_right: f32, has_hint: bool) -> f32 {
    let hint_w = if has_hint { HINT_COLUMN_W } else { 0.0 };
    (row_right - HINT_MARGIN_R - hint_w - TEXT_GAP_R - text_x).max(0.0)
}

/// The text block of a row, centred vertically: the y of the title and, when there is a subtitle, of the subtitle.
/// `title_h` and `sub_h` are the heights of the laid-out lines.
pub fn block_tops(center_y: f32, title_h: f32, sub_h: Option<f32>) -> (f32, Option<f32>) {
    match sub_h {
        None => (center_y - title_h / 2.0, None),
        Some(sub_h) => {
            let block = title_h + TITLE_SUB_GAP + sub_h;
            let title_y = center_y - block / 2.0;
            (title_y, Some(title_y + title_h + TITLE_SUB_GAP))
        }
    }
}

/// The hairline above the first fallback row, inside that row: `(x0, x1, y, width)` for a row between `left` and `right` whose top is
/// `top`, on a screen with `pixels_per_point` device pixels per point. One device pixel wide, 8 pt in from each side.
pub fn separator_line(left: f32, right: f32, top: f32, pixels_per_point: f32) -> (f32, f32, f32, f32) {
    let px = if pixels_per_point.is_finite() && pixels_per_point >= 1.0 {
        1.0 / pixels_per_point
    } else {
        1.0
    };
    (left + 8.0, right - 8.0, top + px / 2.0, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake measure: every character is one unit wide.
    fn fits_in(n: usize) -> impl Fn(&str) -> bool {
        move |s: &str| s.chars().count() <= n
    }

    #[test]
    fn a_subtitle_is_a_path_or_url_by_its_start() {
        assert_eq!(sub_kind("/Applications/Safari.app"), SubKind::Path);
        assert_eq!(sub_kind("  ~/Documents/a.txt"), SubKind::Path);
        assert_eq!(sub_kind("file:///tmp/a"), SubKind::Path);
        assert_eq!(sub_kind("https://example.com/x"), SubKind::Url);
        assert_eq!(sub_kind("HTTP://example.com"), SubKind::Url);
        assert_eq!(sub_kind("Open the docs"), SubKind::Plain);
        assert_eq!(sub_kind("⌘ Reveal in Finder"), SubKind::Plain);
        assert_eq!(sub_kind("~"), SubKind::Plain, "only ~/ starts a path");
        assert_eq!(sub_kind("ftp://x"), SubKind::Plain);
        assert_eq!(sub_kind(""), SubKind::Plain);
    }

    #[test]
    fn the_home_folder_becomes_a_tilde() {
        assert_eq!(tilde("/Users/u/Documents/a.txt", "/Users/u"), "~/Documents/a.txt");
        assert_eq!(tilde("/Users/u", "/Users/u"), "~");
        assert_eq!(
            tilde("/Users/u/x", "/Users/u/"),
            "~/x",
            "a trailing slash on home is ignored"
        );
        assert_eq!(
            tilde("/Users/uu/x", "/Users/u"),
            "/Users/uu/x",
            "a longer name is not the home folder"
        );
        assert_eq!(
            tilde("/Applications/Safari.app", "/Users/u"),
            "/Applications/Safari.app"
        );
        assert_eq!(tilde("/x", ""), "/x");
        assert_eq!(tilde("/x", "/"), "/x");
    }

    #[test]
    fn a_url_loses_its_scheme_and_www() {
        assert_eq!(
            url_display("https://www.google.com/search?q=saf"),
            "google.com/search?q=saf"
        );
        assert_eq!(url_display("http://example.com/a"), "example.com/a");
        assert_eq!(
            url_display("https://en.wikipedia.org/w/index.php?search=saf"),
            "en.wikipedia.org/w/index.php?search=saf"
        );
        assert_eq!(url_display("HTTPS://WWW.Example.com"), "Example.com");
        assert_eq!(url_display("https://wwwx.example.com"), "wwwx.example.com");
    }

    #[test]
    fn subtitle_text_prepares_each_kind() {
        assert_eq!(
            subtitle_text("/Users/u/Documents/a.txt", "/Users/u"),
            (SubKind::Path, "~/Documents/a.txt".to_string())
        );
        assert_eq!(
            subtitle_text("file:///Users/u/a.txt", "/Users/u"),
            (SubKind::Path, "~/a.txt".to_string())
        );
        assert_eq!(
            subtitle_text("https://www.google.com/search?q=x", "/Users/u"),
            (SubKind::Url, "google.com/search?q=x".to_string())
        );
        assert_eq!(
            subtitle_text("Open the docs", "/Users/u"),
            (SubKind::Plain, "Open the docs".to_string())
        );
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        assert_eq!(elide_path("~/a/b/c.txt", fits_in(20)), "~/a/b/c.txt");
        assert_eq!(elide_path("", fits_in(0)), "");
    }

    #[test]
    fn the_middle_of_a_long_path_is_elided_and_the_tail_kept() {
        let p = "~/very/long/dir/file.txt";
        assert_eq!(elide_path(p, fits_in(17)), "~/…/dir/file.txt");
        // more room: more of the tail
        assert_eq!(elide_path(p, fits_in(22)), "~/…/long/dir/file.txt");
        assert_eq!(elide_path(p, fits_in(23)), "~/…/long/dir/file.txt");
        // an absolute path keeps its first component
        assert_eq!(
            elide_path("/Applications/Utilities/Some/Deep/Thing.app", fits_in(30)),
            "/Applications/…/Deep/Thing.app"
        );
    }

    #[test]
    fn the_last_component_is_always_kept_while_it_fits() {
        let p = "/Users/u/very/long/dir/file.txt";
        assert_eq!(elide_path(p, fits_in(18)), "/Users/…/file.txt");
        assert_eq!(elide_path(p, fits_in(17)), "/Users/…/file.txt");
    }

    #[test]
    fn when_even_first_and_last_do_not_fit_the_result_is_cut_at_the_end() {
        let p = "/Applications/Utilities/Some/Deep/Thing.app";
        // "/Applications/…/Thing.app" is 25 characters
        assert_eq!(elide_path(p, fits_in(10)), "/Applicat…");
        assert_eq!(elide_path(p, fits_in(1)), "…");
        assert_eq!(elide_path(p, fits_in(0)), "…", "nothing fits: the ellipsis alone");
    }

    #[test]
    fn short_paths_are_cut_at_the_end() {
        assert_eq!(elide_path("/Applications/Safari.app", fits_in(12)), "/Applicatio…");
        assert_eq!(elide_path("~/notes.txt", fits_in(6)), "~/not…");
    }

    #[test]
    fn the_elided_text_never_gets_longer_than_it_may() {
        let p = "/Users/someone/Library/Application Support/Some App/Profiles/default/cache/index.db";
        for max in 1..=p.chars().count() {
            let out = elide_path(p, fits_in(max));
            assert!(out.chars().count() <= max, "{max}: {out}");
            if max >= p.chars().count() {
                assert_eq!(out, p);
            }
        }
    }

    #[test]
    fn end_elision_handles_multibyte_text() {
        assert_eq!(end_elide("日本語のファイル名", fits_in(5)), "日本語の…");
        assert_eq!(end_elide("abc", fits_in(3)), "abc");
    }

    #[test]
    fn the_text_stops_before_the_hint_column() {
        // the default look: row 556 pt wide, icon 36 at +8, text at +54: 556 - 12 - 40 - 8 - 54 = 442
        assert_eq!(text_max_width(54.0, 556.0, true), 442.0);
        assert_eq!(text_max_width(54.0, 556.0, false), 482.0);
        assert_eq!(text_max_width(600.0, 556.0, true), 0.0, "never negative");
    }

    #[test]
    fn the_text_block_is_centred_in_the_row() {
        // 48 pt row, Inter 16 (about 19.4 tall) and 12 (about 14.5): a block of about 36 pt, 6 pt margins
        let (t, s) = block_tops(24.0, 19.0, Some(14.0));
        assert_eq!(t, 24.0 - 35.0 / 2.0);
        assert_eq!(s, Some(t + 19.0 + 2.0));
        let bottom = t + 19.0 + 2.0 + 14.0;
        assert!(
            ((24.0 - t) - (bottom - 24.0)).abs() < 1e-4,
            "equal margins above and below"
        );
        // without a subtitle the title alone is centred
        assert_eq!(block_tops(24.0, 19.0, None), (14.5, None));
    }

    #[test]
    fn the_separator_is_one_device_pixel_in_the_top_of_the_row() {
        assert_eq!(separator_line(10.0, 566.0, 100.0, 2.0), (18.0, 558.0, 100.25, 0.5));
        assert_eq!(separator_line(10.0, 566.0, 100.0, 1.0), (18.0, 558.0, 100.5, 1.0));
        let (_, _, _, w) = separator_line(0.0, 100.0, 0.0, f32::NAN);
        assert_eq!(w, 1.0, "a nonsensical scale falls back to a point");
    }
}
