//! A machine-readable answer to "what will this workflow be allowed to do?", for the import
//! confirmation and the workflow list (docs/SECURITY.md, section 3).
//!
//! The engine keeps the reference launcher's semantics (a workflow is a program the user chose to run: SEC-03,
//! SEC-13, SEC-14), so it cannot make an untrusted workflow safe. What it can do is describe the
//! workflow precisely enough that the UI can ask first. [`Workflow::trust_summary`] only reads the
//! parsed `info.plist`; it never runs anything and writes nothing. (It looks at the disk in one
//! place: whether a bare `Something.app` application field names an app shipped in the workflow
//! folder, see [`app_is_path`].)

use std::collections::BTreeSet;

use serde::Serialize;

use super::script::interpreter;
use super::{KIND_EXTERNAL_TRIGGER, Node, Workflow};

/// Reported in [`TrustSummary::open_url_schemes`] for URL templates whose scheme is not literal
/// (`{query}://...`, `{var:u}`): the scheme is decided at run time, by data.
pub const SCHEME_DYNAMIC: &str = "{dynamic}";
/// Reported for URL templates that have no `scheme:` prefix at all.
pub const SCHEME_NONE: &str = "(none)";

/// Reported in [`TrustSummary::opens_at_run_time`] for an Open File object whose file field is
/// empty: the executor then opens the incoming argument, which any earlier object (or a Script
/// Filter result) may have set to a path inside the package itself.
pub const INCOMING_ARGUMENT: &str = "(the incoming argument)";

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

/// An application an Open File (`openwith`) or Open URL (`browser`) object hands its target to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LaunchedApp {
    /// The application as written in the workflow (an app name, or a path).
    pub app: String,
    /// A literal path that points into the workflow's own folder (relative, or absolute under
    /// the workflow directory): the workflow ships the code it launches.
    pub shipped: bool,
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
    /// Open File / Launch Files objects whose target is produced at run time: an empty file field
    /// (= the incoming argument, [`INCOMING_ARGUMENT`]) or a path with a `{placeholder}`. The
    /// target can be anything, including an app or script shipped inside the package, so such a
    /// workflow is never passive. Template text as written, sorted and distinct.
    pub opens_at_run_time: Vec<String>,
    /// Applications that Open File (`openwith`) and Open URL (`browser`) launch, sorted and
    /// distinct. The application is started with the target as its document, so a path inside
    /// the package is code the workflow ships and runs.
    pub launched_apps: Vec<LaunchedApp>,
    /// Open File / Launch Files targets that are literal relative paths: the executor resolves
    /// them against the workflow's own folder, so they are files (possibly apps or scripts)
    /// shipped inside the package. As written, sorted and distinct.
    pub opens_shipped_files: Vec<String>,
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
    /// A Script Filter is wired (directly or through other objects) to an object that runs or
    /// opens something: Run Script, Run NSAppleScript, Open URL, Open File, Launch Files, Write
    /// File, Terminal Command, Run Script output or another Script Filter. The items a Script
    /// Filter prints at run time carry `arg`, `quicklookurl` and `variables` that those objects
    /// consume, so what they do is decided by data this summary cannot see.
    pub results_steer_later_steps: bool,
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
            && self.opens_at_run_time.is_empty()
            && self.launched_apps.is_empty()
            && self.opens_shipped_files.is_empty()
            && self.external_triggers == 0
            && self.copies_to_clipboard == 0
            && !self.results_steer_later_steps
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

/// Open File / Launch Files target templates: a target chosen at run time goes to `runtime`,
/// a literal one outside the workflow folder to `files`, a literal relative one (resolved
/// against the workflow folder by the executor) to `shipped`.
fn note_opened(
    template: &str,
    runtime: &mut BTreeSet<String>,
    files: &mut BTreeSet<String>,
    shipped: &mut BTreeSet<String>,
    schemes: &mut BTreeSet<String>,
) {
    let t = template.trim();
    if t.is_empty() {
        runtime.insert(INCOMING_ARGUMENT.into());
    } else if t.contains('{') {
        runtime.insert(t.to_string());
    } else if let Some(scheme) = target_scheme(t) {
        // `https://...`, `mailto:...`: handed to `open` as written, so it opens a URL (a path
        // would be resolved against the workflow folder, a URL is not).
        schemes.insert(scheme.to_ascii_lowercase());
    } else if is_external(t) {
        files.insert(t.to_string());
    } else {
        shipped.insert(t.to_string());
    }
}

