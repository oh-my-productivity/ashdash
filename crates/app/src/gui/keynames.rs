//! Hotkey spec <-> display string, e.g. `alt+shift+space` <-> `⌥⇧Space`.
//!
//! The spec form is what `global_hotkey::hotkey::HotKey::from_str` accepts and
//! what `config.json` stores. The display form is what a preferences window
//! shows and records. Pure functions only.
//!
//! `normalize_spec` / `parse_display` are for the preferences window
//! (TODO(prefs-window)); until it exists only the tests call them.
#![allow(dead_code)]

const CTRL: char = '⌃';
const ALT: char = '⌥';
const SHIFT: char = '⇧';
const CMD: char = '⌘';

/// (spec name, display name) for keys that are not a plain letter/digit/F-key.
const NAMED: &[(&str, &str)] = &[
    ("Space", "Space"),
    ("Enter", "↩"),
    ("Tab", "⇥"),
    ("Escape", "⎋"),
    ("Backspace", "⌫"),
    ("Delete", "⌦"),
    ("ArrowUp", "↑"),
    ("ArrowDown", "↓"),
    ("ArrowLeft", "←"),
    ("ArrowRight", "→"),
    ("Home", "↖"),
    ("End", "↘"),
    ("PageUp", "⇞"),
    ("PageDown", "⇟"),
    ("Minus", "-"),
    ("Equal", "="),
    ("Comma", ","),
    ("Period", "."),
    ("Slash", "/"),
    ("Semicolon", ";"),
    ("Quote", "'"),
    ("Backquote", "`"),
    ("BracketLeft", "["),
    ("BracketRight", "]"),
    ("Backslash", "\\"),
];

/// Extra spellings accepted when reading a spec.
const ALIASES: &[(&str, &str)] = &[("return", "Enter"), ("esc", "Escape"), ("del", "Delete")];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModSet {
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
}

impl ModSet {
    /// Returns false when the name is not a modifier.
    fn add(&mut self, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => self.ctrl = true,
            "alt" | "option" | "opt" => self.alt = true,
            "shift" => self.shift = true,
            "cmd" | "command" | "super" | "meta" => self.cmd = true,
            _ => return false,
        }
        true
    }

    fn glyphs(self) -> String {
        let mut s = String::new();
        for (on, g) in [(self.ctrl, CTRL), (self.alt, ALT), (self.shift, SHIFT), (self.cmd, CMD)] {
            if on {
                s.push(g);
            }
        }
        s
    }

    fn spec(self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (on, n) in [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (self.cmd, "cmd"),
        ] {
            if on {
                v.push(n);
            }
        }
        v
    }

    fn is_empty(self) -> bool {
        self == ModSet::default()
    }
}

/// Canonical code name (`KeyA`, `Digit1`, `F5`, `Space`...) for a spec token.
fn canonical_code(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();
    if let Some((_, c)) = ALIASES.iter().find(|(a, _)| *a == lower) {
        return Some((*c).to_string());
    }
    if let Some((c, _)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(token)) {
        return Some((*c).to_string());
    }
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_alphabetic() => return Some(format!("Key{}", ch.to_ascii_uppercase())),
        (Some(ch), None) if ch.is_ascii_digit() => return Some(format!("Digit{ch}")),
        _ => {}
    }
    if let Some(rest) = lower.strip_prefix("key")
        && rest.len() == 1
        && rest.as_bytes()[0].is_ascii_alphabetic()
    {
        return Some(format!("Key{}", rest.to_ascii_uppercase()));
    }
    if let Some(rest) = lower.strip_prefix("digit")
        && rest.len() == 1
        && rest.as_bytes()[0].is_ascii_digit()
    {
        return Some(format!("Digit{rest}"));
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|r| r.parse::<u8>().ok())
        && (1..=20).contains(&n)
    {
        return Some(format!("F{n}"));
    }
    None
}

fn code_display(code: &str) -> String {
    if let Some((_, d)) = NAMED.iter().find(|(n, _)| *n == code) {
        return (*d).to_string();
    }
    if let Some(l) = code.strip_prefix("Key") {
        return l.to_string();
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return d.to_string();
    }
    code.to_string() // F1..F20
}

/// Spec string (`alt+shift+space`) -> display string (`⌥⇧Space`).
/// `None` when the spec has no key, an unknown key or an unknown modifier.
pub fn display_hotkey(spec: &str) -> Option<String> {
    let (mods, code) = split_spec(spec)?;
    Some(format!("{}{}", mods.glyphs(), code_display(&code)))
}

