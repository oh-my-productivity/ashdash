//! The preferences window's view-model: validation and the edits it makes to the settings.
//!
//! Pure data in, `Result<_, String>` out (the message is what the window shows next to the
//! field). The window never touches `Config` directly: it asks [`Store::edit`], which loads the
//! file strictly, applies one edit, and writes only when the edit is valid and changes
//! something. An unreadable `config.json` is reported and left alone, so a hand-written file
//! with a typo is never overwritten by the window.

use std::path::PathBuf;

use ashdash_core::Config;
use ashdash_core::bookmarks::ALL_SOURCE_IDS;
use ashdash_core::clipboard::Snippet;
use ashdash_core::config::{ClipboardSettings, Paths};
use ashdash_core::providers::PROVIDER_IDS;
use ashdash_core::settings_view::SnippetLoc;
use ashdash_core::websearch::{self, WebSearch};
use ashdash_core::workflow::TrustSummary;
use global_hotkey::hotkey::HotKey;

pub type EditResult = Result<(), String>;

// ------------------------------------------------------------------ pages

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    General,
    Features,
    Workflows,
    About,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::General, Page::Features, Page::Workflows, Page::About];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Features => "Features",
            Page::Workflows => "Workflows",
            Page::About => "About",
        }
    }
}

// ------------------------------------------------------------------ the settings file

/// Reads and writes `config.json` for the window.
pub struct Store {
    paths: Paths,
}

impl Store {
    pub fn new(paths: Paths) -> Store {
        Store { paths }
    }

    /// The settings on disk. A file that exists but cannot be read is an error, not defaults.
    pub fn load(&self) -> Result<Config, String> {
        Config::load_strict(&self.paths).map_err(|e| format!("config.json cannot be read: {e:#}"))
    }

    /// Apply one edit to the file's settings and save. Nothing is written when the edit fails or
    /// changes nothing. Returns the settings now in force on disk.
    pub fn edit(&self, edit: impl FnOnce(&mut Config) -> EditResult) -> Result<Config, String> {
        let old = self
            .load()
            .map_err(|e| format!("{e}. Nothing was changed; fix or delete the file first."))?;
        let mut new = old.clone();
        edit(&mut new)?;
        // The same rule `ashdash config set` holds values to: what is written must load back.
        let json = serde_json::to_value(&new).map_err(|e| e.to_string())?;
        let new: Config = serde_json::from_value(json.clone()).map_err(|e| format!("invalid setting: {e}"))?;
        if serde_json::to_value(&old).ok().as_ref() == Some(&json) {
            return Ok(old);
        }
        new.save(&self.paths)
            .map_err(|e| format!("could not save config.json: {e}"))?;
        Ok(new)
    }
}

// ------------------------------------------------------------------ General

pub const MAX_RESULTS_RANGE: std::ops::RangeInclusive<usize> = 3..=50;

pub fn set_max_results(c: &mut Config, n: usize) -> EditResult {
    if !MAX_RESULTS_RANGE.contains(&n) {
        return Err(format!(
            "show between {} and {} results",
            MAX_RESULTS_RANGE.start(),
            MAX_RESULTS_RANGE.end()
        ));
    }
    c.max_results = n;
    Ok(())
}

/// Hotkey specs that macOS already uses for something else.
const SYSTEM_HOTKEYS: &[(&str, &str)] = &[
    ("cmd+space", "Spotlight"),
    ("cmd+alt+space", "Finder search"),
    ("ctrl+space", "input source switching"),
    ("cmd+tab", "the app switcher"),
    ("cmd+q", "Quit"),
];

/// `Some(what)` when `normalized` collides with a well-known system shortcut.
pub fn system_hotkey_conflict(normalized: &str) -> Option<&'static str> {
    SYSTEM_HOTKEYS
        .iter()
        .find(|(s, _)| s.eq_ignore_ascii_case(normalized))
        .map(|(_, w)| *w)
}

/// A hotkey must parse with the same library that registers it, and carry a modifier (a bare
/// letter would swallow typing everywhere).
pub fn validate_hotkey(spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    let has_modifier = spec.split('+').count() > 1;
    if !has_modifier {
        return Err("a shortcut needs at least one modifier key (Cmd, Option, Ctrl or Shift)".into());
    }
    spec.parse::<HotKey>()
        .map(|_| spec.to_string())
        .map_err(|e| format!("not a valid shortcut: {e}"))
}

pub fn set_hotkey(c: &mut Config, spec: &str) -> EditResult {
    c.hotkey = validate_hotkey(spec)?;
    Ok(())
}

pub fn set_theme(c: &mut Config, theme: Option<PathBuf>) -> EditResult {
    c.theme = theme;
    Ok(())
}

pub fn set_learn_usage(c: &mut Config, on: bool) -> EditResult {
    c.learn_usage = on;
    Ok(())
}

// ------------------------------------------------------------------ Features: providers

/// (id, label, what it does) for every built-in feature, in the engine's precedence order.
pub const PROVIDER_INFO: &[(&str, &str, &str)] = &[
    ("workflow", "Workflows", "Keywords and results of installed workflows"),
    (
        "shell",
        "Terminal commands",
        "Type > followed by a command to run it in Terminal",
    ),
    (
        "files",
        "File search",
        "open, find, in and ' search files with Spotlight",
    ),
    (
        "actions",
        "Universal actions",
        "actions <text or path> lists what you can do with it",
    ),
    ("clipboard", "Clipboard history", "clip lists what you copied"),
    ("snippets", "Snippets", "snip lists your text snippets"),
    (
        "bookmarks",
        "Browser bookmarks",
        "bm searches Safari, Chrome, Firefox and more",
    ),
    ("web-keyword", "Web searches", "g, ddg, wiki and your own keywords"),
    (
        "path",
        "Folder browsing",
        "Type a path starting with / or ~ to browse it",
    ),
    ("calc", "Calculator", "Type an expression to get the answer"),
    ("apps", "Applications", "Find and launch apps"),
    ("system", "System commands", "Lock, sleep, restart, empty trash..."),
    (
        "web-fallback",
        "Fallback searches",
        "Offer web searches when nothing else matches",
    ),
];

