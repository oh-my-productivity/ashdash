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

/// What a reload needs from the launcher window (a trait so tests can stand in for it).
pub trait Window {
    /// Register `new` as the launcher hotkey and release `old` ("" = none); on error the
    /// previous registration must still be in place.
    fn rebind_hotkey(&mut self, old: &str, new: &str) -> Result<(), String>;
    /// The hotkey registration is gone for good: show the startup hint again (`Some`) or not.
    fn set_hotkey_error(&mut self, error: Option<String>);
    fn set_theme(&mut self, theme: Theme);
    /// Re-read workflow hotkeys from `workflow_dirs` (the extra folders now in force; main thread).
    fn reload_workflow_hotkeys(&mut self, workflow_dirs: &[std::path::PathBuf]);
}

/// The real launcher window.
pub struct Host<'a> {
    pub paths: &'a Paths,
    pub manager: &'a mut Option<GlobalHotKeyManager>,
    pub hotkey_error: &'a mut Option<String>,
    pub theme: &'a mut Theme,
    pub automation: &'a mut crate::events::Automation,
}

impl Window for Host<'_> {
    fn rebind_hotkey(&mut self, old: &str, new: &str) -> Result<(), String> {
        hotkey::rebind(self.manager, old, new)?;
        // Workflow hotkeys must not take the key the launcher now owns.
        self.automation.set_launcher_hotkey(new.parse().ok());
        Ok(())
    }

    fn set_hotkey_error(&mut self, error: Option<String>) {
        *self.hotkey_error = error;
    }

    fn set_theme(&mut self, theme: Theme) {
        *self.theme = theme;
    }

    fn reload_workflow_hotkeys(&mut self, workflow_dirs: &[std::path::PathBuf]) {
        self.automation.set_workflow_dirs(self.paths, workflow_dirs);
        self.automation.reload_hotkeys(self.manager.as_ref());
    }
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
    /// Tells the user about a new problem (a notification; replaced in tests).
    notify: Box<dyn Fn(String) + Send>,
}

impl HotReload {
    /// `registered_hotkey`: the spec the launcher registered at start-up, if that worked.
    pub fn new(ctx: &egui::Context, engine: Arc<Engine>, registered_hotkey: Option<String>) -> HotReload {
        HotReload::with_notifier(
            ctx,
            engine,
            registered_hotkey,
            Box::new(|body| {
                std::thread::spawn(move || crate::mac::opendocs::notify("Ashdash settings", &body));
            }),
        )
    }

    pub fn with_notifier(
        ctx: &egui::Context,
        engine: Arc<Engine>,
        registered_hotkey: Option<String>,
        notify: Box<dyn Fn(String) + Send>,
    ) -> HotReload {
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
            notify,
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
    pub fn poll(&mut self, host: &mut dyn Window) {
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
            host.reload_workflow_hotkeys(&self.engine.config_now().workflow_dirs);
        }
        self.publish();
    }

