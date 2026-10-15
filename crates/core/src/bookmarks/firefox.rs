//! Firefox: bookmarks live in `places.sqlite` inside each profile directory.
//!
//! We avoid a SQLite crate dependency by shelling out to the `sqlite3` CLI
//! that ships with macOS (`/usr/bin/sqlite3`). The live database is locked
//! while Firefox runs, so we never open it: the database (and its `-wal`
//! file, where recent changes sit) is copied into a private (0700) temporary
//! directory and queried there with `-readonly`; the copy is deleted
//! afterwards. If `sqlite3` is missing or fails, the source reports an
//! ordinary error (kept as a diagnostic by the cache, not shown as a result).
//!
//! Folder structure is rebuilt from `moz_bookmarks.parent`; the tag tree
//! (`tags________`) is skipped because tagged URLs would otherwise appear
//! twice.
//!
//! Firefox appends to the `-wal` file on every page visit, so the raw
//! fingerprint changes constantly while it is in use. To keep that from
//! turning into a full database copy per recheck, [`FirefoxSource::fingerprint`]
//! reports the fingerprint of the last load until [`MIN_RELOAD_INTERVAL`] has
//! passed, so the cache copies the database at most once a minute.

use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Bookmark, BookmarkSource, FOLDER_SEP, Fingerprint, SourceError, stamp};

const QUERY: &str = "SELECT b.id AS id, b.parent AS parent, b.type AS type, b.guid AS guid, \
    COALESCE(b.title, '') AS title, COALESCE(p.url, '') AS url \
    FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk WHERE b.type IN (1, 2)";
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_DEPTH: usize = 64;
/// Minimum time between two copies of the Firefox database.
pub const MIN_RELOAD_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Throttle {
    last_load: Option<Instant>,
    /// What the files looked like when the last load started.
    fingerprint: Option<Fingerprint>,
}

pub struct FirefoxSource {
    profiles_root: PathBuf,
    sqlite3: PathBuf,
    min_interval: Duration,
    throttle: Mutex<Throttle>,
}

impl FirefoxSource {
    pub fn new(profiles_root: PathBuf) -> Self {
        FirefoxSource {
            profiles_root,
            sqlite3: PathBuf::from("/usr/bin/sqlite3"),
            min_interval: MIN_RELOAD_INTERVAL,
            throttle: Mutex::new(Throttle::default()),
        }
    }

    #[cfg(test)]
    fn with_min_interval(mut self, d: Duration) -> Self {
        self.min_interval = d;
        self
    }

    fn real_fingerprint(&self) -> Fingerprint {
        let mut fp = Vec::new();
        for db in self.databases() {
            fp.push(stamp(&db));
            fp.push(stamp(&wal_of(&db)));
        }
        fp
    }

    #[cfg(test)]
    fn with_sqlite3(mut self, p: PathBuf) -> Self {
        self.sqlite3 = p;
        self
    }

    fn databases(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(&self.profiles_root) else {
            return Vec::new();
        };
        let mut v: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path().join("places.sqlite"))
            .filter(|p| p.is_file())
            .collect();
        v.sort();
        v
    }
}

impl BookmarkSource for FirefoxSource {
    fn id(&self) -> &'static str {
        "firefox"
    }

    fn name(&self) -> &'static str {
        "Firefox"
    }

    fn fingerprint(&self) -> Fingerprint {
        let real = self.real_fingerprint();
        let t = self.throttle.lock().unwrap_or_else(|e| e.into_inner());
        match (t.last_load, &t.fingerprint) {
            // Too soon after the last copy: pretend nothing changed.
            (Some(at), Some(frozen)) if at.elapsed() < self.min_interval => frozen.clone(),
            _ => real,
        }
    }

    fn load(&self) -> Result<Vec<Bookmark>, SourceError> {
        {
            // Recorded before copying, so a change made while we copy shows up next time.
            let fp = self.real_fingerprint();
            let mut t = self.throttle.lock().unwrap_or_else(|e| e.into_inner());
            t.last_load = Some(Instant::now());
            t.fingerprint = Some(fp);
        }
        let mut out = Vec::new();
        let mut first_err = None;
        let dbs = self.databases();
        let mut ok = 0;
        for db in &dbs {
            match self.load_one(db) {
                Ok(mut v) => {
                    ok += 1;
                    out.append(&mut v);
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) if ok == 0 => Err(e),
            _ => Ok(out),
        }
    }
}

