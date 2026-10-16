//! Read and write any setting by dotted path (`clipboard.max_entries`,
//! `bookmarks.enabled_sources`, `file_search.roots`).
//!
//! The configuration is viewed as JSON. A change is applied to that view and
//! then pushed back through [`Config`]'s deserialiser, so a value of the
//! wrong type is rejected with the same rules the file itself is held to, and
//! a key that does not exist is an error instead of being silently dropped
//! (serde ignores unknown keys when loading).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;

use crate::clipboard::snippets::{Collection, Snippet, collections_dir, safe_file_stem, save_collection};
use crate::config::{Config, Paths, write_private};
use crate::theme::Theme;
use crate::workflow::{self, TrustSummary, Workflow};

fn view(config: &Config) -> Value {
    serde_json::to_value(config).unwrap_or(Value::Null)
}

fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn lookup_mut<'a>(root: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get_mut(part)?,
            Value::Array(a) => a.get_mut(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Every settable key (leaf path), e.g. for `ashdash config get` listings and
/// completion. Arrays and `null` options are leaves; objects are descended into.
pub fn keys(config: &Config) -> Vec<String> {
    fn walk(prefix: &str, v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&p, v, out);
                }
            }
            _ => out.push(prefix.to_string()),
        }
    }
    let mut out = Vec::new();
    walk("", &view(config), &mut out);
    out.sort();
    out
}

/// The value at `path` (empty path = the whole configuration).
pub fn get(config: &Config, path: &str) -> Result<Value> {
    let v = view(config);
    lookup(&v, path)
        .cloned()
        .with_context(|| format!("no setting '{path}' (see `ashdash config get`)"))
}

/// Interpret user text as a value for a setting that currently holds `current`.
/// Text that parses as JSON is taken as JSON (`42`, `true`, `null`, `["a","b"]`,
/// `"quoted"`). Otherwise it is a string, or for a list setting a
/// comma-separated list of strings.
fn parse_value(raw: &str, current: &Value) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
        // A bare number for a string setting (`theme 42`) stays a string.
        if !(v.is_number() || v.is_boolean()) || !current.is_string() {
            return v;
        }
    }
    if current.is_array() {
        let parts: Vec<Value> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect();
        return Value::Array(parts);
    }
    Value::String(raw.to_string())
}

/// Set `path` to `raw` and return the validated configuration. `config` is
/// only modified when the change is valid.
pub fn set(config: &mut Config, path: &str, raw: &str) -> Result<()> {
    if path.is_empty() {
        bail!("a setting name is required");
    }
    let mut doc = view(config);
    let current = lookup(&doc, path)
        .cloned()
        .with_context(|| format!("no setting '{path}' (see `ashdash config get`)"))?;
    if current.is_object() {
        bail!("'{path}' is a group of settings; set one of its keys");
    }
    let new = parse_value(raw, &current);
    *lookup_mut(&mut doc, path).context("setting vanished")? = new.clone();
    let updated: Config = serde_json::from_value(doc).with_context(|| format!("invalid value for '{path}': {raw}"))?;
    // Round trip: what was stored must be what comes back (nothing clamped or dropped).
    if lookup(&view(&updated), path) != Some(&new) {
        bail!("invalid value for '{path}': {raw}");
    }
    // A web search the engine would skip is refused here, not dropped later: only entries that
    // are newly bad count, so an old bad entry never blocks unrelated edits or its own repair.
    let was_bad = crate::websearch::checked(&config.web_searches).1.len();
    let (_, now_bad) = crate::websearch::checked(&updated.web_searches);
    if now_bad.len() > was_bad {
        bail!("{}", now_bad.join("; "));
    }
    *config = updated;
    Ok(())
}

// ------------------------------------------------------------------ what changed

/// Top-level setting names whose values differ between two configurations
/// (sorted). The window uses it to decide what to re-apply (`hotkey`, `theme`).
pub fn changed_keys(old: &Config, new: &Config) -> Vec<String> {
    let (a, b) = (view(old), view(new));
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = a
        .keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    out.sort();
    out
}

// ------------------------------------------------------------------ file watching

/// What a file-system event means for the running engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    /// `config.json` was written (by hand, by `ashdash config set`, by the preferences window).
    Config,
    /// Workflows, workflow settings or snippets changed on disk.
    Content,
}

