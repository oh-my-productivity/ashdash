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
    /// `info.plist`). Point this at an existing reference launcher `workflows` directory
    /// to run workflows you already have.
    pub workflow_dirs: Vec<PathBuf>,
    pub web_searches: Vec<WebSearch>,
    /// Path to an `.alfredappearance` theme file.
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
        }
    }
}

impl FileSearchSettings {
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
        let expand = |s: &String| match s.as_str() {
            "~" => home.to_path_buf(),
            _ => match s.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(s),
            },
        };
        let mut excluded: Vec<PathBuf> = self.excluded_paths.iter().map(expand).collect();
        if self.include_icloud_drive {
            excluded = carve_out(excluded, &home.join(ICLOUD_DRIVE), list);
        }
        // Start from the constructor and assign, so fields added to the scope later keep their defaults.
        let mut scope = FileSearchScope::new(self.roots.iter().map(expand).collect(), excluded);
        scope.excluded_names = self.excluded_names.clone();
        scope.excluded_substrings = self.excluded_substrings.clone();
        scope.include_hidden = self.include_hidden;
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
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Config {
        match std::fs::read(paths.config_file()) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .map(migrate)
                .and_then(serde_json::from_value)
                .unwrap_or_else(|e| {
                    eprintln!("ashdash: ignoring invalid config.json: {e}");
                    Config::default()
                }),
            Err(_) => Config::default(),
        }
    }

    /// Like [`Config::load`] but reports a file that exists and cannot be read as
    /// settings instead of falling back to defaults (so a CLI edit never
    /// replaces a hand-written file it failed to parse). A missing file is not
    /// an error.
    pub fn load_strict(paths: &Paths) -> anyhow::Result<Config> {
        let bytes = match std::fs::read(paths.config_file()) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e.into()),
        };
        let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
        Ok(serde_json::from_value(migrate(raw))?)
    }

    /// Writes the current schema version. Refuses to overwrite a file written by a
    /// newer build, which would silently drop settings this build does not know.
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        if let Ok(bytes) = std::fs::read(paths.config_file())
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && v.get("version").and_then(|n| n.as_u64()).unwrap_or(0) > CURRENT_VERSION as u64
        {
            return Err(std::io::Error::other(
                "config.json was written by a newer version; refusing to overwrite it",
            ));
        }
        std::fs::create_dir_all(&paths.data)?;
        let mut out = self.clone();
        out.version = CURRENT_VERSION;
        std::fs::write(paths.config_file(), serde_json::to_vec_pretty(&out)?)
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

/// Write a file readable only by the current user (used for clipboard data).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
