//! Global hotkeys for workflows (`alfred.workflow.trigger.hotkey` objects).
//!
//! # Encoding (inferred from the templates bundled with the reference launcher)
//!
//! Observed in `Hotkeys`, `Advanced Hotkeys`, `Universal Actions`,
//! `Clipboard`, `Show the reference launcher with selected text`... (only `Universal Actions`
//! ships a populated key; the reference launcher strips hotkeys on export):
//!
//! | field | meaning |
//! |---|---|
//! | `hotkey` | Carbon/IOKit virtual key code (`kVK_*`; `96` = F5) |
//! | `hotmod` | `NSEvent.ModifierFlags` raw value: shift `1<<17`, control `1<<18`, option `1<<19`, command `1<<20`, numeric pad `1<<21`, function `1<<23`. The F5 template stores `0x880000` = option + function |
//! | `hotstring` | display string such as `"F5"` (informational) |
//! | `hotkey == 0 && hotmod == 0` | **not set** (key code 0 is "A", but a bare "A" is never a valid hotkey) |
//! | `action` | `0` pass through to the workflow, `1` show the launcher with the argument as query. *Inferred*: template `Show the reference launcher with selected text` is `1` and has no connections, `Clipboard`/`Universal Actions` use `0` |
//! | `argument` | `0` none, `1` selection in macOS, `2` clipboard. *Inferred*: the `Universal Actions` readme says its `2` hotkey pre-populates "your clipboard contents", `Show the reference launcher with selected text` is `1` |
//! | `relatedAppsMode` / `relatedApps` | `0` every app, `1` only while one of `relatedApps` (bundle ids) is frontmost, `2` every app except those. *Inferred*: `Advanced Hotkeys` uses `1` with three apps; `2` follows the reference launcher's UI |
//! | `focusedappvariable` / `focusedappvariablename` | export the frontmost app's bundle id as a variable; the readme/filters of `Advanced Hotkeys` read `{var:focusedapp}` while the stored name is empty, so the empty name means `focusedapp` |
//! | `leftcursor` | cursor placement when showing the launcher; ignored |
//! | `modsmode` | always `0` in templates; non-zero values (hold/release/double-tap style triggers?) are unknown and the hotkey is skipped with a warning |
//!
//! # Wiring
//!
//! `global-hotkey` allows exactly one event handler per process and one
//! installed Carbon handler per manager, so workflow hotkeys share the main
//! hotkey's manager and are dispatched by [`route`], which the main handler
//! calls first. Presses spawn a worker thread: gathering the argument may
//! take hundreds of milliseconds and the Carbon callback runs on the main
//! thread.
//!
//! Firing goes through `Engine::run_node(workflow_id, node_uid, arg, vars)`, which starts the
//! workflow at the hotkey node and records nothing in `usage.json`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use ashdash_core::workflow::{self, Workflow};
use ashdash_core::{Engine, Outcome, Paths};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::events::{AppEvent, EventSink};

pub const KIND_HOTKEY: &str = "alfred.workflow.trigger.hotkey";

// `NSEvent.ModifierFlags` raw values.
const NS_SHIFT: i64 = 1 << 17;
const NS_CONTROL: i64 = 1 << 18;
const NS_OPTION: i64 = 1 << 19;
const NS_COMMAND: i64 = 1 << 20;

// ------------------------------------------------------------------ decoding

/// Carbon virtual key code (`kVK_*`, ANSI layout positions) to a physical key.
pub fn decode_key(code: i64) -> Option<Code> {
    use Code::*;
    Some(match code {
        0 => KeyA,
        1 => KeyS,
        2 => KeyD,
        3 => KeyF,
        4 => KeyH,
        5 => KeyG,
        6 => KeyZ,
        7 => KeyX,
        8 => KeyC,
        9 => KeyV,
        10 => IntlBackslash,
        11 => KeyB,
        12 => KeyQ,
        13 => KeyW,
        14 => KeyE,
        15 => KeyR,
        16 => KeyY,
        17 => KeyT,
        18 => Digit1,
        19 => Digit2,
        20 => Digit3,
        21 => Digit4,
        22 => Digit6,
        23 => Digit5,
        24 => Equal,
        25 => Digit9,
        26 => Digit7,
        27 => Minus,
        28 => Digit8,
        29 => Digit0,
        30 => BracketRight,
        31 => KeyO,
        32 => KeyU,
        33 => BracketLeft,
        34 => KeyI,
        35 => KeyP,
        36 => Enter,
        37 => KeyL,
        38 => KeyJ,
        39 => Quote,
        40 => KeyK,
        41 => Semicolon,
        42 => Backslash,
        43 => Comma,
        44 => Slash,
        45 => KeyN,
        46 => KeyM,
        47 => Period,
        48 => Tab,
        49 => Space,
        50 => Backquote,
        51 => Backspace,
        53 => Escape,
        64 => F17,
        65 => NumpadDecimal,
        67 => NumpadMultiply,
        69 => NumpadAdd,
        75 => NumpadDivide,
        76 => NumpadEnter,
        78 => NumpadSubtract,
        79 => F18,
        80 => F19,
        81 => NumpadEqual,
        82 => Numpad0,
        83 => Numpad1,
        84 => Numpad2,
        85 => Numpad3,
        86 => Numpad4,
        87 => Numpad5,
        88 => Numpad6,
        89 => Numpad7,
        90 => F20,
        91 => Numpad8,
        92 => Numpad9,
        96 => F5,
        97 => F6,
        98 => F7,
        99 => F3,
        100 => F8,
        101 => F9,
        103 => F11,
        105 => F13,
        106 => F16,
        107 => F14,
        109 => F10,
        111 => F12,
        113 => F15,
        114 => Insert,
        115 => Home,
        116 => PageUp,
        117 => Delete,
        118 => F4,
        119 => End,
        120 => F2,
        121 => PageDown,
        122 => F1,
        123 => ArrowLeft,
        124 => ArrowRight,
        125 => ArrowDown,
        126 => ArrowUp,
        _ => return None,
    })
}

