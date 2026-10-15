//! Which reference launcher object types this engine can run, and a survey helper used
//! to check imported workflows (and, in tests, the reference launcher's bundled templates).

use std::collections::BTreeSet;

use super::{KIND_EXTERNAL_TRIGGER, KIND_FILE_FILTER, KIND_KEYWORD, KIND_LIST_FILTER, KIND_SCRIPT_FILTER, Workflow};

/// Objects that start a run from the query bar (see `Workflow::keyword_results`).
pub const INPUT_KINDS: [&str; 4] = [KIND_KEYWORD, KIND_SCRIPT_FILTER, KIND_LIST_FILTER, KIND_FILE_FILTER];

/// Objects that start a run from outside the query bar and are supported.
/// (Hotkey, snippet, fallback and universal-action triggers belong to the
/// automation work package and are reported as unsupported until then.)
pub const ENTRY_KINDS: [&str; 1] = [KIND_EXTERNAL_TRIGGER];

/// Objects that can sit in the middle or at the end of a stream. A test runs
/// every one of them through the executor to keep this list honest.
pub const FLOW_KINDS: &[&str] = &[
    "alfred.workflow.action.script",
    "alfred.workflow.action.applescript",
    "alfred.workflow.action.openurl",
    "alfred.workflow.action.openfile",
    "alfred.workflow.action.launchfiles",
    "alfred.workflow.action.revealfile",
    "alfred.workflow.action.terminalcommand",
    "alfred.workflow.action.itunescommand",
    "alfred.workflow.action.systemcommand",
    "alfred.workflow.action.systemwebsearch",
    "alfred.workflow.action.browseinalfred",
    "alfred.workflow.action.actioninalfred",
    "alfred.workflow.output.notification",
    "alfred.workflow.output.clipboard",
    "alfred.workflow.output.largetype",
    "alfred.workflow.output.script",
    "alfred.workflow.output.writefile",
    "alfred.workflow.output.callexternaltrigger",
    "alfred.workflow.utility.argument",
    "alfred.workflow.utility.junction",
    "alfred.workflow.utility.debug",
    "alfred.workflow.utility.conditional",
    "alfred.workflow.utility.filter",
    "alfred.workflow.utility.replace",
    "alfred.workflow.utility.transform",
    "alfred.workflow.utility.split",
    "alfred.workflow.utility.splitargument",
    "alfred.workflow.utility.merge",
    "alfred.workflow.utility.joinargs",
    "alfred.workflow.utility.joinarguments",
    "alfred.workflow.utility.delay",
];

pub fn is_supported(kind: &str) -> bool {
    INPUT_KINDS.contains(&kind) || ENTRY_KINDS.contains(&kind) || FLOW_KINDS.contains(&kind)
}

impl Workflow {
    /// Distinct object types in this workflow that the engine cannot run,
    /// sorted. Empty means every object is supported.
    pub fn unsupported_kinds(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self
            .nodes
            .iter()
            .map(|n| n.kind.as_str())
            .filter(|k| !is_supported(k))
            .collect();
        set.into_iter().map(str::to_string).collect()
    }
}

/// Distinct unsupported object types across several workflows, sorted.
pub fn unsupported_kinds(workflows: &[Workflow]) -> Vec<String> {
    let set: BTreeSet<String> = workflows.iter().flat_map(|w| w.unsupported_kinds()).collect();
    set.into_iter().collect()
}
