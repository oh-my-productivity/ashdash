//! Dynamic placeholders for snippets (modelled on the reference launcher's documented syntax).
//!
//! | Placeholder | Result |
//! |---|---|
//! | `{clipboard}`, `{clipboard:N}` | Nth previous text clipboard entry (0 = latest); modifiers `.uppercase .lowercase .capitals .trim .reverse .stripdiacritics .stripnonalphanumeric` |
//! | `{date}`, `{time}`, `{datetime}` | `2024-01-31`, `13:05:09`, `2024-01-31 13:05:09` (locale independent) |
//! | `{date +7D -1h:FORMAT}` | date arithmetic (`Y M D h m s`), optional format |
//! | `{isodate}`, `{isotime}`, `{isodatetime}` | like the above; `isodatetime` is RFC 3339 |
//! | `FORMAT` | `short`/`medium`/`long`/`full` (en_US), a Unicode pattern (`yyyy-MM-dd HH:mm`) or, if it contains `%`, a strftime string (`%Y-%m-%d`) |
//! | `{random:uuid}`, `{random:1..5}`, `{random:a,b,c}` | random UUID / integer / list item |
//! | `{snippet:keyword}` | another snippet's text (not expanded recursively) |
//! | `{cursor}` | where the caret should end up after pasting |
//!
//! Unknown placeholders (`{query}`, `{var:x}`, JSON braces...) are left as is.
//! Month and weekday names are English. Date arithmetic works on local
//! wall-clock time and ignores DST transitions.

use std::cell::Cell;

/// The result of expanding a snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Characters between the `{cursor}` marker and the end of the text, i.e.
    /// how many times to press Left after pasting. `None` without a marker.
    pub cursor_back: Option<usize>,
}

/// Looks up the raw text of a snippet by keyword.
pub type SnippetResolver<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Everything expansion depends on, injectable for tests.
pub struct ExpandCtx<'a> {
    /// Text clipboard entries, newest first.
    pub clipboard: &'a [String],
    /// Unix seconds.
    pub now: i64,
    /// Seconds east of UTC for local time.
    pub utc_offset: i32,
    /// Resolves `{snippet:keyword}` to the raw text of that snippet.
    pub snippets: Option<&'a SnippetResolver<'a>>,
    rng: Cell<u64>,
}

impl<'a> ExpandCtx<'a> {
    pub fn new(clipboard: &'a [String], now: i64, utc_offset: i32, seed: u64) -> Self {
        ExpandCtx {
            clipboard,
            now,
            utc_offset,
            snippets: None,
            rng: Cell::new(seed),
        }
    }

    /// Current time, local offset and an OS-seeded random generator.
    pub fn system(clipboard: &'a [String]) -> Self {
        let now = super::now() as i64;
        Self::new(clipboard, now, local_utc_offset(now), os_seed())
    }

    pub fn with_snippets(mut self, f: &'a dyn Fn(&str) -> Option<String>) -> Self {
        self.snippets = Some(f);
        self
    }

