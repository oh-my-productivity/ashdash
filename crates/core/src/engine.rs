//! The query engine: turns typed text into ranked results and performs the
//! chosen action.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Result, bail};

use crate::actions;
use crate::apps::AppEntry;
use crate::clipboard::{self, ClipboardHistory, Snippet};
use crate::config::{Config, Paths};
use crate::index::AppIndex;
use crate::item::{Action, Item, Mods};
use crate::providers::{self, EngineState, Placement, Provider, QueryCtx, sort_items};
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
    apps: Arc<RwLock<Vec<AppEntry>>>,
    /// Keeps the persistent app index refreshed and watched; dropping the engine stops it.
    app_index: Arc<AppIndex>,
    workflows: Arc<RwLock<Vec<Workflow>>>,
    snippets: Arc<RwLock<Vec<Snippet>>>,
    usage: Mutex<Usage>,
    clipboard: Arc<Mutex<ClipboardHistory>>,
    providers: Vec<Box<dyn Provider>>,
    /// Background reload bookkeeping (see [`Engine::reload`]).
    reload_gate: Arc<Mutex<ReloadGate>>,
    /// Serialises writes of per-workflow settings files.
    settings_lock: Mutex<()>,
}

#[derive(Default)]
struct ReloadGate {
    running: bool,
    /// Another reload was requested while one was running.
    again: bool,
}

/// Everything a reload needs, owned so it can run on a background thread.
struct ReloadJob {
    app_index: Arc<AppIndex>,
    workflows: Arc<RwLock<Vec<Workflow>>>,
    snippets: Arc<RwLock<Vec<Snippet>>>,
    workflow_roots: Vec<PathBuf>,
    snippets_file: PathBuf,
}

impl ReloadJob {
    /// Re-scan applications, workflows and snippets. Slow work happens before
    /// the locks are taken, so queries keep being answered from the old data.
    fn run(&self) {
        self.app_index.rescan();
        self.run_content();
    }

    fn run_content(&self) {
        let fresh = workflow::load_all(&self.workflow_roots);
        let snippets = clipboard::load_snippets(&self.snippets_file);
        let old = std::mem::replace(&mut *self.workflows.write().unwrap(), fresh);
        // A delayed continuation may still hold a clone of an old workflow: start it clean.
        for w in &old {
            w.reset_session();
        }
        *self.snippets.write().unwrap() = snippets;
    }
}

/// One entry of a workflow's configuration sheet with its current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSetting {
    pub variable: String,
    pub label: String,
    pub description: String,
    /// Control type from the sheet (`textfield`, `checkbox`, `popupbutton`, ...).
    pub kind: String,
    pub default: String,
    /// What scripts see: the user's value, or the default.
    pub value: String,
    pub is_default: bool,
}

impl Engine {
    pub fn new(paths: Paths, config: Config) -> Engine {
        let usage = Usage::load(paths.usage_file());
        let mut clipboard = ClipboardHistory::load(paths.clipboard_file(), config.clipboard.max_entries);
        config.clipboard.apply(&mut clipboard);
        // Served from the cache at once (or scanned synchronously on the very first run);
        // refreshed and watched in the background afterwards.
        let apps = Arc::new(RwLock::new(Vec::new()));
        let app_index = Arc::new(AppIndex::open(&paths, &config.app_dirs, Arc::clone(&apps)));
        let providers = providers::registry_for(&config);
        let e = Engine {
            paths,
            config,
            apps,
            app_index,
            workflows: Arc::new(RwLock::new(Vec::new())),
            snippets: Arc::new(RwLock::new(Vec::new())),
            usage: Mutex::new(usage),
            clipboard: Arc::new(Mutex::new(clipboard)),
            providers,
            reload_gate: Arc::default(),
            settings_lock: Mutex::new(()),
        };
        e.reload_content();
        e
    }

    /// Re-scan applications, workflows and snippets in the background and
    /// return at once; queries keep using the old data until the new data is
    /// swapped in. Calls made while a reload is running are merged into one
    /// extra pass. Use [`Engine::reload_blocking`] to wait for the result.
    pub fn reload(&self) {
        {
            let mut gate = self.reload_gate.lock().unwrap();
            if gate.running {
                gate.again = true;
                return;
            }
            gate.running = true;
        }
        let job = self.reload_job();
        let gate = Arc::clone(&self.reload_gate);
        let spawned = std::thread::Builder::new()
            .name("ashdash-reload".into())
            .spawn(move || {
                loop {
                    job.run();
                    let mut g = gate.lock().unwrap();
                    if g.again {
                        g.again = false;
                        continue;
                    }
                    g.running = false;
                    break;
                }
            });
        if spawned.is_err() {
            // No thread available: do the work here rather than never.
            self.reload_gate.lock().unwrap().running = false;
            self.reload_blocking();
        }
    }

