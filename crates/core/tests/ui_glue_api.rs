//! The engine entry points the launcher UI relies on for its glue features:
//! "Open Launcher" outcomes, Script Filter placeholder rows, the Open With list
//! and Quick Look through the `Os` process handle. All through the public API,
//! in a fake environment, with a fake `Os` (nothing reaches the machine).

mod common;

use ashdash_core::format_ids::kind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ashdash_core::actions::{Cmd, Os, Process};
use ashdash_core::item::{Action, Mods};
use ashdash_core::{Engine, Outcome};
use common::*;
use serde_json::json;

#[derive(Default)]
struct Spy {
    /// What `output` answers (the `mdfind` listing).
    listing: String,
    spawned: Mutex<Vec<Cmd>>,
    kills: Arc<AtomicUsize>,
}

struct Handle(Arc<AtomicUsize>);

impl Process for Handle {
    fn kill(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn is_running(&self) -> bool {
        true
    }
}

struct SpyOs(Arc<Spy>);

impl Os for SpyOs {
    fn run(&self, _: &Cmd) -> anyhow::Result<()> {
        panic!("nothing may run")
    }
    fn output(&self, _: &Cmd) -> anyhow::Result<Vec<u8>> {
        Ok(self.0.listing.clone().into_bytes())
    }
    fn spawn(&self, cmd: &Cmd) -> anyhow::Result<Box<dyn Process>> {
        self.0.spawned.lock().unwrap().push(cmd.clone());
        Ok(Box::new(Handle(Arc::clone(&self.0.kills))))
    }
}

fn engine_with(env: &Env, spy: &Arc<Spy>) -> Engine {
    Engine::with_env(
        env.paths.clone(),
        env.config(),
        Box::new(SpyOs(Arc::clone(spy))),
        env.root.join("home"),
    )
}

#[test]
fn open_launcher_objects_come_back_as_an_outcome_not_an_error() {
    let env = Env::new("showlauncher", &[]);
    env.install_workflow(
        "user.workflow.go",
        &json!({
            "name": "Go", "bundleid": "qa.go",
            "objects": [
                {"uid": "K", "type": kind::INPUT_KEYWORD, "config": {
                    "keyword": "go", "withspace": true, "argumenttype": 1, "text": "Go", "subtext": ""}},
                {"uid": "B", "type": kind::ACTION_BROWSE, "config": {"path": "open {query}"}}
            ],
            "connections": {"K": [{"destinationuid": "B", "modifiers": 0}]}
        }),
    );
    let spy = Arc::new(Spy::default());
    let e = engine_with(&env, &spy);
    let items = e.query("go docs");
    let item = items
        .iter()
        .find(|i| matches!(i.action, Action::Workflow(_)))
        .expect("workflow row");
    assert_eq!(
        e.execute(item, "go docs", Mods::NONE, true),
        Outcome::ShowLauncher("open docs".into())
    );
}

#[test]
fn a_slow_script_filter_can_show_its_running_text_first() {
    let env = Env::new("pending", &[]);
    let mut doc = script_filter_workflow("sf", "sleep 0; echo '{\"items\":[]}'");
    doc["objects"][0]["config"]["runningsubtext"] = json!("Looking up {query}…");
    doc["objects"][0]["config"]["title"] = json!("Lookup");
    env.install_workflow("user.workflow.sf", &doc);
    let spy = Arc::new(Spy::default());
    let e = engine_with(&env, &spy);

    let pending = e.pending_items("sf cats");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].subtitle, "Looking up cats…");
    assert!(!pending[0].valid, "a placeholder cannot be run");
    // nothing to show without a typed argument, for other keywords, or for an empty query
    assert!(e.pending_items("sf").is_empty());
    assert!(e.pending_items("other cats").is_empty());
    assert!(e.pending_items("").is_empty());
    assert!(e.pending_items("   ").is_empty());
}

#[test]
fn open_with_lists_applications_found_by_the_os() {
    let env = Env::new("openwith", &[]);
    let spy = Arc::new(Spy {
        listing: "/Applications/Preview.app\n/Applications/Safari.app\n/Applications/Safari.app/Contents/Helper.app\n"
            .into(),
        ..Default::default()
    });
    let e = engine_with(&env, &spy);
    let all = e.open_with_items(Path::new("/tmp/a.pdf"), "", 10);
    let names: Vec<&str> = all.iter().map(|i| i.title.as_str()).collect();
    assert!(names.contains(&"Preview") && names.contains(&"Safari"), "{names:?}");
    assert!(!names.contains(&"Helper"), "nested helper apps are skipped");
    let filtered = e.open_with_items(Path::new("/tmp/a.pdf"), "saf", 10);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].title, "Safari");
    assert_eq!(
        e.open_with_items(Path::new("/tmp/a.pdf"), "", 1).len(),
        1,
        "limit applies"
    );
}

#[test]
fn quick_look_goes_through_the_os_and_can_be_killed() {
    let env = Env::new("ql", &[]);
    let spy = Arc::new(Spy::default());
    let e = engine_with(&env, &spy);
    let file: PathBuf = env.root.join("note.txt");
    std::fs::write(&file, "x").unwrap();

    let handle = e.spawn_quick_look(std::slice::from_ref(&file)).unwrap();
    {
        let spawned = spy.spawned.lock().unwrap();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].program, "qlmanage");
        assert!(spawned[0].args.iter().any(|a| a == "-p"));
        assert!(spawned[0].args.iter().any(|a| a == file.as_os_str()));
    }
    assert_eq!(spy.kills.load(Ordering::SeqCst), 0);
    handle.kill();
    assert_eq!(spy.kills.load(Ordering::SeqCst), 1);

    // option-looking and relative paths are refused before anything starts
    assert!(e.spawn_quick_look(&[PathBuf::from("-x")]).is_err());
    assert!(e.spawn_quick_look(&[PathBuf::from("relative.txt")]).is_err());
    assert_eq!(spy.spawned.lock().unwrap().len(), 1);
}