pub fn provider_enabled(c: &Config, id: &str) -> bool {
    !c.providers_disabled.iter().any(|d| d == id)
}

pub fn set_provider_enabled(c: &mut Config, id: &str, enabled: bool) -> EditResult {
    if !PROVIDER_IDS.contains(&id) {
        return Err(format!("unknown feature '{id}'"));
    }
    c.providers_disabled.retain(|d| d != id);
    if !enabled {
        c.providers_disabled.push(id.to_string());
    }
    Ok(())
}

// ------------------------------------------------------------------ Features: file search

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileList {
    Roots,
    ExcludedPaths,
    ExcludedNames,
    ExcludedSubstrings,
}

impl FileList {
    pub const ALL: [FileList; 4] = [
        FileList::Roots,
        FileList::ExcludedPaths,
        FileList::ExcludedNames,
        FileList::ExcludedSubstrings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            FileList::Roots => "Search in",
            FileList::ExcludedPaths => "Never search in",
            FileList::ExcludedNames => "Skip folders and files named",
            FileList::ExcludedSubstrings => "Skip paths containing",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            FileList::Roots | FileList::ExcludedPaths => "~/Documents or /Volumes/Data",
            FileList::ExcludedNames => "node_modules",
            FileList::ExcludedSubstrings => "/.cache/",
        }
    }

    pub fn get(self, c: &Config) -> &[String] {
        match self {
            FileList::Roots => &c.file_search.roots,
            FileList::ExcludedPaths => &c.file_search.excluded_paths,
            FileList::ExcludedNames => &c.file_search.excluded_names,
            FileList::ExcludedSubstrings => &c.file_search.excluded_substrings,
        }
    }

    fn get_mut(self, c: &mut Config) -> &mut Vec<String> {
        match self {
            FileList::Roots => &mut c.file_search.roots,
            FileList::ExcludedPaths => &mut c.file_search.excluded_paths,
            FileList::ExcludedNames => &mut c.file_search.excluded_names,
            FileList::ExcludedSubstrings => &mut c.file_search.excluded_substrings,
        }
    }
}

const MAX_LIST_ENTRY: usize = 1024;

/// A cleaned, valid list entry or the reason it is not one.
pub fn validate_list_entry(list: FileList, raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("enter a value".into());
    }
    if s.len() > MAX_LIST_ENTRY || s.chars().any(char::is_control) {
        return Err("that does not look like a path".into());
    }
    match list {
        FileList::Roots | FileList::ExcludedPaths => {
            if !(s == "~" || s.starts_with("~/") || s.starts_with('/')) {
                return Err("use a full path, or one starting with ~/".into());
            }
            // a trailing slash would make "~/Docs/" differ from "~/Docs" in comparisons
            let t = if s.len() > 1 { s.trim_end_matches('/') } else { s };
            Ok(t.to_string())
        }
        FileList::ExcludedNames => {
            if s.contains('/') {
                return Err("a name cannot contain /".into());
            }
            Ok(s.to_string())
        }
        FileList::ExcludedSubstrings => Ok(s.to_string()),
    }
}

pub fn add_list_entry(c: &mut Config, list: FileList, raw: &str) -> EditResult {
    let value = validate_list_entry(list, raw)?;
    let items = list.get_mut(c);
    if items.contains(&value) {
        return Err("it is in the list already".into());
    }
    items.push(value);
    Ok(())
}

pub fn remove_list_entry(c: &mut Config, list: FileList, index: usize) -> EditResult {
    let items = list.get_mut(c);
    if index >= items.len() {
        return Err("that entry is gone".into());
    }
    if list == FileList::Roots && items.len() == 1 {
        return Err("keep at least one folder to search in".into());
    }
    items.remove(index);
    Ok(())
}

pub fn set_include_hidden(c: &mut Config, on: bool) -> EditResult {
    c.file_search.include_hidden = on;
    Ok(())
}

pub fn set_include_icloud(c: &mut Config, on: bool) -> EditResult {
    c.file_search.include_icloud_drive = on;
    Ok(())
}

// ------------------------------------------------------------------ Features: bookmarks

pub fn bookmark_source_label(id: &str) -> String {
    match id {
        "safari" => "Safari",
        "chrome" => "Google Chrome",
        "chromium" => "Chromium",
        "edge" => "Microsoft Edge",
        "brave" => "Brave",
        "vivaldi" => "Vivaldi",
        "opera" => "Opera",
        "arc" => "Arc",
        "firefox" => "Firefox",
        other => other,
    }
    .to_string()
}

pub fn bookmark_source_enabled(c: &Config, id: &str) -> bool {
    c.bookmarks.enabled_sources.iter().any(|s| s == id)
}