/// Quiet period after the last event before a batch is reported.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// A steady stream of events cannot postpone a report longer than this.
const WATCH_MAX_DELAY: Duration = Duration::from_secs(3);

/// The (canonical) locations that matter, for [`classify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPlan {
    data: PathBuf,
    config_file: PathBuf,
    snippets_file: PathBuf,
    snippets_dir: PathBuf,
    workflow_config_dir: PathBuf,
    workflow_roots: Vec<PathBuf>,
}

impl WatchPlan {
    /// `extra_roots` are the `workflow_dirs` of the configuration. Paths are canonicalised where
    /// they exist (FSEvents reports real paths, while a temp dir is usually a symlink).
    pub fn new(paths: &Paths, extra_roots: &[PathBuf]) -> WatchPlan {
        let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
        let data = canon(paths.data.clone());
        let under = |p: &Path| match p.strip_prefix(&paths.data) {
            Ok(rel) => data.join(rel),
            Err(_) => p.to_path_buf(),
        };
        let mut roots = vec![under(&paths.workflows_dir())];
        roots.extend(extra_roots.iter().cloned().map(canon));
        WatchPlan {
            config_file: under(&paths.config_file()),
            snippets_file: under(&paths.snippets_file()),
            snippets_dir: collections_dir(&under(&paths.snippets_file())),
            workflow_config_dir: under(&paths.workflow_config_dir()),
            workflow_roots: roots,
            data,
        }
    }
}

/// Decide what, if anything, a changed path means. Everything else in the data folder
/// (clipboard history, usage, temp files, caches a workflow writes into its own folder) is
/// ignored, so ordinary use never triggers a reload.
pub fn classify(plan: &WatchPlan, path: &Path) -> Option<Change> {
    if path == plan.config_file {
        return Some(Change::Config);
    }
    if path == plan.snippets_file {
        return Some(Change::Content);
    }
    let is_json = path.extension().is_some_and(|e| e == "json");
    if (path.starts_with(&plan.snippets_dir) || path.starts_with(&plan.workflow_config_dir)) && is_json {
        return Some(Change::Content);
    }
    for root in &plan.workflow_roots {
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let parts: Vec<_> = rel.components().collect();
        let hidden = |c: &std::path::Component| c.as_os_str().to_string_lossy().starts_with('.');
        return match parts.as_slice() {
            // a workflow folder appeared or vanished (import, removal, drag in Finder)
            [dir] if !hidden(dir) => Some(Change::Content),
            // its definition changed (enable/disable, hand edit)
            [dir, file] if !hidden(dir) && file.as_os_str() == "info.plist" => Some(Change::Content),
            _ => None,
        };
    }
    None
}

enum WatchMsg {
    Path(PathBuf),
    /// The watcher lost track of events: assume everything changed.
    Everything,
    Stop,
}

