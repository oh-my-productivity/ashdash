//! Browser bookmarks: sources, a background-loaded cache and the settings
//! that choose which sources are read.
//!
//! Layout:
//! * [`BookmarkSource`] is the abstraction every browser implements
//!   (`safari`, `chromium`, `arc`, `firefox`). A source reports a cheap
//!   [`Fingerprint`] of its files and can `load` them.
//! * [`cache::BookmarkCache`] keeps the merged, de-duplicated list in memory,
//!   reloads a source only when its fingerprint (mtime + size) changed, and
//!   does all disk work on a background thread.
//! * `providers/bookmarks.rs` turns the cache into launcher results.
//!
//! A source whose files cannot be read because of macOS privacy protection
//! (Safari without Full Disk Access) reports [`SourceErrorKind::PermissionDenied`];
//! the provider turns that into one explanatory result item.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub mod arc;
pub mod cache;
pub mod chromium;
pub mod firefox;
pub mod safari;
pub mod url;

pub use cache::{BookmarkCache, Issue, Snapshot};

/// Separator used when joining folder names into a display path.
pub const FOLDER_SEP: &str = " › ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// Folder path for display, e.g. `Bookmarks Bar › Dev`. Empty at the top level.
    pub folder: String,
    /// Human-readable origin, e.g. `Chrome` or `Brave (Profile 1)`.
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceErrorKind {
    /// The OS refused to let us read the file (e.g. Full Disk Access).
    PermissionDenied,
    /// Malformed content, missing helper tool, I/O failure...
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub kind: SourceErrorKind,
    pub message: String,
}

impl SourceError {
    pub fn other(message: impl Into<String>) -> Self {
        SourceError {
            kind: SourceErrorKind::Other,
            message: message.into(),
        }
    }

    pub fn from_io(path: &Path, e: &io::Error) -> Self {
        let kind = if e.kind() == io::ErrorKind::PermissionDenied {
            SourceErrorKind::PermissionDenied
        } else {
            SourceErrorKind::Other
        };
        SourceError {
            kind,
            message: format!("{}: {e}", path.display()),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// `(path, Some((mtime, size)))`, or `None` when the file is missing or cannot be stat'ed.
pub type Fingerprint = Vec<(PathBuf, Option<(SystemTime, u64)>)>;

pub fn stamp(path: &Path) -> (PathBuf, Option<(SystemTime, u64)>) {
    let st = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    (path.to_path_buf(), st)
}

pub trait BookmarkSource: Send + Sync {
    /// Stable identifier used in [`BookmarkSettings::enabled_sources`].
    fn id(&self) -> &'static str;
    /// Display name (`Safari`, `Chrome`...).
    fn name(&self) -> &'static str;
    /// Cheap change detector; may touch the file system (stat, read_dir) but
    /// never reads file contents.
    fn fingerprint(&self) -> Fingerprint;
    /// Read every bookmark. A missing file is not an error (empty list).
    fn load(&self) -> Result<Vec<Bookmark>, SourceError>;
}

/// Ids of every built-in source, in default precedence order (earlier wins on
/// duplicate URLs).
pub const ALL_SOURCE_IDS: &[&str] = &[
    "safari",
    "chrome",
    "chrome-beta",
    "chrome-canary",
    "chromium",
    "brave",
    "edge",
    "vivaldi",
    "opera",
    "arc",
    "firefox",
];

/// User-facing bookmark settings. Wiring into `Config` is a follow-up: add
/// `#[serde(default)] pub bookmarks: BookmarkSettings` to `config::Config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookmarkSettings {
    /// Source ids (see [`ALL_SOURCE_IDS`]) to read, in precedence order.
    pub enabled_sources: Vec<String>,
    /// Also contribute ranked bookmark results to ordinary (non-`bm`) queries.
    pub include_in_general_search: bool,
}

impl Default for BookmarkSettings {
    fn default() -> Self {
        BookmarkSettings {
            enabled_sources: ALL_SOURCE_IDS.iter().map(|s| s.to_string()).collect(),
            include_in_general_search: true,
        }
    }
}

/// Build the sources named in `enabled` (unknown ids are ignored) for the
/// given home directory. Nothing is read from disk here.
pub fn build_sources(home: &Path, enabled: &[String]) -> Vec<Box<dyn BookmarkSource>> {
    let support = home.join("Library/Application Support");
    let chromium = |id, name, dirs: &[&str]| -> Box<dyn BookmarkSource> {
        Box::new(chromium::ChromiumSource::new(
            id,
            name,
            dirs.iter().map(|d| support.join(d)).collect(),
        ))
    };
    let mut out: Vec<Box<dyn BookmarkSource>> = Vec::new();
    for id in enabled {
        let src: Box<dyn BookmarkSource> = match id.as_str() {
            "safari" => Box::new(safari::SafariSource::new(home.join("Library/Safari/Bookmarks.plist"))),
            "chrome" => chromium("chrome", "Chrome", &["Google/Chrome"]),
            "chrome-beta" => chromium("chrome-beta", "Chrome Beta", &["Google/Chrome Beta"]),
            "chrome-canary" => chromium("chrome-canary", "Chrome Canary", &["Google/Chrome Canary"]),
            "chromium" => chromium("chromium", "Chromium", &["Chromium"]),
            "brave" => chromium(
                "brave",
                "Brave",
                &[
                    "BraveSoftware/Brave-Browser",
                    "BraveSoftware/Brave-Browser-Beta",
                    "BraveSoftware/Brave-Browser-Dev",
                    "BraveSoftware/Brave-Browser-Nightly",
                ],
            ),
            "edge" => chromium(
                "edge",
                "Edge",
                &[
                    "Microsoft Edge",
                    "Microsoft Edge Beta",
                    "Microsoft Edge Dev",
                    "Microsoft Edge Canary",
                ],
            ),
            "vivaldi" => chromium("vivaldi", "Vivaldi", &["Vivaldi"]),
            "opera" => chromium(
                "opera",
                "Opera",
                &["com.operasoftware.Opera", "com.operasoftware.OperaNext"],
            ),
            "arc" => Box::new(arc::ArcSource::new(support.join("Arc/StorableSidebar.json"))),
            "firefox" => Box::new(firefox::FirefoxSource::new(support.join("Firefox/Profiles"))),
            _ => continue,
        };
        if !out.iter().any(|s| s.id() == src.id()) {
            out.push(src);
        }
    }
    out
}

/// Read a whole file; `Ok(None)` when it does not exist.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, SourceError> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SourceError::from_io(path, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_everything() {
        let s = BookmarkSettings::default();
        assert!(s.include_in_general_search);
        assert_eq!(s.enabled_sources.len(), ALL_SOURCE_IDS.len());
    }