fn split_spec(spec: &str) -> Option<(ModSet, String)> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    let mut set = ModSet::default();
    for m in mods {
        if !set.add(m) {
            return None;
        }
    }
    // `cmd++` style specs (the key is "+") are not supported by the hotkey crate either.
    Some((set, canonical_code(key)?))
}

/// Normalise any accepted spelling of a spec to the canonical form that
/// `HotKey::from_str` parses (`Alt+Shift+Space` -> `alt+shift+Space`).
pub fn normalize_spec(spec: &str) -> Option<String> {
    let (mods, code) = split_spec(spec)?;
    let mut v: Vec<String> = mods.spec().into_iter().map(String::from).collect();
    v.push(code);
    Some(v.join("+"))
}

/// Display string (`⌥⇧Space`) -> canonical spec (`alt+shift+Space`).
/// Requires at least one modifier, as the launcher hotkey must not eat plain typing.
pub fn parse_display(display: &str) -> Option<String> {
    let mut mods = ModSet::default();
    let mut rest = display.trim();
    while let Some(ch) = rest.chars().next() {
        match ch {
            CTRL => mods.ctrl = true,
            ALT => mods.alt = true,
            SHIFT => mods.shift = true,
            CMD => mods.cmd = true,
            _ => break,
        }
        rest = &rest[ch.len_utf8()..];
    }
    if mods.is_empty() || rest.is_empty() {
        return None;
    }
    let code = NAMED
        .iter()
        .find(|(_, d)| *d == rest)
        .map(|(c, _)| (*c).to_string())
        .or_else(|| canonical_code(rest))?;
    let mut v: Vec<String> = mods.spec().into_iter().map(String::from).collect();
    v.push(code);
    Some(v.join("+"))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use global_hotkey::hotkey::HotKey;

    use super::*;

    #[test]
    fn default_hotkey_displays_with_apple_order() {
        assert_eq!(display_hotkey("alt+shift+space").as_deref(), Some("⌥⇧Space"));
        assert_eq!(display_hotkey("cmd+ctrl+shift+alt+k").as_deref(), Some("⌃⌥⇧⌘K"));
    }

    #[test]
    fn spellings_are_accepted() {
        assert_eq!(display_hotkey("Option+Command+KeyJ").as_deref(), Some("⌥⌘J"));
        assert_eq!(display_hotkey("super+Digit1").as_deref(), Some("⌘1"));
        assert_eq!(display_hotkey("ctrl+f12").as_deref(), Some("⌃F12"));
        assert_eq!(display_hotkey("cmd+return").as_deref(), Some("⌘↩"));
        assert_eq!(display_hotkey("alt+Esc").as_deref(), Some("⌥⎋"));
    }

    #[test]
    fn invalid_specs_are_rejected() {
        assert_eq!(display_hotkey(""), None);
        assert_eq!(display_hotkey("alt+"), None);
        assert_eq!(display_hotkey("hyper+space"), None);
        assert_eq!(display_hotkey("alt+f99"), None);
        assert_eq!(display_hotkey("alt+shift+nonsense"), None);
    }

    #[test]
    fn parse_display_round_trips() {
        for spec in [
            "alt+shift+Space",
            "ctrl+alt+shift+cmd+KeyK",
            "cmd+Digit1",
            "ctrl+F5",
            "alt+Enter",
            "cmd+Backslash",
            "shift+ArrowUp",
        ] {
            let shown = display_hotkey(spec).unwrap();
            assert_eq!(parse_display(&shown).as_deref(), Some(spec), "via {shown}");
        }
    }

    #[test]
    fn parse_display_requires_a_modifier_and_a_key() {
        assert_eq!(parse_display("Space"), None);
        assert_eq!(parse_display("⌥"), None);
        assert_eq!(parse_display(""), None);
        assert_eq!(parse_display("⌥Banana"), None);
        assert_eq!(parse_display("⌥ ⇧Space"), None);
    }

    #[test]
    fn normalised_specs_parse_with_the_hotkey_crate() {
        for spec in ["alt+shift+space", "Option+Command+j", "ctrl+f5", "cmd+1", "alt+Return"] {
            let n = normalize_spec(spec).unwrap();
            assert!(HotKey::from_str(&n).is_ok(), "{spec} -> {n}");
        }
    }

    #[test]
    fn default_config_hotkey_is_displayable() {
        assert!(display_hotkey(ashdash_core::brand::DEFAULT_HOTKEY).is_some());
    }
}
