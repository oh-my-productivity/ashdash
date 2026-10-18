//! Pure helpers behind the Utilities objects: match modes (Conditional,
//! Filter), Replace, Split, Transform.
//!
//! The numeric codes:
//! * Conditional match modes 0..=4 = equal, not equal, greater than, less
//!   than, regex. Greater/less compare as integers, the first matching
//!   condition wins.
//! * The legacy Filter offers equal / not equal / regex.
//! * Transform entries: trim, upper, lower, capital, reverse, strip
//!   diacritics, strip non-alphanumeric = 0..=6.
//! * Replace: string / regex / empty input, in that order.
//!
//! Regexes use the `regex` crate: ICU features without a counterpart (look
//! around, back-references) fail to compile and the condition does not match.

use regex::{NoExpand, RegexBuilder};

pub(super) const MATCH_EQUAL: i64 = 0;
pub(super) const MATCH_NOT_EQUAL: i64 = 1;
pub(super) const MATCH_GREATER: i64 = 2;
pub(super) const MATCH_LESS: i64 = 3;
pub(super) const MATCH_REGEX: i64 = 4;
/// The legacy Filter's third popup entry.
pub(super) const FILTER_REGEX: i64 = 2;

/// `NSString.integerValue`: optional whitespace and sign, then digits; anything
/// else (including no digits) is 0. Saturates instead of overflowing.
pub(super) fn int_value(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut n: i64 = 0;
    for c in digits.chars().take_while(char::is_ascii_digit) {
        n = n.saturating_mul(10).saturating_add(i64::from(c as u8 - b'0'));
    }
    if neg { -n } else { n }
}

/// Evaluate one Conditional / Filter row. `Err` carries a message for a
/// pattern that cannot be used (unknown mode, bad regex).
pub(super) fn matches(
    mode: i64,
    input: &str,
    pattern: &str,
    case_sensitive: bool,
    regex_mode: i64,
) -> Result<bool, String> {
    let eq = || {
        if case_sensitive {
            input == pattern
        } else {
            input.to_lowercase() == pattern.to_lowercase()
        }
    };
    if mode == regex_mode || (regex_mode == FILTER_REGEX && mode == MATCH_REGEX) {
        let re = RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| format!("invalid regex '{pattern}': {e}"))?;
        return Ok(re.is_match(input));
    }
    match mode {
        MATCH_EQUAL => Ok(eq()),
        MATCH_NOT_EQUAL => Ok(!eq()),
        MATCH_GREATER if regex_mode == MATCH_REGEX => Ok(int_value(input) > int_value(pattern)),
        MATCH_LESS if regex_mode == MATCH_REGEX => Ok(int_value(input) < int_value(pattern)),
        other => Err(format!("unknown match mode {other}")),
    }
}

// ---------------------------------------------------------------- replace

