//! Executing a workflow graph when a result is actioned, plus `{query}` /
//! `{var:name}` template rendering.
//!
//! Execution model (confirmed by the reference launcher's "Stream Order" template): the
//! outgoing wires of an object run top to bottom by destination Y position,
//! each stream depth-first to completion; a Delay yields, so its continuation
//! runs after the other streams. Side effects are not performed here: they are
//! returned as [`Effect`]s and the caller performs them. Scripts are the one
//! exception, they run in-line because their output feeds the next object.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

use super::model::{str_map, stringify};
use super::script::{ScriptOutput, escape_query, run_applescript, run_node_script};
use super::utility::{self, MATCH_REGEX, ReplaceOpts};
use super::{MAX_GRAPH_STEPS, SCRIPT_ACTION_TIMEOUT};
use super::{Node, Workflow, WorkflowAction};
use crate::config::Paths;
use crate::files::expand_tilde;
use crate::item::Mods;
use crate::websearch::percent_encode;

/// How a file written by Write File is combined with existing content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Overwrite,
    Append,
    Prepend,
}

/// A Write File object's job, resolved and ready to perform.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteFile {
    pub path: PathBuf,
    pub contents: String,
    pub mode: WriteMode,
    pub create_dirs: bool,
    pub allow_empty: bool,
}

impl WriteFile {
    pub fn perform(&self) -> Result<()> {
        if self.contents.is_empty() && !self.allow_empty {
            return Ok(());
        }
        if self.create_dirs
            && let Some(dir) = self.path.parent()
        {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let ctx = || format!("writing {}", self.path.display());
        match self.mode {
            WriteMode::Overwrite => std::fs::write(&self.path, &self.contents).with_context(ctx),
            WriteMode::Append => {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .with_context(ctx)?;
                f.write_all(self.contents.as_bytes()).with_context(ctx)
            }
            WriteMode::Prepend => {
                let old = std::fs::read_to_string(&self.path).unwrap_or_default();
                std::fs::write(&self.path, format!("{}{old}", self.contents)).with_context(ctx)
            }
        }
    }
}

/// The rest of a workflow stream, parked by a Delay object. It owns everything
/// needed to continue, so the caller can run it later on any thread.
#[derive(Debug, Clone)]
pub struct Resume {
    wf: Workflow,
    paths: Paths,
    from_uid: String,
    st: State,
    args: Option<Vec<String>>,
    budget: usize,
}

impl PartialEq for Resume {
    fn eq(&self, o: &Self) -> bool {
        self.wf.id == o.wf.id
            && self.from_uid == o.from_uid
            && self.st.arg == o.st.arg
            && self.st.vars == o.st.vars
            && self.args == o.args
    }
}

impl Resume {
    /// Run the parked stream now and return its effects.
    pub fn run(self) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut budget = self.budget;
        let tasks = self
            .wf
            .next_tasks(&self.from_uid, 0, &Branch::All, &self.st, &self.args);
        self.wf.run_stream(tasks, &self.paths, &mut fx, &mut budget);
        fx
    }
}

/// Side effects requested by a workflow run. The caller performs them.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    OpenUrl {
        url: String,
        browser: Option<String>,
    },
    OpenPath {
        path: PathBuf,
        with: Option<String>,
    },
    Reveal(PathBuf),
    Copy(String),
    Notify {
        title: String,
        text: String,
    },
    LargeType(String),
    Terminal(String),
    Warn(String),
    /// Continue a stream after `seconds` (the Delay utility). Performing it
    /// must not block: schedule `resume.run()` and apply its effects later,
    /// see [`spawn_delayed`].
    Delay {
        seconds: f64,
        resume: Box<Resume>,
    },
    /// Write File output.
    WriteFile(WriteFile),
    /// Run an AppleScript snippet (Music/iTunes commands); fire and forget.
    AppleScript(String),
    /// Show the launcher with this query (Browse in the reference launcher, Universal
    /// Actions). Needs UI integration the core cannot do alone.
    ShowLauncher {
        query: String,
    },
    /// Debug utility output. See [`debug_log`].
    Debug(String),
}

