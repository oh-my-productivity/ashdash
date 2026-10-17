//! Workflow data model and loading from `info.plist`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use super::runtime::Runtime;
use super::userconfig::{self, ConfigVar};
use crate::config::Paths;

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
    /// `sourceoutputuid`: which output of the source object this wire leaves
    /// from. Only Conditional objects have several outputs; the else output
    /// (and every ordinary object) has none.
    pub source_output: Option<String>,
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
    /// Workflow variables: the `variables` dictionary of `info.plist`, with
    /// the defaults of every user-configuration entry filled in.
    pub variables: BTreeMap<String, String>,
    pub version: String,
    /// Entries of the workflow configuration sheet (`userconfigurationconfig`).
    pub user_config: Vec<ConfigVar>,
    /// Variables the reference launcher strips when exporting (`variablesdontexport`).
    pub dont_export: Vec<String>,
    /// Session variables, script-filter cache and user-set values. Shared by
    /// clones so a deferred (delayed) continuation sees the same state.
    pub(super) runtime: Arc<Runtime>,
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

/// A workflow's `info.plist` holds its scripts, so it can be big; one over this is not read.
pub const MAX_INFO_PLIST_BYTES: u64 = 16 * 1024 * 1024;

/// Read `<dir>/info.plist` the only safe way: bounded in size, nesting and number of values
/// ([`crate::theme::parse_plist`]). `plist::Value::from_file` builds and drops its tree
/// recursively, so a few KB of nested arrays in a downloaded package aborts the process.
/// Every reader of a workflow's `info.plist` goes through here.
pub fn read_info_plist(dir: &Path) -> Result<plist::Value> {
    let file = dir.join("info.plist");
    let bytes = crate::theme::read_limited(&file, MAX_INFO_PLIST_BYTES)?;
    crate::theme::parse_plist(&bytes).with_context(|| format!("parsing {}", file.display()))
}

impl Workflow {
    pub fn load(dir: &Path) -> Result<Workflow> {
        let v = read_info_plist(dir).with_context(|| format!("reading {}/info.plist", dir.display()))?;
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
                        source_output: c
                            .get("sourceoutputuid")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                    });
                }
            }
        }
        // Streams leaving one object run top to bottom by the Y position of
        // their destination on the canvas (observed in the reference launcher's "Stream
        // Order" template). The sort is stable, so without `uidata` the file
        // order is kept.
        let ypos = |uid: &str| {
            j.get("uidata")
                .and_then(|u| u.get(uid))
                .and_then(|u| u.get("ypos"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };
        for list in connections.values_mut() {
            list.sort_by(|a, b| {
                ypos(&a.dest)
                    .partial_cmp(&ypos(&b.dest))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        let text = |k: &str| j.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut variables = str_map(j.get("variables"));
        let user_config = userconfig::parse(j.get("userconfigurationconfig"));
        for c in &user_config {
            variables.entry(c.variable.clone()).or_insert_with(|| c.default.clone());
        }
        Ok(Workflow {
            id: id.to_string(),
            dir: dir.to_path_buf(),
            name: text("name"),
            bundle_id: text("bundleid"),
            disabled: j.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            nodes,
            connections,
            variables,
            version: text("version"),
            user_config,
            dont_export: j
                .get("variablesdontexport")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            runtime: Arc::new(Runtime::default()),
        })
    }

    pub(super) fn node(&self, uid: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.uid == uid)
    }

    /// Workflow variables as scripts and templates see them: `info.plist`
    /// variables and configuration defaults, overridden by values the user
    /// saved for this workflow (see [`Workflow::user_values_file`]).
    pub fn base_vars(&self, paths: &Paths) -> BTreeMap<String, String> {
        let mut vars = self.variables.clone();
        vars.extend(self.runtime.user_values(&self.user_values_file(paths)));
        vars
    }

    /// Where user-set configuration values live: a flat JSON object
    /// `{"variable": "value"}` per workflow. A preferences UI writes it; it is
    /// read once per loaded workflow and again after a reload.
    pub fn user_values_file(&self, paths: &Paths) -> PathBuf {
        paths
            .data
            .join("workflow-config")
            .join(format!("{}.json", self.data_key().replace(['/', '\\'], "_")))
    }

    /// Variables that survive an export (everything but `variablesdontexport`).
    pub fn exportable_variables(&self) -> BTreeMap<String, String> {
        self.variables
            .iter()
            .filter(|(k, _)| !self.dont_export.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Forget session variables and cached Script Filter output (what the reference launcher
    /// does when the launcher closes / the workflow is reloaded).
    pub fn reset_session(&self) {
        self.runtime.reset();
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
///
/// Hidden folders (a name starting with `.`) are not workflows: the importer keeps staging
/// folders, the new tree and the previous version of a workflow being replaced there
/// (`.import-*`), and a reload during an install must not see a second copy of it. Leftovers of
/// an interrupted install are cleaned up first (`package::sweep_interrupted_installs`).
pub fn load_all(roots: &[PathBuf]) -> Vec<Workflow> {
    let mut out = Vec::new();
    for root in roots {
        super::package::sweep_interrupted_installs(root, super::package::STALE_SWAP);
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let hidden = d.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if !hidden && d.join("info.plist").is_file() {
                match Workflow::load(&d) {
                    Ok(w) => out.push(w),
                    Err(e) => eprintln!("ashdash: skipping workflow {}: {e:#}", d.display()),
                }
            }
        }
    }
    out
}
