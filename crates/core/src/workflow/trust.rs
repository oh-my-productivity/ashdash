//! A machine-readable answer to "what will this workflow be allowed to do?", for the import
//! confirmation and the workflow list (docs/SECURITY.md, section 3).
//!
//! The engine keeps the reference launcher's semantics (a workflow is a program the user chose to run: SEC-03,
//! SEC-13, SEC-14), so it cannot make an untrusted workflow safe. What it can do is describe the
//! workflow precisely enough that the UI can ask first. [`Workflow::trust_summary`] only reads the
//! parsed `info.plist`; it never runs anything and never touches the disk.

use std::collections::BTreeSet;

use serde::Serialize;

use super::script::interpreter;
use super::{KIND_EXTERNAL_TRIGGER, Node, Workflow};

/// Reported in [`TrustSummary::open_url_schemes`] for URL templates whose scheme is not literal
/// (`{query}://...`, `{var:u}`): the scheme is decided at run time, by data.
pub const SCHEME_DYNAMIC: &str = "{dynamic}";
/// Reported for URL templates that have no `scheme:` prefix at all.
pub const SCHEME_NONE: &str = "(none)";

/// Longest terminal command template kept verbatim in the summary (characters).
const MAX_COMMAND_CHARS: usize = 500;
/// Longest Write File path template kept verbatim in the summary (characters).
const MAX_PATH_CHARS: usize = 500;

/// Where a Write File object's relative paths land (`relativepathmode`), or `Anywhere` when the
/// path is not confined to one of the workflow's own folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePlace {
    /// Relative path, resolved inside the workflow's data folder.
    WorkflowData,
    /// Relative path, resolved inside the workflow's cache folder.
    WorkflowCache,
    /// Relative path, resolved inside the workflow's own (installed) folder.
    WorkflowFolder,
    /// An absolute or `~` path, a path with `..`, or one with a `{placeholder}` (decided by data
    /// at run time and able to bring in `/` or `..`): it can name any file the user can write.
    Anywhere,
}

/// How the file is combined with what is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    Overwrite,
    Append,
    Prepend,
}

/// One Write File object (`alfred.workflow.output.writefile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileWriteInfo {
    /// The file name template as written in the workflow (cut at 500 characters).
    pub path: String,
    pub place: WritePlace,
    pub kind: WriteKind,
    /// The path contains a `{placeholder}`: part of it is decided at run time.
    pub dynamic: bool,
    /// The object may create missing folders on the way.
    pub creates_folders: bool,
}

impl FileWriteInfo {
    /// True when the target is not confined to the workflow's own folders.
    pub fn is_outside(&self) -> bool {
        self.place == WritePlace::Anywhere
    }

    /// A short phrase: `appends to ~/.zshenv`, `overwrites notes.txt (its data folder)`.
    pub fn describe(&self) -> String {
        let verb = match self.kind {
            WriteKind::Overwrite => "overwrites",
            WriteKind::Append => "appends to",
            WriteKind::Prepend => "prepends to",
        };
        let place = match self.place {
            WritePlace::WorkflowData => " (its data folder)",
            WritePlace::WorkflowCache => " (its cache folder)",
            WritePlace::WorkflowFolder => " (its own folder)",
            WritePlace::Anywhere => "",
        };
        let dynamic = if self.dynamic { " (decided at run time)" } else { "" };
        format!("{verb} {}{place}{dynamic}", self.path)
    }
}