/// Keeps the sources in the engine's own order, whatever order they were toggled in.
pub fn set_bookmark_source(c: &mut Config, id: &str, on: bool) -> EditResult {
    if !ALL_SOURCE_IDS.contains(&id) {
        return Err(format!("unknown bookmark source '{id}'"));
    }
    let mut enabled: Vec<String> = ALL_SOURCE_IDS
        .iter()
        .filter(|s| **s != id && bookmark_source_enabled(c, s) || **s == id && on)
        .map(|s| s.to_string())
        .collect();
    // sources named in the file that this build does not know stay where they were
    for extra in c
        .bookmarks
        .enabled_sources
        .iter()
        .filter(|s| !ALL_SOURCE_IDS.contains(&s.as_str()))
    {
        enabled.push(extra.clone());
    }
    c.bookmarks.enabled_sources = enabled;
    Ok(())
}

pub fn set_bookmarks_in_general_search(c: &mut Config, on: bool) -> EditResult {
    c.bookmarks.include_in_general_search = on;
    Ok(())
}

// ------------------------------------------------------------------ Features: clipboard

/// The clipboard fields as the user types them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardForm {
    pub max_entries: String,
    /// Empty or 0 = keep forever.
    pub max_age_days: String,
    pub max_image_mb: String,
}

pub const MAX_CLIPBOARD_ENTRIES: usize = 10_000;
pub const MAX_CLIPBOARD_DAYS: u32 = 3650;
pub const MAX_IMAGE_MB: u64 = 256;

impl ClipboardForm {
    pub fn from_settings(s: &ClipboardSettings) -> ClipboardForm {
        ClipboardForm {
            max_entries: s.max_entries.to_string(),
            max_age_days: s.max_age_days.map(|d| d.to_string()).unwrap_or_default(),
            max_image_mb: format_mb(s.max_image_bytes),
        }
    }

    /// The three numbers, or the first problem found.
    pub fn parse(&self) -> Result<(usize, Option<u32>, usize), String> {
        let entries: usize = self
            .max_entries
            .trim()
            .parse()
            .map_err(|_| "entries: enter a whole number".to_string())?;
        if !(1..=MAX_CLIPBOARD_ENTRIES).contains(&entries) {
            return Err(format!("entries: between 1 and {MAX_CLIPBOARD_ENTRIES}"));
        }
        let age = match self.max_age_days.trim() {
            "" => None,
            t => {
                let d: u32 = t
                    .parse()
                    .map_err(|_| "days: enter a whole number, or leave empty".to_string())?;
                if d > MAX_CLIPBOARD_DAYS {
                    return Err(format!("days: at most {MAX_CLIPBOARD_DAYS}"));
                }
                (d > 0).then_some(d)
            }
        };
        let mb: f64 = self
            .max_image_mb
            .trim()
            .parse()
            .map_err(|_| "image size: enter a number of megabytes".to_string())?;
        if !mb.is_finite() || !(0.0..=MAX_IMAGE_MB as f64).contains(&mb) {
            return Err(format!("image size: between 0 and {MAX_IMAGE_MB} MB"));
        }
        Ok((entries, age, (mb * 1_048_576.0).round() as usize))
    }
}

fn format_mb(bytes: usize) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb.fract().abs() < 0.005 {
        format!("{}", mb.round() as u64)
    } else {
        format!("{mb:.2}")
    }
}

pub fn apply_clipboard_form(c: &mut Config, form: &ClipboardForm) -> EditResult {
    let (entries, age, bytes) = form.parse()?;
    c.clipboard.max_entries = entries;
    c.clipboard.max_age_days = age;
    c.clipboard.max_image_bytes = bytes;
    Ok(())
}

/// A bundle identifier: reverse-DNS, ASCII, at least two parts.
pub fn validate_bundle_id(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    let ok = s.len() <= 255
        && s.contains('.')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
    if ok {
        Ok(s.to_string())
    } else {
        Err("use a bundle identifier such as com.example.app".into())
    }
}

pub fn add_ignored_app(c: &mut Config, raw: &str) -> EditResult {
    let id = validate_bundle_id(raw)?;
    if c.clipboard.ignored_apps.iter().any(|a| a.eq_ignore_ascii_case(&id)) {
        return Err("it is in the list already".into());
    }
    c.clipboard.ignored_apps.push(id);
    Ok(())
}

pub fn remove_ignored_app(c: &mut Config, index: usize) -> EditResult {
    if index >= c.clipboard.ignored_apps.len() {
        return Err("that entry is gone".into());
    }
    c.clipboard.ignored_apps.remove(index);
    Ok(())
}

// ------------------------------------------------------------------ Features: web searches

/// Keywords the other features answer to; a web search with one of them would never be reached.
const RESERVED_KEYWORDS: &[&str] = &["open", "find", "in", "bm", "clip", "clipboard", "snip", "actions"];

/// A web search as typed in the editor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebSearchDraft {
    pub keyword: String,
    pub title: String,
    pub url: String,
    pub fallback: bool,
}

impl WebSearchDraft {
    pub fn from_search(w: &WebSearch) -> WebSearchDraft {
        WebSearchDraft {
            keyword: w.keyword.clone(),
            title: w.title.clone(),
            url: w.url.clone(),
            fallback: w.fallback,
        }
    }
}

pub fn validate_keyword(raw: &str) -> Result<String, String> {
    let k = raw.trim();
    if k.is_empty() {
        return Err("enter a keyword".into());
    }
    if k.chars().count() > 32 {
        return Err("keyword: at most 32 characters".into());
    }
    if k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a keyword cannot contain spaces".into());
    }
    if k.starts_with(['\'', '>', '/', '~']) {
        return Err("a keyword cannot start with ' > / or ~ (they have their own meaning)".into());
    }
    if RESERVED_KEYWORDS.iter().any(|r| r.eq_ignore_ascii_case(k)) {
        return Err(format!("'{k}' is used by another feature"));
    }
    Ok(k.to_string())
}