/// Sleep `seconds` on a background thread, then run the parked stream and hand
/// its effects to `apply`. Never blocks the caller.
pub fn spawn_delayed(seconds: f64, resume: Resume, apply: fn(Vec<Effect>) -> String) {
    let d = std::time::Duration::from_secs_f64(seconds.clamp(0.0, 24.0 * 3600.0));
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let _ = apply(resume.run());
    });
}

/// Print Debug utility output to stderr when `ASHDASH_WORKFLOW_DEBUG` is set.
pub fn debug_log(text: &str) {
    if std::env::var_os("ASHDASH_WORKFLOW_DEBUG").is_some() {
        eprintln!("workflow debug: {text}");
    }
}

// ---------------------------------------------------------------- templating

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{(query|allvars|var:[^}]+)\}").expect("valid regex"))
}

/// Substitute `{query}`, `{var:name}` and `{allvars}` (all variables as a JSON
/// object) in a single pass (substituted text is never re-expanded).
pub fn render(template: &str, query: &str, vars: &BTreeMap<String, String>) -> String {
    template_re()
        .replace_all(template, |c: &regex::Captures| {
            let key = &c[1];
            if key == "query" {
                query.to_string()
            } else if key == "allvars" {
                serde_json::to_string(vars).unwrap_or_default()
            } else {
                vars.get(&key[4..]).cloned().unwrap_or_default()
            }
        })
        .into_owned()
}

#[derive(Debug, Clone)]
pub(super) struct State {
    arg: String,
    vars: BTreeMap<String, String>,
}

/// Which outgoing wires of an object to follow.
pub(super) enum Branch {
    All,
    /// A Conditional output (the `uid` of the matched condition).
    Output(String),
    /// A Conditional's else output (wires without a `sourceoutputuid`).
    Else,
}

/// What to do after an object ran.
enum Flow {
    Next,
    Stop,
    Branch(Branch),
    Delay(f64),
    /// Continue below another object (a called External Trigger) with a new state.
    Jump(String, State),
}

/// One object about to run, with the stream state it receives. Execution is
/// an explicit work stack rather than recursion so that deep or cyclic graphs
/// cannot overflow the thread's stack.
struct Task<'a> {
    node: &'a Node,
    st: State,
    args: Option<Vec<String>>,
}

const SPLIT_KINDS: [&str; 2] = ["alfred.workflow.utility.split", "alfred.workflow.utility.splitargument"];
const MERGE_KINDS: [&str; 3] = [
    "alfred.workflow.utility.merge",
    "alfred.workflow.utility.joinargs",
    "alfred.workflow.utility.joinarguments",
];

fn first_nonempty(node: &Node, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| node.s(k))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Music.app AppleScript for a reference launcher "iTunes command" code. The numbering is
/// fully determined by the names of the reference launcher's bundled one-command templates.
pub(super) fn music_script(code: i64) -> Option<String> {
    let t = |cmd: &str| format!("tell application \"Music\" to {cmd}");
    Some(match code {
        1 => t("playpause"),
        2 => t("next track"),
        3 => t("previous track"),
        5 => t("set sound volume to 100"),
        6 => t("set sound volume to ((sound volume) + 10)"),
        7 => t("set sound volume to ((sound volume) - 10)"),
        8 => t("set mute to (not mute)"),
        9..=13 => t(&format!("set rating of current track to {}", (code - 8) * 20)),
        14 => t("set rating of current track to 0"),
        15 => t("back track"),
        _ => return None,
    })
}

