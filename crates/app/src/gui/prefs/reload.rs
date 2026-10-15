//! Live reload inside the launcher: edits to `config.json` (by hand, by `ashdash config set`, or
//! by the preferences window) and to workflows and snippets take effect without a restart.
//!
//! A watcher (core's `SettingsWatcher`, 300 ms debounce) reports what changed. Workflow and
//! snippet changes are re-read on the watcher's thread (`Engine::reload_content`); settings
//! changes are handed to the window thread, which applies them with `Engine::apply_config` and
//! re-registers the hotkey, reloads the theme and re-reads workflow hotkeys. Whatever goes wrong
//! is kept as a *problem*: it is written to `run/reload-status.json` for the preferences window
//! and announced once with a notification, and the previous settings stay in force.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use ashdash_core::settings_view::{self, Change, SettingsWatcher, WATCH_DEBOUNCE};
use ashdash_core::theme::Theme;
use ashdash_core::{Config, Engine, Paths};
use eframe::egui;
use global_hotkey::GlobalHotKeyManager;

use super::hotkey;
use super::status::{self, LauncherStatus};

/// What the window must do after settings changed, decided from the old and new settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The launcher hotkey has to be registered anew.
    pub rebind_hotkey: Option<String>,
    pub reload_theme: bool,
    pub rearm_watcher: bool,
    /// Anything for `Engine::apply_config` to do.
    pub apply_engine: bool,
}

/// `old` = settings in force, `registered` = the hotkey spec actually registered (empty = none).
pub fn plan(old: &Config, new: &Config, registered: &str) -> Plan {
    let changed = settings_view::changed_keys(old, new);
    let has = |k: &str| changed.iter().any(|c| c == k);
    Plan {
        rebind_hotkey: (new.hotkey != registered).then(|| new.hotkey.clone()),
        reload_theme: has("theme"),
        rearm_watcher: has("workflow_dirs"),
        apply_engine: !changed.is_empty(),
    }
}

enum Msg {
    Config,
    Content,
}

/// Parts of the launcher window the reload needs to change.
pub struct Host<'a> {
    pub manager: &'a mut Option<GlobalHotKeyManager>,
    pub hotkey_error: &'a mut Option<String>,
    pub theme: &'a mut Theme,
    pub automation: &'a mut crate::events::Automation,
}

pub struct HotReload {
    engine: Arc<Engine>,
    paths: Paths,
    watcher: Option<SettingsWatcher>,
    rx: Receiver<Msg>,
    /// The hotkey spec registered right now ("" = none).
    registered_hotkey: String,
    /// Keyed by topic so a later success clears the earlier failure.
    problems: BTreeMap<&'static str, String>,
    restart_required: Vec<String>,
    announced: Option<String>,
}

impl HotReload {
    /// `registered_hotkey`: the spec the launcher registered at start-up, if that worked.
    pub fn new(ctx: &egui::Context, engine: Arc<Engine>, registered_hotkey: Option<String>) -> HotReload {
        let paths = engine.paths.clone();
        let (tx, rx) = channel::<Msg>();
        let watcher = start_watcher(ctx, &engine, tx);
        let mut me = HotReload {
            engine,
            paths,
            watcher,
            rx,
            registered_hotkey: registered_hotkey.unwrap_or_default(),
            problems: BTreeMap::new(),
            restart_required: Vec::new(),
            announced: None,
        };
        if me.watcher.is_none() {
            me.problem(
                "watch",
                "Settings changes are not picked up automatically (file watching failed). Use Reload in the menu.",
            );
        }
        // A settings file that cannot be read means the launcher started with defaults: say so.
        if let Err(e) = Config::load_strict(&me.paths) {
            me.problem(
                "config",
                format!("config.json could not be read ({e:#}); Ashdash is using the default settings."),
            );
        }
        me.publish();
        me
    }

