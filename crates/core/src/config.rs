//! Paths and user configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    pub fn icon_cache(&self) -> PathBuf {
        self.cache.join("icons")
    }
}

/// Schema version of `config.json`. Bump it and extend [`migrate`] whenever
/// the on-disk layout changes.
pub const CURRENT_VERSION: u32 = 1;

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
    pub clipboard_history_size: usize,
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
            clipboard_history_size: 200,
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
        assert_eq!(migrate(json!({"version": 1}))["version"], 1);
        assert_eq!(migrate(json!({"version": 9}))["version"], 9);
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
