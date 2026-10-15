//! The query engine: turns typed text into ranked results and performs the
//! chosen action.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Result, bail};

use crate::apps::{self, AppEntry};
use crate::calc;
use crate::clipboard::{self, ClipboardHistory, Snippet};
use crate::config::{Config, Paths};
use crate::files;
use crate::fuzzy;
use crate::item::{Action, Icon, Item, Mods};
use crate::system::SystemCommand;
use crate::usage::Usage;
use crate::workflow::{self, Effect, Workflow, WorkflowAction};

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    /// Show this text in Large Type.
    LargeType(String),
    /// The action is destructive; ask the user and call again with `confirmed`.
    Confirm(String),
    Error(String),
}

pub struct Engine {
    pub paths: Paths,
    pub config: Config,
    apps: RwLock<Vec<AppEntry>>,
    workflows: RwLock<Vec<Workflow>>,
    snippets: RwLock<Vec<Snippet>>,
    usage: Mutex<Usage>,
    clipboard: Arc<Mutex<ClipboardHistory>>,
}

/// If `q` is `keyword` or `keyword <rest>`, return the (left-trimmed) rest.
fn strip_keyword<'a>(q: &'a str, keyword: &str) -> Option<&'a str> {
    let head = q.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &q[keyword.len()..];
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(' ') {
        Some(rest.trim_start())
    } else {
        None
    }
}

impl Engine {
    pub fn new(paths: Paths, config: Config) -> Engine {
        let usage = Usage::load(paths.usage_file());
        let clipboard = ClipboardHistory::load(paths.clipboard_file(), config.clipboard_history_size);
        let e = Engine {
            paths,
            config,
            apps: RwLock::new(Vec::new()),
            workflows: RwLock::new(Vec::new()),
            snippets: RwLock::new(Vec::new()),
            usage: Mutex::new(usage),
            clipboard: Arc::new(Mutex::new(clipboard)),
        };
        e.reload();
        e
    }

    /// Re-scan applications, workflows and snippets.
    pub fn reload(&self) {
        let found = apps::scan(&self.config.app_dirs);
        *self.apps.write().unwrap() = found;
        let mut roots = vec![self.paths.workflows_dir()];
        roots.extend(self.config.workflow_dirs.iter().cloned());
        *self.workflows.write().unwrap() = workflow::load_all(&roots);
        *self.snippets.write().unwrap() = clipboard::load_snippets(&self.paths.snippets_file());
    }

    pub fn clipboard(&self) -> Arc<Mutex<ClipboardHistory>> {
        Arc::clone(&self.clipboard)
    }

    pub fn app_count(&self) -> usize {
        self.apps.read().unwrap().len()
    }

    pub fn workflow_summaries(&self) -> Vec<(String, String, bool)> {
        self.workflows
            .read()
            .unwrap()
            .iter()
            .map(|w| (w.name.clone(), w.bundle_id.clone(), w.disabled))
            .collect()
    }

    pub fn import_workflow(&self, package: &Path) -> Result<PathBuf> {
        let dest = workflow::import_package(package, &self.paths.workflows_dir())?;
        self.reload();
        Ok(dest)
    }

    // ------------------------------------------------------------ query