    /// Apply whatever the watcher reported since the last call. Cheap when nothing changed.
    pub fn poll(&mut self, host: &mut Host) {
        let (mut config, mut content) = (false, false);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Config => config = true,
                Msg::Content => content = true,
            }
        }
        if !(config || content) {
            return;
        }
        if config {
            self.apply_disk_config(host);
        }
        if content {
            host.automation.reload_hotkeys(host.manager.as_ref());
        }
        self.publish();
    }

    fn apply_disk_config(&mut self, host: &mut Host) {
        let new = match Config::load_strict(&self.paths) {
            Ok(c) => c,
            Err(e) => {
                self.problem(
                    "config",
                    format!("config.json is not valid ({e:#}). The previous settings stay in force."),
                );
                return;
            }
        };
        self.problems.remove("config");
        let old = self.engine.config_now();
        let plan = plan(&old, &new, &self.registered_hotkey);
        if !plan.apply_engine && plan.rebind_hotkey.is_none() {
            return;
        }
        let report = self.engine.apply_config(new.clone());
        self.restart_required = report.restart_required.iter().map(|s| s.to_string()).collect();
        if let Some(spec) = plan.rebind_hotkey {
            match hotkey::rebind(host.manager, &self.registered_hotkey, &spec) {
                Ok(()) => {
                    self.registered_hotkey = spec;
                    *host.hotkey_error = None;
                    self.problems.remove("hotkey");
                }
                Err(e) => {
                    let keep = if self.registered_hotkey.is_empty() {
                        "There is no working launcher hotkey right now.".to_string()
                    } else {
                        "The previous hotkey stays active.".to_string()
                    };
                    self.problem("hotkey", format!("The hotkey '{spec}' did not work: {e}. {keep}"));
                    if self.registered_hotkey.is_empty() {
                        *host.hotkey_error = Some(e);
                    }
                }
            }
        }
        if plan.reload_theme {
            match &new.theme {
                Some(p) => match Theme::load(p) {
                    Ok(t) => {
                        *host.theme = t;
                        self.problems.remove("theme");
                    }
                    Err(e) => self.problem("theme", format!("The theme {} could not be loaded: {e:#}", p.display())),
                },
                None => {
                    *host.theme = Theme::default();
                    self.problems.remove("theme");
                }
            }
        }
        if plan.rearm_watcher {
            if let Some(w) = &self.watcher {
                w.set_extra_roots(&self.paths, &new.workflow_dirs);
            }
            host.automation.reload_hotkeys(host.manager.as_ref());
        }
    }

    fn problem(&mut self, topic: &'static str, text: impl Into<String>) {
        let text = text.into();
        eprintln!("ashdash: {text}");
        self.problems.insert(topic, text);
    }

    /// Write the status file and announce a new problem once.
    fn publish(&mut self) {
        let problems: Vec<String> = self.problems.values().cloned().collect();
        let st = LauncherStatus {
            pid: std::process::id(),
            updated: status::now_secs(),
            problems: problems.clone(),
            restart_required: self.restart_required.clone(),
            hotkey_active: self.registered_hotkey.clone(),
        };
        if let Err(e) = status::write(&self.paths, &st) {
            eprintln!("ashdash: could not write the reload status: {e}");
        }
        match problems.first() {
            Some(p) if self.announced.as_ref() != Some(p) => {
                self.announced = Some(p.clone());
                let body = p.clone();
                std::thread::spawn(move || crate::mac::opendocs::notify("Ashdash settings", &body));
            }
            None => self.announced = None,
            _ => {}
        }
    }
}

fn start_watcher(ctx: &egui::Context, engine: &Arc<Engine>, tx: Sender<Msg>) -> Option<SettingsWatcher> {
    let ctx = ctx.clone();
    let for_thread = Arc::clone(engine);
    SettingsWatcher::start(
        &engine.paths,
        &engine.config_now().workflow_dirs,
        WATCH_DEBOUNCE,
        move |changes| {
            if changes.contains(&Change::Content) {
                // Slow work happens here, off the window thread; queries keep using the old data.
                for_thread.reload_content();
                let _ = tx.send(Msg::Content);
            }
            if changes.contains(&Change::Config) {
                let _ = tx.send(Msg::Config);
            }
            ctx.request_repaint();
        },
    )
    .map_err(|e| eprintln!("ashdash: settings watcher: {e:#}"))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn nothing_changed_means_nothing_to_do() {
        let c = cfg();
        let p = plan(&c, &c, &c.hotkey);
        assert_eq!(p, Plan::default());
    }

    #[test]
    fn a_new_hotkey_is_rebound_once_even_if_the_engine_already_has_it() {
        let old = cfg();
        let mut new = old.clone();
        new.hotkey = "ctrl+alt+KeyJ".into();
        let p = plan(&old, &new, &old.hotkey);
        assert_eq!(p.rebind_hotkey.as_deref(), Some("ctrl+alt+KeyJ"));
        assert!(p.apply_engine);
        // after a failed rebind the engine has the new value but the old key is registered: retry
        let p = plan(&new, &new, &old.hotkey);
        assert_eq!(p.rebind_hotkey.as_deref(), Some("ctrl+alt+KeyJ"));
        // when registration never worked (empty), any hotkey in the file is tried
        assert_eq!(plan(&new, &new, "").rebind_hotkey.as_deref(), Some("ctrl+alt+KeyJ"));
        // and once registered, nothing more
        assert_eq!(plan(&new, &new, "ctrl+alt+KeyJ").rebind_hotkey, None);
    }

    #[test]
    fn other_settings_pick_only_what_they_touch() {
        let old = cfg();
        let mut new = old.clone();
        new.theme = Some("/t.alfredappearance".into());
        let p = plan(&old, &new, &old.hotkey);
        assert!(p.reload_theme && p.apply_engine && !p.rearm_watcher && p.rebind_hotkey.is_none());
        let mut new = old.clone();
        new.workflow_dirs.push("/more".into());
        let p = plan(&old, &new, &old.hotkey);
        assert!(p.rearm_watcher && !p.reload_theme);
        let mut new = old.clone();
        new.providers_disabled.push("calc".into());
        let p = plan(&old, &new, &old.hotkey);
        assert!(p.apply_engine && !p.reload_theme && !p.rearm_watcher);
    }
}
