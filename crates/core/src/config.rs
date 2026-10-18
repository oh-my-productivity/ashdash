//! Paths and user configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bookmarks::BookmarkSettings;
use crate::clipboard::{DEFAULT_IGNORED_APPS, history::DEFAULT_MAX_IMAGE_BYTES};
use crate::files::FileSearchScope;
use crate::websearch::{self, WebSearch};

#[derive(Debug, Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        let data = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::brand::SLUG);
        let cache = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::brand::SLUG);
        Paths { data, cache }
    }

    pub fn under(root: &Path) -> Self {
        Paths {
            data: root.join("data"),
            cache: root.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.data.join("config.json")
    }
    pub fn usage_file(&self) -> PathBuf {
        self.data.join("usage.json")
    }
    pub fn clipboard_file(&self) -> PathBuf {
        self.data.join("clipboard.json")
    }
    pub fn snippets_file(&self) -> PathBuf {
        self.data.join("snippets.json")
    }
    pub fn workflows_dir(&self) -> PathBuf {
        self.data.join("workflows")
    }
    /// Per-workflow configuration values: `<bundle id>.json` files (see
    /// `Workflow::user_values_file`, the single place that names them).
    pub fn workflow_config_dir(&self) -> PathBuf {
        self.data.join("workflow-config")
    }
    /// Advisory lock serialising read-modify-write cycles of `config.json`
    /// (see [`Config::edit_locked`]).
    pub fn config_lock_file(&self) -> PathBuf {
        self.data.join("run").join("config.lock")
    }
    pub fn icon_cache(&self) -> PathBuf {
        self.cache.join("icons")
    }
}

/// Schema version of `config.json`. Bump it and extend [`migrate`] whenever
/// the on-disk layout changes.
pub const CURRENT_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Settings schema version; files without one are version 0.
    pub version: u32,
    /// Global hotkey, e.g. `alt+space` or `cmd+shift+space`.
    pub hotkey: String,
    pub max_results: usize,
    pub app_dirs: Vec<PathBuf>,
    /// Extra directories scanned for workflow folders (each containing
    /// `info.plist`). Point this at an existing folder of
    /// workflows to run workflows you already have.
    pub workflow_dirs: Vec<PathBuf>,
    pub web_searches: Vec<WebSearch>,
    /// Path to an appearance (theme) file.
    pub theme: Option<PathBuf>,
    /// Browser bookmark search (`bm`), added in schema 2.
    pub bookmarks: BookmarkSettings,
    /// What file search (`open`, `find`, `in`, `'`) looks at, added in schema 2.
    pub file_search: FileSearchSettings,
    /// Clipboard history retention and privacy, added in schema 2. Replaces the
    /// schema-1 `clipboard_history_size` (migrated to `clipboard.max_entries`).
    pub clipboard: ClipboardSettings,
    /// Provider ids (see `providers::provider_ids`) that are switched off.
    /// Unknown ids are ignored, so a setting survives a provider being renamed.
    pub providers_disabled: Vec<String>,
    /// Learn from the results you pick to rank them (`usage.json`). Off = nothing is
    /// recorded and existing data does not influence ranking. Preferences switch.
    pub learn_usage: bool,
}

/// Serializable form of [`FileSearchScope`]. Paths may start with `~`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSearchSettings {
    /// Search roots; results outside them are dropped.
    pub roots: Vec<String>,
    /// Folders (and everything below) that are never searched.
    pub excluded_paths: Vec<String>,
    /// Any path component with one of these names excludes the result.
    pub excluded_names: Vec<String>,
    /// Results whose full path contains one of these strings are dropped.
    pub excluded_substrings: Vec<String>,
    pub include_hidden: bool,
    /// Keep iCloud Drive (`~/Library/Mobile Documents/com~apple~CloudDocs`)
    /// searchable although `~/Library` is excluded. See [`ICLOUD_DRIVE`].
    pub include_icloud_drive: bool,
    /// `mdfind` is killed after this many milliseconds (hits found so far are kept).
    /// Out-of-range values are clamped to [`MDFIND_TIMEOUT_MS_RANGE`] when read.
    #[serde(deserialize_with = "de_mdfind_timeout")]
    pub mdfind_timeout_ms: u64,
    /// `mdfind` output beyond this many bytes is discarded ([`MDFIND_MAX_BYTES_RANGE`]).
    #[serde(deserialize_with = "de_mdfind_bytes")]
    pub mdfind_max_bytes: usize,
    /// The directory-walk fallback stops after this many milliseconds ([`WALK_TIMEOUT_MS_RANGE`]).
    #[serde(deserialize_with = "de_walk_timeout")]
    pub walk_timeout_ms: u64,
    /// ...or after visiting this many entries ([`WALK_MAX_ENTRIES_RANGE`]; 0 never means "unlimited").
    #[serde(deserialize_with = "de_walk_entries")]
    pub walk_max_entries: usize,
}

/// Accepted ranges of the file search run budget. A value outside is clamped when the settings are
/// loaded, so a typo (or a hand-edited `0`) can neither disable a limit nor stall the query worker.
pub const MDFIND_TIMEOUT_MS_RANGE: std::ops::RangeInclusive<u64> = 200..=30_000;
pub const MDFIND_MAX_BYTES_RANGE: std::ops::RangeInclusive<usize> = (64 << 10)..=(256 << 20);
pub const WALK_TIMEOUT_MS_RANGE: std::ops::RangeInclusive<u64> = 100..=30_000;
pub const WALK_MAX_ENTRIES_RANGE: std::ops::RangeInclusive<usize> = 1_000..=10_000_000;

fn clamp_de<'de, D, T>(d: D, range: &std::ops::RangeInclusive<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Ord + Copy,
{
    Ok(T::deserialize(d)?.clamp(*range.start(), *range.end()))
}

fn de_mdfind_timeout<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    clamp_de(d, &MDFIND_TIMEOUT_MS_RANGE)
}
fn de_mdfind_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    clamp_de(d, &MDFIND_MAX_BYTES_RANGE)
}
fn de_walk_timeout<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    clamp_de(d, &WALK_TIMEOUT_MS_RANGE)
}
fn de_walk_entries<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    clamp_de(d, &WALK_MAX_ENTRIES_RANGE)
}