    fn apply_disk_config(&mut self, host: &mut dyn Window) {
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
            match host.rebind_hotkey(&self.registered_hotkey, &spec) {
                Ok(()) => {
                    self.registered_hotkey = spec;
                    host.set_hotkey_error(None);
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
                        host.set_hotkey_error(Some(e));
                    }
                }
            }
        }
        if plan.reload_theme {
            match &new.theme {
                Some(p) => match Theme::load(p) {
                    Ok(t) => {
                        host.set_theme(t);
                        self.problems.remove("theme");
                    }
                    Err(e) => self.problem("theme", format!("The theme {} could not be loaded: {e:#}", p.display())),
                },
                None => {
                    host.set_theme(Theme::default());
                    self.problems.remove("theme");
                }
            }
        }
        if plan.rearm_watcher {
            if let Some(w) = &self.watcher {
                w.set_extra_roots(&self.paths, &new.workflow_dirs);
            }
            host.reload_workflow_hotkeys(&new.workflow_dirs);
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
                (self.notify)(p.clone());
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use ashdash_core::actions::{Cmd, Os, Process};

    use super::*;

    /// Records what the reload asks of the window.
    #[derive(Default)]
    struct FakeWindow {
        rebinds: Vec<(String, String)>,
        fail_rebind: bool,
        hotkey_error: Option<Option<String>>,
        themes: Vec<Theme>,
        workflow_hotkey_reloads: usize,
        workflow_hotkey_dirs: Vec<Vec<std::path::PathBuf>>,
    }

    impl Window for FakeWindow {
        fn rebind_hotkey(&mut self, old: &str, new: &str) -> Result<(), String> {
            self.rebinds.push((old.into(), new.into()));
            if self.fail_rebind { Err("taken".into()) } else { Ok(()) }
        }
        fn set_hotkey_error(&mut self, error: Option<String>) {
            self.hotkey_error = Some(error);
        }
        fn set_theme(&mut self, theme: Theme) {
            self.themes.push(theme);
        }
        fn reload_workflow_hotkeys(&mut self, workflow_dirs: &[std::path::PathBuf]) {
            self.workflow_hotkey_reloads += 1;
            self.workflow_hotkey_dirs.push(workflow_dirs.to_vec());
        }
    }

    struct NoOs;
    impl Os for NoOs {
        fn run(&self, _: &Cmd) -> anyhow::Result<()> {
            panic!("nothing may run")
        }
        fn output(&self, _: &Cmd) -> anyhow::Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn spawn(&self, _: &Cmd) -> anyhow::Result<Box<dyn Process>> {
            panic!("nothing may run")
        }
    }

    struct Rig {
        root: std::path::PathBuf,
        paths: Paths,
        engine: Arc<Engine>,
        reload: HotReload,
        notes: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn rig(tag: &str, config_file: Option<&[u8]>) -> Rig {
        let root = std::env::temp_dir().join(format!("oa-reload-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let paths = Paths::under(&root);
        if let Some(bytes) = config_file {
            std::fs::create_dir_all(&paths.data).unwrap();
            std::fs::write(paths.config_file(), bytes).unwrap();
        }
        let config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        let engine = Arc::new(Engine::with_env(
            paths.clone(),
            config,
            Box::new(NoOs),
            root.join("home"),
        ));
        let notes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notes);
        let reload = HotReload::with_notifier(
            &egui::Context::default(),
            Arc::clone(&engine),
            Some(engine.config.hotkey.clone()),
            Box::new(move |t| sink.lock().unwrap().push(t)),
        );
        Rig {
            root,
            paths,
            engine,
            reload,
            notes,
        }
    }

    fn web_search_titles(e: &Engine, q: &str) -> Vec<String> {
        e.query(q).into_iter().map(|i| i.title).collect()
    }

    #[test]
    fn an_edited_config_is_applied_to_the_running_engine() {
        let mut r = rig("apply", None);
        let mut w = FakeWindow::default();
        assert!(
            !web_search_titles(&r.engine, "zork hi")
                .iter()
                .any(|t| t.contains("Zork"))
        );
        let mut c = Config::load(&r.paths);
        c.web_searches.push(ashdash_core::websearch::WebSearch {
            keyword: "zork".into(),
            title: "Zork".into(),
            url: "https://example.test/?q={query}".into(),
            fallback: false,
        });
        c.max_results = 11;
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert!(
            web_search_titles(&r.engine, "zork hi")
                .iter()
                .any(|t| t.contains("Zork"))
        );
        assert_eq!(r.engine.config_now().max_results, 11);
        assert!(
            w.rebinds.is_empty() && w.themes.is_empty(),
            "only what changed is touched"
        );
        assert!(r.notes.lock().unwrap().is_empty());
        assert!(status::read(&r.paths).unwrap().problems.is_empty());
    }

    #[test]
    fn an_invalid_config_is_reported_once_and_changes_nothing() {
        let mut r = rig("invalid", None);
        let mut w = FakeWindow::default();
        let before = serde_json::to_value(r.engine.config_now()).unwrap();
        std::fs::create_dir_all(&r.paths.data).unwrap();
        std::fs::write(r.paths.config_file(), br#"{"max_results": "many"}"#).unwrap();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        assert_eq!(serde_json::to_value(r.engine.config_now()).unwrap(), before);
        let st = status::read(&r.paths).unwrap();
        assert_eq!(st.problems.len(), 1);
        assert!(st.problems[0].contains("config.json is not valid"), "{:?}", st.problems);
        assert_eq!(r.notes.lock().unwrap().len(), 1, "announced once, not on every poll");
        // fixing the file clears the problem
        Config::default().save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        assert!(status::read(&r.paths).unwrap().problems.is_empty());
    }

    #[test]
    fn a_config_that_was_unreadable_at_start_is_reported_at_once() {
        let r = rig("start-broken", Some(b"{ nope"));
        let st = status::read(&r.paths).unwrap();
        assert!(
            st.problems.iter().any(|p| p.contains("could not be read")),
            "{:?}",
            st.problems
        );
        assert_eq!(r.notes.lock().unwrap().len(), 1);
        assert_eq!(st.pid, std::process::id());
        assert_eq!(st.hotkey_active, r.engine.config.hotkey);
    }

    #[test]
    fn a_new_hotkey_is_rebound_and_a_failure_keeps_the_old_one() {
        let mut r = rig("hotkey", None);
        let old = r.engine.config.hotkey.clone();
        let mut w = FakeWindow::default();
        let mut c = Config::load(&r.paths);
        c.hotkey = "ctrl+alt+KeyJ".into();
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.rebinds, vec![(old.clone(), "ctrl+alt+KeyJ".to_string())]);
        assert_eq!(w.hotkey_error, Some(None));
        r.reload.publish();
        assert_eq!(status::read(&r.paths).unwrap().hotkey_active, "ctrl+alt+KeyJ");

        // now a hotkey that cannot be registered
        let mut w = FakeWindow {
            fail_rebind: true,
            ..FakeWindow::default()
        };
        let mut c = Config::load(&r.paths);
        c.hotkey = "ctrl+alt+KeyK".into();
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        let st = status::read(&r.paths).unwrap();
        assert_eq!(st.hotkey_active, "ctrl+alt+KeyJ", "the working hotkey stays");
        assert!(
            st.problems
                .iter()
                .any(|p| p.contains("ctrl+alt+KeyK") && p.contains("taken")),
            "{:?}",
            st.problems
        );
        assert!(st.problems[0].contains("previous hotkey stays active"));
        assert_eq!(w.hotkey_error, None, "the launcher still has a working hotkey");
        // the same value is retried on the next change, not silently forgotten
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.rebinds.len(), 2);
    }

    #[test]
    fn theme_changes_reload_the_theme_and_bad_themes_are_reported() {
        let mut r = rig("theme", None);
        let mut w = FakeWindow::default();
        let good = r.root.join("t.alfredappearance");
        std::fs::write(
            &good,
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>alfredtheme</key><dict>
            <key>name</key><string>Night</string></dict></dict></plist>"#,
        )
        .unwrap();
        let mut c = Config::load(&r.paths);
        c.theme = Some(good);
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.themes.len(), 1);
        assert_eq!(w.themes[0].name, "Night");

        let mut c = Config::load(&r.paths);
        c.theme = Some(r.root.join("missing.alfredappearance"));
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        assert_eq!(w.themes.len(), 1, "the working theme stays");
        assert!(status::read(&r.paths).unwrap().problems[0].contains("could not be loaded"));

        let mut c = Config::load(&r.paths);
        c.theme = None;
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.themes.len(), 2);
        r.reload.publish();
        assert!(status::read(&r.paths).unwrap().problems.is_empty());
    }

    #[test]
    fn restart_only_settings_are_listed_in_the_status() {
        let mut r = rig("restart", None);
        let mut w = FakeWindow::default();
        let mut c = Config::load(&r.paths);
        c.app_dirs.push("/somewhere".into());
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        r.reload.publish();
        assert_eq!(status::read(&r.paths).unwrap().restart_required, vec!["app_dirs"]);
    }

    /// The whole chain with the real file watcher: write the file, wait for the debounce.
    #[test]
    fn edits_on_disk_reach_the_engine_through_the_watcher() {
        let mut r = rig("watch", None);
        let mut w = FakeWindow::default();
        std::thread::sleep(Duration::from_millis(400)); // let FSEvents arm
        let mut c = Config::load(&r.paths);
        c.max_results = 9;
        c.save(&r.paths).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while r.engine.config_now().max_results != 9 && Instant::now() < end {
            r.reload.poll(&mut w);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(r.engine.config_now().max_results, 9, "watcher + debounce + apply");

        // a workflow dropped into the folder is loaded and its hotkeys re-read
        let dir = r.paths.workflows_dir().join("user.workflow.w");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
            <key>name</key><string>Dropped</string><key>bundleid</key><string>dropped.b</string>
            <key>objects</key><array/><key>connections</key><dict/></dict></plist>"#,
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while w.workflow_hotkey_reloads == 0 && Instant::now() < end {
            r.reload.poll(&mut w);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(w.workflow_hotkey_reloads >= 1);
        assert!(r.engine.workflow_summaries().iter().any(|(n, _, _)| n == "Dropped"));
    }

    #[test]
    fn workflow_hotkeys_are_reloaded_from_the_folders_now_in_force() {
        let mut r = rig("hk-dirs", None);
        let mut w = FakeWindow::default();
        let mut c = Config::load(&r.paths);
        c.workflow_dirs = vec![r.root.join("more")];
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.workflow_hotkey_dirs, vec![vec![r.root.join("more")]], "added folder");

        let mut c = Config::load(&r.paths);
        c.workflow_dirs.clear();
        c.save(&r.paths).unwrap();
        r.reload.apply_disk_config(&mut w);
        assert_eq!(w.workflow_hotkey_dirs.last(), Some(&Vec::new()), "removed folder");
    }

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