/// What a workflow can do on the user's machine, as declared in its object graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TrustSummary {
    pub name: String,
    pub bundle_id: String,
    /// Objects that run code: Run Script, Script Filter, Run Script output and Run NSAppleScript.
    pub scripts: usize,
    /// Distinct interpreters used by those objects, sorted (`bash`, `python3`, `osascript`,
    /// `osascript (JavaScript)`, or `script file` for a referenced executable).
    pub interpreters: Vec<String>,
    /// Scripts that paste `{query}` into their source ("with input as {query}", SEC-03): the
    /// mode in which user/third-party text becomes code unless the script quotes it carefully.
    pub query_in_source: usize,
    /// Distinct URL schemes the Open URL objects use, lower-case and sorted; see
    /// [`SCHEME_DYNAMIC`] and [`SCHEME_NONE`]. URLs coming from Script Filter results at run time
    /// are not knowable here.
    pub open_url_schemes: Vec<String>,
    /// Terminal Command templates, in graph order (cut at 500 characters).
    pub terminal_commands: Vec<String>,
    /// Files outside the workflow directory that objects refer to (absolute, `~` or `..` paths in
    /// script files, Open File, Launch Files and the like), sorted and distinct.
    pub external_files: Vec<String>,
    /// External Trigger objects (what other workflows or a URL could start).
    pub external_triggers: usize,
    /// Write File objects, in graph order: which files the workflow can create, overwrite,
    /// append to or prepend to. A Write File object is a persistence primitive (a shell start-up
    /// file, a LaunchAgent plist), so it is reported like a script even though it runs nothing.
    pub file_writes: Vec<FileWriteInfo>,
    /// Number of Copy to Clipboard objects: the workflow can replace what the user copied (for
    /// example with a command that is pasted into a terminal later).
    ///
    /// Reveal in Finder (`action.revealfile`) is deliberately *not* listed: it only selects a
    /// file in a Finder window and neither opens, runs nor changes anything.
    pub copies_to_clipboard: usize,
}

impl TrustSummary {
    /// True when the workflow can execute code or commands at all (everything but pure
    /// notification/clipboard/URL workflows).
    pub fn runs_code(&self) -> bool {
        self.scripts > 0 || !self.terminal_commands.is_empty()
    }

    /// True when the workflow can write files at all.
    pub fn writes_files(&self) -> bool {
        !self.file_writes.is_empty()
    }

    /// True when the workflow can do none of the things this summary reports: it runs nothing,
    /// writes nothing, opens no URLs or files, has no triggers and does not touch the clipboard.
    /// Only then may a UI say that it is harmless.
    pub fn is_passive(&self) -> bool {
        !self.runs_code()
            && self.file_writes.is_empty()
            && self.open_url_schemes.is_empty()
            && self.external_files.is_empty()
            && self.external_triggers == 0
            && self.copies_to_clipboard == 0
    }
}

/// `scheme:` prefix of a URL template, lower-cased.
fn url_scheme(template: &str) -> String {
    let t = template.trim();
    if t.starts_with('{') {
        return SCHEME_DYNAMIC.into();
    }
    match t.split_once(':') {
        Some((s, _))
            if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) =>
        {
            s.to_ascii_lowercase()
        }
        _ => SCHEME_NONE.into(),
    }
}

/// A path template that leaves the workflow directory.
fn is_external(path: &str) -> bool {
    let p = path.trim();
    !p.is_empty()
        && (p.starts_with('/')
            || p.starts_with('~')
            || p.starts_with('{') // decided at run time (e.g. {query}): not inside the workflow
            || std::path::Path::new(p)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)))
}

/// Classify a Write File object the way the executor will resolve it (`exec.rs`).
fn file_write(n: &Node) -> Option<FileWriteInfo> {
    let raw = n.s("filename");
    let t = raw.trim();
    if t.is_empty() {
        return None; // the executor stops with a warning; nothing is written
    }
    let dynamic = t.contains('{');
    // A placeholder anywhere in the path can bring in `/` or `..` (a typed query, a variable), so
    // a dynamic path cannot be shown to stay inside the workflow's folders.
    let anywhere = dynamic
        || t.starts_with('/')
        || t.starts_with('~')
        || std::path::Path::new(t)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    let place = if anywhere {
        WritePlace::Anywhere
    } else {
        match n.i("relativepathmode") {
            1 => WritePlace::WorkflowCache,
            2 => WritePlace::WorkflowFolder,
            _ => WritePlace::WorkflowData,
        }
    };
    let kind = match n.i("type") {
        1 => WriteKind::Append,
        2 => WriteKind::Prepend,
        _ => WriteKind::Overwrite,
    };
    Some(FileWriteInfo {
        path: t.chars().take(MAX_PATH_CHARS).collect(),
        place,
        kind,
        dynamic,
        creates_folders: n.b("createintermediatefolders"),
    })
}

fn interpreter_label(n: &Node) -> String {
    let script_blank = n.s("script").trim().is_empty();
    if script_blank && !n.s("scriptfile").is_empty() {
        return "script file".into();
    }
    match interpreter(n.i("type")) {
        Some(i) => {
            let base = i.program.rsplit('/').next().unwrap_or(i.program);
            if n.i("type") == 7 {
                format!("{base} (JavaScript)")
            } else {
                base.to_string()
            }
        }
        None => format!("unknown script type {}", n.i("type")),
    }
}