/// iCloud Drive's folder, relative to the home directory.
pub const ICLOUD_DRIVE: &str = "Library/Mobile Documents/com~apple~CloudDocs";

impl Default for FileSearchSettings {
    fn default() -> Self {
        FileSearchSettings {
            roots: vec!["~".into()],
            excluded_paths: vec!["~/Library".into()],
            excluded_names: vec!["node_modules".into(), "target".into()],
            excluded_substrings: vec!["/pkg/mod/".into()],
            include_hidden: false,
            include_icloud_drive: true,
            mdfind_timeout_ms: 4_000,
            mdfind_max_bytes: 8 << 20,
            walk_timeout_ms: 2_000,
            walk_max_entries: 200_000,
        }
    }
}

impl FileSearchSettings {
    /// The run budget as the search code wants it. Values set in code (not read from a file) are
    /// clamped here too.
    pub fn budget(&self) -> crate::files::SearchBudget {
        use std::time::Duration;
        let clamp = |v: u64, r: &std::ops::RangeInclusive<u64>| v.clamp(*r.start(), *r.end());
        crate::files::SearchBudget {
            mdfind_timeout: Duration::from_millis(clamp(self.mdfind_timeout_ms, &MDFIND_TIMEOUT_MS_RANGE)),
            mdfind_max_bytes: self
                .mdfind_max_bytes
                .clamp(*MDFIND_MAX_BYTES_RANGE.start(), *MDFIND_MAX_BYTES_RANGE.end()),
            walk_timeout: Duration::from_millis(clamp(self.walk_timeout_ms, &WALK_TIMEOUT_MS_RANGE)),
            walk_max_entries: self
                .walk_max_entries
                .clamp(*WALK_MAX_ENTRIES_RANGE.start(), *WALK_MAX_ENTRIES_RANGE.end()),
        }
    }

    /// The scope for the current user (`~` is this user's home directory).
    pub fn to_scope(&self) -> FileSearchScope {
        self.to_scope_in(&dirs::home_dir().unwrap_or_default())
    }

    /// The scope with `~` meaning `home`; the folder listing used for the
    /// iCloud carve-out is the real file system.
    pub fn to_scope_in(&self, home: &Path) -> FileSearchScope {
        self.to_scope_with(home, &list_dir)
    }

    pub(crate) fn to_scope_with(&self, home: &Path, list: &dyn Fn(&Path) -> Vec<PathBuf>) -> FileSearchScope {
        let expand = |s: &String| expand_tilde(s, home);
        let mut excluded: Vec<PathBuf> = self.excluded_paths.iter().map(expand).collect();
        if self.include_icloud_drive {
            excluded = carve_out(excluded, &home.join(ICLOUD_DRIVE), list);
        }
        // Start from the constructor and assign, so fields added to the scope later keep their defaults.
        let mut scope = FileSearchScope::new(self.roots.iter().map(expand).collect(), excluded);
        scope.excluded_names = self.excluded_names.clone();
        scope.excluded_substrings = self.excluded_substrings.clone();
        scope.include_hidden = self.include_hidden;
        scope.budget = self.budget();
        scope
    }
}

fn list_dir(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Make `keep` searchable although an ancestor of it is excluded. The scope
/// has no "allow" list, so each excluded ancestor is replaced by its
/// children that do not lead to `keep` (and the same one level further down,
/// until `keep` is reached). `keep` must exist, otherwise nothing changes.
/// Folders created later inside those ancestors are searched until the next
/// settings reload.
pub(crate) fn carve_out(excluded: Vec<PathBuf>, keep: &Path, list: &dyn Fn(&Path) -> Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in excluded {
        if !keep.starts_with(&e) {
            out.push(e);
            continue;
        }
        let Ok(rest) = keep.strip_prefix(&e) else { continue };
        let mut cur = e.clone();
        let mut siblings = Vec::new();
        let mut found = true;
        for comp in rest.components() {
            let next = cur.join(comp);
            let children = list(&cur);
            if !children.contains(&next) {
                found = false;
                break;
            }
            siblings.extend(children.into_iter().filter(|c| *c != next));
            cur = next;
        }
        if found {
            out.extend(siblings);
        } else {
            out.push(e);
        }
    }
    out
}

/// Clipboard history settings (privacy and retention).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardSettings {
    /// Maximum number of unpinned entries.
    pub max_entries: usize,
    /// Drop unpinned entries older than this many days (`None` keeps them).
    pub max_age_days: Option<u32>,
    /// Bundle ids of apps whose copies are never recorded.
    pub ignored_apps: Vec<String>,
    /// Images larger than this many bytes are not recorded.
    pub max_image_bytes: usize,
}