fn uuid_v4() -> String {
    use std::hash::{BuildHasher, RandomState};
    let s = RandomState::new();
    let (a, b) = (s.hash_one(1u8), s.hash_one(2u8));
    let mut x = [0u8; 16];
    x[..8].copy_from_slice(&a.to_be_bytes());
    x[8..].copy_from_slice(&b.to_be_bytes());
    x[6] = (x[6] & 0x0f) | 0x40;
    x[8] = (x[8] & 0x3f) | 0x80;
    let h: String = x.iter().map(|b| format!("{b:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn with_uuid(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(e) => format!("{stem}-{}.{}", uuid_v4(), e.to_string_lossy()),
        None => format!("{stem}-{}", uuid_v4()),
    };
    path.with_file_name(name)
}

/// Remove `.` and resolve `..` textually (no disk access, so it also works for paths that do
/// not exist yet).
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Is `path` the `info.plist` of the workflow folder `dir`? Compared textually (after
/// resolving `.`/`..`) and, when both exist, by real location; the file name ignores case
/// because the default macOS volume does.
fn is_info_plist_of(dir: &Path, path: &Path) -> bool {
    let norm = lexical_normalize(path);
    let named = norm
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("info.plist"));
    if !named {
        return false;
    }
    let Some(parent) = norm.parent() else { return false };
    if parent == lexical_normalize(dir) {
        return true;
    }
    match (parent.canonicalize(), dir.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn rend(st: &State, t: &str) -> String {
    render(t, &st.arg, &st.vars)
}

impl Workflow {
    /// The file an Open File / Launch Files object opens: `~` expanded, and a relative path
    /// resolved against the workflow's own folder (the reference launcher's working directory for a workflow)
    /// rather than left to `open`, which would use the launcher's working directory. The trust
    /// summary reports a relative literal as shipped with the workflow (`trust.rs`).
    fn open_target(&self, s: &str) -> PathBuf {
        let p = expand_tilde(s);
        if p.is_relative() {
            lexical_normalize(&self.dir.join(p))
        } else {
            p
        }
    }

    /// The application of Open File (`openwith`) / Open URL (`browser`): an app *name* is passed
    /// on as is, a path is expanded and resolved like [`Self::open_target`].
    fn open_app(&self, app: &str) -> Option<String> {
        if app.is_empty() {
            return None;
        }
        if !super::trust::app_is_path(app) {
            return Some(app.to_string());
        }
        Some(self.open_target(app).display().to_string())
    }
}

impl Workflow {
    /// Walk the graph from the object that produced the result.
    pub fn run(&self, act: &WorkflowAction, pressed: Mods, paths: &Paths) -> Vec<Effect> {
        let mut state = State {
            arg: act.arg.clone(),
            vars: self.base_vars(paths),
        };
        state.vars.extend(act.vars.clone());
        if let Some((_, ov)) = act.mods.iter().find(|(m, _)| *m == pressed) {
            if ov.valid == Some(false) {
                return vec![];
            }
            if let Some(a) = &ov.arg {
                state.arg = a.clone();
            }
            if ov.replaces_vars {
                // A mod's `variables` replace the item's (script-level ones stay,
                // they are part of `ov.vars`).
                state.vars = self.base_vars(paths);
            }
            state.vars.extend(ov.vars.clone());
        }
        let mut effects = Vec::new();
        let mut budget = MAX_GRAPH_STEPS;
        let tasks = self.next_tasks(&act.from_uid, pressed.flags(), &Branch::All, &state, &None);
        self.run_stream(tasks, paths, &mut effects, &mut budget);
        effects
    }

    /// Fire an External Trigger object with an argument and variables, like
    /// `reference://runtrigger`. `None` when the workflow has no such trigger.
    pub fn run_trigger(
        &self,
        trigger_id: &str,
        arg: &str,
        vars: BTreeMap<String, String>,
        paths: &Paths,
    ) -> Option<Vec<Effect>> {
        let node = self.external_trigger(trigger_id)?;
        let act = WorkflowAction {
            workflow: self.id.clone(),
            from_uid: node.uid.clone(),
            arg: arg.to_string(),
            vars,
            ..Default::default()
        };
        Some(self.run(&act, Mods::NONE, paths))
    }

    /// The objects that run next below `from`, in execution order.
    fn next_tasks(
        &self,
        from: &str,
        flags: u64,
        branch: &Branch,
        state: &State,
        args: &Option<Vec<String>>,
    ) -> Vec<Task<'_>> {
        let mut tasks = Vec::new();
        for conn in self.connections.get(from).into_iter().flatten() {
            if conn.modifiers != flags {
                continue;
            }
            let wanted = match branch {
                Branch::All => true,
                Branch::Output(uid) => conn.source_output.as_deref() == Some(uid.as_str()),
                Branch::Else => conn.source_output.is_none(),
            };
            if !wanted {
                continue;
            }
            let Some(node) = self.node(&conn.dest) else { continue };
            // A list of arguments (Split to arguments) fans out: the next
            // object runs once per argument, unless it is the object that joins them.
            match args.as_ref().filter(|_| !MERGE_KINDS.contains(&node.kind.as_str())) {
                Some(list) => tasks.extend(list.iter().map(|a| Task {
                    node,
                    st: State {
                        arg: a.clone(),
                        vars: state.vars.clone(),
                    },
                    args: None,
                })),
                None => tasks.push(Task {
                    node,
                    st: state.clone(),
                    args: args.clone(),
                }),
            }
        }
        tasks
    }

    /// Run tasks depth-first: each stream goes to completion before the next
    /// sibling starts.
    fn run_stream(&self, tasks: Vec<Task<'_>>, paths: &Paths, fx: &mut Vec<Effect>, budget: &mut usize) {
        let mut stack: Vec<Task> = tasks.into_iter().rev().collect();
        while let Some(Task { node, mut st, mut args }) = stack.pop() {
            if *budget == 0 {
                fx.push(Effect::Warn("workflow graph too deep or cyclic; stopped".into()));
                return;
            }
            *budget -= 1;
            let children = match self.process(node, &mut st, &mut args, paths, fx) {
                Flow::Next => self.next_tasks(&node.uid, 0, &Branch::All, &st, &args),
                Flow::Branch(b) => self.next_tasks(&node.uid, 0, &b, &st, &args),
                Flow::Stop => Vec::new(),
                Flow::Jump(uid, next) => self.next_tasks(&uid, 0, &Branch::All, &next, &None),
                Flow::Delay(seconds) => {
                    fx.push(Effect::Delay {
                        seconds,
                        resume: Box::new(Resume {
                            wf: self.clone(),
                            paths: paths.clone(),
                            from_uid: node.uid.clone(),
                            st,
                            args,
                            budget: *budget,
                        }),
                    });
                    Vec::new()
                }
            };
            stack.extend(children.into_iter().rev());
        }
    }

    fn script_result(&self, res: Result<ScriptOutput>, st: &mut State, fx: &mut Vec<Effect>) -> bool {
        match res {
            Ok(out) => {
                if !out.success {
                    fx.push(Effect::Warn(format!(
                        "{}: script exited with an error: {}",
                        self.name,
                        out.stderr.trim()
                    )));
                }
                apply_script_output(st, &out.stdout);
                true
            }
            Err(e) => {
                fx.push(Effect::Warn(format!("{}: {e:#}", self.name)));
                false
            }
        }
    }

    fn warn(&self, fx: &mut Vec<Effect>, msg: impl std::fmt::Display) {
        fx.push(Effect::Warn(format!("{}: {msg}", self.name)));
    }

    fn process(
        &self,
        node: &Node,
        st: &mut State,
        args: &mut Option<Vec<String>>,
        paths: &Paths,
        fx: &mut Vec<Effect>,
    ) -> Flow {
        let mut flow = Flow::Next;
        match node.kind.as_str() {
            "alfred.workflow.action.script" => {
                let res = run_node_script(node, self, paths, &st.arg, &st.vars, SCRIPT_ACTION_TIMEOUT);
                if !self.script_result(res, st, fx) {
                    return Flow::Stop;
                }
            }
            // Run Script output: same configuration as the action, but nothing follows.
            "alfred.workflow.output.script" => {
                let res = run_node_script(node, self, paths, &st.arg, &st.vars, SCRIPT_ACTION_TIMEOUT);
                self.script_result(res, st, fx);
                return Flow::Stop;
            }
            "alfred.workflow.action.applescript" => {
                let res = run_applescript(
                    &node.s("applescript"),
                    &st.arg,
                    self,
                    paths,
                    &st.vars,
                    SCRIPT_ACTION_TIMEOUT,
                );
                if !self.script_result(res, st, fx) {
                    return Flow::Stop;
                }
            }
            "alfred.workflow.action.openurl" => {
                let utf8 = node.b("utf8");
                let spaces = node.s("spaces");
                let q = if utf8 { percent_encode(&st.arg) } else { st.arg.clone() };
                let q = if spaces.is_empty() {
                    q
                } else {
                    q.replace("%20", &spaces)
                };
                let url = render(&node.s("url"), &q, &st.vars);
                let browser = self.open_app(&node.s("browser"));
                fx.push(Effect::OpenUrl { url, browser });
            }
            "alfred.workflow.action.openfile" => {
                // An empty file field means "the incoming argument" (templates
                // that follow a File Filter leave it blank).
                let mut src = rend(st, &node.s("sourcefile"));
                if src.is_empty() {
                    src = st.arg.clone();
                }
                let with = self.open_app(&node.s("openwith"));
                fx.push(Effect::OpenPath {
                    path: self.open_target(&src),
                    with,
                });
            }
            "alfred.workflow.action.launchfiles" => {
                for p in node.config.get("paths").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(s) = p.as_str() {
                        fx.push(Effect::OpenPath {
                            path: self.open_target(&rend(st, s)),
                            with: None,
                        });
                    }
                }
            }
            "alfred.workflow.action.revealfile" => {
                fx.push(Effect::Reveal(expand_tilde(&st.arg)));
            }
            "alfred.workflow.action.terminalcommand" => {
                let q = escape_query(&st.arg, node.i("escaping"));
                fx.push(Effect::Terminal(render(&node.s("text"), &q, &st.vars)));
            }
            "alfred.workflow.action.itunescommand" => match music_script(node.i("command")) {
                Some(s) => fx.push(Effect::AppleScript(s)),
                None => self.warn(fx, format!("unsupported music command {}", node.i("command"))),
            },
            "alfred.workflow.action.systemcommand" => match node.i("command") {
                // Only "Show Trash" is documented (a bundled template's note);
                // other codes are not mapped so a wrong guess can never log
                // someone out or shut the Mac down.
                2 => fx.push(Effect::OpenPath {
                    path: expand_tilde("~/.Trash"),
                    with: None,
                }),
                other => self.warn(fx, format!("system command {other} is not mapped")),
            },
            "alfred.workflow.action.systemwebsearch" => {
                // 1635215215 is 'awgo', the default (Google) search in the bundled template.
                if node.i("searcher") == 1_635_215_215 {
                    fx.push(Effect::OpenUrl {
                        url: format!("https://www.google.com/search?q={}", percent_encode(&st.arg)),
                        browser: self.open_app(&node.s("browser")),
                    });
                } else {
                    self.warn(fx, format!("web search engine {} is not mapped", node.i("searcher")));
                }
            }
            "alfred.workflow.action.browseinalfred" => {
                let p = rend(st, &node.s("path"));
                let p = if p.is_empty() { st.arg.clone() } else { p };
                fx.push(Effect::ShowLauncher { query: p });
            }
            "alfred.workflow.action.actioninalfred" => {
                let p = rend(st, &node.s("path"));
                let p = if p.is_empty() { st.arg.clone() } else { p };
                fx.push(Effect::ShowLauncher { query: p });
            }
            "alfred.workflow.output.notification" => {
                fx.push(Effect::Notify {
                    title: rend(st, &node.s("title")),
                    text: rend(st, &node.s("text")),
                });
            }
            "alfred.workflow.output.clipboard" => {
                fx.push(Effect::Copy(rend(st, &node.s("clipboardtext"))));
            }
            "alfred.workflow.output.largetype" => {
                fx.push(Effect::LargeType(rend(st, &node.s("largetypetext"))));
            }
            "alfred.workflow.output.writefile" => {
                let name = rend(st, &node.s("filename"));
                if name.is_empty() {
                    self.warn(fx, "Write File has no file name");
                    return Flow::Stop;
                }
                let mut path = expand_tilde(&name);
                if path.is_relative() {
                    let key = self.data_key();
                    let base = match node.i("relativepathmode") {
                        1 => paths.cache.join("workflow-cache").join(&key),
                        2 => self.dir.clone(),
                        _ => paths.data.join("workflow-data").join(&key),
                    };
                    path = base.join(path);
                }
                if node.b("adduuid") {
                    path = with_uuid(&path);
                }
                // The import prompt describes the objects that are in the package. A workflow
                // that could overwrite its own info.plist could add objects (scripts) later, so
                // that one file is off limits. Other files in the folder stay writable (the reference launcher
                // semantics), and a script can of course still write anywhere it wants: this
                // closes the declarative loophole, it is not a sandbox (SEC-30).
                if is_info_plist_of(&self.dir, &path) {
                    self.warn(fx, "Write File refuses to change the workflow's own info.plist");
                    return Flow::Stop;
                }
                let mode = match node.i("type") {
                    1 => WriteMode::Append,
                    2 => WriteMode::Prepend,
                    _ => WriteMode::Overwrite,
                };
                fx.push(Effect::WriteFile(WriteFile {
                    path: path.clone(),
                    contents: rend(st, &node.s("filetext")),
                    mode,
                    create_dirs: node.b("createintermediatefolders"),
                    allow_empty: node
                        .config
                        .get("allowemptyfiles")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                }));
                // "The resolved path is outputted from this object."
                st.arg = path.display().to_string();
            }
            "alfred.workflow.output.callexternaltrigger" => {
                let bundle = rend(st, &node.s("workflowbundleid"));
                let trigger = rend(st, &node.s("externaltriggerid"));
                if !(bundle.is_empty() || bundle == "self" || bundle == self.bundle_id) {
                    self.warn(
                        fx,
                        format!("calling trigger '{trigger}' of another workflow ('{bundle}') is not supported from inside a run"),
                    );
                    return Flow::Stop;
                }
                let Some(t) = self.external_trigger(&trigger) else {
                    self.warn(fx, format!("no external trigger '{trigger}'"));
                    return Flow::Stop;
                };
                let next = State {
                    arg: if node.b("passinputasargument") {
                        st.arg.clone()
                    } else {
                        String::new()
                    },
                    vars: if node.b("passvariables") {
                        st.vars.clone()
                    } else {
                        self.base_vars(paths)
                    },
                };
                return Flow::Jump(t.uid.clone(), next);
            }
            "alfred.workflow.utility.argument" => {
                let a = rend(st, &node.s("argument"));
                if !a.is_empty() {
                    st.arg = a;
                }
                let current = st.clone();
                for (k, v) in node
                    .config
                    .get("variables")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    st.vars
                        .insert(k.clone(), render(&stringify(v), &current.arg, &current.vars));
                }
            }
            "alfred.workflow.utility.junction" => {}
            "alfred.workflow.utility.debug" => {
                fx.push(Effect::Debug(rend(st, &node.s("argument"))));
            }
            "alfred.workflow.utility.conditional" => {
                let mut chosen: Option<String> = None;
                for c in node
                    .config
                    .get("conditions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let text = |k: &str| c.get(k).map(stringify).unwrap_or_default();
                    // An empty input field evaluates the incoming {query}.
                    let raw_in = text("inputstring");
                    let input = if raw_in.is_empty() {
                        st.arg.clone()
                    } else {
                        rend(st, &raw_in)
                    };
                    let pattern = rend(st, &text("matchstring"));
                    let mode = c.get("matchmode").and_then(Value::as_i64).unwrap_or(0);
                    let cs = c.get("matchcasesensitive").and_then(Value::as_bool).unwrap_or(false);
                    match utility::matches(mode, &input, &pattern, cs, MATCH_REGEX) {
                        // "The reference launcher will process the output of the first matched condition."
                        Ok(true) => {
                            chosen = Some(text("uid"));
                            break;
                        }
                        Ok(false) => {}
                        Err(e) => self.warn(fx, format!("conditional: {e}")),
                    }
                }
                flow = Flow::Branch(match chosen {
                    Some(uid) => Branch::Output(uid),
                    None => Branch::Else,
                });
            }
            "alfred.workflow.utility.filter" => {
                let raw_in = node.s("inputstring");
                let input = if raw_in.is_empty() {
                    st.arg.clone()
                } else {
                    rend(st, &raw_in)
                };
                let pattern = rend(st, &node.s("matchstring"));
                let cs = node.b("matchcasesensitive");
                match utility::matches(node.i("matchmode"), &input, &pattern, cs, utility::FILTER_REGEX) {
                    Ok(true) => {}
                    Ok(false) => flow = Flow::Stop,
                    Err(e) => {
                        self.warn(fx, format!("filter: {e}"));
                        return Flow::Stop;
                    }
                }
            }
            "alfred.workflow.utility.replace" => {
                let opts = ReplaceOpts {
                    mode: node.i("matchmode"),
                    case_insensitive: node.b("caseinsensitive") || node.b("regexcaseinsensitive"),
                    multiline: node.b("regexmultiline"),
                };
                let pattern = rend(st, &node.s("matchstring"));
                let with = rend(st, &node.s("replacestring"));
                match utility::replace(&st.arg, &pattern, &with, &opts) {
                    Ok(s) => st.arg = s,
                    Err(e) => {
                        self.warn(fx, format!("replace: {e}"));
                        return Flow::Stop;
                    }
                }
            }
            "alfred.workflow.utility.transform" => match utility::transform(node.i("type"), &st.arg) {
                Ok(s) => st.arg = s,
                Err(e) => {
                    self.warn(fx, format!("transform: {e}"));
                    return Flow::Stop;
                }
            },
            k if SPLIT_KINDS.contains(&k) => {
                let delim = {
                    let d = first_nonempty(node, &["delimiter", "customdelimiter"]);
                    if d.is_empty() { ",".to_string() } else { d }
                };
                let trim = node.b("trimarguments");
                let discard = node.b("discardemptyarguments");
                let parts = utility::split(&st.arg, &delim, trim, discard);
                if node.i("outputas") == 1 {
                    *args = Some(parts);
                } else {
                    let prefix = {
                        let p = node.s("variableprefix");
                        if p.is_empty() { "split".to_string() } else { p }
                    };
                    for (i, p) in parts.into_iter().enumerate() {
                        st.vars.insert(format!("{prefix}{}", i + 1), p);
                    }
                }
            }
            k if MERGE_KINDS.contains(&k) => {
                if let Some(list) = args.take() {
                    let d = first_nonempty(node, &["delimiter", "joinwith", "joinstring", "separator"]);
                    st.arg = list.join(&d);
                }
            }
            "alfred.workflow.utility.delay" => {
                let raw = rend(st, &node.s("seconds"));
                let seconds = match raw.trim().parse::<f64>() {
                    Ok(s) if s.is_finite() && s >= 0.0 => s,
                    _ => {
                        if !raw.trim().is_empty() {
                            self.warn(fx, format!("delay: '{raw}' is not a number of seconds"));
                        }
                        0.0
                    }
                };
                flow = Flow::Delay(seconds);
            }
            other => {
                self.warn(fx, format!("unsupported workflow object '{other}'"));
                return Flow::Stop;
            }
        }
        flow
    }
}

