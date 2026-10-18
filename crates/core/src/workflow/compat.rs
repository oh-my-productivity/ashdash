//! Which workflow object types this engine can run, and a survey helper used
//! to check imported workflows (and, in tests, a folder of sample packages).

use crate::format_ids::kind;
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
    kind::ACTION_SCRIPT,
    kind::ACTION_APPLESCRIPT,
    kind::ACTION_OPENURL,
    kind::ACTION_OPENFILE,
    kind::ACTION_LAUNCHFILES,
    kind::ACTION_REVEALFILE,
    kind::ACTION_TERMINALCOMMAND,
    kind::ACTION_ITUNESCOMMAND,
    kind::ACTION_SYSTEMCOMMAND,
    kind::ACTION_SYSTEMWEBSEARCH,
    kind::ACTION_BROWSE,
    kind::ACTION_UNIVERSAL,
    kind::OUTPUT_NOTIFICATION,
    kind::OUTPUT_CLIPBOARD,
    kind::OUTPUT_LARGETYPE,
    kind::OUTPUT_SCRIPT,
    kind::OUTPUT_WRITEFILE,
    kind::OUTPUT_CALLEXTERNALTRIGGER,
    kind::UTILITY_ARGUMENT,
    kind::UTILITY_JUNCTION,
    kind::UTILITY_DEBUG,
    kind::UTILITY_CONDITIONAL,
    kind::UTILITY_FILTER,
    kind::UTILITY_REPLACE,
    kind::UTILITY_TRANSFORM,
    kind::UTILITY_SPLIT,
    kind::UTILITY_SPLITARGUMENT,
    kind::UTILITY_MERGE,
    kind::UTILITY_JOINARGS,
    kind::UTILITY_JOINARGUMENTS,
    kind::UTILITY_DELAY,
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