impl ClipboardSettings {
    /// Apply the retention policy, ignore list and image size cap to `history`.
    pub fn apply(&self, history: &mut crate::clipboard::ClipboardHistory) {
        history.set_ignored_apps(self.ignored_apps.clone());
        history.set_max_image_bytes(self.max_image_bytes);
        history.set_retention(
            self.max_entries,
            self.max_age_days.filter(|d| *d > 0).map(|d| d as u64 * 86_400),
        );
    }
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        ClipboardSettings {
            max_entries: 200,
            max_age_days: None,
            ignored_apps: DEFAULT_IGNORED_APPS.iter().map(|s| s.to_string()).collect(),
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Config {
            version: CURRENT_VERSION,
            hotkey: crate::brand::DEFAULT_HOTKEY.into(),
            max_results: 30,
            app_dirs: vec![
                PathBuf::from("/Applications"),
                PathBuf::from("/System/Applications"),
                PathBuf::from("/System/Library/CoreServices/Applications"),
                PathBuf::from("/System/Library/CoreServices/Finder.app"),
                home.join("Applications"),
            ],
            workflow_dirs: Vec::new(),
            web_searches: websearch::defaults(),
            theme: None,
            bookmarks: BookmarkSettings::default(),
            file_search: FileSearchSettings::default(),
            clipboard: ClipboardSettings::default(),
            providers_disabled: Vec::new(),
            learn_usage: true,
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Config {
        Config::load_in(paths, &dirs::home_dir().unwrap_or_default())
    }

    /// [`Config::load`] with `~` meaning `home`.
    pub fn load_in(paths: &Paths, home: &Path) -> Config {
        match std::fs::read(paths.config_file()) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .map(migrate)
                .and_then(serde_json::from_value)
                .map(|c: Config| c.with_tilde_expanded(home))
                .unwrap_or_else(|e| {
                    eprintln!("ashdash: ignoring invalid config.json: {e}");
                    Config::default()
                }),
            Err(_) => Config::default(),
        }
    }

    /// `app_dirs`, `workflow_dirs` and `theme` with a leading `~` or `~/` replaced by `home`
    /// (as `file_search.*` already is at use). Only loading does this: the file keeps what the
    /// user wrote, see [`Config::save`].
    fn with_tilde_expanded(mut self, home: &Path) -> Config {
        let expand = |p: &PathBuf| expand_tilde_path(p, home);
        self.app_dirs = self.app_dirs.iter().map(expand).collect();
        self.workflow_dirs = self.workflow_dirs.iter().map(expand).collect();
        self.theme = self.theme.as_ref().map(expand);
        self
    }

    /// Like [`Config::load`] but reports a file that exists and cannot be read as
    /// settings instead of falling back to defaults (so a CLI edit never
    /// replaces a hand-written file it failed to parse). A missing file is not
    /// an error.
    pub fn load_strict(paths: &Paths) -> anyhow::Result<Config> {
        Config::load_strict_in(paths, &dirs::home_dir().unwrap_or_default())
    }

    /// [`Config::load_strict`] with `~` meaning `home`.
    pub fn load_strict_in(paths: &Paths, home: &Path) -> anyhow::Result<Config> {
        let bytes = match std::fs::read(paths.config_file()) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e.into()),
        };
        let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
        Ok(serde_json::from_value::<Config>(migrate(raw))?.with_tilde_expanded(home))
    }

    /// Read-modify-write of `config.json` under an exclusive advisory `flock` on
    /// `run/config.lock`, so the preferences window, `ashdash config set` and a theme import
    /// cannot overwrite each other's edit with a stale copy. The file is read strictly (an
    /// unreadable file is an error, never replaced by defaults); `edit` runs on a copy and its
    /// error leaves the file alone; nothing is written when `edit` changed nothing. Returns the
    /// settings now on disk and `edit`'s result.
    ///
    /// Keys in `config.json` that this build does not know (hand edits) survive the save, at the
    /// top level and inside nested objects (see [`Config::save`]); a file from a *newer* config
    /// version is refused instead.
    pub fn edit_locked<T>(
        paths: &Paths,
        edit: impl FnOnce(&mut Config) -> anyhow::Result<T>,
    ) -> anyhow::Result<(Config, T)> {
        use anyhow::Context;
        let _lock = ConfigLock::acquire(paths).context("locking config.json")?;
        let old = Config::load_strict(paths)?;
        let mut new = old.clone();
        let out = edit(&mut new)?;
        if serde_json::to_value(&old)? == serde_json::to_value(&new)? {
            return Ok((old, out));
        }
        new.save(paths).context("saving config.json")?;
        Ok((new, out))
    }

    /// Writes the current schema version. Refuses to overwrite a file written by a
    /// newer build, which would silently drop settings this build does not know.
    ///
    /// Keys of the existing file that [`Config`] has no field for are kept: the settings are
    /// merged over the (migrated) file contents, recursing into objects. Arrays (web searches,
    /// root lists) are replaced as a whole, so unknown keys inside array elements are not kept.
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        self.save_in(paths, &dirs::home_dir().unwrap_or_default())
    }

    /// [`Config::save`] with `~` meaning `home`.
    ///
    /// Paths are loaded with `~` expanded, but a save writes back the spelling the file has:
    /// an entry of `app_dirs` / `workflow_dirs` (or the `theme`) that is the expansion of a
    /// `~...` entry already in the file is written as that `~...` entry. A path the user typed
    /// or set (`ashdash config set ...`) is written as given.
    pub fn save_in(&self, paths: &Paths, home: &Path) -> std::io::Result<()> {
        let mut base = serde_json::Value::Null;
        if let Ok(bytes) = std::fs::read(paths.config_file())
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            if v.get("version").and_then(|n| n.as_u64()).unwrap_or(0) > CURRENT_VERSION as u64 {
                return Err(std::io::Error::other(
                    "config.json was written by a newer version; refusing to overwrite it",
                ));
            }
            base = migrate(v);
        }
        let mut out = self.clone();
        out.version = CURRENT_VERSION;
        let mut merged = serde_json::to_value(&out)?;
        keep_tilde_spelling(&mut merged, &base, home);
        if base.is_object() {
            merge_unknown(&mut base, &mut merged);
            merged = base;
        }
        // 0600, written through a unique temp file and renamed (SEC-18)
        write_private(&paths.config_file(), &serde_json::to_vec_pretty(&merged)?)
    }
}

/// `~` and `~/rest` mean the home folder; every other string is a path as it stands
/// (`~user/x` is not expanded).
///
/// Extra slashes after the `~` do not send the path to the filesystem root (`~//a` is
/// `$HOME/a`), and with no home folder (an empty path) nothing is expanded: `~/wf` stays the
/// literal, missing path it was written as rather than becoming the relative path `wf`.
pub fn expand_tilde(s: &str, home: &Path) -> PathBuf {
    if home.as_os_str().is_empty() {
        return PathBuf::from(s);
    }
    match s {
        "~" => home.to_path_buf(),
        _ => match s.strip_prefix("~/") {
            Some(rest) => match rest.trim_start_matches('/') {
                "" => home.to_path_buf(),
                rest => home.join(rest),
            },
            None => PathBuf::from(s),
        },
    }
}

fn expand_tilde_path(p: &Path, home: &Path) -> PathBuf {
    match p.to_str() {
        Some(s) => expand_tilde(s, home),
        None => p.to_path_buf(),
    }
}