    pub fn query(&self, input: &str) -> Vec<Item> {
        let q = input.trim_start();
        if q.trim().is_empty() {
            return Vec::new();
        }
        let max = self.config.max_results;

        // 1. Workflow keywords take over the list when one is fully typed.
        let mut wf_items = Vec::new();
        let mut exclusive = false;
        for wf in self.workflows.read().unwrap().iter() {
            let (items, ex) = wf.keyword_items(q, &self.paths);
            exclusive |= ex;
            wf_items.extend(items);
        }
        if exclusive {
            sort_items(&mut wf_items);
            wf_items.truncate(max);
            return wf_items;
        }

        // 2. Built-in prefixed commands.
        if let Some(cmd) = q.strip_prefix('>') {
            let cmd = cmd.trim();
            let it = Item::new(
                "shell",
                if cmd.is_empty() {
                    "Run a command in Terminal".to_string()
                } else {
                    format!("Run in Terminal: {cmd}")
                },
            )
            .subtitle("↩ to run")
            .icon(Icon::Glyph(">"))
            .valid(!cmd.is_empty())
            .action(Action::Shell(cmd.to_string()));
            return vec![it];
        }
        for kw in ["open", "find"] {
            if let Some(rest) = strip_keyword(q, kw) {
                return self.file_items(kw, rest);
            }
        }
        if let Some(rest) = strip_keyword(q, "clipboard").or_else(|| strip_keyword(q, "clip")) {
            return self.clipboard_items(rest);
        }
        if let Some(rest) = strip_keyword(q, "snip") {
            return self.snippet_items(rest);
        }
        for ws in &self.config.web_searches {
            if let Some(rest) = strip_keyword(q, &ws.keyword)
                && !rest.is_empty()
            {
                return vec![web_item(&ws.title, &ws.url_for(rest), rest)];
            }
        }
        if q.starts_with('/') || q.starts_with('~') {
            let paths = files::browse(q, max);
            if !paths.is_empty() {
                return paths.iter().map(|p| path_item(p)).collect();
            }
        }

        // 3. General search.
        let mut items = wf_items;
        let expr = q
            .strip_prefix('=')
            .map(str::trim)
            .or_else(|| calc::looks_like_expression(q).then_some(q));
        if let Some(v) = expr.and_then(calc::eval) {
            let s = calc::format(v);
            let mut it = Item::new(format!("calc:{s}"), s.clone())
                .subtitle(format!("{} = {s} — ↩ to copy", expr.unwrap_or("")))
                .icon(Icon::Glyph("="))
                .action(Action::Copy(s.clone()))
                .score(5000.0);
            it.large_type = Some(s.clone());
            it.copy_text = Some(s);
            items.push(it);
        }

        for app in self.apps.read().unwrap().iter() {
            if let Some(s) = fuzzy::score(q, &app.name) {
                let mut it = Item::new(format!("app:{}", app.path.display()), &app.name)
                    .subtitle(files::tilde(&app.path))
                    .icon(Icon::App(app.path.clone()))
                    .action(Action::Open(app.path.clone()))
                    .score(s);
                it.cmd_action = Some(Action::Reveal(app.path.clone()));
                items.push(it);
            }
        }
        for sc in SystemCommand::ALL {
            if let Some(s) = fuzzy::score(q, sc.title()) {
                items.push(
                    Item::new(sc.id(), sc.title())
                        .subtitle(sc.subtitle())
                        .icon(Icon::Glyph("⚙"))
                        .action(Action::System(sc))
                        .score(s),
                );
            }
        }

        {
            let usage = self.usage.lock().unwrap();
            for it in items.iter_mut().filter(|i| i.score < 5000.0 && !i.id.starts_with("wf")) {
                it.score += usage.boost(q, &it.id);
            }
        }
        sort_items(&mut items);
        items.truncate(max);

        // 4. Fallback web searches always close the list.
        for (i, ws) in self.config.web_searches.iter().filter(|w| w.fallback).enumerate() {
            let mut it = web_item(&ws.title, &ws.url_for(q.trim()), q.trim());
            it.id = format!("fallback:{}", ws.keyword);
            it.score = -1000.0 - i as f64;
            items.push(it);
        }
        items
    }

    fn file_items(&self, kw: &str, rest: &str) -> Vec<Item> {
        if rest.is_empty() {
            return vec![
                Item::new(format!("kw:{kw}"), format!("{kw} <file name>"))
                    .subtitle("Search files in your home folder by name")
                    .icon(Icon::Glyph("▤"))
                    .valid(false),
            ];
        }
        let found = files::search(rest, self.config.max_results);
        if found.is_empty() {
            return vec![
                Item::new("nofiles", "No files found")
                    .subtitle(rest.to_string())
                    .valid(false),
            ];
        }
        found.iter().map(|p| path_item(p)).collect()
    }

    fn clipboard_items(&self, rest: &str) -> Vec<Item> {
        let hits = self.clipboard.lock().unwrap().search(rest, self.config.max_results);
        if hits.is_empty() {
            return vec![Item::new("noclip", "Clipboard history is empty").valid(false)];
        }
        hits.iter()
            .enumerate()
            .map(|(i, e)| {
                let first = e.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                let lines = e.text.lines().count();
                let mut it = Item::new(format!("clip:{i}:{}", e.ts), truncate(first, 120))
                    .subtitle(format!(
                        "{} chars, {} line(s) — ↩ to copy",
                        e.text.chars().count(),
                        lines
                    ))
                    .icon(Icon::Glyph("⎘"))
                    .action(Action::Copy(e.text.clone()))
                    .score(10_000.0 - i as f64);
                it.large_type = Some(e.text.clone());
                it
            })
            .collect()
    }