/// The `scheme` of a target that starts with `scheme:` before any `/` (`https://x`, `mailto:a@b.c`,
/// `tel:1`). Such a target is a URL, not a path: the executor hands it to `open` unchanged and
/// the summary lists the scheme. A plain relative path (`Documents/y`) has none.
pub(super) fn target_scheme(target: &str) -> Option<&str> {
    let (scheme, _) = target.split_once(':')?;
    let mut chars = scheme.chars();
    (chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
    .then_some(scheme)
}

/// An application field (`openwith`, `browser`) is a path rather than an app name when it has a
/// `/`, or ends in `.app` *and* such a bundle exists in the workflow folder `dir`. A bare
/// `Safari.app` that is not shipped is a name, passed to `open -a` as written; a path is resolved
/// by the executor against the workflow folder. The classifier and the executor both call this.
pub(super) fn app_is_path(app: &str, dir: &std::path::Path) -> bool {
    if app.contains('/') {
        return true;
    }
    app.to_ascii_lowercase().ends_with(".app") && std::fs::symlink_metadata(dir.join(app)).is_ok()
}

/// Record the application field of an Open File / Open URL object.
fn note_app(app: &str, dir: &std::path::Path, apps: &mut BTreeSet<LaunchedApp>) {
    let a = app.trim();
    if a.is_empty() {
        return;
    }
    let literal = !a.contains('{');
    let relative = app_is_path(a, dir) && !a.starts_with('/') && !a.starts_with('~');
    // `~/Library/.../workflows/<this folder>/payload.app` is as shipped as an absolute path; the
    // path is expanded and normalised first so `..` cannot make something else look shipped.
    let expanded = super::exec::lexical_normalize(&crate::files::expand_tilde(a));
    let under_dir = expanded.is_absolute() && expanded.starts_with(super::exec::lexical_normalize(dir));
    apps.insert(LaunchedApp {
        app: a.to_string(),
        shipped: literal && (relative || under_dir),
    });
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

/// Object kinds that run or open something with the data they receive.
const ACTING_KINDS: &[&str] = &[
    "alfred.workflow.action.script",
    "alfred.workflow.output.script",
    "alfred.workflow.action.applescript",
    "alfred.workflow.input.scriptfilter",
    "alfred.workflow.action.openurl",
    "alfred.workflow.action.openfile",
    "alfred.workflow.action.launchfiles",
    "alfred.workflow.output.writefile",
    "alfred.workflow.action.terminalcommand",
];

impl Workflow {
    /// True when some Script Filter reaches (along `connections`, any depth) an object of one of
    /// the [`ACTING_KINDS`].
    fn script_results_reach_actions(&self) -> bool {
        let kind_of = |uid: &str| self.nodes.iter().find(|n| n.uid == uid).map(|n| n.kind.as_str());
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack: Vec<&str> = self
            .nodes
            .iter()
            .filter(|n| n.kind == super::KIND_SCRIPT_FILTER)
            .map(|n| n.uid.as_str())
            .collect();
        while let Some(uid) = stack.pop() {
            for c in self.connections.get(uid).into_iter().flatten() {
                if !seen.insert(c.dest.as_str()) {
                    continue;
                }
                if kind_of(&c.dest).is_some_and(|k| ACTING_KINDS.contains(&k)) {
                    return true;
                }
                stack.push(c.dest.as_str());
            }
        }
        false
    }

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
        let mut runtime = BTreeSet::new();
        let mut apps = BTreeSet::new();
        let mut shipped = BTreeSet::new();
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
                    note_app(&n.s("browser"), &self.dir, &mut apps);
                }
                // "Search the web" opens a fixed https URL, in the `browser` application if set.
                "alfred.workflow.action.systemwebsearch" => {
                    schemes.insert("https".into());
                    note_app(&n.s("browser"), &self.dir, &mut apps);
                }
                "alfred.workflow.action.terminalcommand" => {
                    let t: String = n.s("text").chars().take(MAX_COMMAND_CHARS).collect();
                    s.terminal_commands.push(t);
                }
                "alfred.workflow.action.openfile" => {
                    note_opened(&n.s("sourcefile"), &mut runtime, &mut files, &mut shipped, &mut schemes);
                    note_app(&n.s("openwith"), &self.dir, &mut apps);
                }
                "alfred.workflow.action.launchfiles" => {
                    for p in n.config.get("paths").and_then(|p| p.as_array()).into_iter().flatten() {
                        if let Some(p) = p.as_str() {
                            note_opened(p, &mut runtime, &mut files, &mut shipped, &mut schemes);
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
        s.opens_at_run_time = runtime.into_iter().collect();
        s.launched_apps = apps.into_iter().collect();
        s.opens_shipped_files = shipped.into_iter().collect();
        s.results_steer_later_steps = self.script_results_reach_actions();
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

    fn wf_wired(objects: serde_json::Value, connections: serde_json::Value) -> Workflow {
        let doc = json!({"name": "T", "bundleid": "t.trust", "objects": objects, "connections": connections});
        Workflow::from_json(std::path::Path::new("/tmp/oa-trust"), "t", &doc).unwrap()
    }

    fn wire(to: &str) -> serde_json::Value {
        json!({"destinationuid": to, "modifiers": 0})
    }

    #[test]
    fn script_filter_results_steering_later_steps_are_flagged() {
        let sf =
            json!({"uid": "SF", "type": "alfred.workflow.input.scriptfilter", "config": {"type": 0, "script": "echo"}});
        // each acting kind, directly and through a junction
        for kind in [
            "alfred.workflow.action.script",
            "alfred.workflow.output.script",
            "alfred.workflow.action.applescript",
            "alfred.workflow.action.openurl",
            "alfred.workflow.action.openfile",
            "alfred.workflow.action.launchfiles",
            "alfred.workflow.output.writefile",
            "alfred.workflow.action.terminalcommand",
        ] {
            let direct = wf_wired(
                json!([sf, {"uid": "X", "type": kind, "config": {}}]),
                json!({"SF": [wire("X")]}),
            )
            .trust_summary();
            assert!(direct.results_steer_later_steps, "{kind} direct");
            assert!(!direct.is_passive(), "{kind} direct");
            let via = wf_wired(
                json!([sf, {"uid": "J", "type": "alfred.workflow.utility.junction", "config": {}},
                       {"uid": "X", "type": kind, "config": {}}]),
                json!({"SF": [wire("J")], "J": [wire("X")]}),
            )
            .trust_summary();
            assert!(via.results_steer_later_steps, "{kind} via junction");
        }
    }

    #[test]
    fn script_filter_into_harmless_steps_or_unwired_acting_steps_is_not_flagged() {
        let sf =
            json!({"uid": "SF", "type": "alfred.workflow.input.scriptfilter", "config": {"type": 0, "script": "echo"}});
        let acting = json!({"uid": "A", "type": "alfred.workflow.action.openurl", "config": {"url": "https://x.test"}});
        let note = json!({"uid": "N", "type": "alfred.workflow.output.notification", "config": {}});
        // only a notification / clipboard after it
        let s = wf_wired(json!([sf, note, acting]), json!({"SF": [wire("N")]})).trust_summary();
        assert!(!s.results_steer_later_steps);
        // an acting object that no Script Filter reaches (keyword -> url)
        let kw = json!({"uid": "K", "type": "alfred.workflow.input.keyword", "config": {"keyword": "k"}});
        let s = wf_wired(json!([sf, kw, acting]), json!({"K": [wire("A")]})).trust_summary();
        assert!(!s.results_steer_later_steps);
        // a cycle terminates
        let j1 = json!({"uid": "J1", "type": "alfred.workflow.utility.junction", "config": {}});
        let j2 = json!({"uid": "J2", "type": "alfred.workflow.utility.junction", "config": {}});
        let s = wf_wired(
            json!([sf, j1, j2]),
            json!({"SF": [wire("J1")], "J1": [wire("J2")], "J2": [wire("J1")]}),
        )
        .trust_summary();
        assert!(!s.results_steer_later_steps);
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
        // O3 (empty field) opens whatever arrives as the argument: reported, not skipped.
        assert_eq!(s.opens_at_run_time, [INCOMING_ARGUMENT]);
        // O2 and the relative Launch Files path are shipped with the workflow.
        assert_eq!(s.opens_shipped_files, ["inside/x", "local/readme.md"]);
    }

    #[test]
    fn open_file_with_an_empty_or_dynamic_field_is_never_passive() {
        // The three-object graph from the re-review (N1): keyword -> Arg and Vars sets the
        // argument to a file shipped in the package -> Open File with an empty field. It runs
        // no script of its own and names no external file, yet it launches shipped code.
        let s = wf(json!([
            {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {"keyword": "go", "argumenttype": 2}},
            {"uid": "A", "type": "alfred.workflow.utility.argument",
             "config": {"argument": "~/Library/Application Support/reference-launchero/workflows/user.workflow.t.trust/payload.app"}},
            {"uid": "O", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": ""}},
        ]))
        .trust_summary();
        assert_eq!(s.scripts, 0);
        assert!(!s.runs_code(), "nothing here is a script or a Terminal command ...");
        assert!(!s.is_passive(), "... but it opens a file chosen at run time");
        assert_eq!(s.opens_at_run_time, [INCOMING_ARGUMENT]);

        // Placeholders are decided by data, in Open File and in Launch Files.
        let s = wf(json!([
            {"uid": "O", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "{query}"}},
            {"uid": "P", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "{var:app}/Contents"}},
            {"uid": "L", "type": "alfred.workflow.action.launchfiles", "config": {"paths": ["{var:p}", "/Applications/Safari.app"]}},
        ]))
        .trust_summary();
        assert!(!s.is_passive());
        assert_eq!(s.opens_at_run_time, ["{query}", "{var:app}/Contents", "{var:p}"]);
        assert_eq!(s.external_files, ["/Applications/Safari.app"]);
    }

    #[test]
    fn open_file_openwith_and_open_url_browser_are_launched_applications() {
        // Review item 1: Keyword -> Open File ("."), `openwith` an app inside the package. No
        // script, no external file, no URL: it still launches shipped code.
        let (w, dir) = wf_in(
            "launched",
            json!([
                {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {"keyword": "go"}},
                {"uid": "O", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": ".", "openwith": "payload.app"}},
            ]),
        );
        let s = w.trust_summary();
        let _ = std::fs::remove_dir_all(dir);
        assert!(!s.runs_code());
        assert!(!s.is_passive(), "{s:?}");
        assert_eq!(
            s.launched_apps,
            [LaunchedApp {
                app: "payload.app".into(),
                shipped: true
            }]
        );

        // The same with the absolute installed path from the review.
        let s = wf(json!([
            {"uid": "O", "type": "alfred.workflow.action.openfile",
             "config": {"sourcefile": "Applications",
                        "openwith": "~/Library/Application Support/reference-launchero/workflows/user.workflow.t.trust/payload.app"}},
        ]))
        .trust_summary();
        assert!(!s.is_passive(), "{s:?}");
        assert_eq!(s.launched_apps.len(), 1);

        // Open URL `browser`: named apps and system apps are listed but not "shipped".
        let s = wf(json!([
            {"uid": "U", "type": "alfred.workflow.action.openurl",
             "config": {"url": "https://x.test", "browser": "Safari"}},
            {"uid": "V", "type": "alfred.workflow.action.openurl",
             "config": {"url": "https://x.test", "browser": "./bin/Browser.app"}},
            {"uid": "W", "type": "alfred.workflow.action.openurl",
             "config": {"url": "https://x.test", "browser": ""}},
        ]))
        .trust_summary();
        assert_eq!(
            s.launched_apps,
            [
                LaunchedApp {
                    app: "./bin/Browser.app".into(),
                    shipped: true
                },
                LaunchedApp {
                    app: "Safari".into(),
                    shipped: false
                },
            ]
        );
        assert!(s.launched_apps.iter().any(|a| a.shipped));
    }

    #[test]
    fn a_relative_literal_file_is_a_file_shipped_with_the_workflow() {
        // `open` resolves a relative path against the launcher's working directory; the
        // executor resolves it against the workflow folder, so the summary calls it shipped.
        let s = wf(json!([
            {"uid": "O", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "payload.app"}},
            {"uid": "L", "type": "alfred.workflow.action.launchfiles", "config": {"paths": ["tools/run.command"]}},
        ]))
        .trust_summary();
        assert!(!s.is_passive());
        assert_eq!(s.opens_shipped_files, ["payload.app", "tools/run.command"]);
        assert!(s.external_files.is_empty() && s.opens_at_run_time.is_empty());
    }

    /// A workflow whose folder really exists (`payload.app` is a bundle inside it).
    fn wf_in(tag: &str, objects: serde_json::Value) -> (Workflow, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("oa-trust-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("payload.app/Contents")).unwrap();
        let doc = json!({"name": "T", "bundleid": "t.trust", "objects": objects, "connections": {}});
        (Workflow::from_json(&dir, "t", &doc).unwrap(), dir)
    }

    #[test]
    fn a_bare_app_name_ending_in_dot_app_is_a_name_unless_the_bundle_is_in_the_package() {
        let (w, dir) = wf_in(
            "apps",
            json!([
                {"uid": "A", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": ".", "openwith": "payload.app"}},
                {"uid": "B", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": ".", "openwith": "Safari.app"}},
                {"uid": "C", "type": "alfred.workflow.action.openurl",
                 "config": {"url": "https://x.test", "browser": "com.google.Chrome"}},
                {"uid": "D", "type": "alfred.workflow.action.openurl",
                 "config": {"url": "https://x.test", "browser": "sub/Tool.app"}},
            ]),
        );
        let s = w.trust_summary();
        let shipped = |name: &str| s.launched_apps.iter().find(|a| a.app == name).map(|a| a.shipped);
        assert_eq!(shipped("payload.app"), Some(true), "{s:?}");
        assert_eq!(
            shipped("Safari.app"),
            Some(false),
            "an app name, not <workflow>/Safari.app"
        );
        assert_eq!(shipped("com.google.Chrome"), Some(false));
        assert_eq!(
            shipped("sub/Tool.app"),
            Some(true),
            "a relative path is the workflow's own"
        );
        assert!(!s.is_passive());
        // the executor reads the same fields the same way (see exec.rs tests)
        assert!(app_is_path("payload.app", &dir));
        assert!(!app_is_path("Safari.app", &dir));
        assert!(!app_is_path("Safari", &dir));
        assert!(app_is_path("sub/Tool.app", &dir));
        assert!(app_is_path("/Applications/Safari.app", &dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn urls_given_to_open_file_are_urls_not_files_shipped_with_the_workflow() {
        let s = wf(json!([
            {"uid": "A", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "https://example.com/x"}},
            {"uid": "B", "type": "alfred.workflow.action.openfile", "config": {"sourcefile": "mailto:a@b.c"}},
            {"uid": "C", "type": "alfred.workflow.action.launchfiles", "config": {"paths": ["tel:12345", "Documents/y"]}},
        ]))
        .trust_summary();
        assert_eq!(s.open_url_schemes, ["https", "mailto", "tel"]);
        assert_eq!(s.opens_shipped_files, ["Documents/y"]);
        assert!(s.external_files.is_empty());
        assert!(!s.is_passive());
        assert_eq!(target_scheme("https://example.com/x"), Some("https"));
        assert_eq!(target_scheme("mailto:a@b.c"), Some("mailto"));
        assert_eq!(target_scheme("Documents/y"), None);
        assert_eq!(target_scheme("dir/a:b"), None, "a colon after a slash is a file name");
        assert_eq!(target_scheme(":x"), None);
        assert_eq!(target_scheme("1a:x"), None);
    }

    #[test]
    fn a_tilde_path_into_the_workflows_own_folder_is_shipped() {
        let home = dirs::home_dir().unwrap();
        let dir = home.join("Library/Application Support/ashdash/workflows/user.workflow.t.own");
        let doc = |app: &str| {
            json!({"name": "T", "bundleid": "t.own", "connections": {}, "objects": [
                {"uid": "O", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": "Applications", "openwith": app}}]})
        };
        let launched = |app: &str| {
            Workflow::from_json(&dir, "t", &doc(app))
                .unwrap()
                .trust_summary()
                .launched_apps
        };
        let own = "~/Library/Application Support/ashdash/workflows/user.workflow.t.own/payload.app";
        assert_eq!(
            launched(own),
            [LaunchedApp {
                app: own.into(),
                shipped: true
            }]
        );
        // the same folder spelled out
        let abs = format!("{}/payload.app", dir.display());
        assert!(launched(&abs)[0].shipped);
        // another workflow's folder, `..` back out of this one, or the home folder are not
        for other in [
            "~/Library/Application Support/ashdash/workflows/user.workflow.t.other/payload.app",
            "~/Library/Application Support/ashdash/workflows/user.workflow.t.own/../user.workflow.t.other/payload.app",
            "~/payload.app",
        ] {
            assert!(!launched(other)[0].shipped, "{other}");
        }
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
        assert!(!with(|t| t.opens_at_run_time.push(INCOMING_ARGUMENT.into())));
        assert!(!with(|t| t.launched_apps.push(LaunchedApp {
            app: "Safari".into(),
            shipped: false
        })));
        assert!(!with(|t| t.opens_shipped_files.push("a.app".into())));
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