fn wal_of(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

/// A private temp directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "ashdash-ff-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::DirBuilder::new().mode(0o700).create(&p)?;
        Ok(Scratch(p))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl FirefoxSource {
    fn load_one(&self, db: &Path) -> Result<Vec<Bookmark>, SourceError> {
        let scratch = Scratch::new().map_err(|e| SourceError::other(format!("temp dir: {e}")))?;
        let copy = scratch.0.join("places.sqlite");
        std::fs::copy(db, &copy).map_err(|e| SourceError::from_io(db, &e))?;
        let wal = wal_of(db);
        if wal.is_file() {
            std::fs::copy(&wal, wal_of(&copy)).map_err(|e| SourceError::from_io(&wal, &e))?;
        }
        let stdout = run(Command::new(&self.sqlite3)
            .arg("-readonly")
            .arg("-json")
            .arg(&copy)
            .arg(QUERY))?;
        parse_rows(&stdout)
    }
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, SourceError> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SourceError::other(format!("cannot run sqlite3: {e}")))?;
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SourceError::other("sqlite3 timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(SourceError::other(format!("sqlite3: {e}"))),
        }
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let msg = String::from_utf8_lossy(&err);
        Err(SourceError::other(format!(
            "sqlite3 failed: {}",
            msg.lines().next().unwrap_or("unknown error")
        )))
    }
}

struct Row {
    parent: i64,
    kind: i64,
    guid: String,
    title: String,
    url: String,
}