    fn snippet_items(&self, rest: &str) -> Vec<Item> {
        let clip = self.clipboard.lock().unwrap().latest().unwrap_or("").to_string();
        let snippets = self.snippets.read().unwrap();
        let mut v: Vec<(f64, &Snippet)> = snippets
            .iter()
            .filter_map(|s| {
                let a = fuzzy::score(rest, &s.name);
                let b = fuzzy::score(rest, &s.keyword);
                match (a, b) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (x, y) => x.or(y),
                }
                .map(|sc| (sc, s))
            })
            .collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        if v.is_empty() {
            return vec![
                Item::new("nosnip", "No snippets")
                    .subtitle(format!("Add some to {}", self.paths.snippets_file().display()))
                    .valid(false),
            ];
        }
        v.into_iter()
            .map(|(sc, s)| {
                let text = clipboard::expand_snippet(&s.text, &clip);
                let mut it = Item::new(format!("snip:{}", s.keyword), &s.name)
                    .subtitle(truncate(text.lines().next().unwrap_or(""), 100))
                    .icon(Icon::Glyph("✂"))
                    .action(Action::Copy(text.clone()))
                    .score(sc);
                it.large_type = Some(text);
                it
            })
            .collect()
    }

    // ------------------------------------------------------------ execution

    pub fn execute(&self, item: &Item, query: &str, mods: Mods, confirmed: bool) -> Outcome {
        if !item.valid {
            return Outcome::Done;
        }
        let action = match (&item.cmd_action, mods.cmd) {
            (Some(a), true) => a.clone(),
            _ => item.action.clone(),
        };
        if let Action::System(cmd) = &action
            && cmd.needs_confirmation()
            && !confirmed
        {
            return Outcome::Confirm(format!("{}? Press ↩ again to confirm.", cmd.title()));
        }
        let outcome = self.perform(&action, mods);
        if outcome == Outcome::Done || matches!(outcome, Outcome::LargeType(_)) {
            self.usage.lock().unwrap().record(query, &item.id);
        }
        outcome
    }

    fn perform(&self, action: &Action, mods: Mods) -> Outcome {
        let result: Result<Option<String>> = (|| {
            match action {
                Action::None => {}
                Action::Open(p) => open_path(p, None)?,
                Action::Reveal(p) => reveal(p)?,
                Action::OpenUrl(u) => open_url(u, None)?,
                Action::Copy(t) => copy_to_clipboard(t)?,
                Action::System(c) => c.run()?,
                Action::Shell(cmd) => run_in_terminal(cmd)?,
                Action::Workflow(a) => return Ok(self.run_workflow(a, mods)),
            }
            Ok(None)
        })();
        match result {
            Ok(None) => Outcome::Done,
            Ok(Some(msg)) => msg_to_outcome(msg),
            Err(e) => Outcome::Error(format!("{e:#}")),
        }
    }

    fn run_workflow(&self, act: &WorkflowAction, mods: Mods) -> Option<String> {
        let effects = {
            let wfs = self.workflows.read().unwrap();
            let wf = wfs.iter().find(|w| w.id == act.workflow)?;
            wf.run(act, mods, &self.paths)
        };
        Some(apply_effects(effects))
    }

    /// Fire a workflow's External Trigger, like `reference://runtrigger`.
    pub fn run_trigger(&self, bundle_id: &str, trigger_id: &str, arg: &str) -> Outcome {
        let effects = {
            let wfs = self.workflows.read().unwrap();
            let Some(wf) = wfs.iter().find(|w| w.bundle_id == bundle_id && !w.disabled) else {
                return Outcome::Error(format!("no enabled workflow with bundle id '{bundle_id}'"));
            };
            let Some(node) = wf.external_trigger(trigger_id) else {
                return Outcome::Error(format!("workflow has no external trigger '{trigger_id}'"));
            };
            let act = WorkflowAction {
                workflow: wf.id.clone(),
                from_uid: node.uid.clone(),
                arg: arg.to_string(),
                vars: Default::default(),
                mods: vec![],
            };
            wf.run(&act, Mods::NONE, &self.paths)
        };
        msg_to_outcome(apply_effects(effects))
    }
}

// Effects are reported back as a tagged string so `perform` can stay uniform.
const TAG_LARGE: &str = "\u{1}large:";
const TAG_ERR: &str = "\u{1}err:";