fn is_function_key(c: Code) -> bool {
    use Code::*;
    matches!(
        c,
        F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12 | F13 | F14 | F15 | F16 | F17 | F18 | F19 | F20
    )
}

/// `NSEvent.ModifierFlags` to hotkey modifiers. Caps lock, numeric pad and
/// function flags carry no meaning for a hotkey and are dropped.
pub fn decode_mods(flags: i64) -> Modifiers {
    let mut m = Modifiers::empty();
    if flags & NS_SHIFT != 0 {
        m |= Modifiers::SHIFT;
    }
    if flags & NS_CONTROL != 0 {
        m |= Modifiers::CONTROL;
    }
    if flags & NS_OPTION != 0 {
        m |= Modifiers::ALT;
    }
    if flags & NS_COMMAND != 0 {
        m |= Modifiers::SUPER;
    }
    m
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PassThrough,
    ShowLauncher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSource {
    None,
    Selection,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelatedApps {
    All,
    Only(Vec<String>),
    Except(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub workflow_id: String,
    pub workflow_name: String,
    pub bundle_id: String,
    pub node_uid: String,
    pub key: Code,
    pub mods: Modifiers,
    pub action: HotkeyAction,
    pub argument: ArgSource,
    pub related: RelatedApps,
    /// Variable that receives the frontmost app's bundle id.
    pub focused_var: Option<String>,
    pub display: String,
}

impl HotkeySpec {
    pub fn hotkey(&self) -> HotKey {
        HotKey::new(Some(self.mods), self.key)
    }
    fn label(&self) -> String {
        format!(
            "{} ({})",
            self.workflow_name,
            if self.display.is_empty() { "?" } else { &self.display }
        )
    }
}

/// Read one hotkey object. `Ok(None)` = the object has no key assigned.
pub fn spec_from_node(wf: &Workflow, uid: &str, config: &serde_json::Value) -> Result<Option<HotkeySpec>, String> {
    let int = |k: &str| config.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
    let text = |k: &str| {
        config
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let (code, flags) = (int("hotkey"), int("hotmod"));
    if code == 0 && flags == 0 {
        return Ok(None);
    }
    let key = decode_key(code).ok_or_else(|| format!("unsupported key code {code}"))?;
    let mods = decode_mods(flags);
    if !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER) && !is_function_key(key) {
        return Err("a hotkey needs control, option or command (or be a function key)".into());
    }
    if int("modsmode") != 0 {
        return Err(format!("trigger mode {} is not supported yet", int("modsmode")));
    }
    let action = match int("action") {
        0 => HotkeyAction::PassThrough,
        1 => HotkeyAction::ShowLauncher,
        n => return Err(format!("unknown hotkey action {n}")),
    };
    let argument = match int("argument") {
        0 => ArgSource::None,
        1 => ArgSource::Selection,
        2 => ArgSource::Clipboard,
        n => return Err(format!("unknown hotkey argument mode {n}")),
    };
    let apps: Vec<String> = config
        .get("relatedApps")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let related = match int("relatedAppsMode") {
        0 => RelatedApps::All,
        1 => RelatedApps::Only(apps),
        2 => RelatedApps::Except(apps),
        n => return Err(format!("unknown related-apps mode {n}")),
    };
    let focused_var = config
        .get("focusedappvariable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        .then(|| match text("focusedappvariablename") {
            n if n.trim().is_empty() => "focusedapp".to_string(),
            n => n,
        });
    Ok(Some(HotkeySpec {
        workflow_id: wf.id.clone(),
        workflow_name: wf.name.clone(),
        bundle_id: wf.bundle_id.clone(),
        node_uid: uid.to_string(),
        key,
        mods,
        action,
        argument,
        related,
        focused_var,
        display: text("hotstring"),
    }))
}

/// All usable hotkeys of the enabled workflows, plus a warning per object that
/// has a key assigned but cannot be used.
pub fn collect(workflows: &[Workflow]) -> (Vec<HotkeySpec>, Vec<String>) {
    let (mut specs, mut warnings) = (Vec::new(), Vec::new());
    let mut seen_ids = std::collections::HashSet::new();
    for wf in workflows.iter().filter(|w| !w.disabled) {
        // Hotkeys fire by workflow id (directory name); two enabled copies with the
        // same directory name in different roots cannot be told apart.
        if !seen_ids.insert(wf.id.as_str()) {
            if wf.nodes.iter().any(|n| n.kind == KIND_HOTKEY) {
                warnings.push(format!(
                    "workflow '{}': another enabled workflow folder is also named '{}'; its hotkeys are ignored",
                    wf.name, wf.id
                ));
            }
            continue;
        }
        for node in wf.nodes.iter().filter(|n| n.kind == KIND_HOTKEY) {
            match spec_from_node(wf, &node.uid, &node.config) {
                Ok(Some(s)) => specs.push(s),
                Ok(None) => {}
                Err(e) => warnings.push(format!("workflow '{}': hotkey ignored: {e}", wf.name)),
            }
        }
    }
    (specs, warnings)
}

/// Decide which hotkeys can be registered: the first workflow to claim a
/// combination wins; combinations used by the launcher itself are refused.
pub fn plan_registrations(
    specs: Vec<HotkeySpec>,
    reserved: &[HotKey],
) -> (Vec<(HotKey, Arc<HotkeySpec>)>, Vec<String>) {
    let mut taken: HashMap<u32, String> = reserved
        .iter()
        .map(|h| (h.id(), "the launcher hotkey".to_string()))
        .collect();
    let (mut plan, mut warnings) = (Vec::new(), Vec::new());
    for spec in specs {
        let hk = spec.hotkey();
        if let Some(owner) = taken.get(&hk.id()) {
            warnings.push(format!(
                "hotkey of {} conflicts with {owner}; not registered",
                spec.label()
            ));
            continue;
        }
        taken.insert(hk.id(), spec.label());
        plan.push((hk, Arc::new(spec)));
    }
    (plan, warnings)
}

// ------------------------------------------------------------------ firing

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// The frontmost app is excluded by the related-apps setting.
    RelatedApps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Skip(Skip),
    Run { vars: BTreeMap<String, String> },
}

/// Pure decision: should this press do anything, and with which variables?
pub fn plan(spec: &HotkeySpec, frontmost: Option<&str>) -> Plan {
    let listed = |apps: &[String]| frontmost.is_some_and(|f| apps.iter().any(|a| a == f));
    let allowed = match &spec.related {
        RelatedApps::All => true,
        RelatedApps::Only(apps) => listed(apps),
        RelatedApps::Except(apps) => !listed(apps),
    };
    if !allowed {
        return Plan::Skip(Skip::RelatedApps);
    }
    let mut vars = BTreeMap::new();
    if let (Some(name), Some(app)) = (&spec.focused_var, frontmost) {
        vars.insert(name.clone(), app.to_string());
    }
    Plan::Run { vars }
}

/// Everything firing a hotkey needs from the outside world.
pub trait HotkeyHost: Send + Sync {
    fn frontmost_bundle(&self) -> Option<String>;
    fn read_argument(&self, source: ArgSource) -> Result<String, String>;
    fn run_node(&self, spec: &HotkeySpec, arg: &str, vars: BTreeMap<String, String>) -> Outcome;
    fn show_launcher(&self, query: Option<String>);
    fn report(&self, outcome: Outcome);
    /// A background refresh published a new [`Snapshot`]: ask the window thread to register it
    /// ([`WorkflowHotkeys::apply`]). Called from the refresh thread.
    fn hotkeys_refreshed(&self) {}
}

pub fn fire(host: &dyn HotkeyHost, spec: &HotkeySpec) {
    let front = host.frontmost_bundle();
    let Plan::Run { vars } = plan(spec, front.as_deref()) else {
        return;
    };
    let arg = match host.read_argument(spec.argument) {
        Ok(a) => a,
        Err(e) => {
            host.report(Outcome::Error(format!("{}: {e}", spec.workflow_name)));
            return;
        }
    };
    match spec.action {
        HotkeyAction::ShowLauncher => host.show_launcher((!arg.is_empty()).then_some(arg)),
        HotkeyAction::PassThrough => {
            let outcome = host.run_node(spec, &arg, vars);
            if outcome != Outcome::Done {
                host.report(outcome);
            }
        }
    }
}

// ------------------------------------------------------------------ registration

type Route = Arc<dyn Fn() + Send + Sync>;

fn routes() -> &'static Mutex<HashMap<u32, Route>> {
    static ROUTES: OnceLock<Mutex<HashMap<u32, Route>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}