/// Collects raw events until things go quiet and reports the distinct [`Change`]s.
fn debounce_loop(
    rx: mpsc::Receiver<WatchMsg>,
    plan: Arc<Mutex<WatchPlan>>,
    quiet: Duration,
    report: impl Fn(BTreeSet<Change>),
) {
    loop {
        let mut pending = BTreeSet::new();
        let absorb = |m: WatchMsg, pending: &mut BTreeSet<Change>| -> bool {
            match m {
                WatchMsg::Stop => return false,
                WatchMsg::Everything => {
                    pending.insert(Change::Config);
                    pending.insert(Change::Content);
                }
                WatchMsg::Path(p) => {
                    if let Some(c) = classify(&plan.lock().unwrap(), &p) {
                        pending.insert(c);
                    }
                }
            }
            true
        };
        match rx.recv() {
            Ok(m) => {
                if !absorb(m, &mut pending) {
                    return;
                }
            }
            Err(_) => return,
        }
        let started = Instant::now();
        loop {
            match rx.recv_timeout(quiet) {
                Ok(m) => {
                    if !absorb(m, &mut pending) {
                        return;
                    }
                    if started.elapsed() > WATCH_MAX_DELAY {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if !pending.is_empty() {
            report(pending);
        }
    }
}

/// Watches `config.json`, snippets and the workflow folders and reports debounced
/// [`Change`]s from a background thread. Dropping it stops everything.
pub struct SettingsWatcher {
    watcher: Mutex<RecommendedWatcher>,
    plan: Arc<Mutex<WatchPlan>>,
    tx: Sender<WatchMsg>,
    watched_roots: Mutex<Vec<PathBuf>>,
}

impl SettingsWatcher {
    /// Start watching. The folders are created (private) when missing so that creation of the
    /// first workflow or snippet file is seen too. `report` runs on the watcher's thread.
    pub fn start(
        paths: &Paths,
        extra_roots: &[PathBuf],
        quiet: Duration,
        report: impl Fn(BTreeSet<Change>) + Send + 'static,
    ) -> Result<SettingsWatcher> {
        use std::os::unix::fs::DirBuilderExt;
        for d in [
            &paths.data,
            &paths.workflows_dir(),
            &paths.workflow_config_dir(),
            &crate::clipboard::snippets::collections_dir(&paths.snippets_file()),
        ] {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d)?;
        }
        let plan = Arc::new(Mutex::new(WatchPlan::new(paths, extra_roots)));
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => {}
            Ok(ev) => {
                for p in ev.paths {
                    let _ = events.send(WatchMsg::Path(p));
                }
            }
            Err(_) => {
                let _ = events.send(WatchMsg::Everything);
            }
        })
        .context("starting the file watcher")?;
        {
            let plan = plan.lock().unwrap();
            // data dir: only config.json / snippets.json matter, but they are replaced by rename
            watcher.watch(&plan.data, RecursiveMode::NonRecursive)?;
            watcher.watch(&plan.snippets_dir, RecursiveMode::NonRecursive)?;
            watcher.watch(&plan.workflow_config_dir, RecursiveMode::NonRecursive)?;
        }
        let me = SettingsWatcher {
            watcher: Mutex::new(watcher),
            plan: Arc::clone(&plan),
            tx,
            watched_roots: Mutex::new(Vec::new()),
        };
        me.watch_roots();
        std::thread::Builder::new()
            .name("ashdash-settings-watch".into())
            .spawn(move || debounce_loop(rx, plan, quiet, report))
            .context("starting the watcher thread")?;
        Ok(me)
    }

    /// Watch the workflow folders of the plan; folders that do not exist (yet) are skipped.
    fn watch_roots(&self) {
        let roots = self.plan.lock().unwrap().workflow_roots.clone();
        let mut w = self.watcher.lock().unwrap();
        let mut watched = self.watched_roots.lock().unwrap();
        for old in watched.drain(..) {
            let _ = w.unwatch(&old);
        }
        for r in roots {
            if r.is_dir() && w.watch(&r, RecursiveMode::Recursive).is_ok() {
                watched.push(r);
            }
        }
    }

    /// The `workflow_dirs` setting changed.
    pub fn set_extra_roots(&self, paths: &Paths, extra_roots: &[PathBuf]) {
        *self.plan.lock().unwrap() = WatchPlan::new(paths, extra_roots);
        self.watch_roots();
    }
}

impl Drop for SettingsWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(WatchMsg::Stop);
    }
}

// ------------------------------------------------------------------ workflows on disk

/// An installed workflow as the preferences list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowInfo {
    /// Directory name.
    pub id: String,
    pub dir: PathBuf,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub disabled: bool,
    /// Lives in Ashdash's own workflows folder, so it can be disabled and removed here. Workflows
    /// from `workflow_dirs` (for example a reference launcher folder) are only listed: they belong to
    /// whoever owns that folder.
    pub managed: bool,
    /// Entries of its configuration sheet.
    pub settings: usize,
    pub trust: TrustSummary,
}

/// Every workflow found on disk (own folder first, then `extra_roots`), read fresh.
pub fn list_workflows(paths: &Paths, extra_roots: &[PathBuf]) -> Vec<WorkflowInfo> {
    let own = paths.workflows_dir();
    let mut roots = vec![own.clone()];
    roots.extend(extra_roots.iter().cloned());
    workflow::load_all(&roots)
        .into_iter()
        .map(|w| WorkflowInfo {
            managed: w.dir.parent() == Some(own.as_path()),
            settings: w.user_config.len(),
            trust: w.trust_summary(),
            id: w.id,
            dir: w.dir,
            name: w.name,
            bundle_id: w.bundle_id,
            version: w.version,
            disabled: w.disabled,
        })
        .collect()
}

