//! Workflow data model and loading from `info.plist`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Node {
    pub uid: String,
    pub kind: String,
    pub config: Value,
}

impl Node {
    pub(super) fn s(&self, key: &str) -> String {
        match self.config.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }
    pub(super) fn i(&self, key: &str) -> i64 {
        self.config.get(key).and_then(Value::as_i64).unwrap_or(0)
    }
    pub(super) fn b(&self, key: &str) -> bool {
        self.config.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub dest: String,
    pub modifiers: u64,
}

#[derive(Debug, Clone)]
pub struct Workflow {
    /// Directory name; unique key within a loaded set.
    pub id: String,
    pub dir: PathBuf,
    pub name: String,
    pub bundle_id: String,
    pub disabled: bool,
    pub nodes: Vec<Node>,
    pub connections: HashMap<String, Vec<Connection>>,
    pub variables: BTreeMap<String, String>,
}

// ---------------------------------------------------------------- loading

fn plist_to_json(v: &plist::Value) -> Value {
    match v {
        plist::Value::Array(a) => Value::Array(a.iter().map(plist_to_json).collect()),
        plist::Value::Dictionary(d) => Value::Object(d.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect()),
        plist::Value::Boolean(b) => Value::Bool(*b),
        plist::Value::Real(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        plist::Value::Integer(i) => {
            if let Some(n) = i.as_signed() {
                Value::from(n)
            } else if let Some(n) = i.as_unsigned() {
                Value::from(n)
            } else {
                Value::Null
            }
        }
        plist::Value::String(s) => Value::String(s.clone()),
        _ => Value::Null,
    }
}

pub(super) fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

pub(super) fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), stringify(v))).collect())
        .unwrap_or_default()
}

impl Workflow {
    pub fn load(dir: &Path) -> Result<Workflow> {
        let v = plist::Value::from_file(dir.join("info.plist"))
            .with_context(|| format!("reading {}/info.plist", dir.display()))?;
        let id = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        Workflow::from_json(dir, &id, &plist_to_json(&v))
    }

    pub fn from_json(dir: &Path, id: &str, j: &Value) -> Result<Workflow> {
        let mut nodes = Vec::new();
        for o in j.get("objects").and_then(Value::as_array).into_iter().flatten() {
            let uid = o
                .get("uid")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("object without uid"))?;
            nodes.push(Node {
                uid: uid.to_string(),
                kind: o.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
                config: o.get("config").cloned().unwrap_or(Value::Null),
            });
        }
        let mut connections: HashMap<String, Vec<Connection>> = HashMap::new();
        for (from, list) in j.get("connections").and_then(Value::as_object).into_iter().flatten() {
            for c in list.as_array().into_iter().flatten() {
                if let Some(dest) = c.get("destinationuid").and_then(Value::as_str) {
                    connections.entry(from.clone()).or_default().push(Connection {
                        dest: dest.to_string(),
                        modifiers: c.get("modifiers").and_then(Value::as_u64).unwrap_or(0),
                    });
                }
            }
        }
        let text = |k: &str| j.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(Workflow {
            id: id.to_string(),
            dir: dir.to_path_buf(),
            name: text("name"),
            bundle_id: text("bundleid"),
            disabled: j.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            nodes,
            connections,
            variables: str_map(j.get("variables")),
        })
    }

    pub(super) fn node(&self, uid: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.uid == uid)
    }

    pub(super) fn data_key(&self) -> String {
        if self.bundle_id.is_empty() {
            self.id.clone()
        } else {
            self.bundle_id.clone()
        }
    }
}

/// Load every workflow found directly under each directory in `roots`.
pub fn load_all(roots: &[PathBuf]) -> Vec<Workflow> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            if d.join("info.plist").is_file() {
                match Workflow::load(&d) {
                    Ok(w) => out.push(w),
                    Err(e) => eprintln!("ashdash: skipping workflow {}: {e:#}", d.display()),
                }
            }
        }
    }
    out
}