/// Convert an ICU replacement template (`$1`, `\$`) to the `regex` crate's
/// (`${1}`, `$$`) so that `$1x` means group 1 followed by "x".
fn icu_template(t: &str) -> String {
    let mut out = String::with_capacity(t.len() + 4);
    let cs: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' if i + 1 < cs.len() => {
                if cs[i + 1] == '$' {
                    out.push_str("$$");
                } else {
                    out.push(cs[i + 1]);
                }
                i += 2;
            }
            '$' => {
                let mut j = i + 1;
                while j < cs.len() && cs[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    out.push_str("${");
                    out.extend(&cs[i + 1..j]);
                    out.push('}');
                } else {
                    out.push_str("$$");
                }
                i = j;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

pub(super) struct ReplaceOpts {
    pub mode: i64,
    pub case_insensitive: bool,
    pub multiline: bool,
}

pub(super) fn replace(input: &str, pattern: &str, with: &str, o: &ReplaceOpts) -> Result<String, String> {
    match o.mode {
        2 => Ok(if input.is_empty() {
            with.to_string()
        } else {
            input.to_string()
        }),
        1 => {
            let re = RegexBuilder::new(pattern)
                .case_insensitive(o.case_insensitive)
                .multi_line(o.multiline)
                .build()
                .map_err(|e| format!("invalid regex '{pattern}': {e}"))?;
            Ok(re.replace_all(input, icu_template(with).as_str()).into_owned())
        }
        0 => {
            if pattern.is_empty() {
                return Ok(input.to_string());
            }
            let re = RegexBuilder::new(&regex::escape(pattern))
                .case_insensitive(o.case_insensitive)
                .build()
                .map_err(|e| e.to_string())?;
            Ok(re.replace_all(input, NoExpand(with)).into_owned())
        }
        other => Err(format!("unknown replace mode {other}")),
    }
}

// ---------------------------------------------------------------- split

fn unescape_delimiter(d: &str) -> String {
    d.replace("\\n", "\n").replace("\\t", "\t").replace("\\r", "\r")
}

pub(super) fn split(input: &str, delimiter: &str, trim: bool, discard_empty: bool) -> Vec<String> {
    let delim = unescape_delimiter(delimiter);
    let parts: Vec<&str> = if delim.is_empty() {
        vec![input]
    } else {
        input.split(delim.as_str()).collect()
    };
    parts
        .into_iter()
        .map(|p| if trim { p.trim() } else { p })
        .filter(|p| !(discard_empty && p.is_empty()))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------- transform

/// Latin letters with diacritics -> base letters. Covers Latin-1 Supplement
/// and Latin Extended-A plus the common ligatures; combining marks are
/// dropped. (A full Unicode decomposition would need a normalisation crate.)
pub(super) fn strip_diacritics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let rep: &str = match c {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'Ç' | 'Ć' | 'Č' => "C",
            'ç' | 'ć' | 'č' => "c",
            'Ď' | 'Đ' => "D",
            'ď' | 'đ' => "d",
            'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ė' | 'Ę' | 'Ě' => "E",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
            'Ğ' => "G",
            'ğ' => "g",
            'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'Į' | 'İ' => "I",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
            'Ł' => "L",
            'ł' => "l",
            'Ñ' | 'Ń' | 'Ň' => "N",
            'ñ' | 'ń' | 'ň' => "n",
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'Ŕ' | 'Ř' => "R",
            'ŕ' | 'ř' => "r",
            'Ś' | 'Š' | 'Ş' => "S",
            'ś' | 'š' | 'ş' => "s",
            'Ť' | 'Ţ' => "T",
            'ť' | 'ţ' => "t",
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' | 'Ų' => "U",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
            'Ý' | 'Ÿ' => "Y",
            'ý' | 'ÿ' => "y",
            'Ź' | 'Ż' | 'Ž' => "Z",
            'ź' | 'ż' | 'ž' => "z",
            'ß' => "ss",
            'Æ' => "AE",
            'æ' => "ae",
            'Œ' => "OE",
            'œ' => "oe",
            '\u{300}'..='\u{36f}' => "",
            other => {
                out.push(other);
                continue;
            }
        };
        out.push_str(rep);
    }
    out
}

