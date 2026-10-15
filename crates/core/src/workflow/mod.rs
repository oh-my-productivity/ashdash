//! Reference-compatible workflows.
//!
//! A workflow is a directory with an `info.plist` describing a graph of
//! objects (`objects`) wired by `connections`. This module loads that format,
//! implements the Script Filter JSON protocol, and walks the graph when a
//! result is actioned. The format was learned from the workflow templates
//! shipped with the reference launcher and its public workflow documentation; see
//! `docs/REVERSE_ENGINEERING.md` for what is observed vs. inferred.

mod compat;
mod exec;
mod filefilter;
mod filter;
mod model;
mod package;
mod runtime;
mod script;
mod userconfig;
mod utility;
mod xml;

use std::time::Duration;

pub use compat::{ENTRY_KINDS, FLOW_KINDS, INPUT_KINDS, is_supported, unsupported_kinds};
pub use exec::{Effect, Resume, WriteFile, WriteMode, debug_log, render, spawn_delayed};
pub use filter::{KeywordResults, ModOverride, WorkflowAction, rerun_interval};
pub use model::{Connection, Node, Workflow, load_all};
pub use package::import_package;
pub use script::{ScriptOutput, run_osascript};
pub use userconfig::ConfigVar;

pub const KIND_KEYWORD: &str = "alfred.workflow.input.keyword";
pub const KIND_SCRIPT_FILTER: &str = "alfred.workflow.input.scriptfilter";
pub const KIND_LIST_FILTER: &str = "alfred.workflow.input.listfilter";
pub const KIND_FILE_FILTER: &str = "alfred.workflow.input.filefilter";
pub const KIND_EXTERNAL_TRIGGER: &str = "alfred.workflow.trigger.external";

const SCRIPT_FILTER_TIMEOUT: Duration = Duration::from_secs(10);
const SCRIPT_ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_GRAPH_STEPS: usize = 256;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{KIND_SCRIPT_FILTER, Workflow};
    use crate::config::Paths;

    /// JSON -> plist, the way a real `info.plist` would carry the same data.
    pub(super) fn to_plist(j: &serde_json::Value) -> plist::Value {
        use serde_json::Value as J;
        match j {
            J::Null => plist::Value::String(String::new()),
            J::Bool(b) => plist::Value::Boolean(*b),
            J::Number(n) => n
                .as_i64()
                .map(plist::Value::from)
                .unwrap_or_else(|| plist::Value::Real(n.as_f64().unwrap_or_default())),
            J::String(s) => plist::Value::String(s.clone()),
            J::Array(a) => plist::Value::Array(a.iter().map(to_plist).collect()),
            J::Object(o) => plist::Value::Dictionary(o.iter().map(|(k, v)| (k.clone(), to_plist(v))).collect()),
        }
    }

    /// Write `doc` as `<root>/<id>/info.plist` and load it back through the
    /// plist path (so tests exercise plist -> JSON conversion too).
    pub(super) fn load_via_plist(root: &Path, id: &str, doc: &serde_json::Value) -> Workflow {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        to_plist(doc).to_file_xml(dir.join("info.plist")).unwrap();
        Workflow::load(&dir).unwrap()
    }

    pub(super) fn temp_paths(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-wf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (Paths::under(&root), root)
    }

    pub(super) fn sample(root: &Path) -> Workflow {
        let doc = json!({
            "name": "Sample", "bundleid": "test.sample",
            "variables": {"greeting": "hi"},
            "objects": [
                {"uid": "SF", "type": KIND_SCRIPT_FILTER, "config": {
                    "keyword": "demo", "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1,
                    "title": "Demo", "subtext": "type something",
                    "script": "printf '{\"items\":[{\"title\":\"one: %s\",\"arg\":\"A %s\",\"mods\":{\"cmd\":{\"arg\":\"CMD\"}}},{\"title\":\"two\",\"valid\":false}]}' \"$1\" \"$1\""
                }},
                {"uid": "SC", "type": "alfred.workflow.action.script", "config": {
                    "type": 0, "scriptargtype": 1, "script": "echo -n \"got:$1:$greeting\""
                }},
                {"uid": "N", "type": "alfred.workflow.output.notification", "config": {"title": "T", "text": "{query}/{var:greeting}"}},
                {"uid": "U", "type": "alfred.workflow.action.openurl", "config": {"url": "https://x.test/?q={query}", "utf8": true}},
            ],
            "connections": {
                "SF": [{"destinationuid": "SC", "modifiers": 0}, {"destinationuid": "U", "modifiers": 1048576}],
                "SC": [{"destinationuid": "N", "modifiers": 0}],
            }
        });
        Workflow::from_json(root, "sample", &doc).unwrap()
    }
}