impl Workflow {
    /// Describe what this workflow can do. See [`TrustSummary`].
    pub fn trust_summary(&self) -> TrustSummary {
        let mut s = TrustSummary {
            name: self.name.clone(),
            bundle_id: self.bundle_id.clone(),
            ..Default::default()
        };
        let mut interpreters = BTreeSet::new();
        let mut schemes = BTreeSet::new();
        let mut files = BTreeSet::new();
        for n in &self.nodes {
            match n.kind.as_str() {
                "alfred.workflow.action.script"
                | "alfred.workflow.output.script"
                | "alfred.workflow.input.scriptfilter" => {
                    s.scripts += 1;
                    interpreters.insert(interpreter_label(n));
                    let file = n.s("scriptfile");
                    if n.s("script").trim().is_empty() {
                        if is_external(&file) {
                            files.insert(file);
                        }
                    } else if n.i("scriptargtype") == 0 && n.s("script").contains("{query}") {
                        s.query_in_source += 1;
                    }
                }
                "alfred.workflow.action.applescript" => {
                    s.scripts += 1;
                    interpreters.insert("osascript".into());
                    if n.s("applescript").contains("{query}") {
                        s.query_in_source += 1;
                    }
                }
                "alfred.workflow.action.openurl" => {
                    schemes.insert(url_scheme(&n.s("url")));
                }
                "alfred.workflow.action.terminalcommand" => {
                    let t: String = n.s("text").chars().take(MAX_COMMAND_CHARS).collect();
                    s.terminal_commands.push(t);
                }
                "alfred.workflow.action.openfile" => {
                    let f = n.s("sourcefile");
                    if is_external(&f) {
                        files.insert(f);
                    }
                }
                "alfred.workflow.action.launchfiles" => {
                    for p in n.config.get("paths").and_then(|p| p.as_array()).into_iter().flatten() {
                        if let Some(p) = p.as_str().filter(|p| is_external(p)) {
                            files.insert(p.to_string());
                        }
                    }
                }
                "alfred.workflow.output.writefile" => {
                    s.file_writes.extend(file_write(n));
                }
                "alfred.workflow.output.clipboard" => s.copies_to_clipboard += 1,
                KIND_EXTERNAL_TRIGGER => s.external_triggers += 1,
                _ => {}
            }
        }
        s.interpreters = interpreters.into_iter().collect();
        s.open_url_schemes = schemes.into_iter().collect();
        s.external_files = files.into_iter().collect();
        s
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn wf(objects: serde_json::Value) -> Workflow {
        let doc = json!({"name": "T", "bundleid": "t.trust", "objects": objects, "connections": {}});
        Workflow::from_json(std::path::Path::new("/tmp/oa-trust"), "t", &doc).unwrap()
    }

    #[test]
    fn empty_and_passive_workflows_run_no_code() {
        let s = wf(json!([])).trust_summary();
        assert_eq!(
            s,
            TrustSummary {
                name: "T".into(),
                bundle_id: "t.trust".into(),
                ..Default::default()
            }
        );
        assert!(!s.runs_code());
        let s = wf(json!([
            {"uid": "N", "type": "alfred.workflow.output.notification", "config": {"title": "a", "text": "b"}},
            {"uid": "C", "type": "alfred.workflow.output.clipboard", "config": {}},
        ]))
        .trust_summary();
        assert!(!s.runs_code() && s.scripts == 0 && s.open_url_schemes.is_empty());
    }

    #[test]
    fn scripts_interpreters_and_query_substitution_are_counted() {
        let s = wf(json!([
            {"uid": "A", "type": "alfred.workflow.action.script", "config": {"type": 0, "scriptargtype": 1, "script": "echo \"$1\""}},
            {"uid": "B", "type": "alfred.workflow.action.script", "config": {"type": 0, "scriptargtype": 0, "script": "echo {query}"}},
            {"uid": "C", "type": "alfred.workflow.input.scriptfilter", "config": {"type": 3, "scriptargtype": 0, "script": "print('{query}')"}},
            {"uid": "D", "type": "alfred.workflow.output.script", "config": {"type": 7, "script": "1"}},
            {"uid": "E", "type": "alfred.workflow.action.applescript", "config": {"applescript": "display dialog \"{query}\""}},
            {"uid": "F", "type": "alfred.workflow.action.script", "config": {"type": 0, "script": "", "scriptfile": "run.sh"}},
            {"uid": "G", "type": "alfred.workflow.action.script", "config": {"type": 99, "script": "x"}},
        ]))
        .trust_summary();
        assert_eq!(s.scripts, 7);
        assert_eq!(s.query_in_source, 3); // B, C, E
        assert_eq!(
            s.interpreters,
            [
                "bash",
                "osascript",
                "osascript (JavaScript)",
                "python3",
                "script file",
                "unknown script type 99"
            ]
        );
        assert!(s.runs_code());
        assert!(s.external_files.is_empty(), "run.sh lives inside the workflow");
    }

    #[test]
    fn open_url_schemes_are_collected_lowercase_and_distinct() {
        let s = wf(json!([
            {"uid": "A", "type": "alfred.workflow.action.openurl", "config": {"url": "https://x.test/?q={query}"}},
            {"uid": "B", "type": "alfred.workflow.action.openurl", "config": {"url": "HTTPS://y.test"}},
            {"uid": "C", "type": "alfred.workflow.action.openurl", "config": {"url": "x-apple.systempreferences:com.apple.x"}},
            {"uid": "D", "type": "alfred.workflow.action.openurl", "config": {"url": "{query}"}},
            {"uid": "E", "type": "alfred.workflow.action.openurl", "config": {"url": "example.com/path"}},
            {"uid": "F", "type": "alfred.workflow.action.openurl", "config": {"url": "file:///Applications"}},
            {"uid": "G", "type": "alfred.workflow.action.openurl", "config": {"url": "1bad://x"}},
        ]))
        .trust_summary();
        assert_eq!(
            s.open_url_schemes,
            ["(none)", "file", "https", "x-apple.systempreferences", "{dynamic}"]
        );
        assert!(!s.runs_code());
    }

    #[test]
    fn terminal_commands_and_external_files_are_listed() {
        let long = "x".repeat(900);
        let s = wf(json!([
            {"uid": "T1", "type": "alfred.workflow.action.terminalcommand", "config": {"text": "brew update && brew upgrade"}},
            {"uid": "T2", "type": "alfred.workflow.action.terminalcommand", "config": {"text": long}},
            {"uid": "O1", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "~/Documents/a.txt"}},
            {"uid": "O2", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "local/readme.md"}},
            {"uid": "O3", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": ""}},
            {"uid": "L1", "type": "alfred.workflow.action.launchfiles", "config": {"paths": ["/Applications/Safari.app", "inside/x", "../up/y"]}},
            {"uid": "S1", "type": "alfred.workflow.action.script", "config": {"type": 0, "script": "", "scriptfile": "/usr/local/bin/tool"}},
            {"uid": "S2", "type": "alfred.workflow.action.script", "config": {"type": 0, "script": " ", "scriptfile": "../shared/x.sh"}},
            {"uid": "X", "type": "alfred.workflow.trigger.external", "config": {"triggerid": "go"}},
        ]))
        .trust_summary();
        assert_eq!(s.terminal_commands.len(), 2);
        assert_eq!(s.terminal_commands[0], "brew update && brew upgrade");
        assert_eq!(s.terminal_commands[1].chars().count(), 500);
        assert_eq!(
            s.external_files,
            [
                "../shared/x.sh",
                "../up/y",
                "/Applications/Safari.app",
                "/usr/local/bin/tool",
                "~/Documents/a.txt"
            ]
        );
        assert_eq!(s.external_triggers, 1);
        assert!(s.runs_code());
    }

