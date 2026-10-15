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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
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
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("ashdash: ignoring invalid config.json: {e}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        std::fs::create_dir_all(&paths.data)?;
        std::fs::write(paths.config_file(), serde_json::to_vec_pretty(self)?)
    }
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
