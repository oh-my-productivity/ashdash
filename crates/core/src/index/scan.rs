//! Walking application directories and mapping file-system events onto them.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use super::store::Record;

/// `.app` bundles are looked for this many levels below each configured directory.
pub const MAX_DEPTH: usize = 3;

pub type Records = BTreeMap<PathBuf, Record>;

/// Faceless helpers (System Events, ...) are not launchable by users.
pub(crate) fn is_background_only(app: &Path) -> bool {
    plist::Value::from_file(app.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| v.as_dictionary()?.get("LSBackgroundOnly")?.as_boolean())
        .unwrap_or(false)
}

fn mtime_ns(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = m.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

/// Change stamp of a bundle: cheap (one or two `stat`s) compared to parsing its Info.plist.
fn stamp(app: &Path) -> i64 {
    mtime_ns(&app.join("Contents/Info.plist"))
        .or_else(|| mtime_ns(app))
        .unwrap_or(0)
}

/// Describe one bundle, re-using `prev` when its stamp is unchanged.
pub fn probe(app: &Path, prev: Option<&Record>) -> Option<Record> {
    let name = app.file_stem()?.to_str()?;
    let mtime = stamp(app);
    if let Some(p) = prev
        && p.mtime == mtime
        && p.path == app
    {
        return Some(p.clone());
    }
    Some(Record {
        name: name.to_string(),
        path: app.to_path_buf(),
        mtime,
        background: is_background_only(app),
    })
}

/// Find bundles below `root` (at most `max_depth` levels, never inside a bundle) into `out`.
pub fn walk(root: &Path, max_depth: usize, prev: &Records, out: &mut Records) {
    let mut it = WalkDir::new(root).max_depth(max_depth).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        // `is_dir` follows symlinks: Safari lives behind one on recent macOS.
        if p.extension().is_some_and(|e| e == "app") && p.is_dir() {
            it.skip_current_dir();
            if let Some(rec) = probe(p, prev.get(p)) {
                out.entry(rec.path.clone()).or_insert(rec);
            }
        }
    }
}

/// Scan every directory. `prev` lets unchanged bundles skip the plist parse.
pub fn scan_all(dirs: &[PathBuf], prev: &Records) -> Records {
    let mut out = Records::new();
    for d in dirs {
        walk(d, MAX_DEPTH, prev, &mut out);
    }
    out
}

/// A watched directory, with its canonical form: FSEvents reports real paths
/// (`/private/var/...` for `/var/...`), while records use the configured spelling.
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub canon: PathBuf,
}

impl Root {
    pub fn new(path: &Path) -> Root {
        Root {
            path: path.to_path_buf(),
            canon: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        }
    }
}

/// What to re-examine after an event on some path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// One `.app`: re-probe it, or drop it when it is gone.
    Bundle(PathBuf),
    /// A directory (or vanished path): re-walk `depth` levels below it and prune what is gone.
    Subtree { path: PathBuf, depth: usize },
}

/// Map an event path to the smallest piece of the index that may have changed.
pub fn classify(roots: &[Root], event: &Path) -> Option<Target> {
    for root in roots {
        let rel = event
            .strip_prefix(&root.canon)
            .or_else(|_| event.strip_prefix(&root.path));
        let Ok(rel) = rel else { continue };
        let mut cur = root.path.clone();
        let mut depth = 0;
        for c in rel.components() {
            let Component::Normal(name) = c else { return None };
            depth += 1;
            cur.push(name);
            if depth <= MAX_DEPTH && Path::new(name).extension().is_some_and(|e| e == "app") {
                // the outermost bundle owns everything below it
                return Some(Target::Bundle(cur));
            }
        }
        // `then`, not `then_some`: the depth subtraction must stay lazy
        return (depth < MAX_DEPTH).then(|| Target::Subtree {
            path: cur,
            depth: MAX_DEPTH - depth,
        });
    }
    None
}