fn msg_to_outcome(m: String) -> Outcome {
    if let Some(t) = m.strip_prefix(TAG_LARGE) {
        Outcome::LargeType(t.to_string())
    } else if let Some(e) = m.strip_prefix(TAG_ERR) {
        Outcome::Error(e.to_string())
    } else {
        Outcome::Done
    }
}

fn apply_effects(effects: Vec<Effect>) -> String {
    let mut large = None;
    let mut errors = Vec::new();
    for fx in effects {
        let r: Result<()> = match fx {
            Effect::OpenUrl { url, browser } => open_url(&url, browser.as_deref()),
            Effect::OpenPath { path, with } => open_path(&path, with.as_deref()),
            Effect::Reveal(p) => reveal(&p),
            Effect::Copy(t) => copy_to_clipboard(&t),
            Effect::Notify { title, text } => notify(&title, &text),
            Effect::LargeType(t) => {
                large = Some(t);
                Ok(())
            }
            Effect::Terminal(c) => run_in_terminal(&c),
            Effect::Warn(w) => {
                errors.push(w);
                Ok(())
            }
        };
        if let Err(e) = r {
            errors.push(format!("{e:#}"));
        }
    }
    if !errors.is_empty() {
        format!("{TAG_ERR}{}", errors.join("; "))
    } else if let Some(t) = large {
        format!("{TAG_LARGE}{t}")
    } else {
        String::new()
    }
}

fn sort_items(items: &mut [Item]) {
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

fn web_item(engine: &str, url: &str, query: &str) -> Item {
    Item::new(
        format!("web:{engine}:{query}"),
        format!("Search {engine} for '{query}'"),
    )
    .subtitle(url.to_string())
    .icon(Icon::Glyph("🔍"))
    .action(Action::OpenUrl(url.to_string()))
}

fn path_item(p: &Path) -> Item {
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string());
    let is_app = p.extension().is_some_and(|e| e == "app");
    let mut it = Item::new(format!("file:{}", p.display()), name)
        .subtitle(files::tilde(p))
        .icon(if is_app {
            Icon::App(p.to_path_buf())
        } else if p.is_dir() {
            Icon::Glyph("▸")
        } else {
            Icon::Glyph("▤")
        })
        .action(Action::Open(p.to_path_buf()));
    it.cmd_action = Some(Action::Reveal(p.to_path_buf()));
    if p.is_dir() {
        it.autocomplete = Some(format!("{}/", files::tilde(p)));
    }
    it
}

// ---------------------------------------------------------------- OS actions

fn no_dash(s: &str, what: &str) -> Result<()> {
    if s.starts_with('-') || s.is_empty() {
        bail!("refusing to open {what} '{s}'");
    }
    Ok(())
}

fn run_status(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        bail!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn open_path(p: &Path, with: Option<&str>) -> Result<()> {
    let s = p.to_string_lossy();
    no_dash(&s, "path")?;
    let mut c = Command::new("open");
    if let Some(app) = with {
        no_dash(app, "application")?;
        c.arg("-a").arg(app);
    }
    run_status(c.arg(&*s), "open")
}

pub fn reveal(p: &Path) -> Result<()> {
    run_status(Command::new("open").arg("-R").arg(p), "reveal")
}

pub fn open_url(url: &str, browser: Option<&str>) -> Result<()> {
    no_dash(url, "URL")?;
    let mut c = Command::new("open");
    if let Some(app) = browser {
        no_dash(app, "browser")?;
        c.arg("-a").arg(app);
    }
    run_status(c.arg(url), "open")
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    child.stdin.take().expect("piped stdin").write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("pbcopy failed");
    }
    Ok(())
}

fn osascript_args(lines: &[&str], args: &[&str]) -> Result<()> {
    let mut c = Command::new("osascript");
    for l in lines {
        c.arg("-e").arg(l);
    }
    c.args(args);
    run_status(&mut c, "osascript")
}

/// Strings reach AppleScript through `argv`, never spliced into source.
pub fn notify(title: &str, text: &str) -> Result<()> {
    osascript_args(
        &[
            "on run argv",
            "display notification (item 1 of argv) with title (item 2 of argv)",
            "end run",
        ],
        &[text, title],
    )
}