    #[test]
    fn write_file_is_reported_and_is_never_passive() {
        let s = wf(json!([
            {"uid": "W", "type": "alfred.workflow.output.writefile", "config": {"filename": "~/.zshenv", "type": 1, "filetext": "x"}},
        ]))
        .trust_summary();
        assert!(!s.runs_code(), "it runs no code ...");
        assert!(s.writes_files(), "... but it writes files");
        assert!(!s.is_passive());
        assert_eq!(s.file_writes.len(), 1);
        let w = &s.file_writes[0];
        assert_eq!(
            (w.path.as_str(), w.place, w.kind, w.dynamic),
            ("~/.zshenv", WritePlace::Anywhere, WriteKind::Append, false)
        );
        assert!(w.is_outside());
        assert_eq!(w.describe(), "appends to ~/.zshenv");
    }

    #[test]
    fn write_file_targets_are_classified_like_the_executor_resolves_them() {
        let one = |cfg: serde_json::Value| {
            let s =
                wf(json!([{"uid": "W", "type": "alfred.workflow.output.writefile", "config": cfg}])).trust_summary();
            s.file_writes.into_iter().next()
        };
        // relative paths land in the workflow's own folders
        let d = one(json!({"filename": "notes.txt", "relativepathmode": 0})).unwrap();
        assert_eq!((d.place, d.kind), (WritePlace::WorkflowData, WriteKind::Overwrite));
        assert_eq!(d.describe(), "overwrites notes.txt (its data folder)");
        assert_eq!(
            one(json!({"filename": "a.txt", "relativepathmode": 1})).unwrap().place,
            WritePlace::WorkflowCache
        );
        let f = one(json!({"filename": "run.sh", "relativepathmode": 2, "type": 2, "createintermediatefolders": true}))
            .unwrap();
        assert_eq!(
            (f.place, f.kind, f.creates_folders),
            (WritePlace::WorkflowFolder, WriteKind::Prepend, true)
        );
        // ... unless they climb out, are absolute, or are decided at run time
        for (name, dynamic) in [
            ("../../.zshrc", false),
            ("/Library/LaunchAgents/x.plist", false),
            ("~/Library/LaunchAgents/x.plist", false),
            ("{query}", true),
            ("sub/{query}.txt", true),
            ("{var:dir}/x", true),
        ] {
            let w = one(json!({ "filename": name })).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(w.place, WritePlace::Anywhere, "{name}");
            assert_eq!(w.dynamic, dynamic, "{name}");
            assert!(w.is_outside());
        }
        assert!(
            one(json!({"filename": "sub/{query}.txt"}))
                .unwrap()
                .describe()
                .contains("decided at run time")
        );
        // no file name: the executor stops, nothing is written
        assert!(one(json!({"filename": "  "})).is_none());
        // long templates are cut
        assert_eq!(
            one(json!({"filename": "x".repeat(900)})).unwrap().path.chars().count(),
            500
        );
    }