    fn next_u64(&self) -> u64 {
        // splitmix64
        let mut z = self.rng.get().wrapping_add(0x9e37_79b9_7f4a_7c15);
        self.rng.set(z);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Seed from /dev/urandom, falling back to the clock. Snippet randomness is
/// not security-sensitive.
fn os_seed() -> u64 {
    use std::io::Read;
    let mut b = [0u8; 8];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
    {
        return u64::from_le_bytes(b);
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678)
}

/// Seconds east of UTC for the system time zone at `unix`.
#[cfg(all(unix, target_pointer_width = "64"))]
pub fn local_utc_offset(unix: i64) -> i32 {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: std::ffi::c_long,
        zone: *const std::ffi::c_char,
    }
    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
    // SAFETY: `localtime_r` is thread-safe and writes a complete `struct tm`
    // (layout above matches the BSD/glibc definition) into `tm` when it
    // returns non-null; we only read it in that case.
    unsafe {
        if localtime_r(&unix, tm.as_mut_ptr()).is_null() {
            return 0;
        }
        tm.assume_init().gmtoff as i32
    }
}

#[cfg(not(all(unix, target_pointer_width = "64")))]
pub fn local_utc_offset(_unix: i64) -> i32 {
    0
}

/// Expand every supported placeholder in `text`.
pub fn expand(text: &str, ctx: &ExpandCtx) -> Expanded {
    expand_inner(text, ctx, true)
}

fn expand_inner(text: &str, ctx: &ExpandCtx, allow_snippet: bool) -> Expanded {
    let mut out = String::with_capacity(text.len());
    let mut cursor_at: Option<usize> = None;
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        // The placeholder body ends at the next '}' and may not contain '{' or a newline.
        let body_end = after.find(['}', '{', '\n']).filter(|&i| after.as_bytes()[i] == b'}');
        let Some(end) = body_end else {
            out.push('{');
            rest = after;
            continue;
        };
        let inner = &after[..end];
        if inner == "cursor" {
            if cursor_at.is_none() {
                cursor_at = Some(out.chars().count());
            }
            rest = &after[end + 1..];
            continue;
        }
        match placeholder(inner, ctx, allow_snippet) {
            Some(s) => {
                out.push_str(&s);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let cursor_back = cursor_at.map(|at| out.chars().count() - at);
    Expanded { text: out, cursor_back }
}

fn placeholder(inner: &str, ctx: &ExpandCtx, allow_snippet: bool) -> Option<String> {
    let head_end = inner.find([':', '.', ' ']).unwrap_or(inner.len());
    let (head, rest) = inner.split_at(head_end);
    match head {
        "clipboard" => clipboard_placeholder(rest, ctx),
        "date" | "time" | "datetime" | "isodate" | "isotime" | "isodatetime" => date_placeholder(head, rest, ctx),
        "random" => random_placeholder(rest.strip_prefix(':')?, ctx),
        "snippet" if allow_snippet => {
            let raw = ctx.snippets?(rest.strip_prefix(':')?.trim())?;
            Some(expand_inner(&raw, ctx, false).text)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- clipboard

fn clipboard_placeholder(rest: &str, ctx: &ExpandCtx) -> Option<String> {
    // rest: "", ":N", ":N.mod.mod", ".mod.mod"
    let (index, mods) = match rest.strip_prefix(':') {
        Some(r) => match r.split_once('.') {
            Some((i, m)) => (i, Some(m)),
            None => (r, None),
        },
        None => ("", rest.strip_prefix('.')),
    };
    let n: usize = if index.is_empty() {
        0
    } else {
        index.trim().parse().ok()?
    };
    let mut s = ctx.clipboard.get(n).cloned().unwrap_or_default();
    for m in mods.into_iter().flat_map(|m| m.split('.')) {
        s = apply_modifier(m, s)?;
    }
    Some(s)
}

fn apply_modifier(m: &str, s: String) -> Option<String> {
    Some(match m {
        "uppercase" => s.to_uppercase(),
        "lowercase" => s.to_lowercase(),
        "capitals" | "capitalcase" => capitalize_words(&s),
        "trim" => s.trim().to_string(),
        "reverse" => s.chars().rev().collect(),
        "stripdiacritics" => s.chars().map(fold_diacritic).collect(),
        "stripnonalphanumeric" => s.chars().filter(|c| c.is_alphanumeric()).collect(),
        _ => return None, // unknown modifier: leave the whole placeholder untouched
    })
}

fn capitalize_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// Fold common Latin accented letters to ASCII (no normalization tables
/// available without a dependency; covers Latin-1 and the usual extras).
fn fold_diacritic(c: char) -> char {
    const FOLD: &[(&str, char)] = &[
        ("àáâãäåāăą", 'a'),
        ("çćĉč", 'c'),
        ("ďđ", 'd'),
        ("èéêëēĕėęě", 'e'),
        ("ĝğģ", 'g'),
        ("ĥ", 'h'),
        ("ìíîïĩīĭįı", 'i'),
        ("ĵ", 'j'),
        ("ķ", 'k'),
        ("ĺļľł", 'l'),
        ("ñńņň", 'n'),
        ("òóôõöøōŏő", 'o'),
        ("ŕŗř", 'r'),
        ("śŝşš", 's'),
        ("ţť", 't'),
        ("ùúûüũūŭůűų", 'u'),
        ("ŵ", 'w'),
        ("ýÿŷ", 'y'),
        ("źżž", 'z'),
        ("ÀÁÂÃÄÅĀĂĄ", 'A'),
        ("ÇĆĈČ", 'C'),
        ("ĎĐ", 'D'),
        ("ÈÉÊËĒĔĖĘĚ", 'E'),
        ("ĜĞĢ", 'G'),
        ("Ĥ", 'H'),
        ("ÌÍÎÏĨĪĬĮİ", 'I'),
        ("Ĵ", 'J'),
        ("Ķ", 'K'),
        ("ĹĻĽŁ", 'L'),
        ("ÑŃŅŇ", 'N'),
        ("ÒÓÔÕÖØŌŎŐ", 'O'),
        ("ŔŖŘ", 'R'),
        ("ŚŜŞŠ", 'S'),
        ("ŢŤ", 'T'),
        ("ÙÚÛÜŨŪŬŮŰŲ", 'U'),
        ("Ŵ", 'W'),
        ("ÝŸŶ", 'Y'),
        ("ŹŻŽ", 'Z'),
    ];
    FOLD.iter().find(|(from, _)| from.contains(c)).map_or(c, |(_, to)| *to)
}

// ---------------------------------------------------------------- random

fn random_placeholder(arg: &str, ctx: &ExpandCtx) -> Option<String> {
    if arg.eq_ignore_ascii_case("uuid") {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&ctx.next_u64().to_le_bytes());
        b[8..].copy_from_slice(&ctx.next_u64().to_le_bytes());
        b[6] = (b[6] & 0x0f) | 0x40; // version 4
        b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
        let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
        return Some(format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        ));
    }
    if let Some((lo, hi)) = arg.split_once("..") {
        let parse = |s: &str, default: i64| -> Option<i64> {
            let s = s.trim();
            if s.is_empty() { Some(default) } else { s.parse().ok() }
        };
        let (mut lo, mut hi) = (parse(lo, 0)?, parse(hi, i32::MAX as i64)?);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        let span = (hi as i128 - lo as i128 + 1) as u128;
        let pick = (ctx.next_u64() as u128 % span) as i128 + lo as i128;
        return Some(pick.to_string());
    }
    let items: Vec<&str> = arg.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if items.is_empty() {
        return None;
    }
    Some(items[(ctx.next_u64() % items.len() as u64) as usize].to_string())
}

// ---------------------------------------------------------------- dates

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Local civil date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    /// Seconds east of UTC.
    pub offset: i32,
}

impl DateTime {
    pub fn from_unix(unix: i64, offset: i32) -> Self {
        let local = unix + offset as i64;
        let days = local.div_euclid(86_400);
        let tod = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (tod / 3600) as u32,
            min: ((tod / 60) % 60) as u32,
            sec: (tod % 60) as u32,
            offset,
        }
    }

    pub fn to_unix(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + self.hour as i64 * 3600
            + self.min as i64 * 60
            + self.sec as i64
            - self.offset as i64
    }

    /// 0 = Sunday.
    pub fn weekday(&self) -> usize {
        (days_from_civil(self.year, self.month, self.day) + 4).rem_euclid(7) as usize
    }

    /// 1-based day of the year.
    pub fn yday(&self) -> u32 {
        (days_from_civil(self.year, self.month, self.day) - days_from_civil(self.year, 1, 1)) as u32 + 1
    }

    fn add_months(&mut self, months: i64) {
        let total = self.year * 12 + (self.month as i64 - 1) + months;
        self.year = total.div_euclid(12);
        self.month = total.rem_euclid(12) as u32 + 1;
        self.day = self.day.min(days_in_month(self.year, self.month));
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Date,
    Time,
    DateTime,
}

fn date_placeholder(head: &str, rest: &str, ctx: &ExpandCtx) -> Option<String> {
    let (part, iso) = match head {
        "date" => (Part::Date, false),
        "time" => (Part::Time, false),
        "datetime" => (Part::DateTime, false),
        "isodate" => (Part::Date, true),
        "isotime" => (Part::Time, true),
        _ => (Part::DateTime, true),
    };
    // rest: "", ":FMT", " +1D -2h", " +1D:FMT"
    let (arith, fmt) = match rest.split_once(':') {
        Some((a, f)) => (a, Some(f)),
        None => (rest, None),
    };
    if !arith.is_empty() && !arith.starts_with(' ') {
        return None;
    }
    let mut dt = DateTime::from_unix(ctx.now, ctx.utc_offset);
    let mut seconds = 0i64;
    for tok in arith.split_whitespace() {
        let (sign, body) = match tok.as_bytes().first()? {
            b'+' => (1i64, &tok[1..]),
            b'-' => (-1i64, &tok[1..]),
            _ => return None,
        };
        let unit = body.chars().last()?;
        let n: i64 = body[..body.len() - unit.len_utf8()].parse().ok()?;
        match unit {
            'Y' => dt.add_months(sign * n * 12),
            'M' => dt.add_months(sign * n),
            'D' | 'd' => seconds += sign * n * 86_400,
            'h' => seconds += sign * n * 3600,
            'm' => seconds += sign * n * 60,
            's' => seconds += sign * n,
            _ => return None,
        }
    }
    dt = DateTime::from_unix(dt.to_unix() + seconds, ctx.utc_offset);
    Some(format_date(&dt, part, iso, fmt))
}

fn format_date(dt: &DateTime, part: Part, iso: bool, fmt: Option<&str>) -> String {
    let pattern: String = match fmt {
        None | Some("") => default_pattern(part, iso).to_string(),
        Some(f) => match named_pattern(f.trim(), part) {
            Some(p) => p,
            None if f.contains('%') => return strftime(f, dt),
            None => f.to_string(),
        },
    };
    unicode_format(&pattern, dt)
}

fn default_pattern(part: Part, iso: bool) -> &'static str {
    match (part, iso) {
        (Part::Date, _) => "yyyy-MM-dd",
        (Part::Time, _) => "HH:mm:ss",
        (Part::DateTime, false) => "yyyy-MM-dd HH:mm:ss",
        (Part::DateTime, true) => "yyyy-MM-dd'T'HH:mm:ssxxx",
    }
}

/// `short`/`medium`/`long`/`full`, in en_US style.
fn named_pattern(name: &str, part: Part) -> Option<String> {
    let (date, time, join) = match name {
        "short" => ("M/d/yy", "h:mm a", ", "),
        "medium" => ("MMM d, yyyy", "h:mm:ss a", ", "),
        "long" => ("MMMM d, yyyy", "h:mm:ss a Z", " 'at' "),
        "full" => ("EEEE, MMMM d, yyyy", "h:mm:ss a Z", " 'at' "),
        _ => return None,
    };
    Some(match part {
        Part::Date => date.to_string(),
        Part::Time => time.to_string(),
        Part::DateTime => format!("{date}{join}{time}"),
    })
}

fn pad(n: i64, width: usize) -> String {
    format!("{n:0width$}")
}

fn fmt_offset(offset: i32, colon: bool, zulu: bool, short: bool) -> String {
    if zulu && offset == 0 {
        return "Z".into();
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let a = offset.abs();
    let (h, m) = (a / 3600, (a / 60) % 60);
    if short && m == 0 {
        format!("{sign}{h:02}")
    } else if colon {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}")
    }
}

/// Unicode TR35 date pattern subset (letters y M L d D E e a H h K k m s S Z z x X G, quotes).
pub fn unicode_format(pattern: &str, dt: &DateTime) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let h12 = if dt.hour.is_multiple_of(12) { 12 } else { dt.hour % 12 };
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = chars[i..].iter().take_while(|x| **x == c).count();
        i += n;
        let month_name = MONTHS[dt.month as usize - 1];
        let day_name = DAYS[dt.weekday()];
        match c {
            'y' if n == 2 => out.push_str(&pad(dt.year.rem_euclid(100), 2)),
            'y' => out.push_str(&pad(dt.year, n)),
            'M' | 'L' => match n {
                1 | 2 => out.push_str(&pad(dt.month as i64, n)),
                3 => out.push_str(&month_name[..3]),
                4 => out.push_str(month_name),
                _ => out.push_str(&month_name[..1]),
            },
            'd' => out.push_str(&pad(dt.day as i64, n.min(2))),
            'D' => out.push_str(&pad(dt.yday() as i64, n.min(3))),
            'E' => match n {
                1..=3 => out.push_str(&day_name[..3]),
                4 => out.push_str(day_name),
                5 => out.push_str(&day_name[..1]),
                _ => out.push_str(&day_name[..2]),
            },
            'e' | 'c' => out.push_str(&pad(dt.weekday() as i64 + 1, n.min(2))),
            'a' => out.push_str(if dt.hour < 12 { "AM" } else { "PM" }),
            'H' => out.push_str(&pad(dt.hour as i64, n.min(2))),
            'h' => out.push_str(&pad(h12 as i64, n.min(2))),
            'K' => out.push_str(&pad((dt.hour % 12) as i64, n.min(2))),
            'k' => out.push_str(&pad(if dt.hour == 0 { 24 } else { dt.hour as i64 }, n.min(2))),
            'm' => out.push_str(&pad(dt.min as i64, n.min(2))),
            's' => out.push_str(&pad(dt.sec as i64, n.min(2))),
            'S' => out.push_str(&"0".repeat(n)), // no sub-second precision
            'G' => out.push_str(if n == 4 { "Anno Domini" } else { "AD" }),
            'Z' => match n {
                1..=3 => out.push_str(&fmt_offset(dt.offset, false, false, false)),
                4 => out.push_str(&format!("GMT{}", fmt_offset(dt.offset, true, false, false))),
                _ => out.push_str(&fmt_offset(dt.offset, true, true, false)),
            },
            'z' | 'O' => {
                if dt.offset == 0 {
                    out.push_str(if c == 'z' { "UTC" } else { "GMT" });
                } else {
                    out.push_str(&format!("GMT{}", fmt_offset(dt.offset, true, false, false)));
                }
            }
            'x' | 'X' => {
                let zulu = c == 'X';
                out.push_str(&match n {
                    1 => fmt_offset(dt.offset, false, zulu, true),
                    2 | 4 => fmt_offset(dt.offset, false, zulu, false),
                    _ => fmt_offset(dt.offset, true, zulu, false),
                })
            }
            other => out.extend(std::iter::repeat_n(other, n)),
        }
    }
    out
}