    #[test]
    fn settings_deserialize_with_missing_fields() {
        let s: BookmarkSettings = serde_json::from_str(r#"{"enabled_sources":["safari"]}"#).unwrap();
        assert_eq!(s.enabled_sources, vec!["safari"]);
        assert!(s.include_in_general_search);
    }

    #[test]
    fn build_sources_ignores_unknown_and_duplicate_ids() {
        let home = std::env::temp_dir().join("oa-bm-nohome");
        let ids: Vec<String> = ["chrome", "nope", "chrome", "safari"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let srcs = build_sources(&home, &ids);
        let got: Vec<_> = srcs.iter().map(|s| s.id()).collect();
        assert_eq!(got, vec!["chrome", "safari"]);
    }

    #[test]
    fn every_known_id_builds_a_source() {
        let home = std::env::temp_dir().join("oa-bm-nohome");
        let ids: Vec<String> = ALL_SOURCE_IDS.iter().map(|s| s.to_string()).collect();
        assert_eq!(build_sources(&home, &ids).len(), ALL_SOURCE_IDS.len());
    }

    /// Manual smoke test against the real home directory. Prints only counts
    /// and error kinds, never titles or URLs:
    /// `cargo test -p ashdash-core real_home_smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_home_smoke() {
        let home = dirs::home_dir().unwrap();
        let ids: Vec<String> = ALL_SOURCE_IDS.iter().map(|s| s.to_string()).collect();
        for src in build_sources(&home, &ids) {
            let started = std::time::Instant::now();
            let files = src.fingerprint().len();
            match src.load() {
                Ok(v) => println!(
                    "{:<14} files={files} bookmarks={} ({:?})",
                    src.id(),
                    v.len(),
                    started.elapsed()
                ),
                Err(e) => println!(
                    "{:<14} files={files} ERROR kind={:?} ({:?})",
                    src.id(),
                    e.kind,
                    started.elapsed()
                ),
            }
        }
    }
}