pub(super) fn transform(kind: i64, s: &str) -> Result<String, String> {
    Ok(match kind {
        0 => s.trim().to_string(),
        1 => s.to_uppercase(),
        2 => s.to_lowercase(),
        3 => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for c in s.chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start = !c.is_alphanumeric();
            }
            out
        }
        4 => s.chars().rev().collect(),
        5 => strip_diacritics(s),
        6 => s.chars().filter(|c| c.is_alphanumeric()).collect(),
        other => return Err(format!("unknown transform type {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_value_follows_nsstring() {
        assert_eq!(int_value("42"), 42);
        assert_eq!(int_value("  -7abc"), -7);
        assert_eq!(int_value("+3"), 3);
        assert_eq!(int_value("3.9"), 3);
        assert_eq!(int_value("abc"), 0);
        assert_eq!(int_value(""), 0);
        assert_eq!(int_value("99999999999999999999999"), i64::MAX);
    }

    #[test]
    fn conditional_modes() {
        let m = |mode, i, p, cs| matches(mode, i, p, cs, MATCH_REGEX);
        assert_eq!(m(MATCH_EQUAL, "Foo", "foo", false), Ok(true));
        assert_eq!(m(MATCH_EQUAL, "Foo", "foo", true), Ok(false));
        assert_eq!(m(MATCH_NOT_EQUAL, "a", "b", false), Ok(true));
        assert_eq!(m(MATCH_NOT_EQUAL, "A", "a", false), Ok(false));
        assert_eq!(m(MATCH_GREATER, "10", "9", false), Ok(true));
        assert_eq!(m(MATCH_GREATER, "9", "10", false), Ok(false)); // integers, not strings
        assert_eq!(m(MATCH_LESS, "-1", "0", false), Ok(true));
        assert_eq!(m(MATCH_REGEX, "abc123", r"\d+$", false), Ok(true));
        assert_eq!(m(MATCH_REGEX, "ABC", "abc", false), Ok(true));
        assert_eq!(m(MATCH_REGEX, "ABC", "abc", true), Ok(false));
        assert!(m(MATCH_REGEX, "x", "(", false).is_err());
        assert!(m(99, "x", "x", false).is_err());
    }

    #[test]
    fn legacy_filter_modes() {
        let m = |mode, i, p| matches(mode, i, p, true, FILTER_REGEX);
        assert_eq!(m(0, "a", "a"), Ok(true));
        assert_eq!(m(1, "a", "a"), Ok(false));
        assert_eq!(m(2, "abc", "^a"), Ok(true));
        // greater/less do not exist in the legacy filter
        assert!(m(MATCH_LESS, "2", "1").is_err());
    }

    #[test]
    fn replace_modes() {
        let o = |mode, ci, ml| ReplaceOpts {
            mode,
            case_insensitive: ci,
            multiline: ml,
        };
        assert_eq!(replace("a.b.c", ".", "-", &o(0, false, false)).unwrap(), "a-b-c");
        assert_eq!(
            replace("Hello hello", "hello", "$1", &o(0, true, false)).unwrap(),
            "$1 $1"
        );
        assert_eq!(
            replace("Hello hello", "hello", "x", &o(0, false, false)).unwrap(),
            "Hello x"
        );
        assert_eq!(
            replace("john smith", r"(\w+) (\w+)", "$2, $1x", &o(1, false, false)).unwrap(),
            "smith, johnx"
        );
        assert_eq!(replace("cost", "cost", r"\$5", &o(1, false, false)).unwrap(), "$5");
        assert_eq!(replace("a\nb", "^b", "X", &o(1, false, true)).unwrap(), "a\nX");
        assert_eq!(replace("a\nb", "^b", "X", &o(1, false, false)).unwrap(), "a\nb");
        assert_eq!(replace("", "", "dflt", &o(2, false, false)).unwrap(), "dflt");
        assert_eq!(replace("kept", "", "dflt", &o(2, false, false)).unwrap(), "kept");
        assert!(replace("x", "(", "y", &o(1, false, false)).is_err());
        assert!(replace("x", "x", "y", &o(9, false, false)).is_err());
    }

    #[test]
    fn split_options() {
        assert_eq!(split("a, b,,c", ",", false, false), ["a", " b", "", "c"]);
        assert_eq!(split("a, b,,c", ",", true, false), ["a", "b", "", "c"]);
        assert_eq!(split("a, b,,c", ",", true, true), ["a", "b", "c"]);
        assert_eq!(split("a\nb", "\\n", false, false), ["a", "b"]);
        assert_eq!(split("abc", "", false, false), ["abc"]);
    }

    #[test]
    fn transforms() {
        let t = |k, s| transform(k, s).unwrap();
        assert_eq!(t(0, "  hi \n"), "hi");
        assert_eq!(t(1, "straße"), "STRASSE");
        assert_eq!(t(2, "HeLLo"), "hello");
        assert_eq!(t(3, "hello wORLD-foo"), "Hello World-Foo");
        assert_eq!(t(4, "abc"), "cba");
        assert_eq!(t(5, "Crème brûlée Łódź"), "Creme brulee Lodz");
        assert_eq!(t(6, "a-b c_1!"), "abc1");
        assert!(transform(42, "x").is_err());
    }
}