/// strftime subset: `%Y %y %C %m %d %e %H %I %M %S %p %A %a %B %b %h %j %u %w
/// %z %Z %F %T %R %D %s %n %t %%`, plus the `-` (no padding) flag, e.g. `%-d`.
pub fn strftime(fmt: &str, dt: &DateTime) -> String {
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    let h12 = if dt.hour.is_multiple_of(12) { 12 } else { dt.hour % 12 };
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let nopad = it.peek() == Some(&'-');
        if nopad {
            it.next();
        }
        let num = |n: i64, w: usize| if nopad { n.to_string() } else { pad(n, w) };
        let Some(d) = it.next() else {
            out.push('%');
            break;
        };
        let month_name = MONTHS[dt.month as usize - 1];
        let day_name = DAYS[dt.weekday()];
        match d {
            'Y' => out.push_str(&dt.year.to_string()),
            'y' => out.push_str(&num(dt.year.rem_euclid(100), 2)),
            'C' => out.push_str(&num(dt.year.div_euclid(100), 2)),
            'm' => out.push_str(&num(dt.month as i64, 2)),
            'd' => out.push_str(&num(dt.day as i64, 2)),
            'e' => out.push_str(&if nopad {
                dt.day.to_string()
            } else {
                format!("{:>2}", dt.day)
            }),
            'H' => out.push_str(&num(dt.hour as i64, 2)),
            'I' => out.push_str(&num(h12 as i64, 2)),
            'M' => out.push_str(&num(dt.min as i64, 2)),
            'S' => out.push_str(&num(dt.sec as i64, 2)),
            'p' => out.push_str(if dt.hour < 12 { "AM" } else { "PM" }),
            'A' => out.push_str(day_name),
            'a' => out.push_str(&day_name[..3]),
            'B' => out.push_str(month_name),
            'b' | 'h' => out.push_str(&month_name[..3]),
            'j' => out.push_str(&num(dt.yday() as i64, 3)),
            'u' => out.push_str(&(if dt.weekday() == 0 { 7 } else { dt.weekday() }).to_string()),
            'w' => out.push_str(&dt.weekday().to_string()),
            'z' => out.push_str(&fmt_offset(dt.offset, false, false, false)),
            'Z' => out.push_str(&if dt.offset == 0 {
                "UTC".to_string()
            } else {
                fmt_offset(dt.offset, false, false, false)
            }),
            'F' => out.push_str(&format!("{}-{:02}-{:02}", dt.year, dt.month, dt.day)),
            'T' => out.push_str(&format!("{:02}:{:02}:{:02}", dt.hour, dt.min, dt.sec)),
            'R' => out.push_str(&format!("{:02}:{:02}", dt.hour, dt.min)),
            'D' => out.push_str(&format!("{:02}/{:02}/{:02}", dt.month, dt.day, dt.year.rem_euclid(100))),
            's' => out.push_str(&dt.to_unix().to_string()),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '%' => out.push('%'),
            other => {
                out.push('%');
                if nopad {
                    out.push('-');
                }
                out.push(other);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2024-03-05 (Tuesday) 14:07:09 UTC
    const T: i64 = 1_709_647_629;

    fn ctx(clip: &[String]) -> ExpandCtx<'_> {
        ExpandCtx::new(clip, T, 0, 42)
    }

    fn ex(text: &str) -> String {
        expand(text, &ctx(&[])).text
    }

    fn clips(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn civil_roundtrip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        for d in [-800_000i64, -1, 0, 1, 59, 60, 11_016, 19_782, 2_000_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        let dt = DateTime::from_unix(T, 0);
        assert_eq!(
            (dt.year, dt.month, dt.day, dt.hour, dt.min, dt.sec),
            (2024, 3, 5, 14, 7, 9)
        );
        assert_eq!(dt.weekday(), 2);
        assert_eq!(dt.yday(), 65);
        assert_eq!(dt.to_unix(), T);
    }

    #[test]
    fn basic_date_time_placeholders() {
        assert_eq!(ex("{date}"), "2024-03-05");
        assert_eq!(ex("{time}"), "14:07:09");
        assert_eq!(ex("{datetime}"), "2024-03-05 14:07:09");
        assert_eq!(ex("{isodatetime}"), "2024-03-05T14:07:09+00:00");
        assert_eq!(ex("{isodate}|{isotime}"), "2024-03-05|14:07:09");
    }

    #[test]
    fn local_offset_applies() {
        let c = ExpandCtx::new(&[], T, 5 * 3600 + 1800, 1);
        assert_eq!(expand("{datetime}", &c).text, "2024-03-05 19:37:09");
        assert_eq!(expand("{isodatetime}", &c).text, "2024-03-05T19:37:09+05:30");
        let c = ExpandCtx::new(&[], T, -8 * 3600, 1);
        assert_eq!(expand("{time:HH:mm Z}", &c).text, "06:07 -0800");
        // crossing midnight changes the date
        let c = ExpandCtx::new(&[], T, 10 * 3600, 1);
        assert_eq!(expand("{date}", &c).text, "2024-03-06");
    }

    #[test]
    fn strftime_formats() {
        assert_eq!(ex("{date:%Y-%m-%d}"), "2024-03-05");
        assert_eq!(ex("{datetime:%d/%m/%y %H:%M:%S}"), "05/03/24 14:07:09");
        assert_eq!(ex("{date:%A, %B %-d %Y}"), "Tuesday, March 5 2024");
        assert_eq!(ex("{date:%a %b %e %j}"), "Tue Mar  5 065");
        assert_eq!(ex("{time:%I:%M %p}"), "02:07 PM");
        assert_eq!(ex("{date:%F %T %%}"), "2024-03-05 14:07:09 %");
        assert_eq!(ex("{date:%s}"), T.to_string());
        assert_eq!(ex("{date:%Q}"), "%Q", "unknown directive kept");
    }

    #[test]
    fn unicode_formats() {
        assert_eq!(ex("{date:yyyy-MM-dd}"), "2024-03-05");
        assert_eq!(ex("{date:dd MMMM yyyy}"), "05 March 2024");
        assert_eq!(ex("{date:EEEE}"), "Tuesday");
        assert_eq!(ex("{date:EEE, d MMM yy}"), "Tue, 5 Mar 24");
        assert_eq!(ex("{time:h:mm a}"), "2:07 PM");
        assert_eq!(ex("{datetime:yyyy-MM-dd'T'HH:mm:ss}"), "2024-03-05T14:07:09");
        assert_eq!(ex("{date:'it''s' MMM}"), "it's Mar");
        assert_eq!(ex("{date:MMMM yyyy}"), "March 2024");
    }

    #[test]
    fn named_formats() {
        assert_eq!(ex("{date:short}"), "3/5/24");
        assert_eq!(ex("{date:medium}"), "Mar 5, 2024");
        assert_eq!(ex("{date:long}"), "March 5, 2024");
        assert_eq!(ex("{date:full}"), "Tuesday, March 5, 2024");
        assert_eq!(ex("{time:short}"), "2:07 PM");
        assert_eq!(ex("{datetime:medium}"), "Mar 5, 2024, 2:07:09 PM");
        assert_eq!(ex("{datetime:long}"), "March 5, 2024 at 2:07:09 PM +0000");
    }

    #[test]
    fn date_arithmetic() {
        assert_eq!(ex("{date +7D}"), "2024-03-12");
        assert_eq!(ex("{date -5D}"), "2024-02-29"); // leap year
        assert_eq!(ex("{date +1M}"), "2024-04-05");
        assert_eq!(ex("{date +1Y:yyyy}"), "2025");
        assert_eq!(ex("{time -3h -20m}"), "10:47:09");
        assert_eq!(ex("{time +10m +30s:HH:mm:ss}"), "14:17:39");
        assert_eq!(ex("{datetime -2Y +2M:yyyy-MM}"), "2022-05");
        assert_eq!(ex("{date +1M +7D:MMMM}"), "April");
    }

    #[test]
    fn month_arithmetic_clamps_day() {
        // 2024-01-31 + 1 month -> 2024-02-29
        let jan31 = days_from_civil(2024, 1, 31) * 86_400;
        let c = ExpandCtx::new(&[], jan31, 0, 1);
        assert_eq!(expand("{date +1M}", &c).text, "2024-02-29");
        assert_eq!(expand("{date -2M}", &c).text, "2023-11-30");
    }

    #[test]
    fn invalid_date_placeholders_are_left_alone() {
        for s in [
            "{date +x}",
            "{date+1D}",
            "{date 1D}",
            "{dates}",
            "{date +1Q}",
            "{time +D}",
        ] {
            assert_eq!(ex(s), s, "{s}");
        }
    }

    #[test]
    fn clipboard_placeholders() {
        let c = clips(&["third", "second", "first"]);
        let cx = ctx(&c);
        assert_eq!(expand("{clipboard}", &cx).text, "third");
        assert_eq!(expand("{clipboard:0}", &cx).text, "third");
        assert_eq!(expand("{clipboard:2}", &cx).text, "first");
        assert_eq!(
            expand("Hi {clipboard:2}, {clipboard:1}: {clipboard:0}.", &cx).text,
            "Hi first, second: third."
        );
        assert_eq!(expand("[{clipboard:9}]", &cx).text, "[]", "out of range is empty");
        assert_eq!(expand("{clipboard:x}", &cx).text, "{clipboard:x}");
        assert_eq!(expand("[{clipboard}]", &ctx(&[])).text, "[]");
    }

    #[test]
    fn clipboard_modifiers() {
        let c = clips(&["  Héllo wORLD! 123  "]);
        let cx = ctx(&c);
        assert_eq!(expand("{clipboard.trim}", &cx).text, "Héllo wORLD! 123");
        assert_eq!(expand("{clipboard:0.uppercase}", &cx).text, "  HÉLLO WORLD! 123  ");
        assert_eq!(expand("{clipboard.trim.lowercase}", &cx).text, "héllo world! 123");
        assert_eq!(expand("{clipboard.trim.capitals}", &cx).text, "Héllo World! 123");
        assert_eq!(expand("{clipboard.trim.stripdiacritics}", &cx).text, "Hello wORLD! 123");
        assert_eq!(
            expand("{clipboard.trim.stripnonalphanumeric}", &cx).text,
            "HéllowORLD123"
        );
        assert_eq!(expand("{clipboard.trim.reverse}", &cx).text, "321 !DLROw olléH");
        assert_eq!(expand("{clipboard.bogus}", &cx).text, "{clipboard.bogus}");
    }

    #[test]
    fn random_placeholders() {
        let uuid = ex("{random:uuid}");
        assert_eq!(uuid.len(), 36);
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4'), "version nibble: {uuid}");
        assert!("89AB".contains(&parts[3][..1]), "variant nibble: {uuid}");
        assert!(uuid.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        // Two draws differ.
        let c = ctx(&[]);
        assert_ne!(expand("{random:uuid}", &c).text, expand("{random:uuid}", &c).text);
        // Deterministic for a seed.
        assert_eq!(ex("{random:uuid}"), ex("{random:uuid}"));
        // Ranges stay within bounds, including all values over many draws.
        let c = ctx(&[]);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            let n: i64 = expand("{random:1..5}", &c).text.parse().unwrap();
            assert!((1..=5).contains(&n));
            seen.insert(n);
        }
        assert_eq!(seen.len(), 5);
        let n: i64 = expand("{random:5..1}", &c).text.parse().unwrap();
        assert!((1..=5).contains(&n), "swapped bounds");
        let n: i64 = expand("{random:..}", &c).text.parse().unwrap();
        assert!(n >= 0);
        let w = expand("{random:cheese, apple ,biscuits}", &c).text;
        assert!(["cheese", "apple", "biscuits"].contains(&w.as_str()), "{w}");
        assert_eq!(expand("{random}", &c).text, "{random}");
        assert_eq!(expand("{random:}", &c).text, "{random:}");
    }

    #[test]
    fn cursor_marker() {
        let r = expand("Dear {cursor},\nBye", &ctx(&[]));
        assert_eq!(r.text, "Dear ,\nBye");
        assert_eq!(r.cursor_back, Some(5)); // ",\nBye"
        let r = expand("a{cursor}", &ctx(&[]));
        assert_eq!((r.text.as_str(), r.cursor_back), ("a", Some(0)));
        let r = expand("{cursor}é{clipboard}", &ExpandCtx::new(&clips(&["xy"]), T, 0, 1));
        assert_eq!(
            (r.text.as_str(), r.cursor_back),
            ("éxy", Some(3)),
            "counts characters after expansion"
        );
        // only the first marker counts; later ones vanish
        let r = expand("a{cursor}b{cursor}c", &ctx(&[]));
        assert_eq!((r.text.as_str(), r.cursor_back), ("abc", Some(2)));
        assert_eq!(expand("no marker", &ctx(&[])).cursor_back, None);
    }

    #[test]
    fn snippet_include_is_not_recursive() {
        let lookup = |k: &str| match k {
            "sig" => Some("Best, {date:yyyy} {snippet:other}".to_string()),
            _ => None,
        };
        let c = ExpandCtx::new(&[], T, 0, 1).with_snippets(&lookup);
        assert_eq!(expand("{snippet:sig}", &c).text, "Best, 2024 {snippet:other}");
        assert_eq!(expand("{snippet:missing}", &c).text, "{snippet:missing}");
        assert_eq!(
            expand("{snippet:sig}", &ctx(&[])).text,
            "{snippet:sig}",
            "no lookup available"
        );
    }

    #[test]
    fn unknown_and_malformed_braces_are_literal() {
        for s in [
            "{query}",
            "{var:x}",
            "{}",
            "{",
            "}",
            "{ date }",
            r#"{"a": {"b": 1}}"#,
            "{date",
            "{{date}",
            "{da\nte}",
        ] {
            let got = ex(s);
            if s == "{{date}" {
                assert_eq!(got, "{2024-03-05", "inner placeholder still expands");
            } else {
                assert_eq!(got, s, "{s}");
            }
        }
        assert_eq!(ex("a {date} b {date:%Y} c"), "a 2024-03-05 b 2024 c");
        assert_eq!(ex("plain text"), "plain text");
        assert_eq!(ex("ünïcode {date} ✓"), "ünïcode 2024-03-05 ✓");
    }

    #[test]
    fn local_offset_is_sane() {
        let off = local_utc_offset(T);
        assert!((-14 * 3600..=14 * 3600).contains(&off), "{off}");
    }
}