/// `dir` must be a real (non-symlink) folder directly inside Ashdash's workflows folder.
fn owned_workflow_dir(paths: &Paths, dir: &Path) -> Result<PathBuf> {
    let root = std::fs::canonicalize(paths.workflows_dir()).context("no workflows folder")?;
    let name = dir.file_name().context("not a workflow folder")?;
    let candidate = root.join(name);
    let md = std::fs::symlink_metadata(&candidate).with_context(|| format!("no workflow at {}", dir.display()))?;
    if !md.is_dir() || name.to_string_lossy().starts_with('.') {
        bail!("{} is not an installed workflow", dir.display());
    }
    if std::fs::canonicalize(dir).ok().as_deref() != Some(candidate.as_path()) {
        bail!("{} is not in the workflows folder", dir.display());
    }
    Ok(candidate)
}

/// Enable or disable a workflow of Ashdash's own folder by writing the `disabled` key of its
/// `info.plist` (the same key the reference launcher uses). Atomic; other keys are kept.
pub fn set_workflow_enabled(paths: &Paths, dir: &Path, enabled: bool) -> Result<()> {
    let dir = owned_workflow_dir(paths, dir)?;
    let file = dir.join("info.plist");
    if !std::fs::symlink_metadata(&file)?.is_file() {
        bail!("info.plist is not a regular file");
    }
    let mut doc = plist::Value::from_file(&file).context("reading info.plist")?;
    let Some(d) = doc.as_dictionary_mut() else {
        bail!("info.plist is not a dictionary");
    };
    if enabled {
        d.remove("disabled");
    } else {
        d.insert("disabled".into(), plist::Value::Boolean(true));
    }
    let tmp = dir.join(format!(".info.plist.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        doc.to_file_xml(&tmp)?;
        std::fs::set_permissions(&tmp, std::fs::metadata(&file)?.permissions())?;
        std::fs::rename(&tmp, &file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Delete a workflow of Ashdash's own folder. Its saved configuration values stay (they are
/// found again if the workflow is reinstalled).
pub fn remove_workflow(paths: &Paths, dir: &Path) -> Result<()> {
    let dir = owned_workflow_dir(paths, dir)?;
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}

/// The configuration sheet of `wf` with each entry's current value (`Engine::workflow_settings`
/// and the preferences window share this).
pub fn workflow_settings_of(wf: &Workflow, paths: &Paths) -> Vec<crate::engine::WorkflowSetting> {
    let vars = wf.base_vars(paths);
    wf.user_config
        .iter()
        .map(|c| {
            let value = vars.get(&c.variable).cloned().unwrap_or_else(|| c.default.clone());
            crate::engine::WorkflowSetting {
                variable: c.variable.clone(),
                label: c.label.clone(),
                description: c.description.clone(),
                kind: c.kind.clone(),
                is_default: value == c.default,
                default: c.default.clone(),
                value,
            }
        })
        .collect()
}

/// Store (`Some`) or forget (`None`) the user's value for one entry of `wf`'s configuration
/// sheet, in `<data>/workflow-config/<bundle id>.json` (0600). The caller has checked that
/// `variable` is on the sheet and resets the loaded workflow's cached values afterwards.
pub fn write_workflow_value(wf: &Workflow, paths: &Paths, variable: &str, value: Option<&str>) -> Result<()> {
    let file = wf.user_values_file(paths);
    let mut map: serde_json::Map<String, Value> = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    match value {
        Some(v) => map.insert(variable.to_string(), v.into()),
        None => map.remove(variable),
    };
    write_private(&file, &serde_json::to_vec_pretty(&map)?)?;
    Ok(())
}

// ------------------------------------------------------------------ themes

/// Largest theme file accepted.
pub const MAX_THEME_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeInfo {
    pub name: String,
    pub path: PathBuf,
}

fn themes_dir(paths: &Paths) -> PathBuf {
    paths.data.join("themes")
}

/// `.alfredappearance` files in `<data>/themes/` that parse, by name.
pub fn list_themes(paths: &Paths) -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = std::fs::read_dir(themes_dir(paths))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "alfredappearance") && p.is_file())
                .filter_map(|p| {
                    let t = Theme::load(&p).ok()?;
                    let name = if t.name.is_empty() {
                        p.file_stem()?.to_string_lossy().into_owned()
                    } else {
                        t.name
                    };
                    Some(ThemeInfo { name, path: p })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Validate `file` as a theme and copy it into `<data>/themes/`; returns the copy. Does not
/// select it (that is the `theme` setting).
pub fn import_theme(paths: &Paths, file: &Path) -> Result<PathBuf> {
    let size = std::fs::metadata(file)
        .with_context(|| format!("cannot read {}", file.display()))?
        .len();
    if size > MAX_THEME_BYTES {
        bail!("the theme file is unusually large ({size} bytes)");
    }
    let theme = Theme::load(file).context("not a readable theme")?;
    let bytes = std::fs::read(file)?;
    let stem = safe_file_stem(if theme.name.is_empty() {
        file.file_stem().and_then(|s| s.to_str()).unwrap_or("theme")
    } else {
        &theme.name
    });
    let dest = themes_dir(paths).join(format!("{stem}.alfredappearance"));
    write_private(&dest, &bytes).context("copying the theme")?;
    Ok(dest)
}

// ------------------------------------------------------------------ snippets on disk

/// Where a snippet is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetLoc {
    /// Index into the flat `snippets.json`.
    Flat(usize),
    /// (collection file, index inside it)
    File(usize, usize),
}

struct FileCollection {
    /// `None` until first saved.
    path: Option<PathBuf>,
    collection: Collection,
    dirty: bool,
}

/// The snippets as files: the flat `snippets.json` plus one file per collection. Edits are made
/// in memory; [`SnippetLibrary::save`] rewrites only the files that changed and never touches
/// files it could not read.
pub struct SnippetLibrary {
    flat: Vec<Snippet>,
    flat_dirty: bool,
    files: Vec<FileCollection>,
    /// Collection files removed since loading.
    removed: Vec<PathBuf>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CollectionFile {
    Full(Collection),
    Bare(Vec<Snippet>),
}

impl SnippetLibrary {
    pub fn load(snippets_file: &Path) -> SnippetLibrary {
        let flat: Vec<Snippet> = std::fs::read(snippets_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let dir = collections_dir(snippets_file);
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        paths.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        let mut files = Vec::new();
        for path in paths {
            let Some(parsed) = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<CollectionFile>(&b).ok())
            else {
                continue;
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut c = match parsed {
                CollectionFile::Full(c) => c,
                CollectionFile::Bare(snippets) => Collection {
                    snippets,
                    ..Collection::default()
                },
            };
            if c.name.trim().is_empty() {
                c.name = stem;
            }
            for s in &mut c.snippets {
                s.collection = c.name.clone();
                if !c.keyword_prefix.is_empty() {
                    s.keyword_prefix = c.keyword_prefix.clone();
                }
                if !c.keyword_suffix.is_empty() {
                    s.keyword_suffix = c.keyword_suffix.clone();
                }
            }
            files.push(FileCollection {
                path: Some(path),
                collection: c,
                dirty: false,
            });
        }
        SnippetLibrary {
            flat,
            flat_dirty: false,
            files,
            removed: Vec::new(),
        }
    }

    /// Every snippet with its location, in display order: flat file first, then collections.
    pub fn entries(&self) -> Vec<(SnippetLoc, &Snippet)> {
        let mut v: Vec<(SnippetLoc, &Snippet)> = self
            .flat
            .iter()
            .enumerate()
            .map(|(i, s)| (SnippetLoc::Flat(i), s))
            .collect();
        for (fi, f) in self.files.iter().enumerate() {
            v.extend(
                f.collection
                    .snippets
                    .iter()
                    .enumerate()
                    .map(|(si, s)| (SnippetLoc::File(fi, si), s)),
            );
        }
        v
    }

    /// Distinct collection names (including empty collection files), sorted.
    pub fn collection_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .flat
            .iter()
            .map(|s| s.collection.clone())
            .chain(self.files.iter().map(|f| f.collection.name.clone()))
            .filter(|c| !c.is_empty())
            .collect();
        v.sort_by_key(|c| c.to_lowercase());
        v.dedup();
        v
    }

    pub fn get(&self, loc: SnippetLoc) -> Option<&Snippet> {
        match loc {
            SnippetLoc::Flat(i) => self.flat.get(i),
            SnippetLoc::File(f, i) => self.files.get(f)?.collection.snippets.get(i),
        }
    }

    /// Create an empty collection. Errors when the name is empty or already used.
    pub fn add_collection(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a collection needs a name");
        }
        if self.collection_names().iter().any(|n| n.eq_ignore_ascii_case(name)) {
            bail!("a collection named '{name}' exists already");
        }
        self.files.push(FileCollection {
            path: None,
            collection: Collection {
                name: name.to_string(),
                ..Collection::default()
            },
            dirty: true,
        });
        Ok(())
    }

    /// Delete a collection and its snippets.
    pub fn remove_collection(&mut self, name: &str) {
        let mut i = 0;
        while i < self.files.len() {
            if self.files[i].collection.name == name {
                let f = self.files.remove(i);
                self.removed.extend(f.path);
            } else {
                i += 1;
            }
        }
        let before = self.flat.len();
        self.flat.retain(|s| s.collection != name);
        self.flat_dirty |= self.flat.len() != before;
    }

    /// Add a snippet to the collection named in `s.collection` ("" = the flat file).
    pub fn add(&mut self, s: Snippet) {
        if s.collection.is_empty() {
            self.flat.push(s);
            self.flat_dirty = true;
        } else if let Some(f) = self.files.iter_mut().find(|f| f.collection.name == s.collection) {
            f.collection.snippets.push(s);
            f.dirty = true;
        } else if self.flat.iter().any(|o| o.collection == s.collection) {
            self.flat.push(s);
            self.flat_dirty = true;
        } else {
            let name = s.collection.clone();
            self.files.push(FileCollection {
                path: None,
                collection: Collection {
                    name,
                    snippets: vec![s],
                    ..Collection::default()
                },
                dirty: true,
            });
        }
    }

    /// Replace the snippet at `loc` (moving it when its collection changed).
    pub fn replace(&mut self, loc: SnippetLoc, s: Snippet) {
        let same = self.get(loc).is_some_and(|o| o.collection == s.collection);
        if !same {
            self.remove(loc);
            self.add(s);
            return;
        }
        match loc {
            SnippetLoc::Flat(i) => {
                if let Some(o) = self.flat.get_mut(i) {
                    *o = s;
                    self.flat_dirty = true;
                }
            }
            SnippetLoc::File(f, i) => {
                if let Some(file) = self.files.get_mut(f)
                    && let Some(o) = file.collection.snippets.get_mut(i)
                {
                    *o = s;
                    file.dirty = true;
                }
            }
        }
    }

    pub fn remove(&mut self, loc: SnippetLoc) {
        match loc {
            SnippetLoc::Flat(i) if i < self.flat.len() => {
                self.flat.remove(i);
                self.flat_dirty = true;
            }
            SnippetLoc::File(f, i) => {
                if let Some(file) = self.files.get_mut(f)
                    && i < file.collection.snippets.len()
                {
                    file.collection.snippets.remove(i);
                    file.dirty = true;
                }
            }
            _ => {}
        }
    }

    /// Write what changed. Collection files get the collection-level prefix/suffix back and lose
    /// the in-memory `collection` field (the file identifies it).
    pub fn save(&mut self, snippets_file: &Path) -> Result<()> {
        let dir = collections_dir(snippets_file);
        for gone in std::mem::take(&mut self.removed) {
            match std::fs::remove_file(&gone) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", gone.display())),
            }
        }
        for f in &mut self.files {
            if !f.dirty {
                continue;
            }
            let mut c = f.collection.clone();
            for s in &mut c.snippets {
                s.collection.clear();
                if s.keyword_prefix == c.keyword_prefix {
                    s.keyword_prefix.clear();
                }
                if s.keyword_suffix == c.keyword_suffix {
                    s.keyword_suffix.clear();
                }
            }
            let target = match &f.path {
                Some(p) => {
                    write_private(p, &serde_json::to_vec_pretty(&c)?)?;
                    p.clone()
                }
                None => {
                    let want = dir.join(format!("{}.json", safe_file_stem(&c.name)));
                    if want.exists() {
                        bail!("{} exists already; choose another collection name", want.display());
                    }
                    save_collection(&dir, &c)?
                }
            };
            f.path = Some(target);
            f.dirty = false;
        }
        if self.flat_dirty {
            write_private(snippets_file, &serde_json::to_vec_pretty(&self.flat)?)?;
            self.flat_dirty = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn get_reads_nested_values_and_whole_documents() {
        let c = Config::default();
        assert_eq!(get(&c, "clipboard.max_entries").unwrap(), json!(200));
        assert_eq!(get(&c, "max_results").unwrap(), json!(30));
        assert_eq!(get(&c, "theme").unwrap(), Value::Null);
        assert_eq!(get(&c, "bookmarks.include_in_general_search").unwrap(), json!(true));
        assert!(get(&c, "").unwrap().is_object());
        assert_eq!(get(&c, "web_searches.0.keyword").unwrap(), json!("google"));
        assert!(get(&c, "clipboard.nope").is_err());
        assert!(get(&c, "web_searches.9999").is_err());
    }

    #[test]
    fn set_refuses_a_web_search_url_the_engine_would_skip() {
        // Re-review N6: `config set` used to accept it and the launcher dropped it silently.
        let mut c = Config::default();
        let before = serde_json::to_value(&c).unwrap();
        for bad in [
            "file:///etc/{query}",
            "javascript:alert({query})",
            "https://user:pw@x.test/?q={query}",
            "https://x.test/",
        ] {
            let err = set(&mut c, "web_searches.0.url", bad).unwrap_err();
            assert!(format!("{err:#}").contains("not allowed"), "{bad}: {err:#}");
            assert_eq!(serde_json::to_value(&c).unwrap(), before, "{bad}: nothing is written");
        }
        set(&mut c, "web_searches.0.url", "https://example.test/?q={query}").unwrap();
        assert_eq!(c.web_searches[0].url, "https://example.test/?q={query}");
        // an entry that is already bad does not stop other, unrelated edits
        c.web_searches[1].url = "file:///x/{query}".into();
        set(&mut c, "max_results", "12").unwrap();
        assert_eq!(c.max_results, 12);
        // and the bad entry can be fixed
        set(&mut c, "web_searches.1.url", "https://y.test/?q={query}").unwrap();
    }

    #[test]
    fn set_validates_types_and_unknown_keys() {
        let mut c = Config::default();
        set(&mut c, "clipboard.max_entries", "50").unwrap();
        assert_eq!(c.clipboard.max_entries, 50);
        set(&mut c, "clipboard.max_age_days", "30").unwrap();
        assert_eq!(c.clipboard.max_age_days, Some(30));
        set(&mut c, "clipboard.max_age_days", "null").unwrap();
        assert_eq!(c.clipboard.max_age_days, None);
        set(&mut c, "hotkey", "cmd+space").unwrap();
        assert_eq!(c.hotkey, "cmd+space");
        set(&mut c, "file_search.include_hidden", "true").unwrap();
        assert!(c.file_search.include_hidden);

        let before = serde_json::to_value(&c).unwrap();
        for (k, v) in [
            ("clipboard.max_entries", "lots"),
            ("clipboard.max_entries", "-3"),
            ("max_results", "1.5"),
            ("file_search.include_hidden", "maybe"),
            ("no_such_key", "1"),
            ("clipboard.max_entires", "1"),
            ("clipboard", "{}"),
            ("", "1"),
        ] {
            assert!(set(&mut c, k, v).is_err(), "{k}={v}");
        }
        assert_eq!(serde_json::to_value(&c).unwrap(), before, "failed sets change nothing");
    }

    #[test]
    fn strings_lists_and_options_accept_plain_text() {
        let mut c = Config::default();
        set(&mut c, "hotkey", "alt+space").unwrap();
        set(&mut c, "theme", "/tmp/dark.alfredappearance").unwrap();
        assert_eq!(
            c.theme.as_deref(),
            Some(std::path::Path::new("/tmp/dark.alfredappearance"))
        );
        set(&mut c, "providers_disabled", "calc, system").unwrap();
        assert_eq!(c.providers_disabled, vec!["calc", "system"]);
        set(&mut c, "providers_disabled", r#"["shell"]"#).unwrap();
        assert_eq!(c.providers_disabled, vec!["shell"]);
        set(&mut c, "providers_disabled", "[]").unwrap();
        assert!(c.providers_disabled.is_empty());
        // a number-looking value for a string setting stays text
        set(&mut c, "hotkey", "42").unwrap();
        assert_eq!(c.hotkey, "42");
        set(&mut c, "clipboard.ignored_apps", "com.a.b").unwrap();
        assert_eq!(c.clipboard.ignored_apps, vec!["com.a.b"]);
    }

    #[test]
    fn keys_lists_leaves() {
        let k = keys(&Config::default());
        for want in [
            "hotkey",
            "clipboard.max_entries",
            "clipboard.ignored_apps",
            "file_search.roots",
            "bookmarks.enabled_sources",
            "providers_disabled",
            "theme",
        ] {
            assert!(k.iter().any(|x| x == want), "missing {want} in {k:?}");
        }
        assert!(!k.iter().any(|x| x == "clipboard"));
    }
    // ---- watching

    fn plan_for(root: &Path) -> (Paths, WatchPlan) {
        let paths = Paths::under(root);
        let plan = WatchPlan::new(&paths, &[PathBuf::from("/elsewhere/workflows")]);
        (paths, plan)
    }

    #[test]
    fn classify_picks_out_only_what_matters() {
        let (paths, plan) = plan_for(Path::new("/nonexistent-ashdash-watch"));
        let d = &paths.data;
        let c = |p: PathBuf| classify(&plan, &p);
        assert_eq!(c(d.join("config.json")), Some(Change::Config));
        assert_eq!(c(d.join("snippets.json")), Some(Change::Content));
        assert_eq!(c(d.join("snippets/Work.json")), Some(Change::Content));
        assert_eq!(c(d.join("workflow-config/b.json")), Some(Change::Content));
        assert_eq!(c(d.join("workflows/user.workflow.x")), Some(Change::Content));
        assert_eq!(c(d.join("workflows/user.workflow.x/info.plist")), Some(Change::Content));
        assert_eq!(
            c(PathBuf::from("/elsewhere/workflows/mine/info.plist")),
            Some(Change::Content)
        );
        // noise: our own temp files, history, usage, staging folders, files a workflow writes itself
        assert_eq!(c(d.join("config.json.12-0.tmp")), None);
        assert_eq!(c(d.join("clipboard.json")), None);
        assert_eq!(c(d.join("usage.json")), None);
        assert_eq!(c(d.join("workflow-config/b.json.1-0.tmp")), None);
        assert_eq!(c(d.join("workflows/.import-1-2-3")), None);
        assert_eq!(c(d.join("workflows/.import-1-2-3/info.plist")), None);
        assert_eq!(c(d.join("workflows/user.workflow.x/cache.json")), None);
        assert_eq!(c(d.join("workflows/user.workflow.x/sub/info.plist")), None);
        assert_eq!(c(PathBuf::from("/elsewhere/other")), None);
    }

    #[test]
    fn a_burst_of_events_becomes_one_report() {
        let (_, plan) = plan_for(Path::new("/nonexistent-ashdash-debounce"));
        let cfg = plan.config_file.clone();
        let wf = plan.workflow_roots[0].join("w/info.plist");
        let noise = plan.data.join("clipboard.json");
        let plan = Arc::new(Mutex::new(plan));
        let (tx, rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel();
        std::thread::spawn(move || {
            debounce_loop(rx, plan, Duration::from_millis(60), move |set| {
                let _ = out_tx.send(set);
            })
        });
        for p in [&cfg, &cfg, &noise, &wf, &cfg] {
            tx.send(WatchMsg::Path(p.clone())).unwrap();
        }
        let got = out_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(got, BTreeSet::from([Change::Config, Change::Content]));
        // noise alone reports nothing
        tx.send(WatchMsg::Path(noise)).unwrap();
        assert!(out_rx.recv_timeout(Duration::from_millis(300)).is_err());
        // a lost-track event means everything
        tx.send(WatchMsg::Everything).unwrap();
        assert_eq!(
            out_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            BTreeSet::from([Change::Config, Change::Content])
        );
        tx.send(WatchMsg::Stop).unwrap();
    }

    #[test]
    fn changed_keys_names_top_level_differences() {
        let a = Config::default();
        let mut b = a.clone();
        assert!(changed_keys(&a, &b).is_empty());
        b.hotkey = "cmd+space".into();
        b.clipboard.max_entries = 3;
        b.theme = Some("/x".into());
        assert_eq!(changed_keys(&a, &b), vec!["clipboard", "hotkey", "theme"]);
    }
}