/// In `merged` (the settings about to be written), put back the `~...` spelling that `old` (the
/// file as it is) uses for the same folder, for `app_dirs`, `workflow_dirs` and `theme`.
fn keep_tilde_spelling(merged: &mut serde_json::Value, old: &serde_json::Value, home: &Path) {
    use serde_json::Value;
    // the `~...` strings of one old setting, with the path each stands for
    let spelled = |v: Option<&Value>| -> Vec<(PathBuf, String)> {
        let strings: Vec<&str> = match v {
            Some(Value::String(s)) => vec![s.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        strings
            .into_iter()
            .filter(|s| s.starts_with('~'))
            .map(|s| (expand_tilde(s, home), s.to_string()))
            .collect()
    };
    let rewrite = |item: &mut Value, old: &[(PathBuf, String)]| {
        if let Some(s) = item.as_str()
            && let Some((_, raw)) = old.iter().find(|(p, _)| *p == expand_tilde(s, home))
        {
            *item = Value::String(raw.clone());
        }
    };
    for key in ["app_dirs", "workflow_dirs", "theme"] {
        let old_spelled = spelled(old.get(key));
        if old_spelled.is_empty() {
            continue;
        }
        match merged.get_mut(key) {
            Some(Value::Array(items)) => items.iter_mut().for_each(|i| rewrite(i, &old_spelled)),
            Some(item @ Value::String(_)) => rewrite(item, &old_spelled),
            _ => {}
        }
    }
}

/// Overlay `new` onto `base`: every key of `new` wins, keys only in `base` stay, nested objects
/// merge recursively, anything else (arrays, scalars, type changes) is taken from `new`.
/// Afterwards `base` is the result; `new` is left empty where it was moved from.
fn merge_unknown(base: &mut serde_json::Value, new: &mut serde_json::Value) {
    match (base.as_object_mut(), new.as_object_mut()) {
        (Some(b), Some(n)) => {
            for (k, v) in std::mem::take(n) {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_object() && v.is_object() => {
                        let mut v = v;
                        merge_unknown(existing, &mut v);
                    }
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        _ => *base = std::mem::take(new),
    }
}

/// Upgrade raw settings JSON to [`CURRENT_VERSION`], one step at a time.
/// Runs in memory only; the file is rewritten on the next `save`.
fn migrate(mut raw: serde_json::Value) -> serde_json::Value {
    let Some(obj) = raw.as_object_mut() else {
        return raw;
    };
    let mut v = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    // Files from a newer build (v > CURRENT_VERSION) pass through untouched.
    while v < CURRENT_VERSION {
        match v {
            // 0 -> 1: introduced the `version` field; no other changes.
            0 => {}
            // 1 -> 2: `clipboard_history_size` became `clipboard.max_entries`; bookmark,
            // file search, provider and clipboard settings were added (all defaulted).
            1 => {
                if let Some(size) = obj.remove("clipboard_history_size") {
                    let clip = obj.entry("clipboard").or_insert_with(|| serde_json::json!({}));
                    if let Some(c) = clip.as_object_mut() {
                        c.entry("max_entries").or_insert(size);
                    }
                }
            }
            _ => unreachable!("no migration from config version {v}"),
        }
        v += 1;
        obj.insert("version".into(), v.into());
    }
    raw
}

/// Holds the exclusive `flock` on [`Paths::config_lock_file`] until dropped (or the process ends).
pub struct ConfigLock(std::fs::File);

impl ConfigLock {
    /// Wait for the lock. Other holders are short read-modify-write cycles.
    pub fn acquire(paths: &Paths) -> std::io::Result<ConfigLock> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let file = paths.config_lock_file();
        if let Some(dir) = file.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&file)?;
        f.lock()?;
        Ok(ConfigLock(f))
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write a file readable only by the current user (config, usage, clipboard data).
///
/// Missing parent directories are created 0700. The bytes go to a uniquely named, exclusively
/// created 0600 temp file next to the target (so concurrent writers and stale leftovers cannot
/// interfere or leak an old, laxer mode) which is then renamed over the target (SEC-18).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format_ids::appearance_file;

    #[test]
    fn save_refuses_to_clobber_newer_config() {
        let root = std::env::temp_dir().join(format!("oa-cfg-newer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), r#"{"version": 99, "future": true}"#).unwrap();
        assert!(Config::default().save(&paths).is_err());
        assert!(std::fs::read_to_string(paths.config_file()).unwrap().contains("future"));
        let _ = std::fs::remove_dir_all(&root);
    }

    use serde_json::json;

    #[test]
    fn unknown_keys_survive_a_locked_edit() {
        let paths = tmp_paths("unknown-keys");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(
            paths.config_file(),
            serde_json::to_vec(&json!({
                "version": CURRENT_VERSION,
                "max_results": 5,
                "future_key": {"a": [1, 2, {"b": null}], "c": "d"},
                "clipboard": {"max_entries": 7, "future_nested": [true]},
                "file_search": {"future_fs": 1.5},
            }))
            .unwrap(),
        )
        .unwrap();
        let (cfg, ()) = Config::edit_locked(&paths, |c| {
            c.max_results = 9;
            c.clipboard.max_entries = 8;
            Ok(())
        })
        .unwrap();
        assert_eq!(cfg.max_results, 9);
        let after: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap();
        assert_eq!(after["future_key"], json!({"a": [1, 2, {"b": null}], "c": "d"}));
        assert_eq!(after["clipboard"]["future_nested"], json!([true]));
        assert_eq!(after["clipboard"]["max_entries"], 8);
        assert_eq!(after["file_search"]["future_fs"], 1.5);
        assert_eq!(after["max_results"], 9);
        // the typed settings still load, and plain save() keeps them too
        Config::load_strict(&paths).unwrap().save(&paths).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap();
        assert_eq!(again, after);
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn migrated_keys_do_not_come_back_after_a_save() {
        let paths = tmp_paths("unknown-migrated");
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(
            paths.config_file(),
            br#"{"version": 1, "clipboard_history_size": 12, "extra": 1}"#,
        )
        .unwrap();
        Config::edit_locked(&paths, |c| {
            c.max_results = 3;
            Ok(())
        })
        .unwrap();
        let after: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap();
        assert!(after.get("clipboard_history_size").is_none());
        assert_eq!(after["clipboard"]["max_entries"], 12);
        assert_eq!(after["extra"], 1);
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    fn tmp_paths(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("oa-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Paths::under(&root)
    }

    #[test]
    fn concurrent_locked_edits_do_not_lose_updates() {
        let paths = tmp_paths("lock-threads");
        let start = Config::default().max_results;
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let paths = paths.clone();
                std::thread::spawn(move || {
                    Config::edit_locked(&paths, |c| {
                        let n = c.max_results;
                        // widen the race window between read and write
                        std::thread::sleep(std::time::Duration::from_millis(15));
                        c.max_results = n + 1;
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(Config::load_strict(&paths).unwrap().max_results, start + 12);
        assert_eq!(mode(&paths.config_lock_file()), 0o600);
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn edit_locked_writes_nothing_on_error_or_no_change() {
        let paths = tmp_paths("lock-nowrite");
        assert!(Config::edit_locked(&paths, |_| Ok(())).is_ok());
        assert!(!paths.config_file().exists(), "unchanged settings are not written");
        let r: anyhow::Result<(Config, ())> = Config::edit_locked(&paths, |c| {
            c.max_results = 3;
            anyhow::bail!("refused")
        });
        assert!(r.is_err());
        assert!(!paths.config_file().exists());
        // an unreadable file is an error, never replaced by defaults
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), b"{ nope").unwrap();
        assert!(
            Config::edit_locked(&paths, |c| {
                c.max_results = 4;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(std::fs::read(paths.config_file()).unwrap(), b"{ nope");
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    /// The lock is a real `flock`: another process cannot take it while we hold it.
    #[test]
    fn the_lock_excludes_other_processes() {
        if let Ok(dir) = std::env::var("OA_CFG_LOCK_CHILD") {
            let paths = Paths::under(Path::new(&dir));
            let f = std::fs::File::open(paths.config_lock_file()).unwrap();
            let blocked = matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock));
            std::process::exit(if blocked { 0 } else { 7 });
        }
        let paths = tmp_paths("lock-proc");
        let guard = ConfigLock::acquire(&paths).unwrap();
        let root = paths.data.parent().unwrap().to_path_buf();
        let run = || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "config::tests::the_lock_excludes_other_processes",
                    "--nocapture",
                ])
                .env("OA_CFG_LOCK_CHILD", &root)
                .status()
                .unwrap()
        };
        assert!(run().success(), "child could take the lock while it was held");
        drop(guard);
        assert_eq!(run().code(), Some(7), "child could not take a released lock");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn config_and_data_dir_are_private() {
        let root = std::env::temp_dir().join(format!("oa-cfg-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        Config::default().save(&paths).unwrap();
        assert_eq!(mode(&paths.config_file()), 0o600);
        assert_eq!(mode(&paths.data), 0o700);
        // saving again replaces the file without loosening it, and leaves no temp files behind
        Config::default().save(&paths).unwrap();
        assert_eq!(mode(&paths.config_file()), 0o600);
        let names: Vec<_> = std::fs::read_dir(&paths.data).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn write_private_ignores_stale_loose_temp_files_and_concurrent_writers() {
        let root = std::env::temp_dir().join(format!("oa-wp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("f.json");
        // a crash leftover with the old shared name and a loose mode
        let stale = root.join("f.tmp");
        std::fs::write(&stale, b"old").unwrap();
        std::thread::scope(|s| {
            for i in 0..8 {
                let target = target.clone();
                s.spawn(move || {
                    for j in 0..20 {
                        write_private(&target, format!("{i}-{j}").as_bytes()).unwrap();
                    }
                });
            }
        });
        assert_eq!(mode(&target), 0o600);
        assert!(std::fs::read_to_string(&target).unwrap().contains('-'));
        let tmps = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp") && e.path() != stale)
            .count();
        assert_eq!(tmps, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrates_unversioned_config_to_current() {
        let out = migrate(json!({"hotkey": "cmd+space", "max_results": 7}));
        assert_eq!(out["version"], CURRENT_VERSION);
        assert_eq!(out["hotkey"], "cmd+space");
        assert_eq!(out["max_results"], 7);
    }

    #[test]
    fn current_and_newer_versions_are_left_alone() {
        assert_eq!(migrate(json!({"version": 2}))["version"], 2);
        assert_eq!(migrate(json!({"version": 9}))["version"], 9);
    }

    #[test]
    fn migration_1_to_2_moves_the_history_size() {
        let out = migrate(json!({"version": 1, "clipboard_history_size": 75, "hotkey": "alt+space"}));
        assert_eq!(out["version"], 2);
        assert_eq!(out["clipboard"]["max_entries"], 75);
        assert!(out.get("clipboard_history_size").is_none());
        let c: Config = serde_json::from_value(out).unwrap();
        assert_eq!(c.clipboard.max_entries, 75);
        assert_eq!(c.hotkey, "alt+space");
        // everything new is defaulted
        assert_eq!(c.clipboard.ignored_apps, ClipboardSettings::default().ignored_apps);
        assert_eq!(c.bookmarks, BookmarkSettings::default());
        assert_eq!(c.file_search, FileSearchSettings::default());
        assert!(c.providers_disabled.is_empty());
    }

    #[test]
    fn migration_from_unversioned_goes_all_the_way() {
        let out = migrate(json!({"clipboard_history_size": 10}));
        assert_eq!(
            (out["version"].as_u64(), out["clipboard"]["max_entries"].as_u64()),
            (Some(2), Some(10))
        );
    }

    #[test]
    fn migration_keeps_an_explicit_clipboard_section() {
        let out = migrate(
            json!({"version": 1, "clipboard_history_size": 75, "clipboard": {"max_entries": 5, "max_age_days": 3}}),
        );
        assert_eq!(out["clipboard"]["max_entries"], 5, "an existing value wins");
        assert_eq!(out["clipboard"]["max_age_days"], 3);
        // garbage in the new section does not panic
        let _ = migrate(json!({"version": 1, "clipboard_history_size": 3, "clipboard": 7}));
    }

    #[test]
    fn v2_fields_have_sane_defaults() {
        let c = Config::default();
        assert_eq!(c.version, CURRENT_VERSION);
        assert_eq!(c.clipboard.max_entries, 200);
        assert_eq!(c.clipboard.max_age_days, None);
        assert_eq!(c.clipboard.max_image_bytes, DEFAULT_MAX_IMAGE_BYTES);
        assert!(
            c.clipboard
                .ignored_apps
                .iter()
                .any(|a| a.contains("1password") || a.contains("keychain") || a.contains('.'))
        );
        assert!(c.providers_disabled.is_empty());
        assert_eq!(c.bookmarks, BookmarkSettings::default());
        assert!(c.file_search.include_icloud_drive);
    }

    #[test]
    fn missing_v2_sections_deserialize_to_defaults() {
        let c: Config = serde_json::from_value(json!({"version": 2, "max_results": 3})).unwrap();
        assert_eq!(c.max_results, 3);
        assert_eq!(c.clipboard, ClipboardSettings::default());
        // partial sections fill the rest from defaults
        let c: Config =
            serde_json::from_value(json!({"clipboard": {"max_age_days": 7}, "file_search": {"include_hidden": true}}))
                .unwrap();
        assert_eq!(c.clipboard.max_age_days, Some(7));
        assert_eq!(c.clipboard.max_entries, 200);
        assert!(c.file_search.include_hidden);
        assert_eq!(c.file_search.roots, vec!["~"]);
    }

    #[test]
    fn every_v2_field_round_trips_through_save_and_load() {
        let root = std::env::temp_dir().join(format!("oa-cfg-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        let mut c = Config::default();
        c.bookmarks.enabled_sources = vec!["safari".into()];
        c.bookmarks.include_in_general_search = false;
        c.file_search = FileSearchSettings {
            roots: vec!["~/Documents".into(), "/Volumes/x".into()],
            excluded_paths: vec!["~/Documents/old".into()],
            excluded_names: vec!["build".into()],
            excluded_substrings: vec![],
            include_hidden: true,
            include_icloud_drive: false,
            mdfind_timeout_ms: 1500,
            mdfind_max_bytes: 1 << 20,
            walk_timeout_ms: 700,
            walk_max_entries: 5000,
        };
        c.clipboard = ClipboardSettings {
            max_entries: 12,
            max_age_days: Some(14),
            ignored_apps: vec!["com.example.vault".into()],
            max_image_bytes: 1234,
        };
        c.providers_disabled = vec!["calc".into(), "shell".into()];
        c.save(&paths).unwrap();
        let back = Config::load(&paths);
        assert_eq!(back.bookmarks, c.bookmarks);
        assert_eq!(back.file_search, c.file_search);
        assert_eq!(back.clipboard, c.clipboard);
        assert_eq!(back.providers_disabled, c.providers_disabled);
        assert_eq!(back.version, CURRENT_VERSION);
        // a v1 file on disk is upgraded on load and rewritten as v2 on save
        std::fs::write(paths.config_file(), br#"{"version":1,"clipboard_history_size":33}"#).unwrap();
        let up = Config::load_strict(&paths).unwrap();
        assert_eq!(up.clipboard.max_entries, 33);
        up.save(&paths).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        assert_eq!(raw["clipboard"]["max_entries"], 33);
        assert!(raw.get("clipboard_history_size").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_strict_reports_broken_files_but_not_missing_ones() {
        let root = std::env::temp_dir().join(format!("oa-cfg-strict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        assert!(Config::load_strict(&paths).is_ok());
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), b"{ not json").unwrap();
        assert!(Config::load_strict(&paths).is_err());
        std::fs::write(paths.config_file(), br#"{"max_results": "many"}"#).unwrap();
        assert!(Config::load_strict(&paths).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_search_settings_expand_tilde_into_a_scope() {
        let home = Path::new("/home/u");
        let s = FileSearchSettings {
            include_icloud_drive: false,
            ..FileSearchSettings::default()
        };
        let scope = s.to_scope_with(home, &|_| vec![]);
        assert_eq!(scope.roots, vec![PathBuf::from("/home/u")]);
        assert_eq!(scope.excluded_paths, vec![PathBuf::from("/home/u/Library")]);
        assert!(!scope.allows(Path::new("/home/u/Library/Mail/x.txt")));
        assert!(scope.allows(Path::new("/home/u/Documents/x.txt")));
        assert!(!scope.allows(Path::new("/home/u/Documents/node_modules/x.txt")));
        assert!(!scope.allows(Path::new("/home/u/.secret")));
        let with_hidden = FileSearchSettings {
            include_hidden: true,
            ..s.clone()
        };
        assert!(
            with_hidden
                .to_scope_with(home, &|_| vec![])
                .allows(Path::new("/home/u/.secret"))
        );
        // the default scope is the same as the one the file search shipped with
        assert_eq!(
            FileSearchSettings::default().to_scope_with(home, &|_| vec![]),
            FileSearchScope::home_default(home)
        );
    }

    #[test]
    fn icloud_drive_is_carved_out_of_the_excluded_library() {
        let home = std::env::temp_dir().join(format!("oa-cfg-icloud-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let icloud = home.join(ICLOUD_DRIVE);
        std::fs::create_dir_all(&icloud).unwrap();
        for d in [
            "Library/Mail",
            "Library/Caches",
            "Library/Mobile Documents/com~apple~Notes",
            "Documents",
        ] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let scope = FileSearchSettings::default().to_scope_in(&home);
        assert!(
            scope.allows(&icloud.join("Taxes/2024.pdf")),
            "iCloud Drive files are searchable"
        );
        assert!(scope.allows(&icloud));
        assert!(!scope.allows(&home.join("Library/Mail/msg.emlx")));
        assert!(!scope.allows(&home.join("Library/Caches/x")));
        assert!(!scope.allows(&home.join("Library/Mobile Documents/com~apple~Notes/x")));
        assert!(scope.allows(&home.join("Documents/a.txt")));
        // switched off: the whole library stays excluded
        let off = FileSearchSettings {
            include_icloud_drive: false,
            ..FileSearchSettings::default()
        };
        assert!(!off.to_scope_in(&home).allows(&icloud.join("Taxes/2024.pdf")));
        // no iCloud folder on this machine: nothing changes
        let bare = std::env::temp_dir().join(format!("oa-cfg-icloud-bare-{}", std::process::id()));
        std::fs::create_dir_all(bare.join("Library/Mail")).unwrap();
        let scope = FileSearchSettings::default().to_scope_in(&bare);
        assert_eq!(scope.excluded_paths, vec![bare.join("Library")]);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn carve_out_is_pure_and_leaves_unrelated_exclusions_alone() {
        let list = |p: &Path| -> Vec<PathBuf> {
            match p.to_str().unwrap() {
                "/h/L" => vec!["/h/L/A".into(), "/h/L/B".into()],
                "/h/L/A" => vec!["/h/L/A/keep".into(), "/h/L/A/other".into()],
                _ => vec![],
            }
        };
        let out = carve_out(
            vec!["/h/L".into(), "/h/x".into(), "/h/L/A/keep/inner".into()],
            Path::new("/h/L/A/keep"),
            &list,
        );
        assert_eq!(
            out,
            vec![
                PathBuf::from("/h/L/B"),
                PathBuf::from("/h/L/A/other"),
                PathBuf::from("/h/x"),
                PathBuf::from("/h/L/A/keep/inner"),
            ]
        );
    }

    #[test]
    fn load_upgrades_and_save_writes_version() {
        let root = std::env::temp_dir().join(format!("oa-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), br#"{"max_results": 5}"#).unwrap();
        let c = Config::load(&paths);
        assert_eq!((c.version, c.max_results), (CURRENT_VERSION, 5));
        std::fs::remove_file(paths.config_file()).unwrap();
        c.save(&paths).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap();
        assert_eq!(raw["version"], CURRENT_VERSION);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_search_budget_defaults_match_the_search_code() {
        let d = FileSearchSettings::default();
        assert_eq!(d.budget(), crate::files::SearchBudget::default());
        assert_eq!(
            d.to_scope_in(Path::new("/h")).budget,
            crate::files::SearchBudget::default()
        );
    }

    #[test]
    fn file_search_budget_reaches_the_scope_and_is_clamped() {
        use std::time::Duration;
        let s = FileSearchSettings {
            mdfind_timeout_ms: 1500,
            mdfind_max_bytes: 1 << 20,
            walk_timeout_ms: 700,
            walk_max_entries: 5000,
            ..FileSearchSettings::default()
        };
        let b = s.to_scope_in(Path::new("/h")).budget;
        assert_eq!(b.mdfind_timeout, Duration::from_millis(1500));
        assert_eq!(b.mdfind_max_bytes, 1 << 20);
        assert_eq!(b.walk_timeout, Duration::from_millis(700));
        assert_eq!(b.walk_max_entries, 5000);
        // in-code values are clamped as well
        let wild = FileSearchSettings {
            mdfind_timeout_ms: 0,
            mdfind_max_bytes: 0,
            walk_timeout_ms: u64::MAX,
            walk_max_entries: 0,
            ..FileSearchSettings::default()
        }
        .budget();
        assert_eq!(
            wild.mdfind_timeout,
            Duration::from_millis(*MDFIND_TIMEOUT_MS_RANGE.start())
        );
        assert_eq!(wild.mdfind_max_bytes, *MDFIND_MAX_BYTES_RANGE.start());
        assert_eq!(wild.walk_timeout, Duration::from_millis(*WALK_TIMEOUT_MS_RANGE.end()));
        assert_eq!(
            wild.walk_max_entries,
            *WALK_MAX_ENTRIES_RANGE.start(),
            "0 is not 'unlimited'"
        );
    }

    #[test]
    fn file_search_budget_in_a_file_is_clamped_and_missing_keys_default() {
        let c: Config = serde_json::from_str(
            r#"{"file_search":{"mdfind_timeout_ms":0,"mdfind_max_bytes":99999999999,"walk_max_entries":0}}"#,
        )
        .unwrap();
        assert_eq!(c.file_search.mdfind_timeout_ms, *MDFIND_TIMEOUT_MS_RANGE.start());
        assert_eq!(c.file_search.mdfind_max_bytes, *MDFIND_MAX_BYTES_RANGE.end());
        assert_eq!(c.file_search.walk_max_entries, *WALK_MAX_ENTRIES_RANGE.start());
        assert_eq!(c.file_search.walk_timeout_ms, 2_000, "unset key keeps its default");
        // wrong types are errors (the whole file is then ignored by `load`), never panics
        for bad in [
            r#"{"file_search":{"mdfind_timeout_ms":-1}}"#,
            r#"{"file_search":{"walk_max_entries":"lots"}}"#,
            r#"{"file_search":{"mdfind_max_bytes":1.5}}"#,
        ] {
            assert!(serde_json::from_str::<Config>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_search_budget_keys_are_settable_and_validated() {
        let mut c = Config::default();
        crate::settings_view::set(&mut c, "file_search.mdfind_timeout_ms", "2500").unwrap();
        assert_eq!(c.file_search.mdfind_timeout_ms, 2500);
        crate::settings_view::set(&mut c, "file_search.walk_max_entries", "50000").unwrap();
        assert_eq!(c.file_search.walk_max_entries, 50_000);
        for (k, v) in [
            ("file_search.mdfind_timeout_ms", "0"),
            ("file_search.mdfind_timeout_ms", "9999999"),
            ("file_search.mdfind_max_bytes", "10"),
            ("file_search.walk_timeout_ms", "-5"),
            ("file_search.walk_max_entries", "0"),
        ] {
            let before = c.file_search.clone();
            assert!(crate::settings_view::set(&mut c, k, v).is_err(), "{k}={v}");
            assert_eq!(c.file_search, before, "{k}={v} changed nothing");
        }
        let keys = crate::settings_view::keys(&c);
        for k in [
            "mdfind_timeout_ms",
            "mdfind_max_bytes",
            "walk_timeout_ms",
            "walk_max_entries",
        ] {
            assert!(keys.contains(&format!("file_search.{k}")), "{k}");
        }
    }

    // ---- `~` in workflow_dirs, app_dirs and theme (docs/PRODUCT.md section 8)

    fn write_raw(paths: &Paths, v: serde_json::Value) {
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn read_raw(paths: &Paths) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(paths.config_file()).unwrap()).unwrap()
    }

    #[test]
    fn tilde_in_paths_is_expanded_on_load() {
        let paths = tmp_paths("tilde-load");
        let home = Path::new("/home/u");
        write_raw(
            &paths,
            json!({
                "version": CURRENT_VERSION,
                "workflow_dirs": ["~/dotfiles/workflows", "~", "/abs/wf", "rel/wf", "~other/x", "a~/b"],
                "app_dirs": ["~/Apps"],
                "theme": appearance_file("~/themes/Midnight"),
            }),
        );
        for c in [
            Config::load_in(&paths, home),
            Config::load_strict_in(&paths, home).unwrap(),
        ] {
            assert_eq!(
                c.workflow_dirs,
                [
                    "/home/u/dotfiles/workflows",
                    "/home/u",
                    "/abs/wf",
                    "rel/wf",
                    "~other/x",
                    "a~/b"
                ]
                .map(PathBuf::from)
            );
            assert_eq!(c.app_dirs, [PathBuf::from("/home/u/Apps")]);
            assert_eq!(c.theme, Some(PathBuf::from(appearance_file("/home/u/themes/Midnight"))));
        }
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn extra_slashes_after_the_tilde_stay_inside_home() {
        let home = Path::new("/home/u");
        assert_eq!(expand_tilde("~//a", home), PathBuf::from("/home/u/a"));
        assert_eq!(expand_tilde("~///a//b", home), PathBuf::from("/home/u/a//b"));
        assert_eq!(expand_tilde("~/a", home), PathBuf::from("/home/u/a"));
        assert_eq!(expand_tilde("~/", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("~//", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde("~user/x", home), PathBuf::from("~user/x"));
    }

    #[test]
    fn an_empty_home_disables_the_expansion() {
        let none = Path::new("");
        assert_eq!(expand_tilde("~/wf", none), PathBuf::from("~/wf"));
        assert_eq!(expand_tilde("~", none), PathBuf::from("~"));
        let paths = tmp_paths("tilde-no-home");
        write_raw(
            &paths,
            json!({"version": CURRENT_VERSION, "workflow_dirs": ["~/wf", "/abs"], "theme": appearance_file("~/t")}),
        );
        let c = Config::load_in(&paths, none);
        assert_eq!(c.workflow_dirs, [PathBuf::from("~/wf"), PathBuf::from("/abs")]);
        assert_eq!(c.theme, Some(PathBuf::from(appearance_file("~/t"))));
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn a_save_keeps_the_tilde_the_user_wrote() {
        let paths = tmp_paths("tilde-save");
        let home = Path::new("/home/u");
        write_raw(
            &paths,
            json!({
                "version": CURRENT_VERSION,
                "workflow_dirs": ["~/dotfiles/workflows", "/abs/wf"],
                "app_dirs": ["~/Apps", "/Applications"],
                "theme": appearance_file("~/themes/T"),
                "future_key": 1,
            }),
        );
        let mut c = Config::load_strict_in(&paths, home).unwrap();
        c.max_results = 7; // an unrelated change, like a toggle in preferences
        c.save_in(&paths, home).unwrap();
        let raw = read_raw(&paths);
        assert_eq!(raw["max_results"], 7);
        assert_eq!(raw["workflow_dirs"], json!(["~/dotfiles/workflows", "/abs/wf"]));
        assert_eq!(raw["app_dirs"], json!(["~/Apps", "/Applications"]));
        assert_eq!(raw["theme"], appearance_file("~/themes/T"));
        assert_eq!(raw["future_key"], 1);

        // reorder, remove and add: kept entries keep their spelling, new ones are written as given
        c.workflow_dirs = vec![
            PathBuf::from("/new/abs"),
            PathBuf::from("/home/u/dotfiles/workflows"),
            PathBuf::from("~/typed/tilde"),
        ];
        c.app_dirs.truncate(1);
        c.theme = Some(PathBuf::from(appearance_file("/elsewhere/T")));
        c.save_in(&paths, home).unwrap();
        let raw = read_raw(&paths);
        assert_eq!(
            raw["workflow_dirs"],
            json!(["/new/abs", "~/dotfiles/workflows", "~/typed/tilde"])
        );
        assert_eq!(raw["app_dirs"], json!(["~/Apps"]));
        assert_eq!(raw["theme"], appearance_file("/elsewhere/T"));

        // and the next load sees the same folders again
        let c2 = Config::load_strict_in(&paths, home).unwrap();
        assert_eq!(c2.workflow_dirs[1], PathBuf::from("/home/u/dotfiles/workflows"));
        assert_eq!(c2.workflow_dirs[2], PathBuf::from("/home/u/typed/tilde"));
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn a_path_that_was_absolute_in_the_file_stays_absolute() {
        let paths = tmp_paths("tilde-abs");
        let home = Path::new("/home/u");
        write_raw(
            &paths,
            json!({"version": CURRENT_VERSION, "workflow_dirs": ["/home/u/wf"]}),
        );
        let c = Config::load_strict_in(&paths, home).unwrap();
        c.save_in(&paths, home).unwrap();
        assert_eq!(read_raw(&paths)["workflow_dirs"], json!(["/home/u/wf"]));
        let _ = std::fs::remove_dir_all(&paths.data);
    }

    #[test]
    fn config_set_with_a_tilde_is_stored_as_written() {
        // `ashdash config set workflow_dirs '["~/dotfiles/workflows"]'` goes through edit_locked
        let paths = tmp_paths("tilde-set");
        let (c, ()) = Config::edit_locked(&paths, |c| {
            c.workflow_dirs = vec![PathBuf::from("~/dotfiles/workflows")];
            c.theme = Some(PathBuf::from(appearance_file("~/t")));
            Ok(())
        })
        .unwrap();
        assert_eq!(c.workflow_dirs, [PathBuf::from("~/dotfiles/workflows")]);
        let raw = read_raw(&paths);
        assert_eq!(raw["workflow_dirs"], json!(["~/dotfiles/workflows"]));
        assert_eq!(raw["theme"], appearance_file("~/t"));
        let _ = std::fs::remove_dir_all(&paths.data);
    }
}