fn parse_rows(stdout: &[u8]) -> Result<Vec<Bookmark>, SourceError> {
    // `-json` prints nothing at all for an empty result set.
    if stdout.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Vec::new());
    }
    let v: Value =
        serde_json::from_slice(stdout).map_err(|e| SourceError::other(format!("unexpected sqlite3 output ({e})")))?;
    let rows = v
        .as_array()
        .ok_or_else(|| SourceError::other("unexpected sqlite3 output"))?;
    let mut by_id = std::collections::HashMap::new();
    for r in rows {
        let (Some(id), Some(parent), Some(kind)) = (
            r.get("id").and_then(Value::as_i64),
            r.get("parent").and_then(Value::as_i64),
            r.get("type").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        by_id.insert(
            id,
            Row {
                parent,
                kind,
                guid: s("guid"),
                title: s("title"),
                url: s("url"),
            },
        );
    }

    let mut out = Vec::new();
    'rows: for row in by_id.values().filter(|r| r.kind == 1 && !r.url.is_empty()) {
        let mut folders = Vec::new();
        let mut cur = row.parent;
        for _ in 0..MAX_DEPTH {
            let Some(p) = by_id.get(&cur) else { break };
            match p.guid.as_str() {
                "tags________" => continue 'rows,
                "root________" => break,
                "menu________" => folders.push("Bookmarks Menu".to_string()),
                "toolbar_____" => folders.push("Bookmarks Toolbar".to_string()),
                "unfiled_____" => folders.push("Other Bookmarks".to_string()),
                "mobile______" => folders.push("Mobile Bookmarks".to_string()),
                _ => folders.push(if p.title.is_empty() {
                    "Untitled".into()
                } else {
                    p.title.clone()
                }),
            }
            if p.parent == cur {
                break;
            }
            cur = p.parent;
        }
        folders.reverse();
        out.push(Bookmark {
            title: row.title.clone(),
            url: row.url.clone(),
            folder: folders.join(FOLDER_SEP),
            source: "Firefox".to_string(),
        });
    }
    // HashMap order is arbitrary; keep results stable.
    out.sort_by(|a, b| (&a.folder, &a.title, &a.url).cmp(&(&b.folder, &b.title, &b.url)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-bm-ff-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn have_sqlite3() -> bool {
        Command::new("/usr/bin/sqlite3").arg("--version").output().is_ok()
    }

    /// Builds a synthetic places.sqlite with the real table/column names.
    fn make_db(path: &Path) {
        let sql = "\
CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT);
CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER, parent INTEGER, title TEXT, guid TEXT);
INSERT INTO moz_places VALUES (1,'https://www.mozilla.org/'),(2,'https://crates.io/'),(3,'place:type=6'),(4,'javascript:void(0)');
INSERT INTO moz_bookmarks VALUES
 (1,2,NULL,0,'','root________'),
 (2,2,NULL,1,'menu','menu________'),
 (3,2,NULL,1,'toolbar','toolbar_____'),
 (4,2,NULL,1,'','tags________'),
 (5,2,NULL,3,'Dev','folderguid01'),
 (6,1,1,2,'Mozilla','bm0000000001'),
 (7,1,2,5,'Crates','bm0000000002'),
 (8,1,3,2,'Smart query','bm0000000003'),
 (9,2,NULL,4,'rusty','tagguid00001'),
 (10,1,2,9,'Crates tagged','bm0000000004'),
 (11,1,4,2,'Bookmarklet','bm0000000005');";
        let st = Command::new("/usr/bin/sqlite3").arg(path).arg(sql).status().unwrap();
        assert!(st.success());
    }

    #[test]
    fn reads_a_snapshot_copy_with_folders_and_skips_tags() {
        if !have_sqlite3() {
            return;
        }
        let root = tmp("ok");
        let prof = root.join("abcd.default-release");
        std::fs::create_dir_all(&prof).unwrap();
        make_db(&prof.join("places.sqlite"));
        let src = FirefoxSource::new(root.clone());
        let before = std::fs::metadata(prof.join("places.sqlite")).unwrap().len();
        let got = src.load().unwrap();
        let rows: Vec<_> = got.iter().map(|b| (b.title.as_str(), b.folder.as_str())).collect();
        // The provider-level cache drops place:/javascript: later; the source
        // reports everything with a URL except tagged duplicates.
        assert!(rows.contains(&("Mozilla", "Bookmarks Menu")));
        assert!(rows.contains(&("Crates", "Bookmarks Toolbar › Dev")));
        assert!(!rows.iter().any(|r| r.0 == "Crates tagged"));
        // The original is untouched and no scratch dir is left behind.
        assert_eq!(std::fs::metadata(prof.join("places.sqlite")).unwrap().len(), before);
        assert_eq!(src.fingerprint().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_profiles_missing_tool_and_corrupt_db() {
        let root = tmp("bad");
        // No profiles at all.
        assert!(FirefoxSource::new(root.join("none")).load().unwrap().is_empty());
        let prof = root.join("p.default");
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(prof.join("places.sqlite"), b"this is not a database").unwrap();
        // Missing sqlite3 binary.
        let e = FirefoxSource::new(root.clone())
            .with_sqlite3(root.join("no-such-sqlite3"))
            .load()
            .unwrap_err();
        assert!(e.message.contains("sqlite3"));
        if have_sqlite3() {
            // Corrupt database.
            assert!(FirefoxSource::new(root.clone()).load().is_err());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wal_churn_does_not_change_the_fingerprint_within_the_throttle_window() {
        let root = tmp("throttle");
        let prof = root.join("p.default");
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(prof.join("places.sqlite"), b"db").unwrap();
        let wal = prof.join("places.sqlite-wal");
        let make = |interval| {
            FirefoxSource::new(root.clone())
                .with_sqlite3(root.join("no-such-sqlite3")) // load() fails, which still counts as a load
                .with_min_interval(interval)
        };

        let throttled = make(Duration::from_secs(3600));
        let before_any_load = throttled.fingerprint();
        let _ = throttled.load();
        std::fs::write(&wal, b"firefox visited a page").unwrap();
        assert_eq!(
            throttled.fingerprint(),
            before_any_load,
            "frozen to the state at the last load"
        );

        // the same churn is visible once the throttle is off
        let open = make(Duration::ZERO);
        let _ = open.load();
        let seen = open.fingerprint();
        std::fs::write(&wal, b"firefox visited another page, so the file is longer").unwrap();
        assert_ne!(open.fingerprint(), seen);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_output_and_garbage_output() {
        assert!(parse_rows(b"").unwrap().is_empty());
        assert!(parse_rows(b"\n").unwrap().is_empty());
        assert!(parse_rows(b"not json").is_err());
        assert!(parse_rows(b"{}").is_err());
    }
}
