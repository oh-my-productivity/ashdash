//! The on-disk form of the app index: a small, versioned JSON file in the cache dir.
//!
//! The cache is derived data: anything unexpected (missing, corrupt, other version, other set
//! of directories) is a plain miss and the caller falls back to scanning.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when the layout of [`Record`] / the file changes; older and newer files are ignored.
pub const INDEX_VERSION: u32 = 1;

/// File name inside the cache directory.
pub const FILE_NAME: &str = "apps-index.json";

/// One application bundle as found by a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub path: PathBuf,
    /// Modification time (ns since the Unix epoch) of `Contents/Info.plist`, or of the bundle
    /// directory when there is none. A changed stamp means the record must be re-read.
    pub mtime: i64,
    /// `LSBackgroundOnly`: kept in the index so it is not re-parsed, but never shown.
    pub background: bool,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    dirs: Vec<PathBuf>,
    apps: Vec<Record>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Miss {
    Missing,
    Corrupt,
    Version(u32),
    DifferentDirs,
}

/// Read the cache. Records that cannot belong to a scan of `dirs` are dropped, so a hand-edited
/// or damaged file cannot make the launcher open arbitrary paths.
pub fn load(file: &Path, dirs: &[PathBuf]) -> Result<Vec<Record>, Miss> {
    let bytes = std::fs::read(file).map_err(|_| Miss::Missing)?;
    let parsed: CacheFile = match serde_json::from_slice(&bytes) {
        Ok(p) => p,
        Err(_) => {
            // A file from another version may not even have our shape; still report its version.
            let v = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("version")?.as_u64());
            return Err(match v {
                Some(v) if v != INDEX_VERSION as u64 => Miss::Version(v.min(u32::MAX as u64) as u32),
                _ => Miss::Corrupt,
            });
        }
    };
    if parsed.version != INDEX_VERSION {
        return Err(Miss::Version(parsed.version));
    }
    if parsed.dirs != dirs {
        return Err(Miss::DifferentDirs);
    }
    Ok(parsed.apps.into_iter().filter(|r| plausible(r, dirs)).collect())
}

fn plausible(r: &Record, dirs: &[PathBuf]) -> bool {
    r.path.is_absolute()
        && r.path.extension().is_some_and(|e| e == "app")
        && r.path.file_stem().and_then(|s| s.to_str()) == Some(r.name.as_str())
        && dirs.iter().any(|d| r.path.starts_with(d))
}

/// Atomically (re)write the cache with owner-only permissions. With `create_parents == false`
/// only the cache directory itself is created, never its ancestors: if a whole data root was
/// deleted underneath us (tests, uninstall) we must not resurrect it.
pub fn save<'a>(
    file: &Path,
    dirs: &[PathBuf],
    records: impl Iterator<Item = &'a Record>,
    create_parents: bool,
) -> std::io::Result<()> {
    if let Some(parent) = file.parent()
        && !parent.exists()
    {
        if create_parents {
            std::fs::create_dir_all(parent)?;
        } else {
            std::fs::create_dir(parent)?;
        }
    }
    let out = CacheFile {
        version: INDEX_VERSION,
        dirs: dirs.to_vec(),
        // JSON cannot carry non-UTF-8 paths; such apps are simply re-found by the next scan
        apps: records
            .filter(|r| r.path.to_str().is_some())
            .cloned()
            .collect(),
    };
    let bytes = serde_json::to_vec(&out).map_err(std::io::Error::other)?;
    crate::config::write_private(file, &bytes)
}
