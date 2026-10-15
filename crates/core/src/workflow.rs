//! Reference-compatible workflows.
//!
//! A workflow is a directory with an `info.plist` describing a graph of
//! objects (`objects`) wired by `connections`. This module loads that format,
//! implements the Script Filter JSON protocol, and walks the graph when a
//! result is actioned. The format was learned from the workflow templates
//! shipped with the reference launcher and its public workflow documentation; see
//! `docs/REVERSE_ENGINEERING.md` for what is observed vs. inferred.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde_json::Value;

use crate::config::Paths;
use crate::files::expand_tilde;
use crate::fuzzy;
use crate::item::{Action, Icon, Item, Mods};
use crate::websearch::percent_encode;

pub const KIND_KEYWORD: &str = "alfred.workflow.input.keyword";
pub const KIND_SCRIPT_FILTER: &str = "alfred.workflow.input.scriptfilter";
pub const KIND_EXTERNAL_TRIGGER: &str = "alfred.workflow.trigger.external";

const SCRIPT_FILTER_TIMEOUT: Duration = Duration::from_secs(10);
const SCRIPT_ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_GRAPH_STEPS: usize = 256;

#[derive(Debug, Clone)]
pub struct Node {
    pub uid: String,
    pub kind: String,
    pub config: Value,
}

impl Node {
    fn s(&self, key: &str) -> String {
        match self.config.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }
    fn i(&self, key: &str) -> i64 {
        self.config.get(key).and_then(Value::as_i64).unwrap_or(0)
    }
    fn b(&self, key: &str) -> bool {
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

/// Per-modifier override carried by a Script Filter item (`mods`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModOverride {
    pub arg: Option<String>,
    pub valid: Option<bool>,
    pub vars: BTreeMap<String, String>,
}

/// Everything needed to continue a workflow once the user actions a result.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowAction {
    pub workflow: String,
    pub from_uid: String,
    pub arg: String,
    pub vars: BTreeMap<String, String>,
    pub mods: Vec<(Mods, ModOverride)>,
}

/// Side effects requested by a workflow run. The caller performs them.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    OpenUrl { url: String, browser: Option<String> },
    OpenPath { path: PathBuf, with: Option<String> },
    Reveal(PathBuf),
    Copy(String),
    Notify { title: String, text: String },
    LargeType(String),
    Terminal(String),
    Warn(String),
}

// ---------------------------------------------------------------- loading

fn plist_to_json(v: &plist::Value) -> Value {
    match v {
        plist::Value::Array(a) => Value::Array(a.iter().map(plist_to_json).collect()),
        plist::Value::Dictionary(d) => {
            Value::Object(d.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect())
        }
        plist::Value::Boolean(b) => Value::Bool(*b),
        plist::Value::Real(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
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

fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), stringify(v))).collect())
        .unwrap_or_default()
}

impl Workflow {
    pub fn load(dir: &Path) -> Result<Workflow> {
        let v = plist::Value::from_file(dir.join("info.plist"))
            .with_context(|| format!("reading {}/info.plist", dir.display()))?;
        let id = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        Workflow::from_json(dir, &id, &plist_to_json(&v))
    }