/// Called from the process-wide hotkey event handler. Returns whether the
/// event belonged to a workflow hotkey.
pub fn route(ev: &GlobalHotKeyEvent) -> bool {
    let route = routes().lock().unwrap().get(&ev.id).cloned();
    match route {
        Some(r) => {
            if ev.state == HotKeyState::Pressed {
                r();
            }
            true
        }
        None => false,
    }
}

/// The hotkeys found in the workflow folders at one moment, built off the window thread by
/// [`build_snapshot`] and read (never rebuilt) by [`WorkflowHotkeys::apply`].
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Usable hotkeys of the enabled workflows, in folder order.
    pub specs: Vec<HotkeySpec>,
    /// Objects that have a key assigned but cannot be used.
    pub warnings: Vec<String>,
    /// Which refresh produced it; a slower, older refresh never replaces a newer one.
    seq: u64,
}

/// Read the workflow folders and collect their hotkeys. This is the only place the registry
/// touches the disk; it must not run on the window thread.
pub fn build_snapshot(roots: &[PathBuf]) -> Snapshot {
    let workflows = workflow::load_all(roots);
    let (specs, warnings) = collect(&workflows);
    Snapshot {
        specs,
        warnings,
        seq: 0,
    }
}

/// The current [`Snapshot`], swapped atomically (the lock is held only to clone or replace one
/// `Arc`, never across I/O).
#[derive(Default)]
struct SnapshotCell {
    current: Mutex<Arc<Snapshot>>,
    issued: std::sync::atomic::AtomicU64,
}