/// `https://host/path?q={query}`: http(s) only, a literal host, `{query}` after it, no other
/// placeholders, no credentials, no whitespace.
pub fn validate_url_template(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("enter a URL".into());
    }
    if t.len() > 2000 {
        return Err("the URL is too long".into());
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a URL cannot contain spaces".into());
    }
    let Some((scheme, rest)) = t.split_once("://") else {
        return Err("start with https:// (or http://)".into());
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err("only http:// and https:// addresses are allowed".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("the URL needs a host name".into());
    }
    if authority.contains('@') {
        return Err("do not put a user name or password in the URL".into());
    }
    if authority.contains('{') {
        return Err("{query} must come after the host name".into());
    }
    if !t.contains("{query}") {
        return Err("the URL must contain {query} where the search text goes".into());
    }
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        if !rest[i..].starts_with("{query}") {
            return Err("only {query} is supported as a placeholder".into());
        }
        rest = &rest[i + "{query}".len()..];
    }
    Ok(t.to_string())
}

/// A valid search from `draft`, checked against the other searches (`editing` = the one being
/// edited, which may keep its own keyword).
pub fn validate_web_search(
    draft: &WebSearchDraft,
    existing: &[WebSearch],
    editing: Option<usize>,
) -> Result<WebSearch, String> {
    let keyword = validate_keyword(&draft.keyword)?;
    if existing
        .iter()
        .enumerate()
        .any(|(i, w)| Some(i) != editing && w.keyword.eq_ignore_ascii_case(&keyword))
    {
        return Err(format!("'{keyword}' is used by another search"));
    }
    let title = draft.title.trim();
    if title.is_empty() {
        return Err("enter a name".into());
    }
    if title.chars().count() > 64 || title.chars().any(char::is_control) {
        return Err("name: at most 64 characters".into());
    }
    Ok(WebSearch {
        keyword,
        title: title.to_string(),
        url: validate_url_template(&draft.url)?,
        fallback: draft.fallback,
    })
}

/// Add (`editing = None`) or replace a web search.
pub fn save_web_search(c: &mut Config, editing: Option<usize>, draft: &WebSearchDraft) -> EditResult {
    let ws = validate_web_search(draft, &c.web_searches, editing)?;
    match editing {
        Some(i) if i < c.web_searches.len() => c.web_searches[i] = ws,
        Some(_) => return Err("that search is gone".into()),
        None => c.web_searches.push(ws),
    }
    Ok(())
}

pub fn remove_web_search(c: &mut Config, index: usize) -> EditResult {
    if index >= c.web_searches.len() {
        return Err("that search is gone".into());
    }
    c.web_searches.remove(index);
    Ok(())
}

pub fn reset_web_searches(c: &mut Config) -> EditResult {
    c.web_searches = websearch::defaults();
    Ok(())
}

// ------------------------------------------------------------------ Features: snippets

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnippetDraft {
    /// Empty = not in a collection.
    pub collection: String,
    pub name: String,
    pub keyword: String,
    pub text: String,
}

impl SnippetDraft {
    pub fn from_snippet(s: &Snippet) -> SnippetDraft {
        SnippetDraft {
            collection: s.collection.clone(),
            name: s.name.clone(),
            keyword: s.keyword.clone(),
            text: s.text.clone(),
        }
    }
}

pub const MAX_SNIPPET_TEXT: usize = 100_000;

pub fn validate_collection_name(raw: &str) -> Result<String, String> {
    let n = raw.trim();
    if n.chars().count() > 80 || n.chars().any(char::is_control) {
        return Err("collection names are at most 80 characters, without control characters".into());
    }
    Ok(n.to_string())
}

/// A valid snippet from `draft`. `others` = every other snippet (the one being edited left
/// out), for the keyword check; `editing` keeps that snippet's identity and imported fields.
pub fn validate_snippet(
    draft: &SnippetDraft,
    entries: &[(SnippetLoc, Snippet)],
    editing: Option<SnippetLoc>,
) -> Result<Snippet, String> {
    let collection = validate_collection_name(&draft.collection)?;
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("enter a name".into());
    }
    if name.chars().count() > 100 {
        return Err("name: at most 100 characters".into());
    }
    let keyword = draft.keyword.trim();
    if keyword.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a keyword cannot contain spaces".into());
    }
    if keyword.chars().count() > 40 {
        return Err("keyword: at most 40 characters".into());
    }
    if draft.text.trim().is_empty() {
        return Err("the snippet is empty".into());
    }
    if draft.text.len() > MAX_SNIPPET_TEXT {
        return Err(format!("the snippet is longer than {MAX_SNIPPET_TEXT} characters"));
    }
    let base = editing
        .and_then(|loc| entries.iter().find(|(l, _)| *l == loc))
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    let candidate = Snippet {
        name: name.to_string(),
        keyword: keyword.to_string(),
        text: draft.text.clone(),
        collection,
        ..base
    };
    // Prefixes belong to the collection: a snippet that moves takes the new collection's.
    let prefix_source = entries
        .iter()
        .find(|(_, s)| s.collection == candidate.collection && !candidate.collection.is_empty());
    let mut candidate = candidate;
    if let Some((_, s)) = prefix_source {
        candidate.keyword_prefix = s.keyword_prefix.clone();
        candidate.keyword_suffix = s.keyword_suffix.clone();
    } else if editing.is_none() || candidate.collection.is_empty() {
        candidate.keyword_prefix.clear();
        candidate.keyword_suffix.clear();
    }
    let resolved = candidate.resolved_keyword();
    if !resolved.is_empty()
        && entries
            .iter()
            .any(|(l, s)| Some(*l) != editing && s.resolved_keyword().eq_ignore_ascii_case(&resolved))
    {
        return Err(format!("another snippet already uses '{resolved}'"));
    }
    Ok(candidate)
}

