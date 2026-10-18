//! Identifiers defined by the workflow package and appearance file formats.
//!
//! Existing workflow packages, appearance files and snippet collections contain literal
//! identifiers: object types, file extensions, dictionary keys and the names of the script
//! variables. The code has to match them exactly, so they are listed here, once, and every
//! other file refers to these constants. Changing a value breaks compatibility with existing
//! files.
//!
//! The packaging and QA scripts read the same constants through `scripts/format-id.sh` and
//! `scripts/format_ids.py`, which parse this file: keep every constant on one line in the form
//! `pub const NAME: &str = "value";`.

/// Object types found in the `objects` list of a workflow's `info.plist`.
pub mod kind {
    /// Prefix shared by every object type.
    pub const PREFIX: &str = "alfred.workflow.";
    pub const ACTION_UNIVERSAL: &str = "alfred.workflow.action.actioninalfred";
    pub const ACTION_APPLESCRIPT: &str = "alfred.workflow.action.applescript";
    pub const ACTION_BROWSE: &str = "alfred.workflow.action.browseinalfred";
    pub const ACTION_ITUNESCOMMAND: &str = "alfred.workflow.action.itunescommand";
    pub const ACTION_LAUNCHFILES: &str = "alfred.workflow.action.launchfiles";
    pub const ACTION_OPENFILE: &str = "alfred.workflow.action.openfile";
    pub const ACTION_OPENURL: &str = "alfred.workflow.action.openurl";
    pub const ACTION_REVEALFILE: &str = "alfred.workflow.action.revealfile";
    pub const ACTION_SCRIPT: &str = "alfred.workflow.action.script";
    pub const ACTION_SYSTEMCOMMAND: &str = "alfred.workflow.action.systemcommand";
    pub const ACTION_SYSTEMWEBSEARCH: &str = "alfred.workflow.action.systemwebsearch";
    pub const ACTION_TERMINALCOMMAND: &str = "alfred.workflow.action.terminalcommand";
    pub const INPUT_FILEFILTER: &str = "alfred.workflow.input.filefilter";
    pub const INPUT_KEYWORD: &str = "alfred.workflow.input.keyword";
    pub const INPUT_LISTFILTER: &str = "alfred.workflow.input.listfilter";
    pub const INPUT_SCRIPTFILTER: &str = "alfred.workflow.input.scriptfilter";
    pub const OUTPUT_CALLEXTERNALTRIGGER: &str = "alfred.workflow.output.callexternaltrigger";
    pub const OUTPUT_CLIPBOARD: &str = "alfred.workflow.output.clipboard";
    pub const OUTPUT_LARGETYPE: &str = "alfred.workflow.output.largetype";
    pub const OUTPUT_NOTIFICATION: &str = "alfred.workflow.output.notification";
    pub const OUTPUT_SCRIPT: &str = "alfred.workflow.output.script";
    pub const OUTPUT_WRITEFILE: &str = "alfred.workflow.output.writefile";
    pub const TRIGGER_EXTERNAL: &str = "alfred.workflow.trigger.external";
    pub const TRIGGER_HOTKEY: &str = "alfred.workflow.trigger.hotkey";
    pub const UTILITY_ARGUMENT: &str = "alfred.workflow.utility.argument";
    pub const UTILITY_CONDITIONAL: &str = "alfred.workflow.utility.conditional";
    pub const UTILITY_DEBUG: &str = "alfred.workflow.utility.debug";
    pub const UTILITY_DELAY: &str = "alfred.workflow.utility.delay";
    pub const UTILITY_FILTER: &str = "alfred.workflow.utility.filter";
    pub const UTILITY_JOINARGS: &str = "alfred.workflow.utility.joinargs";
    pub const UTILITY_JOINARGUMENTS: &str = "alfred.workflow.utility.joinarguments";
    pub const UTILITY_JUNCTION: &str = "alfred.workflow.utility.junction";
    pub const UTILITY_MERGE: &str = "alfred.workflow.utility.merge";
    pub const UTILITY_REPLACE: &str = "alfred.workflow.utility.replace";
    pub const UTILITY_SPLIT: &str = "alfred.workflow.utility.split";
    pub const UTILITY_SPLITARGUMENT: &str = "alfred.workflow.utility.splitargument";
    pub const UTILITY_TRANSFORM: &str = "alfred.workflow.utility.transform";
}

/// Workflow package file extensions, without the leading dot, lower case.
pub const WORKFLOW_EXT: &str = "alfredworkflow";
pub const WORKFLOW_EXT_3: &str = "alfred3workflow";
pub const WORKFLOW_EXT_4: &str = "alfred4workflow";
pub const WORKFLOW_EXT_5: &str = "alfred5workflow";
/// Every workflow package extension the importer accepts (all of them are zip archives).
pub const WORKFLOW_EXTENSIONS: [&str; 4] = [WORKFLOW_EXT, WORKFLOW_EXT_3, WORKFLOW_EXT_4, WORKFLOW_EXT_5];

/// Appearance (theme) file extension, without the leading dot.
pub const APPEARANCE_EXT: &str = "alfredappearance";
/// Snippet collection file extension, without the leading dot.
pub const SNIPPETS_EXT: &str = "alfredsnippets";

/// Top-level dictionary key of an appearance file.
pub const THEME_KEY: &str = "alfredtheme";
/// Key wrapping one snippet in a snippet collection's JSON files.
pub const SNIPPET_KEY: &str = "alfredsnippet";
/// Key wrapping `arg` and `variables` in the JSON a workflow script prints.
pub const SCRIPT_OUTPUT_KEY: &str = "alfredworkflow";
/// Script Filter configuration key: the launcher, not the script, filters the results.
pub const FILTER_FLAG_KEY: &str = "alfredfiltersresults";

/// Prefix of the script variables the engine owns. Workflows cannot set variables with this prefix.
pub const ENV_PREFIX: &str = "alfred_";
/// Environment variables exported to every script, with the value the engine reports for it.
pub const ENV_VERSION: &str = "alfred_version";
pub const ENV_WORKFLOW_NAME: &str = "alfred_workflow_name";
pub const ENV_WORKFLOW_BUNDLEID: &str = "alfred_workflow_bundleid";
pub const ENV_WORKFLOW_UID: &str = "alfred_workflow_uid";
pub const ENV_WORKFLOW_VERSION: &str = "alfred_workflow_version";
pub const ENV_WORKFLOW_DATA: &str = "alfred_workflow_data";
pub const ENV_WORKFLOW_CACHE: &str = "alfred_workflow_cache";

/// Name of the AppleScript handler that AppleScript objects may define instead of a `run` handler.
pub const SCRIPT_HANDLER: &str = "alfred_script";

/// `stem` plus the workflow package extension: `workflow_file("weather")` is `weather.<ext>`.
pub fn workflow_file(stem: &str) -> String {
    format!("{stem}.{WORKFLOW_EXT}")
}

/// `stem` plus the appearance file extension.
pub fn appearance_file(stem: &str) -> String {
    format!("{stem}.{APPEARANCE_EXT}")
}

/// `stem` plus the snippet collection extension.
pub fn snippets_file(stem: &str) -> String {
    format!("{stem}.{SNIPPETS_EXT}")
}
