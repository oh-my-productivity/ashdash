//! The query engine: turns typed text into ranked results and performs the
//! chosen action.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Result, bail};

use crate::actions::{self, Os};
use crate::apps::AppEntry;
use crate::clipboard::{self, ClipboardHistory, Snippet};
use crate::config::{Config, Paths};
use crate::index::AppIndex;
use crate::item::{Action, Item, Mods};
use crate::providers::{self, EngineState, Placement, Provider, QueryCtx, RegistryDeps, sort_items};
use crate::usage::Usage;
use crate::workflow::{self, Effect, Workflow, WorkflowAction};

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    /// Show this text in Large Type.
    LargeType(String),
    /// Show the launcher with this text typed in (a workflow's Open Launcher object).
    ShowLauncher(String),
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
    /// Settings applied after start-up by [`Engine::apply_config`]; they take precedence over
    /// `config` and `providers` (which stay as the start-up snapshot).
    live: RwLock<Option<Arc<Live>>>,
    /// What providers are built from (kept for [`Engine::apply_config`]).
    deps: RegistryDeps,
    /// Everything that reaches the machine from `perform` (Universal Actions).
    os: Arc<dyn Os>,
    /// Background reload bookkeeping (see [`Engine::reload`]).
    reload_gate: Arc<Mutex<ReloadGate>>,
    /// Same bookkeeping for the workflow/snippet-only refresh started by [`Engine::apply_config`].
    content_gate: Arc<Mutex<ContentGate>>,
    /// Serialises writes of per-workflow settings files.
    settings_lock: Mutex<()>,
    /// Pasteboard and key-press access for clipboard actions (replaceable in tests).
    clip_env: Box<dyn clipboard::command::ClipEnv + Send + Sync>,
    /// What was wrong with the configuration at start-up (web searches that were skipped).
    startup_warnings: Vec<String>,
}

/// Configuration and the providers built from it, swapped as one unit.
struct Live {
    config: Config,
    providers: Vec<Box<dyn Provider>>,
}

/// What [`Engine::apply_config`] could not apply to the running engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Setting names that only take effect after a restart.
    pub restart_required: Vec<&'static str>,
    /// Parts of the new settings that were left out because they are not allowed (a web
    /// search with a non-web URL, say); everything else was applied.
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct ReloadGate {
    running: bool,
    /// Another reload was requested while one was running.
    again: bool,
    /// Tests: make the next worker pass panic (to prove the gate is released).
    #[cfg(test)]
    panic_once: bool,
}

/// Bookkeeping of the background workflow/snippet refresh: the roots of the *latest* request
/// win, so a refresh queued behind a running one scans the newest folder list.
#[derive(Default)]
struct ContentGate {
    running: bool,
    again: bool,
    roots: Vec<PathBuf>,
    /// Tests: make the next worker pass panic (to prove the gate is released).
    #[cfg(test)]
    panic_once: bool,
}

/// Held by a reload worker: if the worker unwinds (a panic while scanning), the gate is released
/// so [`Engine::reload_pending`] cannot stay true for the rest of the session and the next
/// request starts a new worker. Normal exits clear the gate themselves, under the lock that
/// also decides whether another pass is due, and [`GateRelease::disarm`] the guard.
struct GateRelease<T> {
    gate: Arc<Mutex<T>>,
    clear: fn(&mut T),
    armed: bool,
}

impl<T> GateRelease<T> {
    fn new(gate: Arc<Mutex<T>>, clear: fn(&mut T)) -> Self {
        GateRelease {
            gate,
            clear,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<T> Drop for GateRelease<T> {
    fn drop(&mut self) {
        if self.armed {
            let mut g = self.gate.lock().unwrap_or_else(|p| p.into_inner());
            (self.clear)(&mut g);
        }
    }
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
        // The first root is the app's own workflows folder (`reload_job`); the others come from
        // `workflow_dirs` and are never written to.
        if let Some(own) = self.workflow_roots.first() {
            workflow::sweep_interrupted_installs(own);
        }
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
    /// An engine for the real machine: real OS, the current user's home.
    pub fn new(paths: Paths, config: Config) -> Engine {
        Engine::with_deps(paths, config, RegistryDeps::ambient())
    }

    /// Like [`Engine::new`] but every OS-level operation (Spotlight, Finder
    /// scripts, `open`, ...) goes through `os`. The home folder is still the
    /// current user's, so bookmarks and file search read the real one; use
    /// [`Engine::with_env`] to replace that too.
    pub fn with_os(paths: Paths, config: Config, os: Box<dyn Os>) -> Engine {
        Engine::with_deps(
            paths,
            config,
            RegistryDeps {
                os: Arc::from(os),
                ..RegistryDeps::ambient()
            },
        )
    }

    /// A fully injected engine: `os` runs every OS operation and `home` is the
    /// folder bookmarks are read from and files are searched in, so a test
    /// engine never touches the real machine.
    pub fn with_env(paths: Paths, config: Config, os: Box<dyn Os>, home: PathBuf) -> Engine {
        Engine::with_deps(
            paths,
            config,
            RegistryDeps {
                os: Arc::from(os),
                home,
            },
        )
    }

    fn with_deps(paths: Paths, config: Config, deps: RegistryDeps) -> Engine {
        let mut usage = Usage::load(paths.usage_file());
        usage.set_enabled(config.learn_usage);
        let mut clipboard = ClipboardHistory::load(paths.clipboard_file(), config.clipboard.max_entries);
        config.clipboard.apply(&mut clipboard);
        // Served from the cache at once (or scanned synchronously on the very first run);
        // refreshed and watched in the background afterwards.
        let apps = Arc::new(RwLock::new(Vec::new()));
        let app_index = Arc::new(AppIndex::open(&paths, &config.app_dirs, Arc::clone(&apps)));
        let (providers, startup_warnings) = providers::registry_checked(&config, &deps);
        for w in &startup_warnings {
            eprintln!("ashdash: {w}");
        }
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
            live: RwLock::new(None),
            os: Arc::clone(&deps.os),
            deps,
            reload_gate: Arc::default(),
            content_gate: Arc::default(),
            settings_lock: Mutex::new(()),
            clip_env: Box::new(clipboard::command::SystemEnv::default()),
            startup_warnings,
        };
        e.reload_content();
        e
    }

    /// What was wrong with the configuration this engine started with (web searches that were
    /// left out because their URL is not a plain http(s) URL, ...). The window reports these the
    /// same way as problems found by a live reload.
    pub fn startup_warnings(&self) -> &[String] {
        &self.startup_warnings
    }

    /// Replace the pasteboard / key-press backend used by clipboard actions
    /// (integration tests use a fake; the default talks to macOS).
    pub fn with_clip_env(mut self, env: Box<dyn clipboard::command::ClipEnv + Send + Sync>) -> Engine {
        self.clip_env = env;
        self
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
                let mut release = GateRelease::new(Arc::clone(&gate), |g: &mut ReloadGate| {
                    g.running = false;
                    g.again = false;
                });
                loop {
                    #[cfg(test)]
                    if std::mem::take(&mut gate.lock().unwrap().panic_once) {
                        panic!("test: reload worker panics");
                    }
                    job.run();
                    let mut g = gate.lock().unwrap();
                    if g.again {
                        g.again = false;
                        continue;
                    }
                    g.running = false;
                    release.disarm();
                    break;
                }
            });
        if spawned.is_err() {
            // No thread available: do the work here rather than never.
            self.reload_gate.lock().unwrap().running = false;
            self.reload_blocking();
        }
    }