    pub fn from_json(dir: &Path, id: &str, j: &Value) -> Result<Workflow> {
        let mut nodes = Vec::new();
        for o in j.get("objects").and_then(Value::as_array).into_iter().flatten() {
            let uid = o.get("uid").and_then(Value::as_str).ok_or_else(|| anyhow!("object without uid"))?;
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

    fn node(&self, uid: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.uid == uid)
    }

    fn data_key(&self) -> String {
        if self.bundle_id.is_empty() { self.id.clone() } else { self.bundle_id.clone() }
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

/// Install a `.alfredworkflow` package (a zip archive) under `dest_root`.
pub fn import_package(package: &Path, dest_root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_root)?;
    let tmp = dest_root.join(format!(".import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let file = std::fs::File::open(package).with_context(|| format!("opening {}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid workflow archive")?;
    // `extract` rejects entries that would escape the target directory.
    zip.extract(&tmp).context("extracting workflow")?;
    let wf = Workflow::load(&tmp).context("archive has no readable info.plist")?;
    let base: String = if wf.bundle_id.is_empty() { wf.name.clone() } else { wf.bundle_id.clone() };
    let safe: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if safe.is_empty() {
        bail!("workflow has neither a bundle id nor a name");
    }
    let dest = dest_root.join(format!("user.workflow.{safe}"));
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

// ---------------------------------------------------------------- templating

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{(query|var:[^}]+)\}").expect("valid regex"))
}

/// Substitute `{query}` and `{var:name}` in a single pass (substituted text is
/// never re-expanded).
pub fn render(template: &str, query: &str, vars: &BTreeMap<String, String>) -> String {
    template_re()
        .replace_all(template, |c: &regex::Captures| {
            let key = &c[1];
            if key == "query" {
                query.to_string()
            } else {
                vars.get(&key[4..]).cloned().unwrap_or_default()
            }
        })
        .into_owned()
}

// ---------------------------------------------------------------- scripts

#[derive(Debug)]
pub struct ScriptOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

struct Interpreter {
    program: &'static str,
    pre_args: &'static [&'static str],
    ext: &'static str,
}

/// Script types. Values 0 (bash) and 5 (zsh) are confirmed by the bundled
/// templates; the others follow the reference launcher's documented language list and are
/// unverified against the binary.
fn interpreter(kind: i64) -> Option<Interpreter> {
    Some(match kind {
        0 => Interpreter { program: "/bin/bash", pre_args: &[], ext: "sh" },
        1 => Interpreter { program: "/usr/bin/php", pre_args: &[], ext: "php" },
        2 => Interpreter { program: "/usr/bin/ruby", pre_args: &[], ext: "rb" },
        3 => Interpreter { program: "/usr/bin/python3", pre_args: &[], ext: "py" },
        4 => Interpreter { program: "/usr/bin/perl", pre_args: &[], ext: "pl" },
        5 => Interpreter { program: "/bin/zsh", pre_args: &[], ext: "zsh" },
        6 => Interpreter { program: "/usr/bin/osascript", pre_args: &[], ext: "applescript" },
        7 => Interpreter { program: "/usr/bin/osascript", pre_args: &["-l", "JavaScript"], ext: "js" },
        _ => return None,
    })
}

/// Escape `{query}` for substitution into script source. Bit meanings for
/// 2/4/32/64 are inferred from the default mask (102) and the reference launcher's forum
/// guidance; 1/8/16 are brackets/semicolons/spaces in unverified order.
fn escape_query(q: &str, mask: i64) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        let esc = match ch {
            '`' => mask & 2 != 0,
            '"' => mask & 4 != 0,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' => mask & 1 != 0,
            ';' => mask & 8 != 0,
            ' ' => mask & 16 != 0,
            '$' => mask & 32 != 0,
            '\\' => mask & 64 != 0,
            _ => false,
        };
        if esc {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn script_env(wf: &Workflow, paths: &Paths, vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let key = wf.data_key();
    let data = paths.data.join("workflow-data").join(&key);
    let cache = paths.cache.join("workflow-cache").join(&key);
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("alfred_version".into(), "5.0".into());
    env.insert("alfred_workflow_name".into(), wf.name.clone());
    env.insert("alfred_workflow_bundleid".into(), wf.bundle_id.clone());
    env.insert("alfred_workflow_uid".into(), wf.id.clone());
    env.insert("alfred_workflow_data".into(), data.display().to_string());
    env.insert("alfred_workflow_cache".into(), cache.display().to_string());
    for (k, v) in &wf.variables {
        env.insert(k.clone(), v.clone());
    }
    for (k, v) in vars {
        env.insert(k.clone(), v.clone());
    }
    env.into_iter().collect()
}

fn run_node_script(
    node: &Node,
    wf: &Workflow,
    paths: &Paths,
    query: &str,
    vars: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<ScriptOutput> {
    let kind = node.i("type");
    let argtype = node.i("scriptargtype");
    let mut script = node.s("script");
    let scriptfile = node.s("scriptfile");
    let env = script_env(wf, paths, vars);

    let mut cleanup: Option<PathBuf> = None;
    let (program, args): (PathBuf, Vec<String>) = if script.trim().is_empty() && !scriptfile.is_empty() {
        let p = if Path::new(&scriptfile).is_absolute() { PathBuf::from(&scriptfile) } else { wf.dir.join(&scriptfile) };
        (p, vec![])
    } else {
        let interp = interpreter(kind).ok_or_else(|| anyhow!("unsupported script type {kind}"))?;
        if argtype == 0 {
            script = script.replace("{query}", &escape_query(query, node.i("escaping")));
        }
        let dir = paths.cache.join("scripts");
        std::fs::create_dir_all(&dir)?;
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file = dir.join(format!("{}-{}.{}", std::process::id(), n, interp.ext));
        std::fs::write(&file, script)?;
        cleanup = Some(file.clone());
        let mut a: Vec<String> = interp.pre_args.iter().map(|s| s.to_string()).collect();
        a.push(file.display().to_string());
        (PathBuf::from(interp.program), a)
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    if argtype == 1 || (argtype == 0 && cleanup.is_none()) {
        cmd.arg(query);
    }
    cmd.current_dir(&wf.dir)
        .envs(env)
        .stdin(if argtype == 2 { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = run_with_timeout(&mut cmd, if argtype == 2 { Some(query) } else { None }, timeout);
    if let Some(f) = cleanup {
        let _ = std::fs::remove_file(f);
    }
    result
}

fn run_with_timeout(cmd: &mut Command, stdin: Option<&str>, timeout: Duration) -> Result<ScriptOutput> {
    let mut child = cmd.spawn().with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = si.write_all(text.as_bytes());
    }
    let mut so = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut se = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("script timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ok(ScriptOutput { stdout, stderr, success: status.success() })
}

// ---------------------------------------------------------------- keywords & script filters

enum KeywordMatch {
    No,
    /// The query is a leading fragment of the keyword.
    Partial,
    /// Keyword typed in full; the payload is the argument text and whether a
    /// separating space has been typed.
    Full { arg: String, spaced: bool },
}

fn match_keyword(query: &str, keyword: &str, withspace: bool) -> KeywordMatch {
    let q = query.trim_start();
    if keyword.is_empty() || q.is_empty() {
        return KeywordMatch::No;
    }
    let n = keyword.chars().count();
    let idx = q.char_indices().nth(n).map(|(i, _)| i).unwrap_or(q.len());
    let (head, rest) = q.split_at(idx);
    if head.to_lowercase() == keyword.to_lowercase() && head.chars().count() == n {
        if rest.is_empty() {
            return KeywordMatch::Full { arg: String::new(), spaced: false };
        }
        if withspace && !rest.starts_with(' ') {
            return KeywordMatch::No;
        }
        let spaced = rest.starts_with(' ');
        return KeywordMatch::Full { arg: rest.trim_start().to_string(), spaced };
    }
    if q.chars().count() >= 2 && keyword.to_lowercase().starts_with(&q.to_lowercase()) {
        return KeywordMatch::Partial;
    }
    KeywordMatch::No
}

fn resolve_icon(wf: &Workflow, icon: Option<&Value>) -> Icon {
    let Some(icon) = icon else { return Icon::None };
    let path = icon.get("path").and_then(Value::as_str).unwrap_or("");
    let ty = icon.get("type").and_then(Value::as_str).unwrap_or("");
    if path.is_empty() {
        return Icon::None;
    }
    match ty {
        "fileicon" => {
            let p = expand_tilde(path);
            if p.extension().is_some_and(|e| e == "app") { Icon::App(p) } else { Icon::Glyph("▤") }
        }
        "filetype" => Icon::Glyph("▤"),
        _ => {
            let p = if Path::new(path).is_absolute() { PathBuf::from(path) } else { wf.dir.join(path) };
            if p.is_file() { Icon::Image(p) } else { Icon::None }
        }
    }
}

fn workflow_icon(wf: &Workflow) -> Icon {
    let p = wf.dir.join("icon.png");
    if p.is_file() { Icon::Image(p) } else { Icon::Glyph("◆") }
}

impl Workflow {
    /// Result items contributed by this workflow's keyword inputs for `query`.
    /// The second value is true when a keyword fully matched, in which case
    /// the caller should show only these items.
    pub fn keyword_items(&self, query: &str, paths: &Paths) -> (Vec<Item>, bool) {
        let mut items = Vec::new();
        let mut exclusive = false;
        if self.disabled {
            return (items, exclusive);
        }
        for node in &self.nodes {
            if node.kind != KIND_KEYWORD && node.kind != KIND_SCRIPT_FILTER {
                continue;
            }
            let keyword = render(&node.s("keyword"), "", &self.variables);
            let withspace = node.b("withspace");
            let argtype = node.i("argumenttype"); // 0 required, 1 optional, 2 none
            let title = {
                let t = node.s("title");
                if t.is_empty() { node.s("text") } else { t }
            };
            let title = if title.is_empty() { keyword.clone() } else { title };
            let sub = node.s("subtext");
            let placeholder_id = format!("wfkw:{}:{}", self.id, node.uid);

            match match_keyword(query, &keyword, withspace) {
                KeywordMatch::No => {}
                KeywordMatch::Partial => {
                    items.push(
                        Item::new(placeholder_id, title)
                            .subtitle(sub)
                            .icon(workflow_icon(self))
                            .autocomplete(format!("{keyword}{}", if withspace { " " } else { "" }))
                            .valid(false)
                            .score(700.0),
                    );
                }
                KeywordMatch::Full { arg, spaced } => {
                    exclusive = true;
                    let needs_arg = argtype == 0;
                    let satisfied = !needs_arg || !arg.is_empty();
                    if node.kind == KIND_KEYWORD {
                        let subtitle = render(&sub, &arg, &self.variables);
                        let mut it = Item::new(placeholder_id, title)
                            .subtitle(subtitle)
                            .icon(workflow_icon(self))
                            .valid(satisfied)
                            .score(1000.0);
                        if argtype == 2 && !arg.is_empty() {
                            it = it.valid(false);
                        }
                        it.action = Action::Workflow(Box::new(WorkflowAction {
                            workflow: self.id.clone(),
                            from_uid: node.uid.clone(),
                            arg: arg.clone(),
                            vars: BTreeMap::new(),
                            mods: vec![],
                        }));
                        if !satisfied {
                            it.autocomplete = Some(format!("{keyword} "));
                        }
                        items.push(it);
                    } else if (withspace && !spaced) || (needs_arg && arg.is_empty()) {
                        // Script filter waiting for its argument.
                        items.push(
                            Item::new(placeholder_id, title)
                                .subtitle(sub)
                                .icon(workflow_icon(self))
                                .autocomplete(format!("{keyword} "))
                                .valid(false)
                                .score(1000.0),
                        );
                    } else {
                        items.extend(self.run_script_filter(node, &arg, paths));
                    }
                }
            }
        }
        (items, exclusive)
    }

    fn run_script_filter(&self, node: &Node, arg: &str, paths: &Paths) -> Vec<Item> {
        let out = match run_node_script(node, self, paths, arg, &BTreeMap::new(), SCRIPT_FILTER_TIMEOUT) {
            Ok(o) => o,
            Err(e) => return vec![error_item(self, node, &format!("{e:#}"))],
        };
        let parsed = match parse_script_filter(&out.stdout) {
            Ok(p) => p,
            Err(e) => {
                let detail = if out.stderr.trim().is_empty() { format!("{e:#}") } else { out.stderr.trim().to_string() };
                return vec![error_item(self, node, &detail)];
            }
        };
        let filter = node.b("alfredfiltersresults");
        let mut items = Vec::new();
        for (idx, raw) in parsed.items.iter().enumerate() {
            let title = raw.get("title").map(stringify).unwrap_or_default();
            let mut score = 10_000.0 - idx as f64;
            if filter {
                match fuzzy::score(arg, &title) {
                    Some(s) => score += s,
                    None => continue,
                }
            }
            let sub = raw.get("subtitle").map(stringify).unwrap_or_default();
            let item_arg = match raw.get("arg") {
                Some(Value::Array(a)) => a.first().map(stringify).unwrap_or_default(),
                Some(v) => stringify(v),
                None => String::new(),
            };
            let mut vars = parsed.variables.clone();
            vars.extend(str_map(raw.get("variables")));
            let mut mods = Vec::new();
            for (k, m) in raw.get("mods").and_then(Value::as_object).into_iter().flatten() {
                if let Some(mm) = Mods::parse(k) {
                    mods.push((
                        mm,
                        ModOverride {
                            arg: m.get("arg").map(stringify),
                            valid: m.get("valid").and_then(Value::as_bool),
                            vars: str_map(m.get("variables")),
                        },
                    ));
                }
            }
            let uid = raw.get("uid").map(stringify).filter(|s| !s.is_empty()).unwrap_or_else(|| title.clone());
            let text = raw.get("text");
            let mut it = Item::new(format!("wf:{}:{}:{}", self.id, node.uid, uid), title)
                .subtitle(sub)
                .icon(resolve_icon(self, raw.get("icon")))
                .valid(raw.get("valid").and_then(Value::as_bool).unwrap_or(true))
                .score(score);
            it.autocomplete = raw.get("autocomplete").map(stringify);
            it.copy_text = text.and_then(|t| t.get("copy")).map(stringify);
            it.large_type = text.and_then(|t| t.get("largetype")).map(stringify);
            it.action = Action::Workflow(Box::new(WorkflowAction {
                workflow: self.id.clone(),
                from_uid: node.uid.clone(),
                arg: item_arg,
                vars,
                mods,
            }));
            items.push(it);
        }
        if items.is_empty() {
            items.push(
                Item::new(format!("wfempty:{}:{}", self.id, node.uid), "No results")
                    .subtitle(node.s("subtext"))
                    .icon(workflow_icon(self))
                    .valid(false)
                    .score(1.0),
            );
        }
        items
    }

    /// Find an External Trigger object by its trigger id.
    pub fn external_trigger(&self, trigger_id: &str) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|n| n.kind == KIND_EXTERNAL_TRIGGER && n.s("triggerid") == trigger_id)
    }
}

fn error_item(wf: &Workflow, node: &Node, detail: &str) -> Item {
    let first = detail.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
    Item::new(format!("wferr:{}:{}", wf.id, node.uid), format!("{}: script error", wf.name))
        .subtitle(first.to_string())
        .icon(Icon::Glyph("!"))
        .valid(false)
        .score(1.0)
}

struct ParsedFilter {
    items: Vec<Value>,
    variables: BTreeMap<String, String>,
}

fn parse_script_filter(stdout: &str) -> Result<ParsedFilter> {
    let v: Value = serde_json::from_str(stdout.trim()).context("script filter output is not valid JSON")?;
    let items = match v.get("items") {
        Some(Value::Array(a)) => a.clone(),
        _ => bail!("script filter JSON has no \"items\" array"),
    };
    Ok(ParsedFilter { items, variables: str_map(v.get("variables")) })
}

// ---------------------------------------------------------------- execution

#[derive(Clone)]
struct State {
    arg: String,
    vars: BTreeMap<String, String>,
}

impl Workflow {
    /// Walk the graph from the object that produced the result.
    pub fn run(&self, act: &WorkflowAction, pressed: Mods, paths: &Paths) -> Vec<Effect> {
        let mut state = State { arg: act.arg.clone(), vars: self.variables.clone() };
        state.vars.extend(act.vars.clone());
        if let Some((_, ov)) = act.mods.iter().find(|(m, _)| *m == pressed) {
            if ov.valid == Some(false) {
                return vec![];
            }
            if let Some(a) = &ov.arg {
                state.arg = a.clone();
            }
            state.vars.extend(ov.vars.clone());
        }
        let mut effects = Vec::new();
        let mut budget = MAX_GRAPH_STEPS;
        self.follow(&act.from_uid, pressed.flags(), &state, paths, &mut effects, &mut budget);
        effects
    }

    fn follow(&self, from: &str, flags: u64, state: &State, paths: &Paths, fx: &mut Vec<Effect>, budget: &mut usize) {
        for conn in self.connections.get(from).into_iter().flatten() {
            if conn.modifiers != flags {
                continue;
            }
            if *budget == 0 {
                fx.push(Effect::Warn("workflow graph too deep or cyclic; stopped".into()));
                return;
            }
            *budget -= 1;
            if let Some(node) = self.node(&conn.dest) {
                self.process(node, state.clone(), paths, fx, budget);
            }
        }
    }

    fn process(&self, node: &Node, mut st: State, paths: &Paths, fx: &mut Vec<Effect>, budget: &mut usize) {
        let r = |t: &str| render(t, &st.arg, &st.vars);
        match node.kind.as_str() {
            "alfred.workflow.action.script" => {
                match run_node_script(node, self, paths, &st.arg, &st.vars, SCRIPT_ACTION_TIMEOUT) {
                    Ok(out) => {
                        if !out.success {
                            fx.push(Effect::Warn(format!("{}: script exited with an error: {}", self.name, out.stderr.trim())));
                        }
                        apply_script_output(&mut st, &out.stdout);
                    }
                    Err(e) => {
                        fx.push(Effect::Warn(format!("{}: {e:#}", self.name)));
                        return;
                    }
                }
            }
            "alfred.workflow.action.openurl" => {
                let utf8 = node.b("utf8");
                let spaces = node.s("spaces");
                let q = if utf8 { percent_encode(&st.arg) } else { st.arg.clone() };
                let q = if spaces.is_empty() { q } else { q.replace("%20", &spaces) };
                let url = render(&node.s("url"), &q, &st.vars);
                let browser = Some(node.s("browser")).filter(|b| !b.is_empty());
                fx.push(Effect::OpenUrl { url, browser });
            }
            "alfred.workflow.action.openfile" => {
                let path = expand_tilde(&r(&node.s("sourcefile")));
                let with = Some(node.s("openwith")).filter(|w| !w.is_empty());
                fx.push(Effect::OpenPath { path, with });
            }
            "alfred.workflow.action.launchfiles" => {
                for p in node.config.get("paths").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(s) = p.as_str() {
                        fx.push(Effect::OpenPath { path: expand_tilde(&r(s)), with: None });
                    }
                }
            }
            "alfred.workflow.action.revealfile" => {
                fx.push(Effect::Reveal(expand_tilde(&st.arg)));
            }
            "alfred.workflow.action.terminalcommand" => {
                fx.push(Effect::Terminal(r(&node.s("text"))));
            }
            "alfred.workflow.output.notification" => {
                fx.push(Effect::Notify { title: r(&node.s("title")), text: r(&node.s("text")) });
            }
            "alfred.workflow.output.clipboard" => {
                fx.push(Effect::Copy(r(&node.s("clipboardtext"))));
            }
            "alfred.workflow.output.largetype" => {
                fx.push(Effect::LargeType(r(&node.s("largetypetext"))));
            }
            "alfred.workflow.utility.argument" => {
                let a = r(&node.s("argument"));
                if !a.is_empty() {
                    st.arg = a;
                }
                let current = st.clone();
                for (k, v) in node.config.get("variables").and_then(Value::as_object).into_iter().flatten() {
                    st.vars.insert(k.clone(), render(&stringify(v), &current.arg, &current.vars));
                }
            }
            "alfred.workflow.utility.junction" => {}
            other => {
                fx.push(Effect::Warn(format!("{}: unsupported workflow object '{other}'", self.name)));
                return;
            }
        }
        self.follow(&node.uid, 0, &st, paths, fx, budget);
    }
}

/// A Run Script object may print plain text (which becomes the next
/// argument) or the reference launcher's `{"alfredworkflow": {"arg": ..., "variables": {...}}}`.
fn apply_script_output(st: &mut State, stdout: &str) {
    let trimmed = stdout.trim_end_matches(['\n', '\r']);
    if trimmed.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if let Some(w) = v.get("alfredworkflow") {
                if let Some(a) = w.get("arg") {
                    st.arg = stringify(a);
                }
                st.vars.extend(str_map(w.get("variables")));
                return;
            }
        }
    }
    st.arg = trimmed.to_string();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_paths(tag: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-wf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (Paths::under(&root), root)
    }

    fn sample(root: &Path) -> Workflow {
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

    #[test]
    fn render_templates_once() {
        let mut v = BTreeMap::new();
        v.insert("a".to_string(), "{query}".to_string());
        assert_eq!(render("x {query} {var:a} {var:missing}.", "Q", &v), "x Q {query} .");
    }

    #[test]
    fn keyword_matching_modes() {
        assert!(matches!(match_keyword("demo", "demo", true), KeywordMatch::Full { spaced: false, .. }));
        match match_keyword("Demo  hello", "demo", true) {
            KeywordMatch::Full { arg, spaced } => assert!(spaced && arg == "hello"),
            _ => panic!(),
        }
        assert!(matches!(match_keyword("demox", "demo", true), KeywordMatch::No));
        assert!(matches!(match_keyword("de", "demo", true), KeywordMatch::Partial));
        assert!(matches!(match_keyword("d", "demo", true), KeywordMatch::No));
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

        let Action::Workflow(act) = &items[0].action else { panic!() };
        let fx = wf.run(act, Mods::NONE, &paths);
        assert_eq!(fx, vec![Effect::Notify { title: "T".into(), text: "got:A hello:hi/hi".into() }]);

        // cmd modifier: override arg and follow the cmd-only connection
        let cmd = Mods { cmd: true, ..Mods::NONE };
        let fx = wf.run(act, cmd, &paths);
        assert_eq!(fx, vec![Effect::OpenUrl { url: "https://x.test/?q=CMD".into(), browser: None }]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn script_filter_errors_become_items() {
        let (paths, root) = temp_paths("err");
        let mut wf = sample(&root);
        wf.nodes[0].config["script"] = json!("echo not json");
        let (items, _) = wf.keyword_items("demo x", &paths);
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains("script error"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn placeholder_before_space() {
        let (paths, root) = temp_paths("ph");
        let wf = sample(&root);
        let (items, exclusive) = wf.keyword_items("demo", &paths);
        assert!(exclusive);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].autocomplete.as_deref(), Some("demo "));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn query_escaping_follows_mask() {
        assert_eq!(escape_query("a\"b$c d", 4 | 32), "a\\\"b\\$c d");
        assert_eq!(escape_query("a b", 16), "a\\ b");
    }

    #[test]
    fn workflow_json_output_sets_vars() {
        let mut st = State { arg: "x".into(), vars: BTreeMap::new() };
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
        let act = WorkflowAction { workflow: "c".into(), from_uid: "A".into(), arg: String::new(), vars: BTreeMap::new(), mods: vec![] };
        let fx = wf.run(&act, Mods::NONE, &paths);
        assert!(matches!(fx.last(), Some(Effect::Warn(_))));
        let _ = std::fs::remove_dir_all(root);
    }
}
