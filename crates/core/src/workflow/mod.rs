//! Reference-compatible workflows.
//!
//! A workflow is a directory with an `info.plist` describing a graph of
//! objects (`objects`) wired by `connections`. This module loads that format,
//! implements the Script Filter JSON protocol, and walks the graph when a
//! result is actioned. The format was learned from the workflow templates
//! shipped with the reference launcher and its public workflow documentation; see
//! `docs/REVERSE_ENGINEERING.md` for what is observed vs. inferred.

mod exec;
mod filter;
mod model;
mod package;
mod script;

use std::time::Duration;

pub use exec::{Effect, render};
pub use filter::{ModOverride, WorkflowAction};
pub use model::{Connection, Node, Workflow, load_all};
pub use package::import_package;
pub use script::ScriptOutput;

pub const KIND_KEYWORD: &str = "alfred.workflow.input.keyword";
pub const KIND_SCRIPT_FILTER: &str = "alfred.workflow.input.scriptfilter";
pub const KIND_EXTERNAL_TRIGGER: &str = "alfred.workflow.trigger.external";

const SCRIPT_FILTER_TIMEOUT: Duration = Duration::from_secs(10);
const SCRIPT_ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_GRAPH_STEPS: usize = 256;

#[cfg(test)]
mod test_support {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{KIND_SCRIPT_FILTER, Workflow};
    use crate::config::Paths;

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