    /// Whether a background [`Engine::reload`] or content refresh is still working.
    pub fn reload_pending(&self) -> bool {
        self.reload_gate.lock().unwrap().running || self.content_gate.lock().unwrap().running
    }

    /// Like [`Engine::reload_content`] but on a background thread: returns at once, requests
    /// made while a refresh runs are merged into one extra pass over the newest folder list, and
    /// [`Engine::reload_pending`] reports it. Queries keep using the old workflows meanwhile.
    pub fn reload_content_background(&self) {
        let job = self.reload_job();
        {
            let mut gate = self.content_gate.lock().unwrap();
            gate.roots = job.workflow_roots.clone();
            if gate.running {
                gate.again = true;
                return;
            }
            gate.running = true;
        }
        let gate = Arc::clone(&self.content_gate);
        let worker_job = job;
        let spawned = std::thread::Builder::new()
            .name("ashdash-reload-content".into())
            .spawn(move || {
                let mut release = GateRelease::new(Arc::clone(&gate), |g: &mut ContentGate| {
                    g.running = false;
                    g.again = false;
                });
                let mut job = worker_job;
                loop {
                    #[cfg(test)]
                    if std::mem::take(&mut gate.lock().unwrap().panic_once) {
                        panic!("test: content refresh panics");
                    }
                    job.run_content();
                    let mut g = gate.lock().unwrap();
                    if g.again {
                        g.again = false;
                        job.workflow_roots = g.roots.clone();
                        continue;
                    }
                    g.running = false;
                    release.disarm();
                    break;
                }
            });
        if spawned.is_err() {
            self.content_gate.lock().unwrap().running = false;
            self.reload_content();
        }
    }

    /// Re-scan applications, workflows and snippets and return when done
    /// (tests and the command line).
    pub fn reload_blocking(&self) {
        self.reload_job().run();
    }

    /// Workflows and snippets only (applications are handled by the app index). Used by the
    /// settings watcher and the preferences window after they changed those files.
    pub fn reload_content(&self) {
        self.reload_job().run_content();
    }

    fn reload_job(&self) -> ReloadJob {
        let mut roots = vec![self.paths.workflows_dir()];
        roots.extend(self.config_now().workflow_dirs.iter().cloned());
        ReloadJob {
            app_index: Arc::clone(&self.app_index),
            workflows: Arc::clone(&self.workflows),
            snippets: Arc::clone(&self.snippets),
            workflow_roots: roots,
            snippets_file: self.paths.snippets_file(),
        }
    }

    /// The settings in force: the last [`Engine::apply_config`], or the start-up settings.
    pub fn config_now(&self) -> Config {
        match &*self.live.read().unwrap() {
            Some(l) => l.config.clone(),
            None => self.config.clone(),
        }
    }

    /// Put new settings into force without restarting: rebuilds the providers (provider
    /// switches, web searches, file search scope, bookmark sources), re-applies the clipboard
    /// retention / ignore list, the usage-learning switch and the extra workflow folders.
    /// Queries running meanwhile finish with the old settings. Settings that are read once at
    /// start-up are listed in the report (`app_dirs`); the global hotkey and the theme belong to
    /// the window, which applies them itself.
    pub fn apply_config(&self, new: Config) -> ApplyReport {
        let old = self.config_now();
        let mut report = ApplyReport::default();
        if new.app_dirs != old.app_dirs {
            report.restart_required.push("app_dirs");
        }
        let (providers, warnings) = providers::registry_checked(&new, &self.deps);
        report.warnings = warnings;
        new.clipboard.apply(&mut self.clipboard.lock().unwrap());
        self.usage.lock().unwrap().set_enabled(new.learn_usage);
        let workflows_moved = new.workflow_dirs != old.workflow_dirs;
        *self.live.write().unwrap() = Some(Arc::new(Live { config: new, providers }));
        if workflows_moved {
            // The folder scan can be slow and this runs on the window thread: refresh in the
            // background (see `reload_pending`); queries use the old workflows until it lands.
            self.reload_content_background();
        }
        report
    }

    /// Whether result picks are recorded and used for ranking.
    pub fn usage_enabled(&self) -> bool {
        self.usage.lock().unwrap().is_enabled()
    }