    /// Whether a background [`Engine::reload`] is still working.
    pub fn reload_pending(&self) -> bool {
        self.reload_gate.lock().unwrap().running
    }

    /// Re-scan applications, workflows and snippets and return when done
    /// (tests and the command line).
    pub fn reload_blocking(&self) {
        self.reload_job().run();
    }

    /// Workflows and snippets only (applications are handled by the app index).
    fn reload_content(&self) {
        self.reload_job().run_content();
    }

    fn reload_job(&self) -> ReloadJob {
        let mut roots = vec![self.paths.workflows_dir()];
        roots.extend(self.config.workflow_dirs.iter().cloned());
        ReloadJob {
            app_index: Arc::clone(&self.app_index),
            workflows: Arc::clone(&self.workflows),
            snippets: Arc::clone(&self.snippets),
            workflow_roots: roots,
            snippets_file: self.paths.snippets_file(),
        }
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
        // The new workflow must be usable as soon as this returns.
        self.reload_content();
        Ok(dest)
    }

    // ------------------------------------------------------------ workflow settings

    /// The configuration sheet of the workflow with `bundle_id`, with each
    /// entry's current value; `None` when no such workflow is installed.
    pub fn workflow_settings(&self, bundle_id: &str) -> Option<Vec<WorkflowSetting>> {
        let wfs = self.workflows.read().unwrap();
        let wf = wfs.iter().find(|w| !bundle_id.is_empty() && w.bundle_id == bundle_id)?;
        let vars = wf.base_vars(&self.paths);
        Some(
            wf.user_config
                .iter()
                .map(|c| {
                    let value = vars.get(&c.variable).cloned().unwrap_or_else(|| c.default.clone());
                    WorkflowSetting {
                        variable: c.variable.clone(),
                        label: c.label.clone(),
                        description: c.description.clone(),
                        kind: c.kind.clone(),
                        is_default: value == c.default,
                        default: c.default.clone(),
                        value,
                    }
                })
                .collect(),
        )
    }

    /// Save a value for one entry of a workflow's configuration sheet. It is
    /// stored in `<data>/workflow-config/<bundle id>.json` (mode 0600) and is
    /// visible to the workflow's next run as `$variable` / `{var:variable}`.
    pub fn set_workflow_setting(&self, bundle_id: &str, variable: &str, value: &str) -> Result<()> {
        self.update_workflow_values(bundle_id, variable, Some(value))
    }

    /// Forget the saved value of one entry; the sheet's default applies again.
    pub fn reset_workflow_setting(&self, bundle_id: &str, variable: &str) -> Result<()> {
        self.update_workflow_values(bundle_id, variable, None)
    }

    fn update_workflow_values(&self, bundle_id: &str, variable: &str, value: Option<&str>) -> Result<()> {
        let _guard = self.settings_lock.lock().unwrap();
        let wfs = self.workflows.read().unwrap();
        let Some(wf) = wfs.iter().find(|w| !bundle_id.is_empty() && w.bundle_id == bundle_id) else {
            bail!("no workflow with bundle id '{bundle_id}'");
        };
        if !wf.user_config.iter().any(|c| c.variable == variable) {
            bail!("workflow '{}' has no setting '{variable}'", wf.name);
        }
        let file = wf.user_values_file(&self.paths);
        let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        match value {
            Some(v) => map.insert(variable.to_string(), v.into()),
            None => map.remove(variable),
        };
        crate::config::write_private(&file, &serde_json::to_vec_pretty(&map)?)?;
        // The values are cached per loaded workflow: drop the cache so the next run re-reads them.
        wf.reset_session();
        Ok(())
    }

    // ------------------------------------------------------------ query