impl SnapshotCell {
    fn next_seq(&self) -> u64 {
        self.issued.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    fn get(&self) -> Arc<Snapshot> {
        Arc::clone(&self.current.lock().unwrap())
    }

    /// Replace the snapshot unless a newer refresh already did. Returns whether it was stored.
    fn publish(&self, seq: u64, mut snap: Snapshot) -> bool {
        snap.seq = seq;
        let mut cur = self.current.lock().unwrap();
        if seq <= cur.seq {
            return false;
        }
        *cur = Arc::new(snap);
        true
    }
}

/// Workflow hotkeys of the running launcher. Lives on the main thread; only
/// [`refresh_in_background`](Self::refresh_in_background) reads the disk, on its own thread.
pub struct WorkflowHotkeys {
    host: Arc<dyn HotkeyHost>,
    roots: Vec<PathBuf>,
    reserved: Vec<HotKey>,
    snapshot: Arc<SnapshotCell>,
    /// Used only when the launcher's own manager could not be created.
    own_manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    /// Problems found by the last [`apply`](Self::apply).
    pub warnings: Vec<String>,
}

impl WorkflowHotkeys {
    pub fn new(host: Arc<dyn HotkeyHost>, paths: &Paths, extra_dirs: &[PathBuf], main_hotkey: Option<HotKey>) -> Self {
        let mut roots = vec![paths.workflows_dir()];
        roots.extend(extra_dirs.iter().cloned());
        WorkflowHotkeys {
            host,
            roots,
            reserved: main_hotkey.into_iter().collect(),
            snapshot: Arc::default(),
            own_manager: None,
            registered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Replace the folders searched for workflows (the managed folder is always kept first).
    /// Takes effect at the next [`refresh_in_background`](Self::refresh_in_background).
    pub fn set_extra_dirs(&mut self, paths: &Paths, extra_dirs: &[PathBuf]) {
        let mut roots = vec![paths.workflows_dir()];
        roots.extend(extra_dirs.iter().cloned());
        self.roots = roots;
    }

    /// The launcher's own hotkey as registered right now; workflow hotkeys may not take it.
    pub fn set_launcher_hotkey(&mut self, main_hotkey: Option<HotKey>) {
        self.reserved = main_hotkey.into_iter().collect();
    }

    /// The folders searched and the reserved launcher hotkeys (for tests and diagnostics).
    #[cfg(test)]
    pub fn context(&self) -> (&[PathBuf], &[HotKey]) {
        (&self.roots, &self.reserved)
    }

    /// Rebuild the snapshot from the folders on a background thread, publish it, then tell the
    /// host ([`HotkeyHost::hotkeys_refreshed`]) so the window thread calls [`apply`](Self::apply).
    pub fn refresh_in_background(&self) {
        let (roots, cell, host) = (self.roots.clone(), Arc::clone(&self.snapshot), Arc::clone(&self.host));
        let seq = cell.next_seq();
        std::thread::spawn(move || {
            let snap = build_snapshot(&roots);
            if cell.publish(seq, snap) {
                host.hotkeys_refreshed();
            }
        });
    }

    /// Like [`refresh_in_background`](Self::refresh_in_background), on the calling thread and
    /// without notifying the host.
    #[cfg(test)]
    fn refresh_blocking(&self) {
        let seq = self.snapshot.next_seq();
        self.snapshot.publish(seq, build_snapshot(&self.roots));
    }

    /// What [`apply`](Self::apply) would register now: the current snapshot minus combinations
    /// the launcher owns or that two workflows claim, and the warnings that go with it.
    /// Pure: no disk access.
    pub fn planned(&self) -> (Vec<(HotKey, Arc<HotkeySpec>)>, Vec<String>) {
        let snap = self.snapshot.get();
        let mut warnings = snap.warnings.clone();
        let (plan, more) = plan_registrations(snap.specs.clone(), &self.reserved);
        warnings.extend(more);
        (plan, warnings)
    }

    /// Replace all registrations with the ones in the current snapshot (window thread; reads no
    /// folders).
    pub fn apply(&mut self, manager: Option<&GlobalHotKeyManager>) {
        if manager.is_none() && self.own_manager.is_none() {
            self.own_manager = GlobalHotKeyManager::new().ok();
        }
        // If the main handler was not installed (launcher hotkey failed), this
        // one is; otherwise the call is a no-op and the main handler routes.
        GlobalHotKeyEvent::set_event_handler(Some(|ev: GlobalHotKeyEvent| {
            route(&ev);
        }));
        let Some(mgr) = manager.or(self.own_manager.as_ref()) else {
            self.warnings = vec!["workflow hotkeys unavailable: no hotkey manager".into()];
            return;
        };
        for hk in self.registered.drain(..) {
            let _ = mgr.unregister(hk);
        }
        routes().lock().unwrap().clear();

        let (plan, mut warnings) = self.planned();
        for (hk, spec) in plan {
            match mgr.register(hk) {
                Ok(()) => {
                    self.registered.push(hk);
                    let (host, spec2) = (Arc::clone(&self.host), Arc::clone(&spec));
                    let route: Route = Arc::new(move || {
                        let (host, spec) = (Arc::clone(&host), Arc::clone(&spec2));
                        std::thread::spawn(move || fire(host.as_ref(), &spec));
                    });
                    routes().lock().unwrap().insert(hk.id(), route);
                }
                Err(e) => warnings.push(format!("hotkey of {} could not be registered: {e}", spec.label())),
            }
        }
        for w in &warnings {
            eprintln!("ashdash: {w}");
        }
        if !self.registered.is_empty() {
            eprintln!("ashdash: {} workflow hotkey(s) registered", self.registered.len());
        }
        self.warnings = warnings;
    }
}

// ------------------------------------------------------------------ real host

/// The production [`HotkeyHost`]: engine for execution, event sink for the window.
pub struct AppHost {
    pub engine: Arc<Engine>,
    pub sink: Arc<dyn EventSink>,
}

impl HotkeyHost for AppHost {
    fn frontmost_bundle(&self) -> Option<String> {
        use objc2_app_kit::NSWorkspace;
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|a| a.bundleIdentifier())
            .map(|s| s.to_string())
            // The launcher itself can be the active app (it activates at startup and
            // while its window is up); that says nothing about where the user is.
            // An unbundled dev build has no bundle id at all and yields None above.
            .filter(|id| id != ashdash_core::brand::BUNDLE_ID)
    }

    fn read_argument(&self, source: ArgSource) -> Result<String, String> {
        match source {
            ArgSource::None => Ok(String::new()),
            ArgSource::Clipboard => arboard::Clipboard::new()
                .and_then(|mut c| c.get_text())
                .map_err(|e| format!("clipboard has no text ({e})")),
            ArgSource::Selection => capture_selection(),
        }
    }

    fn run_node(&self, spec: &HotkeySpec, arg: &str, vars: BTreeMap<String, String>) -> Outcome {
        self.engine.run_node(&spec.workflow_id, &spec.node_uid, arg, vars)
    }

    fn show_launcher(&self, query: Option<String>) {
        self.sink.emit(match query {
            Some(q) => AppEvent::Search(q),
            None => AppEvent::Show,
        });
    }

    fn hotkeys_refreshed(&self) {
        self.sink.emit(AppEvent::HotkeysRefreshed);
    }

    fn report(&self, outcome: Outcome) {
        self.sink.emit(AppEvent::Outcome(outcome));
    }
}

/// "Selection in macOS": copy with Cmd+C, read the result, put the previous
/// text back. Needs Accessibility/Automation permission for System Events.
/// Side effect: the clipboard watcher sees a probe string and the selection,
/// so they can show up in the clipboard history.
fn capture_selection() -> Result<String, String> {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let before = cb.get_text().ok();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = format!("ashdash-selection-probe-{nanos}");
    cb.set_text(probe.clone()).map_err(|e| e.to_string())?;
    let sent = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "tell application \"System Events\" to keystroke \"c\" using command down",
        ])
        .stdin(std::process::Stdio::null())
        .output();
    let mut result = Err("nothing is selected".to_string());
    match sent {
        Ok(o) if o.status.success() => {
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(700) {
                std::thread::sleep(Duration::from_millis(40));
                if let Ok(t) = cb.get_text()
                    && t != probe
                {
                    result = Ok(t);
                    break;
                }
            }
        }
        Ok(o) => {
            result = Err(format!(
                "could not copy the selection (grant Accessibility/Automation): {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => result = Err(e.to_string()),
    }
    match before {
        Some(b) => {
            let _ = cb.set_text(b);
        }
        None => {
            let _ = cb.clear();
        }
    }
    result
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn wf(name: &str, objects: serde_json::Value) -> Workflow {
        let doc = json!({"name": name, "bundleid": format!("b.{name}"), "objects": objects});
        Workflow::from_json(std::path::Path::new("/nonexistent"), name, &doc).unwrap()
    }

    fn hk(uid: &str, cfg: serde_json::Value) -> serde_json::Value {
        json!({"uid": uid, "type": KIND_HOTKEY, "config": cfg})
    }

    #[test]
    fn decodes_the_template_hotkey() {
        // Universal Actions template: key 96, mods 0x880000, "F5".
        assert_eq!(decode_key(96), Some(Code::F5));
        assert_eq!(decode_mods(8912896), Modifiers::ALT);
        assert_eq!(
            decode_mods(NS_SHIFT | NS_CONTROL | NS_OPTION | NS_COMMAND),
            Modifiers::all() & (Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
        );
        // caps lock (1<<16), numpad (1<<21) and function (1<<23) are dropped
        assert_eq!(decode_mods((1 << 16) | (1 << 21) | (1 << 23)), Modifiers::empty());
        assert_eq!(decode_key(49), Some(Code::Space));
        assert_eq!(decode_key(0), Some(Code::KeyA));
        assert_eq!(decode_key(126), Some(Code::ArrowUp));
        assert_eq!(decode_key(52), None);
        assert_eq!(decode_key(-1), None);
        assert_eq!(decode_key(5000), None);
    }

    #[test]
    fn key_table_has_no_duplicate_targets() {
        let mut seen = std::collections::HashSet::new();
        for c in 0..200 {
            if let Some(code) = decode_key(c) {
                assert!(
                    seen.insert(format!("{code:?}")),
                    "key code {c} maps to a duplicate {code:?}"
                );
            }
        }
        assert!(seen.len() > 90);
    }

    #[test]
    fn spec_from_the_universal_actions_node() {
        let w = wf("ua", json!([]));
        let cfg = json!({"hotkey": 96, "hotmod": 8912896, "action": 0, "relatedAppsMode": 0, "argument": 2,
            "focusedappvariable": false, "focusedappvariablename": "", "hotstring": "F5", "leftcursor": false, "modsmode": 0});
        let s = spec_from_node(&w, "U1", &cfg).unwrap().unwrap();
        assert_eq!((s.key, s.mods), (Code::F5, Modifiers::ALT));
        assert_eq!(
            (s.action, s.argument),
            (HotkeyAction::PassThrough, ArgSource::Clipboard)
        );
        assert_eq!(s.related, RelatedApps::All);
        assert_eq!(s.focused_var, None);
        assert_eq!(s.display, "F5");
    }

    #[test]
    fn unset_and_invalid_hotkeys() {
        let w = wf("x", json!([]));
        // Stripped on export: neither key nor modifiers.
        assert_eq!(
            spec_from_node(&w, "U", &json!({"hotkey": 0, "hotmod": 0})).unwrap(),
            None
        );
        assert_eq!(spec_from_node(&w, "U", &json!({})).unwrap(), None);
        // Option+A is fine, bare A or Shift+A are not.
        assert!(
            spec_from_node(&w, "U", &json!({"hotkey": 0, "hotmod": NS_OPTION}))
                .unwrap()
                .is_some()
        );
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 0, "hotmod": 1})).is_err());
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 0, "hotmod": NS_SHIFT})).is_err());
        // A bare function key is fine.
        assert!(
            spec_from_node(&w, "U", &json!({"hotkey": 122, "hotmod": 0}))
                .unwrap()
                .is_some()
        );
        // Unknown key code, mode, action or argument kind.
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 52, "hotmod": NS_COMMAND})).is_err());
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 1, "hotmod": NS_COMMAND, "modsmode": 2})).is_err());
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 1, "hotmod": NS_COMMAND, "action": 9})).is_err());
        assert!(spec_from_node(&w, "U", &json!({"hotkey": 1, "hotmod": NS_COMMAND, "argument": 9})).is_err());
        assert!(
            spec_from_node(
                &w,
                "U",
                &json!({"hotkey": 1, "hotmod": NS_COMMAND, "relatedAppsMode": 7})
            )
            .is_err()
        );
    }

    #[test]
    fn focused_app_variable_defaults_to_focusedapp() {
        let w = wf("x", json!([]));
        let base = |name: &str| json!({"hotkey": 1, "hotmod": NS_COMMAND, "focusedappvariable": true, "focusedappvariablename": name});
        assert_eq!(
            spec_from_node(&w, "U", &base(""))
                .unwrap()
                .unwrap()
                .focused_var
                .as_deref(),
            Some("focusedapp")
        );
        assert_eq!(
            spec_from_node(&w, "U", &base("app"))
                .unwrap()
                .unwrap()
                .focused_var
                .as_deref(),
            Some("app")
        );
    }

    #[test]
    fn collect_skips_disabled_workflows_and_reports_problems() {
        let good = wf(
            "good",
            json!([
                hk("H1", json!({"hotkey": 1, "hotmod": NS_COMMAND})),
                hk("H2", json!({"hotkey": 0, "hotmod": 0}))
            ]),
        );
        let bad = wf("bad", json!([hk("H1", json!({"hotkey": 52, "hotmod": NS_COMMAND}))]));
        let mut off = wf("off", json!([hk("H1", json!({"hotkey": 2, "hotmod": NS_COMMAND}))]));
        off.disabled = true;
        let (specs, warnings) = collect(&[good, bad, off]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].workflow_name, "good");
        assert_eq!(specs[0].node_uid, "H1");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bad") && warnings[0].contains("52"));
    }

    #[test]
    fn collect_keys_specs_by_workflow_id_not_bundle_id() {
        let mk = |dir: &str, bundle: &str| {
            let doc = json!({"name": dir, "bundleid": bundle, "objects": [
                hk("H1", json!({"hotkey": 1, "hotmod": NS_COMMAND}))]});
            Workflow::from_json(std::path::Path::new("/nonexistent"), dir, &doc).unwrap()
        };
        // empty bundle id and a shared bundle id: both still carry their own directory id
        let (specs, warnings) = collect(&[mk("a", ""), mk("b", "shared"), mk("c", "shared")]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let ids: Vec<_> = specs.iter().map(|s| s.workflow_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        // the same directory name in two roots is ambiguous: the second copy is refused loudly
        let (specs, warnings) = collect(&[mk("dup", "x"), mk("dup", "y")]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].bundle_id, "x");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("dup"));
    }

    struct NoHost;
    impl HotkeyHost for NoHost {
        fn frontmost_bundle(&self) -> Option<String> {
            None
        }
        fn read_argument(&self, _: ArgSource) -> Result<String, String> {
            Ok(String::new())
        }
        fn run_node(&self, _: &HotkeySpec, _: &str, _: BTreeMap<String, String>) -> Outcome {
            Outcome::Done
        }
        fn show_launcher(&self, _: Option<String>) {}
        fn report(&self, _: Outcome) {}
    }

    #[test]
    fn folders_and_launcher_hotkey_follow_live_changes() {
        let paths = Paths::under(&std::env::temp_dir().join("oa-hk-ctx"));
        let old = HotKey::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space);
        let mut w = WorkflowHotkeys::new(Arc::new(NoHost), &paths, &[PathBuf::from("/old")], Some(old));
        assert_eq!(w.context().0, [paths.workflows_dir(), PathBuf::from("/old")]);

        // folders added, removed
        w.set_extra_dirs(&paths, &[PathBuf::from("/new1"), PathBuf::from("/new2")]);
        assert_eq!(
            w.context().0,
            [paths.workflows_dir(), PathBuf::from("/new1"), PathBuf::from("/new2")]
        );
        w.set_extra_dirs(&paths, &[]);
        assert_eq!(w.context().0, [paths.workflows_dir()]);

        // the launcher moved to Cmd+J: that key is now off limits, the old one is free
        let new = HotKey::new(Some(Modifiers::SUPER), Code::KeyJ);
        w.set_launcher_hotkey(Some(new));
        let (plan, warnings) = plan_registrations(
            vec![
                spec("takes-new", Code::KeyJ, Modifiers::SUPER),
                spec("takes-old", Code::Space, Modifiers::ALT | Modifiers::SHIFT),
            ],
            w.context().1,
        );
        let names: Vec<_> = plan.iter().map(|(_, s)| s.workflow_name.as_str()).collect();
        assert_eq!(names, ["takes-old"]);
        assert!(warnings[0].contains("launcher hotkey"));
    }

    /// A workflow folder `<root>/<id>` whose only object is a hotkey (Option+F5, pass through).
    fn write_hotkey_workflow(root: &std::path::Path, id: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>bundleid</key><string>b.{id}</string><key>name</key><string>{id}</string>
<key>connections</key><dict/>
<key>objects</key><array><dict>
<key>uid</key><string>H</string><key>type</key><string>{KIND_HOTKEY}</string>
<key>config</key><dict><key>hotkey</key><integer>96</integer><key>hotmod</key><integer>8912896</integer>
<key>action</key><integer>0</integer><key>argument</key><integer>0</integer></dict>
</dict></array></dict></plist>"#
            ),
        )
        .unwrap();
        dir
    }

    fn registry(root: &std::path::Path, host: Arc<dyn HotkeyHost>) -> WorkflowHotkeys {
        let paths = Paths::under(root);
        let wf_dir = paths.workflows_dir();
        std::fs::create_dir_all(&wf_dir).unwrap();
        WorkflowHotkeys::new(host, &paths, &[], None)
    }

    fn names(w: &WorkflowHotkeys) -> Vec<String> {
        w.planned().0.iter().map(|(_, s)| s.workflow_id.clone()).collect()
    }

    /// The window thread only reads the snapshot: whatever happens to the folders in between
    /// changes nothing until a refresh has published a new one.
    #[test]
    fn the_lookup_path_reads_only_the_snapshot_not_the_folders() {
        let root = std::env::temp_dir().join(format!("oa-hk-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = registry(&root, Arc::new(NoHost));
        let wf_dir = Paths::under(&root).workflows_dir();
        let first = write_hotkey_workflow(&wf_dir, "first");

        // nothing has been scanned yet: the folder exists but the registry does not look
        assert!(names(&w).is_empty(), "planned() must not read the folders");
        w.refresh_blocking();
        assert_eq!(names(&w), ["first"]);

        // folder deleted, another created: the snapshot is unchanged until the next refresh
        std::fs::remove_dir_all(&first).unwrap();
        write_hotkey_workflow(&wf_dir, "second");
        for _ in 0..3 {
            assert_eq!(names(&w), ["first"], "planned() must not read the folders");
        }
        // even an unreadable workflows directory cannot disturb the lookups
        w.refresh_blocking();
        assert_eq!(names(&w), ["second"]);
        std::fs::remove_dir_all(&wf_dir).unwrap();
        assert_eq!(names(&w), ["second"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slow_older_refresh_never_replaces_a_newer_snapshot() {
        let cell = SnapshotCell::default();
        let (old, new) = (cell.next_seq(), cell.next_seq());
        let snap = |name: &str| Snapshot {
            specs: vec![spec(name, Code::F5, Modifiers::ALT)],
            ..Snapshot::default()
        };
        assert!(cell.publish(new, snap("new")));
        assert!(!cell.publish(old, snap("old")), "the older scan finished last");
        assert_eq!(cell.get().specs[0].workflow_id, "new");
        // readers holding the previous Arc keep a consistent view across a swap
        let held = cell.get();
        let newest = cell.next_seq();
        assert!(cell.publish(newest, snap("newest")));
        assert_eq!(held.specs[0].workflow_id, "new");
        assert_eq!(cell.get().specs[0].workflow_id, "newest");
    }

    struct Notifying(Mutex<std::sync::mpsc::Sender<()>>);
    impl HotkeyHost for Notifying {
        fn frontmost_bundle(&self) -> Option<String> {
            None
        }
        fn read_argument(&self, _: ArgSource) -> Result<String, String> {
            Ok(String::new())
        }
        fn run_node(&self, _: &HotkeySpec, _: &str, _: BTreeMap<String, String>) -> Outcome {
            Outcome::Done
        }
        fn show_launcher(&self, _: Option<String>) {}
        fn report(&self, _: Outcome) {}
        fn hotkeys_refreshed(&self) {
            let _ = self.0.lock().unwrap().send(());
        }
    }

    #[test]
    fn the_background_refresh_publishes_then_tells_the_host() {
        let root = std::env::temp_dir().join(format!("oa-hk-bg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (tx, rx) = std::sync::mpsc::channel();
        let w = registry(&root, Arc::new(Notifying(Mutex::new(tx))));
        write_hotkey_workflow(&Paths::under(&root).workflows_dir(), "bg");
        w.refresh_in_background();
        // the call returned without scanning; the host is told once the snapshot is in place
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("host notified");
        assert_eq!(names(&w), ["bg"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn spec(name: &str, key: Code, mods: Modifiers) -> HotkeySpec {
        HotkeySpec {
            workflow_id: name.into(),
            workflow_name: name.into(),
            bundle_id: format!("b.{name}"),
            node_uid: "H".into(),
            key,
            mods,
            action: HotkeyAction::PassThrough,
            argument: ArgSource::None,
            related: RelatedApps::All,
            focused_var: None,
            display: String::new(),
        }
    }

    #[test]
    fn conflicts_are_reported_and_first_claim_wins() {
        let main = HotKey::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space);
        let specs = vec![
            spec("a", Code::KeyA, Modifiers::SUPER),
            spec("b", Code::KeyA, Modifiers::SUPER),
            spec("c", Code::Space, Modifiers::ALT | Modifiers::SHIFT),
            spec("d", Code::KeyB, Modifiers::SUPER),
        ];
        let (plan, warnings) = plan_registrations(specs, &[main]);
        let names: Vec<_> = plan.iter().map(|(_, s)| s.workflow_name.as_str()).collect();
        assert_eq!(names, ["a", "d"]);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains('b') && warnings[0].contains("conflicts with a"));
        assert!(warnings[1].contains("launcher hotkey"));
    }

    #[test]
    fn related_apps_modes() {
        let mut s = spec("r", Code::KeyA, Modifiers::SUPER);
        let run = Plan::Run { vars: BTreeMap::new() };
        assert_eq!(plan(&s, Some("com.x")), run);
        assert_eq!(plan(&s, None), run);

        s.related = RelatedApps::Only(vec!["com.apple.Notes".into()]);
        assert_eq!(plan(&s, Some("com.apple.Notes")), run);
        assert_eq!(plan(&s, Some("com.apple.Safari")), Plan::Skip(Skip::RelatedApps));
        assert_eq!(plan(&s, None), Plan::Skip(Skip::RelatedApps));

        s.related = RelatedApps::Except(vec!["com.apple.Notes".into()]);
        assert_eq!(plan(&s, Some("com.apple.Notes")), Plan::Skip(Skip::RelatedApps));
        assert_eq!(plan(&s, Some("com.apple.Safari")), run);
        assert_eq!(plan(&s, None), run);
    }

    #[test]
    fn focused_app_variable_is_exported() {
        let mut s = spec("v", Code::KeyA, Modifiers::SUPER);
        s.focused_var = Some("focusedapp".into());
        let Plan::Run { vars } = plan(&s, Some("com.apple.Notes")) else {
            panic!()
        };
        assert_eq!(vars.get("focusedapp").map(String::as_str), Some("com.apple.Notes"));
        let Plan::Run { vars } = plan(&s, None) else { panic!() };
        assert!(vars.is_empty());
    }

    // ---- fire() with a fake host

    type FakeRun = (String, String, BTreeMap<String, String>);

    #[derive(Default)]
    struct FakeHost {
        front: Option<String>,
        clip: Option<String>,
        runs: Mutex<Vec<FakeRun>>,
        shown: Mutex<Vec<Option<String>>>,
        reports: Mutex<Vec<Outcome>>,
        outcome: Option<Outcome>,
    }
    impl HotkeyHost for FakeHost {
        fn frontmost_bundle(&self) -> Option<String> {
            self.front.clone()
        }
        fn read_argument(&self, source: ArgSource) -> Result<String, String> {
            match source {
                ArgSource::None => Ok(String::new()),
                ArgSource::Clipboard | ArgSource::Selection => self.clip.clone().ok_or_else(|| "empty".to_string()),
            }
        }
        fn run_node(&self, spec: &HotkeySpec, arg: &str, vars: BTreeMap<String, String>) -> Outcome {
            self.runs
                .lock()
                .unwrap()
                .push((spec.node_uid.clone(), arg.to_string(), vars));
            self.outcome.clone().unwrap_or(Outcome::Done)
        }
        fn show_launcher(&self, query: Option<String>) {
            self.shown.lock().unwrap().push(query);
        }
        fn report(&self, o: Outcome) {
            self.reports.lock().unwrap().push(o);
        }
    }

    #[test]
    fn pass_through_runs_the_node_with_argument_and_variables() {
        let host = FakeHost {
            front: Some("com.apple.Notes".into()),
            clip: Some("clip text".into()),
            ..Default::default()
        };
        let mut s = spec("p", Code::KeyA, Modifiers::SUPER);
        s.argument = ArgSource::Clipboard;
        s.focused_var = Some("focusedapp".into());
        fire(&host, &s);
        let runs = host.runs.lock().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, "H");
        assert_eq!(runs[0].1, "clip text");
        assert_eq!(runs[0].2.get("focusedapp").map(String::as_str), Some("com.apple.Notes"));
        assert!(host.reports.lock().unwrap().is_empty());
    }

    #[test]
    fn show_launcher_action_prefills_the_query() {
        let host = FakeHost {
            clip: Some("hello".into()),
            ..Default::default()
        };
        let mut s = spec("s", Code::KeyA, Modifiers::SUPER);
        s.action = HotkeyAction::ShowLauncher;
        s.argument = ArgSource::Selection;
        fire(&host, &s);
        s.argument = ArgSource::None;
        fire(&host, &s);
        assert_eq!(*host.shown.lock().unwrap(), vec![Some("hello".to_string()), None]);
        assert!(host.runs.lock().unwrap().is_empty());
    }

    #[test]
    fn skipped_apps_do_nothing_and_argument_errors_are_reported() {
        let host = FakeHost {
            front: Some("com.other".into()),
            ..Default::default()
        };
        let mut s = spec("k", Code::KeyA, Modifiers::SUPER);
        s.related = RelatedApps::Only(vec!["com.apple.Notes".into()]);
        fire(&host, &s);
        assert!(host.runs.lock().unwrap().is_empty() && host.reports.lock().unwrap().is_empty());

        let s2 = {
            let mut s = spec("e", Code::KeyA, Modifiers::SUPER);
            s.argument = ArgSource::Clipboard;
            s
        };
        fire(&host, &s2);
        assert!(host.runs.lock().unwrap().is_empty());
        let reports = host.reports.lock().unwrap();
        assert!(matches!(&reports[0], Outcome::Error(m) if m.starts_with("e:")));
    }

    #[test]
    fn non_done_outcomes_are_reported_to_the_window() {
        let host = FakeHost {
            outcome: Some(Outcome::LargeType("big".into())),
            ..Default::default()
        };
        fire(&host, &spec("o", Code::KeyA, Modifiers::SUPER));
        assert_eq!(*host.reports.lock().unwrap(), vec![Outcome::LargeType("big".into())]);
    }

    #[test]
    fn route_ignores_unknown_ids() {
        let ev = GlobalHotKeyEvent {
            id: 0xdead_beef,
            state: HotKeyState::Pressed,
        };
        assert!(!route(&ev));
    }
}