// ------------------------------------------------------------------ Workflows

/// Text lines describing what a workflow can do, for the workflow list. First line is the
/// verdict; the rest are the specifics that were found. One wording for every prompt and list:
/// `gui::importprompt::capability_lines`.
pub fn trust_lines(t: &TrustSummary) -> Vec<String> {
    crate::gui::importprompt::capability_lines(t)
}

/// How a configuration-sheet entry is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingControl {
    Checkbox,
    Number,
    Text,
    MultilineText,
}

pub fn setting_control(kind: &str) -> SettingControl {
    match kind {
        "checkbox" => SettingControl::Checkbox,
        "slider" => SettingControl::Number,
        "textarea" => SettingControl::MultilineText,
        _ => SettingControl::Text,
    }
}

const MAX_SETTING_BYTES: usize = 64 * 1024;

/// The text to store for a configuration-sheet entry of type `kind`.
pub fn validate_setting_value(kind: &str, raw: &str) -> Result<String, String> {
    if raw.len() > MAX_SETTING_BYTES || raw.contains('\0') {
        return Err("that value is too long or contains unsupported characters".into());
    }
    match setting_control(kind) {
        SettingControl::Checkbox => match raw.trim() {
            "1" | "true" => Ok("1".into()),
            "0" | "false" | "" => Ok("0".into()),
            _ => Err("a checkbox is on (1) or off (0)".into()),
        },
        SettingControl::Number => {
            let t = raw.trim();
            match t.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(t.to_string()),
                _ => Err("enter a number".into()),
            }
        }
        _ => Ok(raw.to_string()),
    }
}

// ------------------------------------------------------------------ About