    /// Ask each provider in registry order. An exclusive answer is returned
    /// as-is; everything else is pooled, usage-boosted, sorted and truncated,
    /// and `Trailing` providers (fallback searches) close the list.
    pub fn query(&self, input: &str) -> Vec<Item> {
        let q = input.trim_start();
        if q.trim().is_empty() {
            return Vec::new();
        }
        let state = EngineState {
            config: &self.config,
            paths: &self.paths,
            apps: &self.apps,
            workflows: &self.workflows,
            snippets: &self.snippets,
            clipboard: &self.clipboard,
        };
        let ctx = QueryCtx {
            query: q,
            engine_state: &state,
        };

        let mut items = Vec::new();
        let mut fixed = Vec::new(); // from providers that opt out of learning
        let mut trailing = Vec::new();
        for p in &self.providers {
            let out = p.query(&ctx);
            if out.exclusive {
                let mut v = out.items;
                v.truncate(self.config.max_results);
                return v;
            }
            match (p.placement(), p.learns()) {
                (Placement::Trailing, _) => trailing.extend(out.items),
                (Placement::Ranked, true) => items.extend(out.items),
                (Placement::Ranked, false) => fixed.extend(out.items),
            }
        }

        {
            let usage = self.usage.lock().unwrap();
            for it in items.iter_mut().filter(|i| i.score < 5000.0) {
                it.score += usage.boost(q, &it.id);
            }
        }
        items.extend(fixed);
        sort_items(&mut items);
        items.truncate(self.config.max_results);
        items.extend(trailing);
        items
    }

    /// Universal Actions available for a result (empty when it has none).
    pub fn actions_for(&self, item: &Item) -> Vec<Item> {
        actions::actions_for(item)
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
        if let Action::Universal(u) = &action
            && let Some(prompt) = u.confirmation()
            && !confirmed
        {
            return Outcome::Confirm(prompt);
        }
        if let Action::Clipboard(_) = &action
            && let Some(prompt) = action.confirmation()
            && !confirmed
        {
            return Outcome::Confirm(prompt);
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
                Action::Clipboard(c) => match clipboard::run_command(&self.clipboard, c) {
                    Outcome::Error(m) => bail!("{m}"),
                    Outcome::LargeType(t) => return Ok(Some(format!("{TAG_LARGE}{t}"))),
                    Outcome::Done | Outcome::Confirm(_) => {}
                },
                Action::Universal(u) => match actions::run(&actions::RealOs, u) {
                    Outcome::LargeType(t) => return Ok(Some(format!("{TAG_LARGE}{t}"))),
                    Outcome::Error(e) => bail!("{e}"),
                    Outcome::Done | Outcome::Confirm(_) => {}
                },
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
                ..Default::default()
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
            Effect::Delay { seconds, resume } => {
                workflow::spawn_delayed(seconds, *resume, apply_effects);
                Ok(())
            }
            Effect::WriteFile(w) => w.perform(),
            Effect::AppleScript(s) => workflow::run_osascript(&s).map(|_| ()),
            Effect::ShowLauncher { query } => Err(anyhow::anyhow!(
                "showing the launcher with '{query}' from a workflow is not wired up yet"
            )),
            Effect::Debug(t) => {
                workflow::debug_log(&t);
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
        e.reload_blocking();
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

    /// A workflow keyword equal to a built-in prefix must win: workflows are first in the registry.
    #[test]
    fn workflow_keyword_beats_builtin_prefixes() {
        for builtin in ["clip", "snip", "g"] {
            let (e, root) = engine(&format!("prec-{builtin}"));
            let dir = e.paths.workflows_dir().join("user.workflow.p");
            std::fs::create_dir_all(&dir).unwrap();
            let doc = json!({
                "name": "P", "bundleid": "p.p", "objects": [
                    {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {
                        "keyword": builtin, "withspace": true, "argumenttype": 1, "text": "Mine", "subtext": ""}}
                ],
                "connections": {}
            });
            json_to_plist(&doc).to_file_xml(dir.join("info.plist")).unwrap();
            e.reload_blocking();
            let r = e.query(&format!("{builtin} x"));
            assert_eq!(r.len(), 1, "{builtin}: {r:?}");
            assert_eq!(r[0].title, "Mine", "{builtin}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn exclusive_output_is_truncated_by_the_engine() {
        let (mut e, root) = engine("trunc");
        e.config.max_results = 2;
        let snippets: Vec<_> = (0..5)
            .map(|i| crate::clipboard::Snippet {
                name: format!("s{i}"),
                keyword: format!("k{i}"),
                text: "t".into(),
                ..Default::default()
            })
            .collect();
        *e.snippets.write().unwrap() = snippets;
        assert_eq!(e.query("snip s").len(), 2);
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