    #[test]
    fn clipboard_is_listed_and_reveal_in_finder_is_a_documented_non_capability() {
        let s = wf(json!([
            {"uid": "C", "type": "alfred.workflow.output.clipboard", "config": {}},
            {"uid": "C2", "type": "alfred.workflow.output.clipboard", "config": {}},
        ]))
        .trust_summary();
        assert_eq!(s.copies_to_clipboard, 2);
        assert!(!s.is_passive());
        let s = wf(json!([
            {"uid": "R", "type": "alfred.workflow.action.revealfile", "config": {}},
            {"uid": "N", "type": "alfred.workflow.output.notification", "config": {"title": "a", "text": "b"}},
            {"uid": "L", "type": "alfred.workflow.output.largetype", "config": {}},
        ]))
        .trust_summary();
        assert!(s.is_passive(), "reveal, notification and Large Type change nothing");
    }

    #[test]
    fn passive_means_none_of_the_reported_capabilities() {
        assert!(TrustSummary::default().is_passive());
        let with = |f: fn(&mut TrustSummary)| {
            let mut t = TrustSummary::default();
            f(&mut t);
            t.is_passive()
        };
        assert!(!with(|t| t.scripts = 1));
        assert!(!with(|t| t.terminal_commands.push("ls".into())));
        assert!(!with(|t| t.open_url_schemes.push("https".into())));
        assert!(!with(|t| t.external_files.push("~/x".into())));
        assert!(!with(|t| t.external_triggers = 1));
        assert!(!with(|t| t.copies_to_clipboard = 1));
        assert!(!with(|t| t.file_writes.push(FileWriteInfo {
            path: "a".into(),
            place: WritePlace::WorkflowData,
            kind: WriteKind::Overwrite,
            dynamic: false,
            creates_folders: false
        })));
    }

    #[test]
    fn serializes_for_the_ui() {
        let v = serde_json::to_value(wf(json!([])).trust_summary()).unwrap();
        for k in [
            "scripts",
            "open_url_schemes",
            "terminal_commands",
            "external_files",
            "file_writes",
            "copies_to_clipboard",
        ] {
            assert!(v.get(k).is_some(), "{k} missing from {v}");
        }
    }
}
