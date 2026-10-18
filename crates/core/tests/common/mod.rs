//! Shared fixtures for the integration suites. Everything lives in a unique
//! temp directory; nothing here touches the real user's data or HOME.
#![allow(dead_code)] // each test binary uses a different subset

use ashdash_core::format_ids::kind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ashdash_core::{Config, Engine, Paths};
use serde_json::{Value, json};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("oa-qa-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // Canonicalise so that comparisons with paths returned by the OS work (/var -> /private/var).
        TempDir(dir.canonicalize().unwrap_or(dir))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A complete fake environment: fake `/Applications`, data and cache dirs.
pub struct Env {
    pub root: TempDir,
    pub apps: PathBuf,
    pub paths: Paths,
}

impl Env {
    pub fn new(tag: &str, app_names: &[&str]) -> Env {
        let root = TempDir::new(tag);
        let apps = root.join("apps");
        for a in app_names {
            std::fs::create_dir_all(apps.join(format!("{a}.app"))).unwrap();
        }
        std::fs::create_dir_all(&apps).unwrap();
        let paths = Paths::under(&root.join("env"));
        Env { root, apps, paths }
    }

    pub fn config(&self) -> Config {
        Config {
            app_dirs: vec![self.apps.clone()],
            ..Config::default()
        }
    }

    pub fn engine(&self) -> Engine {
        Engine::new(self.paths.clone(), self.config())
    }

    /// Write a workflow directory (info.plist) into the managed workflows dir.
    pub fn install_workflow(&self, dir_name: &str, doc: &Value) -> PathBuf {
        let dir = self.paths.workflows_dir().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        write_info_plist(&dir, doc);
        dir
    }
}

pub fn json_to_plist(j: &Value) -> plist::Value {
    match j {
        Value::Null => plist::Value::String(String::new()),
        Value::Bool(b) => plist::Value::Boolean(*b),
        Value::Number(n) => n
            .as_i64()
            .map(plist::Value::from)
            .unwrap_or_else(|| plist::Value::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => plist::Value::String(s.clone()),
        Value::Array(a) => plist::Value::Array(a.iter().map(json_to_plist).collect()),
        Value::Object(o) => plist::Value::Dictionary(o.iter().map(|(k, v)| (k.clone(), json_to_plist(v))).collect()),
    }
}

pub fn write_info_plist(dir: &Path, doc: &Value) {
    json_to_plist(doc).to_file_xml(dir.join("info.plist")).unwrap();
}

pub fn plist_bytes(doc: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    json_to_plist(doc).to_writer_xml(&mut buf).unwrap();
    buf
}

/// A minimal valid workflow document: keyword `kw` -> large type "{query}!".
pub fn keyword_workflow(name: &str, bundle: &str, kw: &str) -> Value {
    json!({
        "name": name, "bundleid": bundle,
        "objects": [
            {"uid": "K", "type": kind::INPUT_KEYWORD, "config": {
                "keyword": kw, "withspace": true, "argumenttype": 1, "text": name, "subtext": "sub {query}"}},
            {"uid": "L", "type": kind::OUTPUT_LARGETYPE, "config": {"largetypetext": "{query}!"}}
        ],
        "connections": {"K": [{"destinationuid": "L", "modifiers": 0}]}
    })
}

/// A workflow with one Script Filter (keyword `kw`, argv mode) whose script is `script`.
pub fn script_filter_workflow(kw: &str, script: &str) -> Value {
    json!({
        "name": "SF", "bundleid": "qa.sf",
        "objects": [
            {"uid": "SF", "type": kind::INPUT_SCRIPTFILTER, "config": {
                "keyword": kw, "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1,
                "title": "SF", "subtext": "", "script": script}},
            {"uid": "N", "type": kind::OUTPUT_NOTIFICATION, "config": {"title": "T", "text": "{query}"}}
        ],
        "connections": {"SF": [{"destinationuid": "N", "modifiers": 0}]}
    })
}

// ---------------------------------------------------------------- zip builders

pub enum Entry<'a> {
    File(&'a str, Vec<u8>),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
}

/// Build a zip at `path` from raw entries. Names are written verbatim (no sanitising),
/// which is the point: these archives are hostile.
pub fn write_zip(path: &Path, entries: &[Entry]) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for e in entries {
        match e {
            Entry::File(name, data) => {
                z.start_file(*name, opts).unwrap();
                z.write_all(data).unwrap();
            }
            Entry::Dir(name) => {
                z.add_directory(*name, opts).unwrap();
            }
            Entry::Symlink(name, target) => {
                z.add_symlink(*name, *target, opts).unwrap();
            }
        }
    }
    z.finish().unwrap();
}

/// Sorted list of every path (relative) under `root`, not following symlinks.
pub fn snapshot(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walkdir_lite(root) {
        out.push(e.strip_prefix(root).unwrap().display().to_string());
    }
    out.sort();
    out
}

fn walkdir_lite(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            out.push(p.clone());
            // symlink_metadata: never descend through links
            if std::fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false) {
                stack.push(p);
            }
        }
    }
    out
}