pub fn run_in_terminal(cmd: &str) -> Result<()> {
    if cmd.trim().is_empty() {
        bail!("empty command");
    }
    osascript_args(
        &[
            "on run argv",
            "tell application \"Terminal\"",
            "activate",
            "do script (item 1 of argv)",
            "end tell",
            "end run",
        ],
        &[cmd],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine(tag: &str) -> (Engine, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-eng-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("apps");
        std::fs::create_dir_all(apps.join("Safari.app")).unwrap();
        std::fs::create_dir_all(apps.join("Visual Studio Code.app")).unwrap();
        std::fs::create_dir_all(apps.join("Calendar.app")).unwrap();
        let paths = Paths::under(&root);
        let config = Config {
            app_dirs: vec![apps],
            ..Config::default()
        };
        (Engine::new(paths, config), root)
    }

    #[test]
    fn finds_apps_by_initials_and_prefix() {
        let (e, root) = engine("apps");
        assert_eq!(e.query("saf")[0].title, "Safari");
        assert_eq!(e.query("vsc")[0].title, "Visual Studio Code");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn calculator_and_fallbacks() {
        let (e, root) = engine("calc");
        let r = e.query("2+3*4");
        assert_eq!(r[0].title, "14");
        assert!(r.last().unwrap().title.starts_with("Search "));
        let r = e.query("= sqrt(81)");
        assert_eq!(r[0].title, "9");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn web_keyword_is_exclusive() {
        let (e, root) = engine("web");
        let r = e.query("g rust lifetimes");
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].action,
            Action::OpenUrl("https://www.google.com/search?q=rust%20lifetimes".into())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn usage_learning_reorders() {
        let (e, root) = engine("learn");
        // "ca" prefixes Calendar; "sa" prefixes Safari. Make "a" ambiguous.
        let before: Vec<_> = e.query("a").iter().take(3).map(|i| i.title.clone()).collect();
        assert!(!before.is_empty());
        let target = e
            .query("a")
            .into_iter()
            .find(|i| i.title == "Visual Studio Code")
            .unwrap();
        for _ in 0..3 {
            e.usage.lock().unwrap().record("a", &target.id);
        }
        assert_eq!(e.query("a")[0].title, "Visual Studio Code");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn destructive_system_commands_need_confirmation() {
        let (e, root) = engine("sys");
        let it = e
            .query("shut down")
            .into_iter()
            .find(|i| i.title == "Shut Down")
            .unwrap();
        assert!(matches!(
            e.execute(&it, "shut down", Mods::NONE, false),
            Outcome::Confirm(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_keyword_through_engine() {
        let (e, root) = engine("wf");
        let dir = e.paths.workflows_dir().join("user.workflow.t");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({
            "name": "T", "bundleid": "t.t", "objects": [
                {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {
                    "keyword": "shout", "withspace": true, "argumenttype": 0, "text": "Shout", "subtext": "say {query}"}},
                {"uid": "L", "type": "alfred.workflow.output.largetype", "config": {"largetypetext": "{query}!"}}
            ],
            "connections": {"K": [{"destinationuid": "L", "modifiers": 0}]}
        });
        let v: plist::Value = serde_json::from_value::<serde_json::Value>(doc)
            .map(|j| json_to_plist(&j))
            .unwrap();
        v.to_file_xml(dir.join("info.plist")).unwrap();
        e.reload();
        let r = e.query("shout hello");
        assert_eq!(r[0].title, "Shout");
        assert_eq!(r[0].subtitle, "say hello");
        assert_eq!(
            e.execute(&r[0], "shout hello", Mods::NONE, false),
            Outcome::LargeType("hello!".into())
        );
        // argument required: without one the item is not actionable
        assert!(!e.query("shout ")[0].valid);
        let _ = std::fs::remove_dir_all(root);
    }

    fn json_to_plist(j: &serde_json::Value) -> plist::Value {
        use serde_json::Value as J;
        match j {
            J::Null => plist::Value::String(String::new()),
            J::Bool(b) => plist::Value::Boolean(*b),
            J::Number(n) => n
                .as_i64()
                .map(plist::Value::from)
                .unwrap_or_else(|| plist::Value::Real(n.as_f64().unwrap())),
            J::String(s) => plist::Value::String(s.clone()),
            J::Array(a) => plist::Value::Array(a.iter().map(json_to_plist).collect()),
            J::Object(o) => plist::Value::Dictionary(o.iter().map(|(k, v)| (k.clone(), json_to_plist(v))).collect()),
        }
    }
}