    /// Forget everything learned from earlier picks (memory and `usage.json`).
    pub fn clear_usage(&self) {
        self.usage.lock().unwrap().clear();
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

    /// Install a package without asking. UIs that confirm with the user must use
    /// [`Engine::stage_workflow`] and [`Engine::install_staged`] instead.
    pub fn import_workflow(&self, package: &Path) -> Result<PathBuf> {
        self.install_staged(self.stage_workflow(package)?)
    }

    /// Open a package once for inspection (see `workflow::stage_package`); nothing is installed.
    pub fn stage_workflow(&self, package: &Path) -> Result<workflow::StagedPackage> {
        workflow::stage_package(&self.paths, package)
    }

    /// Install exactly the tree a [`StagedPackage`](workflow::StagedPackage) described.
    pub fn install_staged(&self, staged: workflow::StagedPackage) -> Result<PathBuf> {
        let dest = staged.install()?;
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
        Some(crate::settings_view::workflow_settings_of(wf, &self.paths))
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
        crate::settings_view::write_workflow_value(wf, &self.paths, variable, value)?;
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
        let live = self.live.read().unwrap().clone();
        let (config, providers) = match &live {
            Some(l) => (&l.config, &l.providers),
            None => (&self.config, &self.providers),
        };
        let state = EngineState {
            config,
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
        for p in providers {
            let out = p.query(&ctx);
            if out.exclusive {
                let mut v = out.items;
                v.truncate(config.max_results);
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
        items.truncate(config.max_results);
        items.extend(trailing);
        items
    }

    /// Universal Actions available for a result (empty when it has none).
    pub fn actions_for(&self, item: &Item) -> Vec<Item> {
        actions::actions_for(item)
    }

    /// Placeholder rows (`runningsubtext`) for Script Filters that would run for
    /// `query`, cheap enough to show while the real, slow query is in flight.
    pub fn pending_items(&self, query: &str) -> Vec<Item> {
        let q = query.trim_start();
        if q.is_empty() {
            return Vec::new();
        }
        let wfs = self.workflows.read().unwrap();
        wfs.iter().flat_map(|w| w.pending_items(q, &self.paths)).collect()
    }

    /// Applications to offer for "Open With..." on `path`, filtered by `filter`.
    pub fn open_with_items(&self, path: &Path, filter: &str, limit: usize) -> Vec<Item> {
        actions::open_with_items(self.os.as_ref(), &self.config.app_dirs, path, filter, limit)
    }

    /// Start Quick Look on `paths` through the engine's [`Os`] and return the
    /// handle; `kill()` on it dismisses the preview.
    pub fn spawn_quick_look(&self, paths: &[PathBuf]) -> Result<Box<dyn actions::Process>> {
        actions::spawn_quick_look(self.os.as_ref(), paths)
    }

    // ------------------------------------------------------------ execution

    /// The action `item` runs for `mods` (the Cmd variant when Cmd is held).
    fn effective_action(item: &Item, mods: Mods) -> &Action {
        match (&item.cmd_action, mods.cmd) {
            (Some(a), true) => a,
            _ => &item.action,
        }
    }

    /// The question to ask before running `item` with `mods`, if its action is
    /// destructive. This is the only place that decides: [`Engine::execute`]
    /// asks it, and a UI that wants to prompt first (instead of calling
    /// `execute(.., false)` and handling `Outcome::Confirm`) must use it too,
    /// then call `execute(.., true)` once the user agrees.
    pub fn confirmation_for(&self, item: &Item, mods: Mods) -> Option<String> {
        if !item.valid {
            return None;
        }
        Engine::effective_action(item, mods).confirmation()
    }

    pub fn execute(&self, item: &Item, query: &str, mods: Mods, confirmed: bool) -> Outcome {
        if !item.valid {
            return Outcome::Done;
        }
        if !confirmed && let Some(prompt) = self.confirmation_for(item, mods) {
            return Outcome::Confirm(prompt);
        }
        let action = Engine::effective_action(item, mods).clone();
        let outcome = self.perform(&action, mods);
        // Universal Actions carry the text or path they act on in their id
        // (`act:copy:<text>`); recording that would write it, unbounded and in
        // plaintext, to usage.json. They are never ranked, so nothing is lost.
        // Clipboard rows are ordered by recency, not usage, and the typed query (`clip hunter2`)
        // may be a fragment of a secret: they are not learned either (SEC-19).
        let sensitive = |a: &Action| matches!(a, Action::Universal(_) | Action::Clipboard(_));
        let learnable = !sensitive(&action) && !sensitive(&item.action);
        if learnable
            && (outcome == Outcome::Done || matches!(outcome, Outcome::LargeType(_) | Outcome::ShowLauncher(_)))
        {
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
                Action::Clipboard(c) => match clipboard::command::run_with(&self.clipboard, c, &*self.clip_env) {
                    Outcome::Error(m) => bail!("{m}"),
                    Outcome::LargeType(t) => return Ok(Some(format!("{TAG_LARGE}{t}"))),
                    Outcome::Done | Outcome::Confirm(_) | Outcome::ShowLauncher(_) => {}
                },
                Action::Universal(u) => match actions::run(self.os.as_ref(), u) {
                    Outcome::LargeType(t) => return Ok(Some(format!("{TAG_LARGE}{t}"))),
                    Outcome::Error(e) => bail!("{e}"),
                    Outcome::Done | Outcome::Confirm(_) | Outcome::ShowLauncher(_) => {}
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

    /// Start an enabled workflow at one of its objects (a hotkey, say): `arg` and `vars` are
    /// what that object would have produced. Unlike [`Engine::execute`] this records nothing
    /// in the usage table.
    ///
    /// `workflow_id` is the workflow's directory identity (`Workflow::id`), not its bundle
    /// id: bundle ids may be empty or shared by copies in several roots.
    ///
    /// Callers must derive `node_uid` from an installed workflow's own objects (as the
    /// hotkey registry does); any object kind is accepted, so this must never be reachable
    /// with a uid taken from outside input (URL scheme, IPC, AppleScript).
    pub fn run_node(&self, workflow_id: &str, node_uid: &str, arg: &str, vars: BTreeMap<String, String>) -> Outcome {
        let effects = {
            let wfs = self.workflows.read().unwrap();
            let Some(wf) = wfs.iter().find(|w| w.id == workflow_id && !w.disabled) else {
                return Outcome::Error(format!("no enabled workflow '{workflow_id}'"));
            };
            if !wf.has_node(node_uid) {
                return Outcome::Error(format!("workflow '{}' has no object '{node_uid}'", wf.name));
            }
            let act = WorkflowAction {
                workflow: wf.id.clone(),
                from_uid: node_uid.to_string(),
                arg: arg.to_string(),
                vars,
                ..Default::default()
            };
            wf.run(&act, Mods::NONE, &self.paths)
        };
        msg_to_outcome(apply_effects(effects))
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
const TAG_SHOW: &str = "\u{1}show:";

fn msg_to_outcome(m: String) -> Outcome {
    if let Some(t) = m.strip_prefix(TAG_LARGE) {
        Outcome::LargeType(t.to_string())
    } else if let Some(e) = m.strip_prefix(TAG_ERR) {
        Outcome::Error(e.to_string())
    } else if let Some(q) = m.strip_prefix(TAG_SHOW) {
        Outcome::ShowLauncher(q.to_string())
    } else {
        Outcome::Done
    }
}

fn apply_effects(effects: Vec<Effect>) -> String {
    let mut large = None;
    let mut show = None;
    let mut errors = Vec::new();
    for fx in effects {
        let r: Result<()> = match fx {
            Effect::OpenUrl { url, browser } => open_url(&url, browser.as_deref()),
            Effect::OpenPath { path, with } => open_path(&path, with.as_deref()),
            Effect::Reveal(p) => reveal(&p),
            Effect::Copy(t) => copy_to_clipboard(&t),
            Effect::CopyTransient(t) => copy_to_clipboard_transient(&t),
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
            Effect::ShowLauncher { query } => {
                show = Some(query);
                Ok(())
            }
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
    } else if let Some(q) = show {
        format!("{TAG_SHOW}{q}")
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

/// A command for a macOS system tool, by absolute path (see `actions::system_program`).
fn sys_command(name: &str) -> Command {
    Command::new(actions::system_program(std::ffi::OsStr::new(name)))
}

pub fn open_path(p: &Path, with: Option<&str>) -> Result<()> {
    let s = p.to_string_lossy();
    no_dash(&s, "path")?;
    let mut c = sys_command("open");
    if let Some(app) = with {
        no_dash(app, "application")?;
        c.arg("-a").arg(app);
    }
    run_status(c.arg(&*s), "open")
}

pub fn reveal(p: &Path) -> Result<()> {
    no_dash(&p.to_string_lossy(), "path")?;
    run_status(sys_command("open").arg("-R").arg(p), "reveal")
}

pub fn open_url(url: &str, browser: Option<&str>) -> Result<()> {
    no_dash(url, "URL")?;
    let mut c = sys_command("open");
    if let Some(app) = browser {
        no_dash(app, "browser")?;
        c.arg("-a").arg(app);
    }
    run_status(c.arg(url), "open")
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = sys_command("pbcopy").stdin(Stdio::piped()).spawn()?;
    child.stdin.take().expect("piped stdin").write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("pbcopy failed");
    }
    Ok(())
}

/// The JXA that puts `argv[0]` on the general pasteboard as plain text together with the
/// transient marker type (<http://nspasteboard.org>), which every clipboard manager,
/// this one included, treats as "do not record".
fn transient_copy_script() -> String {
    format!(
        "ObjC.import('AppKit'); function run(argv) {{ var pb = $.NSPasteboard.generalPasteboard; \
         pb.clearContents; pb.setStringForType($(argv[0]), 'public.utf8-plain-text'); \
         pb.setDataForType($.NSData.data, '{}'); }}",
        crate::clipboard::filter::TRANSIENT_TYPE
    )
}

/// The text travels as an argument after `--`, never inside the script source.
fn transient_copy_command(text: &str) -> Command {
    let mut c = sys_command("osascript");
    c.args(["-l", "JavaScript", "-e"])
        .arg(transient_copy_script())
        .arg("--")
        .arg(text);
    c
}

/// Copy to Clipboard with the workflow object's "transient" box ticked.
pub fn copy_to_clipboard_transient(text: &str) -> Result<()> {
    run_status(&mut transient_copy_command(text), "osascript")
}

/// `osascript -e <line>... -- <arg>...`. The `--` is what makes the arguments data: without it a
/// first argument beginning with `-e` is parsed as one more script fragment (SEC-01).
fn osascript_command(lines: &[&str], args: &[&str]) -> Command {
    let mut c = sys_command("osascript");
    for l in lines {
        c.arg("-e").arg(l);
    }
    c.arg("--");
    c.args(args);
    c
}

fn osascript_args(lines: &[&str], args: &[&str]) -> Result<()> {
    run_status(&mut osascript_command(lines, args), "osascript")
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
    use std::time::Duration;

    #[test]
    fn a_transient_copy_carries_the_marker_and_the_text_only_as_an_argument() {
        let c = transient_copy_command("-e hunter2\"; do shell script \"x");
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(&args[..3], ["-l", "JavaScript", "-e"]);
        assert!(args[3].contains(crate::clipboard::filter::TRANSIENT_TYPE));
        assert!(!args[3].contains("hunter2"));
        assert_eq!(args[4], "--");
        assert_eq!(args[5], "-e hunter2\"; do shell script \"x");
        assert_eq!(args.len(), 6);
    }

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

    // ---- Os / home injection, confirmation, usage (FIX-A)

    use crate::actions::testing::{FakeOs, How};

    /// An engine whose Os is a fake and whose home is a temp folder: it cannot touch the machine.
    fn sandboxed(tag: &str) -> (Engine, Arc<FakeOs>, PathBuf) {
        let root = std::env::temp_dir()
            .join(format!("oa-eng-sbx-{tag}-{}", std::process::id()))
            .canonicalize_or_create();
        let fake = Arc::new(FakeOs::default());
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        let e = Engine::with_env(Paths::under(&root), config, Box::new(Arc::clone(&fake)), home);
        (e, fake, root)
    }

    trait CanonOrCreate {
        fn canonicalize_or_create(self) -> PathBuf;
    }

    impl CanonOrCreate for PathBuf {
        fn canonicalize_or_create(self) -> PathBuf {
            let _ = std::fs::remove_dir_all(&self);
            std::fs::create_dir_all(&self).unwrap();
            self.canonicalize().unwrap()
        }
    }

    #[test]
    fn move_to_trash_confirms_then_runs_through_the_injected_os() {
        let (e, fake, root) = sandboxed("trash");
        let f = root.join("files/report.pdf");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"x").unwrap();
        let rows = e.query(&format!("actions {}", f.display()));
        let trash = rows.iter().find(|i| i.title == "Move to Trash").expect("trash row");
        // the question, with the full path
        let ask = e.confirmation_for(trash, Mods::NONE).expect("destructive: must ask");
        assert!(
            ask.contains("\"report.pdf\"") && ask.contains(&f.display().to_string()),
            "{ask}"
        );
        // execute is the authority: unconfirmed gives the same question and runs nothing
        assert_eq!(e.execute(trash, "actions x", Mods::NONE, false), Outcome::Confirm(ask));
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        // confirmed: exactly one Finder script, on the file, through the fake
        assert_eq!(e.execute(trash, "actions x", Mods::NONE, true), Outcome::Done);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].0, How::Run);
        assert_eq!(
            calls[0].1,
            crate::actions::Cmd::osascript(actions::applescripts()[1].1, [&f])
        );
        assert!(f.exists(), "the fake deletes nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn non_destructive_actions_do_not_ask_and_invalid_items_never_do() {
        let (e, _fake, root) = sandboxed("noask");
        let open = Item::new("o", "o").action(Action::Open(PathBuf::from("/tmp")));
        assert_eq!(e.confirmation_for(&open, Mods::NONE), None);
        let large = Item::new("l", "l").action(Action::Universal(actions::UniversalAction::LargeType("x".into())));
        assert_eq!(e.confirmation_for(&large, Mods::NONE), None);
        let shutdown = e
            .query("shut down")
            .into_iter()
            .find(|i| i.title == "Shut Down")
            .unwrap();
        let ask = e.confirmation_for(&shutdown, Mods::NONE).expect("system command asks");
        assert!(ask.contains("Shut Down"), "{ask}");
        // the same string execute reports: one source of truth
        assert_eq!(
            e.execute(&shutdown, "shut down", Mods::NONE, false),
            Outcome::Confirm(ask)
        );
        let invalid = Item::new("i", "i")
            .action(Action::Universal(actions::UniversalAction::Trash(vec![
                "/tmp/x".into(),
            ])))
            .valid(false);
        assert_eq!(e.confirmation_for(&invalid, Mods::NONE), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_cmd_variant_is_what_gets_confirmed() {
        let (e, fake, root) = sandboxed("cmdvariant");
        let f = root.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut it = Item::new("file:a", "a.txt").action(Action::Open(f.clone()));
        it.cmd_action = Some(Action::Universal(actions::UniversalAction::Trash(vec![f.clone()])));
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        assert_eq!(e.confirmation_for(&it, Mods::NONE), None, "plain Return just opens it");
        assert!(e.confirmation_for(&it, cmd).is_some(), "Cmd+Return trashes: ask");
        assert!(matches!(e.execute(&it, "a", cmd, false), Outcome::Confirm(_)));
        assert!(fake.calls().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn universal_actions_are_never_written_to_usage() {
        let (e, _fake, root) = sandboxed("usage");
        let secret = "correct horse battery staple (copied from the clipboard history)";
        let rows = e.query(&format!("actions {secret}"));
        let copy = rows.iter().find(|i| i.title == "Copy").unwrap();
        let large = rows.iter().find(|i| i.title == "Large Type").unwrap();
        assert_eq!(e.execute(copy, "actions x", Mods::NONE, false), Outcome::Done);
        assert!(matches!(
            e.execute(large, "actions x", Mods::NONE, false),
            Outcome::LargeType(_)
        ));
        assert_eq!(e.usage.lock().unwrap().boost("actions x", &copy.id), 0.0);
        let on_disk = std::fs::read_to_string(e.paths.usage_file()).unwrap_or_default();
        assert!(!on_disk.contains("horse") && !on_disk.contains("act:"), "{on_disk}");
        // ...while an ordinary result is still learned
        let open = Item::new("file:/tmp/x", "x").action(Action::Open(PathBuf::from("/tmp/x")));
        let _ = e.execute(&open, "x", Mods::NONE, true);
        assert!(e.paths.usage_file().exists() || e.usage.lock().unwrap().boost("x", "file:/tmp/x") > 0.0);
        // a Cmd variant that is Universal is not recorded either
        let mut it = Item::new("file:/tmp/y", "y").action(Action::Open(PathBuf::from("/tmp/y")));
        it.cmd_action = Some(Action::Universal(actions::UniversalAction::LargeType("zzz".into())));
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        let before = e.usage.lock().unwrap().boost("y", "file:/tmp/y");
        let _ = e.execute(&it, "y", cmd, true);
        assert_eq!(e.usage.lock().unwrap().boost("y", "file:/tmp/y"), before);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn injected_home_decides_which_bookmarks_and_which_files_are_visible() {
        let root = std::env::temp_dir()
            .join(format!("oa-eng-home-{}", std::process::id()))
            .canonicalize_or_create();
        let home = root.join("home");
        let chrome = home.join("Library/Application Support/Google/Chrome/Default");
        std::fs::create_dir_all(&chrome).unwrap();
        std::fs::write(
            chrome.join("Bookmarks"),
            r#"{"roots":{"bookmark_bar":{"type":"folder","name":"bar","children":[
                {"type":"url","name":"Synthetic Fixture Site","url":"https://fixture.example/"}]}}}"#,
        )
        .unwrap();
        let fake = Arc::new(FakeOs::default());
        let config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        let e = Engine::with_env(Paths::under(&root), config, Box::new(Arc::clone(&fake)), home.clone());
        // bookmarks come from the injected home (loaded in the background)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let found = loop {
            let r = e.query("bm synthetic");
            if r.iter().any(|i| i.title == "Synthetic Fixture Site") || std::time::Instant::now() > deadline {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(found.iter().any(|i| i.title == "Synthetic Fixture Site"), "{found:?}");
        // file search is scoped to that home and goes through the fake Os, not the real mdfind
        let _ = e.query("find anything");
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].1.program, "mdfind");
        assert!(
            calls[0].1.args.iter().any(|a| *a == home.as_os_str()),
            "{:?}",
            calls[0].1.args
        );
        assert!(
            !calls[0]
                .1
                .args
                .iter()
                .any(|a| *a == dirs::home_dir().unwrap().as_os_str())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plain_engine_new_in_tests_gets_a_sandboxed_home() {
        // unit tests must never read real browser data or search the real home
        assert!(!providers::ambient_home().exists());
    }

    #[test]
    fn osascript_arguments_follow_a_double_dash() {
        for hostile in ["-eproperty zz : 1", "-e", "--", "-l", "x"] {
            let c = osascript_command(&["on run argv", "end run"], &[hostile, "t"]);
            let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            let dd = args.iter().position(|a| a == "--").expect("no -- separator");
            // every `-e` option comes before the separator, every user string after it
            assert!(args[..dd].iter().step_by(2).all(|a| a == "-e"), "{args:?}");
            assert_eq!(&args[dd + 1..], [hostile, "t"], "{args:?}");
        }
    }

    // ------------------------------------------------------------ wiring (FIX-C)

    use crate::clipboard::ClipCommand;
    use crate::clipboard::command::ClipEnv;
    use crate::clipboard::paste::KeySender;

    /// Records what the clipboard actions would do to the real pasteboard / keyboard.
    #[derive(Default)]
    struct FakeClip {
        trusted: bool,
        log: Mutex<Vec<String>>,
    }

    impl KeySender for FakeClip {
        fn trusted(&self) -> bool {
            self.trusted
        }
        fn tap(&self, keycode: u16, command: bool) -> Result<(), String> {
            self.log
                .lock()
                .unwrap()
                .push(format!("key{keycode}{}", if command { "+cmd" } else { "" }));
            Ok(())
        }
        fn pause(&self, _: Duration) {}
    }

    impl ClipEnv for FakeClip {
        fn restore(&self, _: &ClipboardHistory, e: &crate::clipboard::ClipEntry) -> Result<()> {
            self.log.lock().unwrap().push(format!("restore:{:?}", e.kind));
            Ok(())
        }
        fn copy_text(&self, t: &str) -> Result<()> {
            self.log.lock().unwrap().push(format!("copy:{t}"));
            Ok(())
        }
        fn keys(&self) -> &dyn KeySender {
            self
        }
        fn settle(&self) -> Duration {
            Duration::ZERO
        }
    }

    struct Shared(Arc<FakeClip>);
    impl KeySender for Shared {
        fn trusted(&self) -> bool {
            self.0.trusted()
        }
        fn tap(&self, k: u16, c: bool) -> Result<(), String> {
            self.0.tap(k, c)
        }
        fn pause(&self, d: Duration) {
            self.0.pause(d)
        }
    }

    impl ClipEnv for Shared {
        fn restore(&self, h: &ClipboardHistory, e: &crate::clipboard::ClipEntry) -> Result<()> {
            self.0.restore(h, e)
        }
        fn copy_text(&self, t: &str) -> Result<()> {
            self.0.copy_text(t)
        }
        fn keys(&self) -> &dyn KeySender {
            self
        }
        fn settle(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn engine_with_clip(tag: &str, trusted: bool) -> (Engine, Arc<FakeClip>, PathBuf) {
        let (e, root) = engine(tag);
        let fake = Arc::new(FakeClip {
            trusted,
            ..Default::default()
        });
        (e.with_clip_env(Box::new(Shared(Arc::clone(&fake)))), fake, root)
    }

    #[test]
    fn return_on_a_clip_entry_pastes_it() {
        let (e, fake, root) = engine_with_clip("clip-paste", true);
        e.clipboard().lock().unwrap().push("pasted text");
        let it = e.query("clip pasted").remove(0);
        assert_eq!(e.execute(&it, "clip pasted", Mods::NONE, false), Outcome::Done);
        assert_eq!(*fake.log.lock().unwrap(), vec!["restore:Text", "key9+cmd"]);
        // Cmd+Return copies without pasting
        fake.log.lock().unwrap().clear();
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        assert_eq!(e.execute(&it, "clip pasted", cmd, false), Outcome::Done);
        assert_eq!(*fake.log.lock().unwrap(), vec!["restore:Text"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn paste_without_accessibility_still_copies_and_explains() {
        let (e, fake, root) = engine_with_clip("clip-noax", false);
        e.clipboard().lock().unwrap().push("secret-free text");
        let it = e.query("clip secret").remove(0);
        let Outcome::Error(msg) = e.execute(&it, "clip secret", Mods::NONE, false) else {
            panic!("expected the Accessibility hint")
        };
        assert!(msg.starts_with("Copied."), "{msg}");
        assert!(msg.contains("Accessibility"), "{msg}");
        assert_eq!(
            *fake.log.lock().unwrap(),
            vec!["restore:Text"],
            "copied, no key presses"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn snippets_paste_with_cursor_placement() {
        let (e, fake, root) = engine_with_clip("snip-paste", true);
        std::fs::create_dir_all(&e.paths.data).unwrap();
        std::fs::write(
            e.paths.snippets_file(),
            r#"[{"name":"Greeting","keyword":"hi","text":"Dear {cursor},"}]"#,
        )
        .unwrap();
        e.reload_blocking();
        let it = e.query("snip hi").remove(0);
        assert_eq!(e.execute(&it, "snip hi", Mods::NONE, false), Outcome::Done);
        // "Dear ," with the caret one character from the end: copy, Cmd+V, Left
        assert_eq!(*fake.log.lock().unwrap(), vec!["copy:Dear ,", "key9+cmd", "key123"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clearing_the_history_needs_confirmation_and_keeps_pins() {
        let (e, _fake, root) = engine_with_clip("clip-clear", true);
        {
            let c = e.clipboard();
            let mut h = c.lock().unwrap();
            h.push("keep me");
            h.push("drop me");
            let id = h.entries().iter().find(|x| x.text == "keep me").unwrap().id();
            h.set_pinned(&id, true);
        }
        let row = e.query("clip clear").remove(0);
        assert_eq!(row.action, Action::Clipboard(ClipCommand::Clear));
        assert!(matches!(
            e.execute(&row, "clip clear", Mods::NONE, false),
            Outcome::Confirm(_)
        ));
        assert_eq!(e.clipboard().lock().unwrap().len(), 2, "nothing happened yet");
        assert_eq!(e.execute(&row, "clip clear", Mods::NONE, true), Outcome::Done);
        let c = e.clipboard();
        let h = c.lock().unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h.entries()[0].text, "keep me");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_vanished_clip_entry_is_reported() {
        let (e, _fake, root) = engine_with_clip("clip-gone", true);
        let it = Item::new("clip:x", "x").action(Action::Clipboard(ClipCommand::Paste { id: "nope".into() }));
        assert!(matches!(e.execute(&it, "", Mods::NONE, true), Outcome::Error(_)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clipboard_rows_and_their_queries_are_not_written_to_usage() {
        // SEC-19: `clip <fragment of a secret>` must not land in usage.json.
        let (e, _fake, root) = engine_with_clip("clip-usage", true);
        e.clipboard().lock().unwrap().push("hunter2-correct-horse");
        let q = "clip hunter2-corr";
        let it = e.query(q).remove(0);
        assert_eq!(e.execute(&it, q, Mods::NONE, false), Outcome::Done);
        assert_eq!(e.usage.lock().unwrap().boost(q, &it.id), 0.0);
        let on_disk = std::fs::read_to_string(e.paths.usage_file()).unwrap_or_default();
        assert!(!on_disk.contains("hunter2") && !on_disk.contains("clip:"), "{on_disk}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn disabled_providers_do_not_answer() {
        let root = std::env::temp_dir().join(format!("oa-eng-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("apps");
        std::fs::create_dir_all(apps.join("Safari.app")).unwrap();
        let mk = |disabled: &[&str]| {
            Engine::new(
                Paths::under(&root),
                Config {
                    app_dirs: vec![apps.clone()],
                    providers_disabled: disabled.iter().map(|s| s.to_string()).collect(),
                    ..Config::default()
                },
            )
        };
        let on = mk(&[]);
        assert_eq!(on.query("2+3*4")[0].title, "14");
        assert_eq!(on.query("saf")[0].title, "Safari");
        let off = mk(&["calc", "apps"]);
        assert!(off.query("2+3*4").iter().all(|i| i.title != "14"));
        assert!(off.query("saf").iter().all(|i| i.title != "Safari"));
        // the web fallbacks are a different provider and still close the list
        assert!(off.query("saf").last().unwrap().title.starts_with("Search "));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clipboard_settings_from_the_config_are_applied() {
        let root = std::env::temp_dir().join(format!("oa-eng-clipcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = Config {
            app_dirs: vec![root.join("apps")],
            ..Config::default()
        };
        config.clipboard.max_entries = 3;
        config.clipboard.max_age_days = Some(7);
        config.clipboard.ignored_apps = vec!["com.example.vault".into()];
        config.clipboard.max_image_bytes = 1234;
        let e = Engine::new(Paths::under(&root), config);
        let c = e.clipboard();
        let mut h = c.lock().unwrap();
        assert_eq!(h.ignored_apps(), vec!["com.example.vault"]);
        assert_eq!(h.max_image_bytes(), 1234);
        for i in 0..6 {
            h.push(&format!("entry {i}"));
        }
        assert_eq!(h.len(), 3, "max_entries applies");
        drop(h);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_search_and_bookmark_settings_reach_the_providers() {
        // Not observable without touching Spotlight: the conversion is covered in config tests; here only
        // check an engine builds with a custom scope and still answers ordinary queries.
        let root = std::env::temp_dir().join(format!("oa-eng-fscfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("apps");
        std::fs::create_dir_all(apps.join("Safari.app")).unwrap();
        let mut config = Config {
            app_dirs: vec![apps],
            ..Config::default()
        };
        config.file_search.roots = vec![root.join("docs").display().to_string()];
        config.bookmarks.enabled_sources = vec!["safari".into()];
        config.bookmarks.include_in_general_search = false;
        let e = Engine::new(Paths::under(&root), config);
        assert_eq!(e.query("saf")[0].title, "Safari");
        let _ = std::fs::remove_dir_all(root);
    }

    // ---- workflow settings and reload

    fn write_config_workflow(e: &Engine) {
        let dir = e.paths.workflows_dir().join("user.workflow.cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({
            "name": "Cfg", "bundleid": "cfg.test",
            "variables": {"fixed": "f"},
            "userconfigurationconfig": [
                {"variable": "name", "label": "Name", "type": "textfield", "config": {"default": "Ann"}},
                {"variable": "loud", "type": "checkbox", "config": {"default": false}}
            ],
            "objects": [
                {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {
                    "keyword": "who", "withspace": false, "argumenttype": 2, "text": "Hello {var:name}", "subtext": "loud={var:loud}"}}
            ],
            "connections": {}
        });
        json_to_plist(&doc).to_file_xml(dir.join("info.plist")).unwrap();
        e.reload_blocking();
    }

    #[test]
    fn workflow_settings_default_set_reset_and_validation() {
        let (e, root) = engine("wfcfg");
        write_config_workflow(&e);
        let s = e.workflow_settings("cfg.test").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].variable.as_str(), s[0].value.as_str(), s[0].is_default),
            ("name", "Ann", true)
        );
        assert_eq!((s[1].value.as_str(), s[1].default.as_str()), ("0", "0"));
        assert_eq!(e.query("who")[0].title, "Hello Ann");
        e.set_workflow_setting("cfg.test", "name", "Bob").unwrap();
        let s = e.workflow_settings("cfg.test").unwrap();
        assert_eq!((s[0].value.as_str(), s[0].is_default), ("Bob", false));
        assert_eq!(
            e.query("who")[0].title,
            "Hello Bob",
            "the running workflow sees the new value at once"
        );
        let file = e.paths.workflow_config_dir().join("cfg.test.json");
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved, json!({"name": "Bob"}));
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // it survives a reload and a brand-new engine on the same data dir
        e.reload_blocking();
        assert_eq!(e.query("who")[0].title, "Hello Bob");
        e.set_workflow_setting("cfg.test", "loud", "1").unwrap();
        e.reset_workflow_setting("cfg.test", "name").unwrap();
        let s = e.workflow_settings("cfg.test").unwrap();
        assert_eq!((s[0].value.as_str(), s[0].is_default), ("Ann", true));
        assert_eq!(s[1].value, "1");
        // bad requests
        assert!(e.set_workflow_setting("cfg.test", "nope", "x").is_err());
        assert!(e.set_workflow_setting("missing.bundle", "name", "x").is_err());
        assert!(e.set_workflow_setting("", "name", "x").is_err());
        assert!(e.workflow_settings("missing.bundle").is_none());
        assert!(e.workflow_settings("").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_workflow_settings_file_is_replaced_not_fatal() {
        let (e, root) = engine("wfcfg-bad");
        write_config_workflow(&e);
        std::fs::create_dir_all(e.paths.workflow_config_dir()).unwrap();
        std::fs::write(e.paths.workflow_config_dir().join("cfg.test.json"), b"{ nope").unwrap();
        e.reload_blocking();
        assert_eq!(e.workflow_settings("cfg.test").unwrap()[0].value, "Ann");
        e.set_workflow_setting("cfg.test", "name", "Cy").unwrap();
        assert_eq!(e.workflow_settings("cfg.test").unwrap()[0].value, "Cy");
        let _ = std::fs::remove_dir_all(root);
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn apply_config_with_new_workflow_dirs_does_not_scan_on_the_calling_thread() {
        let (e, root) = engine("apply-bg");
        let extra = root.join("extra-workflows");
        std::fs::create_dir_all(extra.join("copy")).unwrap();
        let doc = json!({"name": "Extra", "bundleid": "extra.b", "objects": [], "connections": {}});
        json_to_plist(&doc)
            .to_file_xml(extra.join("copy").join("info.plist"))
            .unwrap();
        let e = Arc::new(e);
        // Hold the workflow list so a synchronous scan could never finish its swap.
        let guard = e.workflows.write().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(&e);
        std::thread::spawn(move || {
            let mut c = worker.config_now();
            c.workflow_dirs = vec![extra];
            worker.apply_config(c);
            let _ = tx.send(());
        });
        let returned = rx.recv_timeout(Duration::from_secs(5)).is_ok();
        drop(guard);
        assert!(returned, "apply_config waited for the workflow rescan");
        assert!(e.reload_pending() || e.workflow_summaries().iter().any(|(n, _, _)| n == "Extra"));
        wait_until("the background refresh", || !e.reload_pending());
        assert!(e.workflow_summaries().iter().any(|(n, _, _)| n == "Extra"));
        // removing the folder drops its workflows again
        let mut c = e.config_now();
        c.workflow_dirs.clear();
        e.apply_config(c);
        wait_until("the second refresh", || {
            !e.reload_pending() && !e.workflow_summaries().iter().any(|(n, _, _)| n == "Extra")
        });
        let _ = std::fs::remove_dir_all(root);
    }

    /// `~/...` in `app_dirs` and `workflow_dirs` is this user's home folder when the settings are
    /// used (docs/SETTINGS.md), not a folder literally named `~`.
    #[test]
    fn tilde_paths_in_the_settings_file_are_used_as_home_paths() {
        let root = std::env::temp_dir().join(format!("oa-eng-tilde-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join("Apps").join("Zedword.app")).unwrap();
        std::fs::create_dir_all(home.join("dotfiles/workflows/copy")).unwrap();
        let doc = json!({"name": "Dotfile Flow", "bundleid": "dot.flow", "objects": [], "connections": {}});
        json_to_plist(&doc)
            .to_file_xml(home.join("dotfiles/workflows/copy/info.plist"))
            .unwrap();
        let paths = Paths::under(&root);
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(
            paths.config_file(),
            json!({"app_dirs": ["~/Apps"], "workflow_dirs": ["~/dotfiles/workflows"]}).to_string(),
        )
        .unwrap();
        let config = Config::load_in(&paths, &home);
        let e = Engine::new(paths, config);
        assert_eq!(e.query("zedword").first().map(|i| i.title.as_str()), Some("Zedword"));
        wait_until("the workflow scan", || {
            e.workflow_summaries().iter().any(|(n, _, _)| n == "Dotfile Flow")
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn start_up_remembers_what_was_wrong_with_the_configuration() {
        let (e0, root) = engine("startup-ok");
        assert!(e0.startup_warnings().is_empty());
        let mut config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        config.web_searches.push(crate::websearch::WebSearch {
            keyword: "zork".into(),
            title: "Zork".into(),
            url: "file:///etc/{query}".into(),
            fallback: false,
        });
        let e = Engine::new(Paths::under(&root.join("other")), config);
        assert_eq!(e.startup_warnings().len(), 1);
        assert!(e.startup_warnings()[0].contains("zork"), "{:?}", e.startup_warnings());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_panic_in_the_content_refresh_does_not_leave_reload_pending_stuck() {
        let (e, root) = engine("content-panic");
        write_config_workflow(&e);
        e.content_gate.lock().unwrap().panic_once = true;
        e.reload_content_background();
        wait_until("the failed refresh to release the gate", || !e.reload_pending());
        // the gate works again: the next refresh runs and picks up a change
        let plist = e.paths.workflows_dir().join("user.workflow.cfg").join("info.plist");
        let text = std::fs::read_to_string(&plist)
            .unwrap()
            .replace("Hello {var:name}", "Howdy {var:name}");
        std::fs::write(&plist, text).unwrap();
        e.reload_content_background();
        wait_until("the next refresh", || {
            !e.reload_pending() && e.query("who")[0].title == "Howdy Ann"
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_panic_in_the_full_reload_does_not_leave_reload_pending_stuck() {
        let (e, root) = engine("reload-panic");
        e.reload_gate.lock().unwrap().panic_once = true;
        e.reload();
        wait_until("the failed reload to release the gate", || !e.reload_pending());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reload_returns_immediately_and_refreshes_in_the_background() {
        let (e, root) = engine("reload-bg");
        write_config_workflow(&e);
        assert_eq!(e.query("who")[0].title, "Hello Ann");
        // change the workflow on disk, then reload without waiting
        let plist = e.paths.workflows_dir().join("user.workflow.cfg").join("info.plist");
        let text = std::fs::read_to_string(&plist)
            .unwrap()
            .replace("Hello {var:name}", "Howdy {var:name}");
        std::fs::write(&plist, text).unwrap();
        e.reload();
        wait_until("the background reload", || e.query("who")[0].title == "Howdy Ann");
        wait_until("the reload worker to finish", || !e.reload_pending());
        // queries during a reload never see an empty workflow list: hammer it while reloading
        let e = Arc::new(e);
        let reloader = {
            let e = Arc::clone(&e);
            std::thread::spawn(move || {
                for _ in 0..6 {
                    e.reload();
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        for _ in 0..100 {
            assert_eq!(e.query("who")[0].title, "Howdy Ann");
        }
        reloader.join().unwrap();
        wait_until("the reload worker to finish", || !e.reload_pending());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reload_blocking_has_finished_when_it_returns() {
        let (e, root) = engine("reload-block");
        assert!(e.workflow_summaries().is_empty());
        write_config_workflow(&e);
        assert_eq!(e.workflow_summaries().len(), 1);
        assert!(!e.reload_pending());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reload_clears_workflow_session_state() {
        let (e, root) = engine("reload-session");
        write_config_workflow(&e);
        // a value saved behind the engine's back is picked up by a reload (the cache is dropped)
        assert_eq!(e.workflow_settings("cfg.test").unwrap()[0].value, "Ann");
        std::fs::create_dir_all(e.paths.workflow_config_dir()).unwrap();
        std::fs::write(
            e.paths.workflow_config_dir().join("cfg.test.json"),
            br#"{"name":"Dee"}"#,
        )
        .unwrap();
        assert_eq!(
            e.workflow_settings("cfg.test").unwrap()[0].value,
            "Ann",
            "cached until reloaded"
        );
        e.reload_blocking();
        assert_eq!(e.workflow_settings("cfg.test").unwrap()[0].value, "Dee");
        let _ = std::fs::remove_dir_all(root);
    }
}