/// A Run Script object may print plain text (which becomes the next
/// argument) or the reference launcher's `{"alfredworkflow": {"arg": ..., "variables": {...}}}`.
fn apply_script_output(st: &mut State, stdout: &str) {
    let trimmed = stdout.trim_end_matches(['\n', '\r']);
    if trimmed.trim_start().starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && let Some(w) = v.get("alfredworkflow")
    {
        if let Some(a) = w.get("arg") {
            st.arg = stringify(a);
        }
        st.vars.extend(str_map(w.get("variables")));
        return;
    }
    st.arg = trimmed.to_string();
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::item::Action;
    use serde_json::json;

    #[test]
    fn render_templates_once() {
        let mut v = BTreeMap::new();
        v.insert("a".to_string(), "{query}".to_string());
        assert_eq!(render("x {query} {var:a} {var:missing}.", "Q", &v), "x Q {query} .");
    }

    #[test]
    fn script_filter_to_notification_chain() {
        let (paths, root) = temp_paths("chain");
        let wf = sample(&root);
        let (items, exclusive) = wf.keyword_items("demo hello", &paths);
        assert!(exclusive);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "one: hello");
        assert!(!items[1].valid);
        assert!(items[0].score > items[1].score);

        let Action::Workflow(act) = &items[0].action else {
            panic!()
        };
        let fx = wf.run(act, Mods::NONE, &paths);
        assert_eq!(
            fx,
            vec![Effect::Notify {
                title: "T".into(),
                text: "got:A hello:hi/hi".into()
            }]
        );

        // cmd modifier: override arg and follow the cmd-only connection
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        let fx = wf.run(act, cmd, &paths);
        assert_eq!(
            fx,
            vec![Effect::OpenUrl {
                url: "https://x.test/?q=CMD".into(),
                browser: None
            }]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_json_output_sets_vars() {
        let mut st = State {
            arg: "x".into(),
            vars: BTreeMap::new(),
        };
        apply_script_output(&mut st, r#"{"alfredworkflow":{"arg":"new","variables":{"k":1}}}"#);
        assert_eq!(st.arg, "new");
        assert_eq!(st.vars["k"], "1");
        apply_script_output(&mut st, "plain\n");
        assert_eq!(st.arg, "plain");
    }

    #[test]
    fn cycle_is_bounded() {
        let (paths, root) = temp_paths("cycle");
        let doc = json!({
            "name": "C", "bundleid": "c",
            "objects": [
                {"uid": "A", "type": "alfred.workflow.utility.junction", "config": {}},
                {"uid": "B", "type": "alfred.workflow.utility.junction", "config": {}},
            ],
            "connections": {"A": [{"destinationuid": "B", "modifiers": 0}], "B": [{"destinationuid": "A", "modifiers": 0}]}
        });
        let wf = Workflow::from_json(&root, "c", &doc).unwrap();
        let act = WorkflowAction {
            workflow: "c".into(),
            from_uid: "A".into(),
            arg: String::new(),
            vars: BTreeMap::new(),
            mods: vec![],
            ..Default::default()
        };
        let fx = wf.run(&act, Mods::NONE, &paths);
        assert!(matches!(fx.last(), Some(Effect::Warn(_))));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn relative_open_targets_and_apps_resolve_against_the_workflow_folder() {
        // Review item 1: the classifier (trust.rs) calls a relative literal "shipped with the
        // workflow", so the executor must not leave it to the launcher's working directory.
        let (paths, root) = temp_paths("openrel");
        let doc = json!({
            "name": "R", "bundleid": "r",
            "objects": [
                {"uid": "F", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": "payload.app", "openwith": "tools/Viewer.app"}},
                {"uid": "G", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": "/tmp/x", "openwith": "Safari"}},
                {"uid": "H", "type": "alfred.workflow.action.openfile",
                 "config": {"sourcefile": "~/y", "openwith": "/Applications/Safari.app"}},
                {"uid": "L", "type": "alfred.workflow.action.launchfiles", "config": {"paths": ["sub/a", "/abs/b"]}},
                {"uid": "U", "type": "alfred.workflow.action.openurl",
                 "config": {"url": "https://x.test", "browser": "./B.app"}},
            ],
            "connections": {}
        });
        let wf = load_via_plist(&root, "r", &doc);
        let at = |uid: &str| {
            let node = wf.nodes.iter().find(|n| n.uid == uid).unwrap();
            let mut fx = Vec::new();
            let mut st = State {
                arg: String::new(),
                vars: BTreeMap::new(),
            };
            wf.process(node, &mut st, &mut None, &paths, &mut fx);
            fx
        };
        let d = wf.dir.clone();
        let s = |p: PathBuf| p.display().to_string();
        assert_eq!(
            at("F"),
            [Effect::OpenPath {
                path: d.join("payload.app"),
                with: Some(s(d.join("tools/Viewer.app")))
            }]
        );
        assert_eq!(
            at("G"),
            [Effect::OpenPath {
                path: PathBuf::from("/tmp/x"),
                with: Some("Safari".into())
            }]
        );
        assert_eq!(
            at("H"),
            [Effect::OpenPath {
                path: expand_tilde("~/y"),
                with: Some("/Applications/Safari.app".into())
            }]
        );
        assert_eq!(
            at("L"),
            [
                Effect::OpenPath {
                    path: d.join("sub/a"),
                    with: None
                },
                Effect::OpenPath {
                    path: PathBuf::from("/abs/b"),
                    with: None
                },
            ]
        );
        assert_eq!(
            at("U"),
            [Effect::OpenUrl {
                url: "https://x.test".into(),
                browser: Some(s(d.join("B.app")))
            }]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