/// (label, value) rows for the About page.
pub fn about_rows(paths: &Paths) -> Vec<(&'static str, String)> {
    vec![
        ("Version", env!("CARGO_PKG_VERSION").to_string()),
        (
            "Licence",
            option_env!("CARGO_PKG_LICENSE")
                .filter(|l| !l.is_empty())
                .unwrap_or("see the LICENSE files in the source")
                .to_string(),
        ),
        ("Settings file", paths.config_file().display().to_string()),
        ("Data folder", paths.data.display().to_string()),
        ("Workflows folder", paths.workflows_dir().display().to_string()),
        ("Cache folder", paths.cache.display().to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use ashdash_core::config::Paths;

    use super::*;

    fn temp(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (Paths::under(&root), root)
    }

    fn bytes(paths: &Paths) -> Option<Vec<u8>> {
        std::fs::read(paths.config_file()).ok()
    }

    // ---- Store

    #[test]
    fn an_edit_writes_only_when_it_is_valid_and_changes_something() {
        let (paths, root) = temp("store");
        let store = Store::new(paths.clone());
        assert!(bytes(&paths).is_none());

        // invalid: nothing is written, not even the defaults
        assert!(store.edit(|c| set_hotkey(c, "space")).is_err());
        assert!(bytes(&paths).is_none());

        // valid: written, and `ashdash config get` style loading sees it
        let c = store.edit(|c| set_hotkey(c, "alt+shift+KeyJ")).unwrap();
        assert_eq!(c.hotkey, "alt+shift+KeyJ");
        assert_eq!(Config::load_strict(&paths).unwrap().hotkey, "alt+shift+KeyJ");

        // a no-op edit leaves the file byte-identical (and its mtime alone)
        let before = bytes(&paths);
        store.edit(|c| set_hotkey(c, "alt+shift+KeyJ")).unwrap();
        assert_eq!(bytes(&paths), before);

        // a failing edit after a partial change writes nothing
        let r = store.edit(|c| {
            c.max_results = 5;
            Err("nope".into())
        });
        assert!(r.is_err());
        assert_eq!(bytes(&paths), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unreadable_config_file_is_never_overwritten() {
        let (paths, root) = temp("broken");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), b"{ \"hotkey\": ").unwrap();
        let store = Store::new(paths.clone());
        let err = store.edit(|c| set_max_results(c, 10)).unwrap_err();
        assert!(err.contains("config.json"), "{err}");
        assert_eq!(bytes(&paths).unwrap(), b"{ \"hotkey\": ");
        assert!(store.load().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_config_written_by_a_newer_version_is_not_clobbered() {
        let (paths, root) = temp("newer");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), br#"{"version": 99, "future": true}"#).unwrap();
        let store = Store::new(paths.clone());
        assert!(store.edit(|c| set_max_results(c, 10)).is_err());
        assert!(String::from_utf8(bytes(&paths).unwrap()).unwrap().contains("future"));
        let _ = std::fs::remove_dir_all(root);
    }

    // ---- General

    #[test]
    fn hotkeys_need_a_modifier_and_must_parse() {
        assert_eq!(validate_hotkey(" alt+shift+Space ").unwrap(), "alt+shift+Space");
        assert!(validate_hotkey("Space").is_err());
        assert!(validate_hotkey("").is_err());
        assert!(validate_hotkey("alt+").is_err());
        assert!(validate_hotkey("alt+shift+Bogus").is_err());
        assert_eq!(system_hotkey_conflict("cmd+space"), Some("Spotlight"));
        assert_eq!(system_hotkey_conflict("alt+shift+space"), None);
    }

    #[test]
    fn result_count_is_bounded() {
        let mut c = Config::default();
        assert!(set_max_results(&mut c, 2).is_err());
        assert!(set_max_results(&mut c, 51).is_err());
        assert_eq!(c.max_results, 30);
        set_max_results(&mut c, 12).unwrap();
        assert_eq!(c.max_results, 12);
    }

    // ---- providers

    #[test]
    fn the_feature_table_covers_every_provider_in_order() {
        let ids: Vec<&str> = PROVIDER_INFO.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, PROVIDER_IDS);
    }

    #[test]
    fn providers_toggle_and_unknown_ids_are_refused() {
        let mut c = Config::default();
        assert!(provider_enabled(&c, "calc"));
        set_provider_enabled(&mut c, "calc", false).unwrap();
        set_provider_enabled(&mut c, "calc", false).unwrap();
        assert_eq!(c.providers_disabled, vec!["calc"]);
        assert!(!provider_enabled(&c, "calc"));
        set_provider_enabled(&mut c, "calc", true).unwrap();
        assert!(c.providers_disabled.is_empty());
        assert!(set_provider_enabled(&mut c, "nope", false).is_err());
        assert!(c.providers_disabled.is_empty());
    }

    // ---- file search lists

    #[test]
    fn file_list_entries_are_checked_and_deduplicated() {
        let mut c = Config::default();
        add_list_entry(&mut c, FileList::Roots, " ~/Documents/ ").unwrap();
        assert_eq!(c.file_search.roots.last().unwrap(), "~/Documents");
        assert!(add_list_entry(&mut c, FileList::Roots, "~/Documents").is_err());
        assert!(add_list_entry(&mut c, FileList::Roots, "relative/dir").is_err());
        assert!(add_list_entry(&mut c, FileList::Roots, "").is_err());
        assert!(add_list_entry(&mut c, FileList::ExcludedPaths, "/tmp/\nx").is_err());
        assert!(add_list_entry(&mut c, FileList::ExcludedNames, "a/b").is_err());
        add_list_entry(&mut c, FileList::ExcludedNames, ".git").unwrap();
        add_list_entry(&mut c, FileList::ExcludedSubstrings, "/.cache/").unwrap();
        assert!(c.file_search.excluded_names.contains(&".git".to_string()));
        assert!(c.file_search.excluded_substrings.contains(&"/.cache/".to_string()));
        assert_eq!(validate_list_entry(FileList::Roots, "/").unwrap(), "/");
    }

    #[test]
    fn the_last_search_root_cannot_be_removed() {
        let mut c = Config::default();
        assert_eq!(c.file_search.roots.len(), 1);
        assert!(remove_list_entry(&mut c, FileList::Roots, 0).is_err());
        add_list_entry(&mut c, FileList::Roots, "/Volumes/Data").unwrap();
        remove_list_entry(&mut c, FileList::Roots, 0).unwrap();
        assert_eq!(c.file_search.roots, vec!["/Volumes/Data"]);
        assert!(remove_list_entry(&mut c, FileList::ExcludedNames, 99).is_err());
        let n = c.file_search.excluded_names.len();
        remove_list_entry(&mut c, FileList::ExcludedNames, 0).unwrap();
        assert_eq!(c.file_search.excluded_names.len(), n - 1);
    }

    // ---- bookmarks

    #[test]
    fn bookmark_sources_keep_the_engine_order_and_unknown_entries() {
        let mut c = Config::default();
        set_bookmark_source(&mut c, "safari", false).unwrap();
        assert!(!bookmark_source_enabled(&c, "safari"));
        set_bookmark_source(&mut c, "safari", true).unwrap();
        assert_eq!(c.bookmarks.enabled_sources, ALL_SOURCE_IDS);
        c.bookmarks.enabled_sources.push("future-browser".into());
        set_bookmark_source(&mut c, "firefox", false).unwrap();
        assert!(c.bookmarks.enabled_sources.contains(&"future-browser".to_string()));
        assert!(!bookmark_source_enabled(&c, "firefox"));
        assert!(set_bookmark_source(&mut c, "netscape", true).is_err());
        for id in ALL_SOURCE_IDS {
            assert_ne!(bookmark_source_label(id), "", "{id}");
        }
    }

    // ---- clipboard

    fn form(e: &str, d: &str, m: &str) -> ClipboardForm {
        ClipboardForm {
            max_entries: e.into(),
            max_age_days: d.into(),
            max_image_mb: m.into(),
        }
    }

    #[test]
    fn the_clipboard_form_round_trips_the_settings() {
        let s = ClipboardSettings::default();
        let f = ClipboardForm::from_settings(&s);
        let mut c = Config::default();
        c.clipboard.max_entries = 1;
        apply_clipboard_form(&mut c, &f).unwrap();
        assert_eq!(c.clipboard.max_entries, s.max_entries);
        assert_eq!(c.clipboard.max_age_days, s.max_age_days);
        assert_eq!(c.clipboard.max_image_bytes, s.max_image_bytes);
        // an uneven size survives formatting
        let odd = ClipboardSettings {
            max_image_bytes: 2_621_440,
            ..ClipboardSettings::default()
        };
        assert_eq!(ClipboardForm::from_settings(&odd).max_image_mb, "2.50");
    }

    #[test]
    fn clipboard_numbers_are_validated() {
        assert_eq!(form("50", "", "1").parse().unwrap(), (50, None, 1_048_576));
        assert_eq!(form("50", "0", "0").parse().unwrap(), (50, None, 0));
        assert_eq!(form("50", "30", "2.5").parse().unwrap(), (50, Some(30), 2_621_440));
        for bad in [
            form("0", "", "1"),
            form("-1", "", "1"),
            form("lots", "", "1"),
            form("10001", "", "1"),
            form("5", "x", "1"),
            form("5", "99999", "1"),
            form("5", "", "-1"),
            form("5", "", "NaN"),
            form("5", "", "1000"),
        ] {
            assert!(bad.parse().is_err(), "{bad:?}");
        }
        let mut c = Config::default();
        let before = serde_json::to_value(&c).unwrap();
        assert!(apply_clipboard_form(&mut c, &form("0", "", "1")).is_err());
        assert_eq!(serde_json::to_value(&c).unwrap(), before);
    }

    #[test]
    fn ignored_apps_need_bundle_ids() {
        let mut c = Config::default();
        add_ignored_app(&mut c, " com.example.Vault ").unwrap();
        assert!(c.clipboard.ignored_apps.contains(&"com.example.Vault".to_string()));
        assert!(add_ignored_app(&mut c, "COM.EXAMPLE.VAULT").is_err());
        for bad in ["", "vault", ".a.b", "a.b.", "a b.c", "a/b.c", "é.é"] {
            assert!(add_ignored_app(&mut c, bad).is_err(), "{bad:?}");
        }
        let n = c.clipboard.ignored_apps.len();
        remove_ignored_app(&mut c, n - 1).unwrap();
        assert!(remove_ignored_app(&mut c, n + 5).is_err());
    }

    // ---- web searches

    fn draft(k: &str, t: &str, u: &str) -> WebSearchDraft {
        WebSearchDraft {
            keyword: k.into(),
            title: t.into(),
            url: u.into(),
            fallback: false,
        }
    }

    #[test]
    fn url_templates_need_a_host_and_a_query_placeholder() {
        let ok = |u: &str| validate_url_template(u).unwrap();
        assert_eq!(
            ok(" https://example.com/s?q={query} "),
            "https://example.com/s?q={query}"
        );
        ok("http://localhost:8080/?q={query}&x={query}");
        ok("https://example.com/{query}");
        for bad in [
            "",
            "example.com/?q={query}",
            "ftp://example.com/{query}",
            "javascript://x/{query}",
            "file:///tmp/{query}",
            "https://example.com/",
            "https://{query}.example.com/",
            "https://{query}/",
            "https:///?q={query}",
            "https://user:pw@example.com/?q={query}",
            "https://example.com/?q={Query}",
            "https://example.com/?q={query}&r={other}",
            "https://example.com/ ?q={query}",
            "https://example.com/?q={query\n}",
        ] {
            assert!(validate_url_template(bad).is_err(), "{bad:?}");
        }
        assert!(validate_url_template(&format!("https://e.com/{}{{query}}", "a".repeat(2100))).is_err());
    }

    #[test]
    fn keywords_cannot_collide_or_confuse() {
        assert_eq!(validate_keyword(" gh ").unwrap(), "gh");
        for bad in [
            "",
            "two words",
            "bm",
            "OPEN",
            "in",
            "'x",
            ">x",
            "/x",
            "~x",
            &"k".repeat(33),
        ] {
            assert!(validate_keyword(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn web_search_editor_adds_edits_and_removes() {
        let mut c = Config::default();
        let n = c.web_searches.len();
        save_web_search(
            &mut c,
            None,
            &draft("gh", "GitHub", "https://github.com/search?q={query}"),
        )
        .unwrap();
        assert_eq!(c.web_searches.len(), n + 1);
        // duplicate keyword (any case) is refused, but an edit may keep its own
        assert!(save_web_search(&mut c, None, &draft("GH", "Other", "https://x.com/?q={query}")).is_err());
        let idx = n;
        save_web_search(
            &mut c,
            Some(idx),
            &draft("gh", "GitHub code", "https://github.com/search?q={query}&type=code"),
        )
        .unwrap();
        assert_eq!(c.web_searches[idx].title, "GitHub code");
        assert_eq!(c.web_searches.len(), n + 1);
        // invalid edits leave the list untouched
        let before = c.web_searches.clone();
        assert!(save_web_search(&mut c, Some(idx), &draft("gh", "", "https://x.com/?q={query}")).is_err());
        assert!(save_web_search(&mut c, Some(idx), &draft("gh", "T", "x")).is_err());
        assert!(save_web_search(&mut c, Some(999), &draft("zz", "T", "https://x.com/?q={query}")).is_err());
        assert_eq!(c.web_searches, before);
        remove_web_search(&mut c, idx).unwrap();
        assert!(remove_web_search(&mut c, idx).is_err());
        assert_eq!(c.web_searches.len(), n);
        c.web_searches.clear();
        reset_web_searches(&mut c).unwrap();
        assert_eq!(c.web_searches, websearch::defaults());
    }

    // ---- snippets

    fn sn(collection: &str, name: &str, kw: &str) -> Snippet {
        Snippet {
            name: name.into(),
            keyword: kw.into(),
            text: "t".into(),
            collection: collection.into(),
            ..Snippet::default()
        }
    }

    fn sd(collection: &str, name: &str, kw: &str, text: &str) -> SnippetDraft {
        SnippetDraft {
            collection: collection.into(),
            name: name.into(),
            keyword: kw.into(),
            text: text.into(),
        }
    }

    #[test]
    fn snippets_are_validated_and_keywords_stay_unique() {
        let entries = vec![
            (SnippetLoc::Flat(0), sn("", "Sig", "sig")),
            (
                SnippetLoc::File(0, 0),
                Snippet {
                    keyword_prefix: ";".into(),
                    ..sn("Work", "Addr", "addr")
                },
            ),
        ];
        let ok = validate_snippet(&sd("", "Hello", "hi", "hello"), &entries, None).unwrap();
        assert_eq!((ok.name.as_str(), ok.keyword.as_str()), ("Hello", "hi"));
        // keyword collisions use the keyword as typed (prefix included)
        assert!(validate_snippet(&sd("", "Other", "SIG", "x"), &entries, None).is_err());
        assert!(
            validate_snippet(&sd("", "Other", ";addr", "x"), &entries, None).is_err(),
            "same as the prefixed keyword"
        );
        assert!(validate_snippet(&sd("Work", "Other", "addr", "x"), &entries, None).is_err());
        // editing a snippet may keep its own keyword
        assert!(validate_snippet(&sd("", "Sig2", "sig", "x"), &entries, Some(SnippetLoc::Flat(0))).is_ok());
        for bad in [
            sd("", "", "k", "x"),
            sd("", "n", "has space", "x"),
            sd("", "n", "k", "   "),
            sd("", "n", &"k".repeat(41), "x"),
            sd("", "n", "k", &"x".repeat(MAX_SNIPPET_TEXT + 1)),
            sd("bad\u{7}name", "n", "k", "x"),
        ] {
            assert!(validate_snippet(&bad, &entries, None).is_err(), "{bad:?}");
        }
        // empty keyword is allowed (found by name)
        assert!(validate_snippet(&sd("", "n", "", "x"), &entries, None).is_ok());
    }

    #[test]
    fn a_snippet_moved_into_a_collection_takes_its_prefix() {
        let entries = vec![(
            SnippetLoc::File(0, 0),
            Snippet {
                keyword_prefix: ";".into(),
                ..sn("Work", "Addr", "addr")
            },
        )];
        let moved = validate_snippet(&sd("Work", "New", "n", "x"), &entries, None).unwrap();
        assert_eq!(moved.resolved_keyword(), ";n");
        let out = validate_snippet(&sd("", "Addr", "addr", "x"), &entries, Some(SnippetLoc::File(0, 0))).unwrap();
        assert_eq!(out.resolved_keyword(), "addr");
    }

    // ---- workflows

    #[test]
    fn trust_lines_lead_with_the_verdict() {
        let plain = trust_lines(&TrustSummary::default());
        assert_eq!(plain.len(), 1);
        assert!(plain[0].contains("Runs no scripts"));
        let t = TrustSummary {
            scripts: 2,
            interpreters: vec!["bash".into(), "python3".into()],
            query_in_source: 1,
            open_url_schemes: vec!["https".into()],
            terminal_commands: vec!["ls {query}".into()],
            external_files: vec!["~/x".into()],
            external_triggers: 1,
            ..TrustSummary::default()
        };
        let lines = trust_lines(&t);
        assert!(lines[0].contains("Runs code"));
        for want in [
            "2 scripts (bash, python3)",
            "pastes the typed text",
            "ls {query}",
            "https",
            "~/x",
            "external trigger",
        ] {
            assert!(lines.iter().any(|l| l.contains(want)), "missing {want}: {lines:?}");
        }
    }

    #[test]
    fn the_workflow_list_never_calls_a_file_writer_harmless() {
        use ashdash_core::workflow::{FileWriteInfo, WriteKind, WritePlace};
        let t = TrustSummary {
            file_writes: vec![FileWriteInfo {
                path: "~/.zshenv".into(),
                place: WritePlace::Anywhere,
                kind: WriteKind::Append,
                dynamic: false,
                creates_folders: false,
            }],
            ..TrustSummary::default()
        };
        let lines = trust_lines(&t);
        assert!(
            !lines[0].contains("Runs no scripts and no terminal commands"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("appends to ~/.zshenv")), "{lines:?}");
    }

    #[test]
    fn setting_values_follow_the_control_type() {
        assert_eq!(setting_control("checkbox"), SettingControl::Checkbox);
        assert_eq!(setting_control("slider"), SettingControl::Number);
        assert_eq!(setting_control("textarea"), SettingControl::MultilineText);
        assert_eq!(setting_control("popupbutton"), SettingControl::Text);
        assert_eq!(validate_setting_value("checkbox", "true").unwrap(), "1");
        assert_eq!(validate_setting_value("checkbox", "0").unwrap(), "0");
        assert!(validate_setting_value("checkbox", "maybe").is_err());
        assert_eq!(validate_setting_value("slider", " 3.5 ").unwrap(), "3.5");
        assert!(validate_setting_value("slider", "fast").is_err());
        assert!(validate_setting_value("slider", "NaN").is_err());
        assert_eq!(
            validate_setting_value("textfield", "  keep spaces ").unwrap(),
            "  keep spaces "
        );
        assert!(validate_setting_value("textfield", "a\0b").is_err());
        assert!(validate_setting_value("textarea", &"x".repeat(MAX_SETTING_BYTES + 1)).is_err());
    }

    #[test]
    fn about_lists_the_data_locations() {
        let paths = Paths::under(std::path::Path::new("/tmp/x"));
        let rows = about_rows(&paths);
        assert!(rows.iter().any(|(k, v)| *k == "Version" && !v.is_empty()));
        assert!(
            rows.iter()
                .any(|(k, v)| *k == "Settings file" && v.ends_with("config.json"))
        );
    }

    #[test]
    fn pages_are_listed_in_order() {
        let titles: Vec<_> = Page::ALL.iter().map(|p| p.title()).collect();
        assert_eq!(titles, ["General", "Features", "Workflows", "About"]);
        assert_eq!(Page::default(), Page::General);
    }
}
