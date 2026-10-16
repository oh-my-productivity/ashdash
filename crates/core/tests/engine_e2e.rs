//! End-to-end `Engine` behaviour through the public API only, in a fake
//! environment (apps in a temp dir, temp data/cache dirs). No test here ever
//! executes a real OS action: items are neutralised (`Action::None`) before
//! `execute`, and destructive system commands are only ever executed
//! *unconfirmed*.

mod common;

use ashdash_core::item::{Action, Item, Mods};
use ashdash_core::system::SystemCommand;
use ashdash_core::{Engine, Outcome};
use common::*;
use serde_json::json;

const APPS: &[&str] = &[
    "Safari",
    "Safari Technology Preview",
    "Visual Studio Code",
    "Calendar",
    "Calculator",
    "Activity Monitor",
    "System Settings",
    "Archive Utility",
];

fn titles(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.title.as_str()).collect()
}

fn is_fallback(i: &Item) -> bool {
    i.id.starts_with("fallback:")
}

// ---------------------------------------------------------------- ranking

#[test]
fn ranking_prefix_initials_and_exact() {
    let env = Env::new("rank", APPS);
    let e = env.engine();
    assert_eq!(e.app_count(), APPS.len());

    // exact title beats the longer title it prefixes
    let r = e.query("safari");
    assert_eq!(r[0].title, "Safari", "{:?}", titles(&r));
    let pos_tp = r.iter().position(|i| i.title == "Safari Technology Preview").unwrap();
    assert!(pos_tp > 0);

    // prefix
    assert_eq!(e.query("saf")[0].title, "Safari");
    // initials
    assert_eq!(e.query("vsc")[0].title, "Visual Studio Code");
    assert_eq!(e.query("am")[0].title, "Activity Monitor");
    // case-insensitive, leading whitespace ignored
    assert_eq!(e.query("  SAF")[0].title, "Safari");
    // word-prefix across words
    assert_eq!(e.query("sys set")[0].title, "System Settings");
}

#[test]
fn results_are_sorted_and_fallbacks_close_the_list() {
    let env = Env::new("sorted", APPS);
    let e = env.engine();
    let r = e.query("a");
    let ranked: Vec<_> = r.iter().filter(|i| !is_fallback(i)).collect();
    assert!(ranked.len() >= 3);
    assert!(ranked.windows(2).all(|w| w[0].score >= w[1].score), "not sorted: {r:?}");
    // fallbacks are a contiguous suffix
    let first_fb = r.iter().position(is_fallback).expect("fallback searches present");
    assert!(r[first_fb..].iter().all(is_fallback));
    // nothing matches: only the fallbacks remain
    let r = e.query("qqzzxxww");
    assert!(!r.is_empty() && r.iter().all(is_fallback), "{:?}", titles(&r));
}

#[test]
fn blank_queries_yield_nothing() {
    let env = Env::new("blank", APPS);
    let e = env.engine();
    for q in ["", " ", "   \t "] {
        assert!(e.query(q).is_empty(), "query {q:?}");
    }
}

#[test]
fn max_results_truncates_ranked_pool_but_not_trailing_fallbacks() {
    let env = Env::new("max", APPS);
    let mut cfg = env.config();
    cfg.max_results = 2;
    let e = Engine::new(env.paths.clone(), cfg);
    let r = e.query("a");
    let ranked = r.iter().filter(|i| !is_fallback(i)).count();
    assert_eq!(ranked, 2, "{:?}", titles(&r));
    assert!(r.iter().any(is_fallback));
}

#[test]
fn calculator_ranks_first_and_is_not_exclusive() {
    let env = Env::new("calc", APPS);
    let e = env.engine();
    let r = e.query("2+3*4");
    assert_eq!(r[0].title, "14");
    assert!(r.iter().any(is_fallback), "calc must not suppress fallbacks");
    assert_eq!(e.query("= sqrt(81)")[0].title, "9");
}

// ---------------------------------------------------------------- exclusivity / precedence

/// Built-in prefixes that take over the whole list and therefore must not be
/// followed by fallback web searches or app matches.
#[test]
fn builtin_exclusive_prefixes_hide_everything_else() {
    let env = Env::new("excl", APPS);
    let e = env.engine();
    let cases: &[(&str, &str)] = &[
        ("> ls -la", "shell"),
        ("g rust lifetimes", "web keyword"),
        ("ddg something", "web keyword"),
        ("clip", "clipboard"),
        ("clipboard foo", "clipboard"),
        ("snip", "snippets"),
        ("snip foo", "snippets"),
    ];
    for (q, what) in cases {
        let r = e.query(q);
        assert!(!r.is_empty(), "{what}: {q:?} gave nothing");
        assert!(
            !r.iter().any(is_fallback),
            "{what}: {q:?} leaked fallbacks: {:?}",
            titles(&r)
        );
        assert!(
            !r.iter().any(|i| APPS.contains(&i.title.as_str())),
            "{what}: {q:?} leaked apps"
        );
    }
    // The shell item is a single Terminal action and carries the command verbatim.
    let r = e.query("> echo hi");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].action, Action::Shell("echo hi".into()));
    // `> ` with no command is shown but not actionable.
    assert!(!e.query(">")[0].valid);
}

#[test]
fn keyword_must_be_a_whole_word_to_be_exclusive() {
    let env = Env::new("kwword", APPS);
    let e = env.engine();
    // "gx" is not "g x": falls through to the normal pipeline (fallbacks present)
    assert!(e.query("gx foo").iter().any(is_fallback));
    // "clipper" must not be taken as the clipboard keyword
    assert!(e.query("clipper").iter().any(is_fallback));
}

/// A workflow keyword equal to any built-in prefix must win (workflows are first in the registry).
#[test]
fn workflow_keyword_beats_every_builtin_prefix() {
    for (kw, query) in [
        ("g", "g hello"),
        ("ddg", "ddg hello"),
        ("clip", "clip hello"),
        ("clipboard", "clipboard hello"),
        ("snip", "snip hello"),
        ("open", "open hello"),
        ("find", "find hello"),
    ] {
        let env = Env::new(&format!("prec-{kw}"), APPS);
        env.install_workflow("user.workflow.mine", &keyword_workflow("Mine", "qa.mine", kw));
        let e = env.engine();
        let r = e.query(query);
        assert_eq!(r.len(), 1, "{kw}: {:?}", titles(&r));
        assert_eq!(r[0].title, "Mine", "{kw}");
        assert!(matches!(r[0].action, Action::Workflow(_)), "{kw}");
    }
}

/// `>` is the shell prefix; a workflow with a no-space `>` keyword is still first.
#[test]
fn workflow_keyword_beats_shell_prefix() {
    let env = Env::new("prec-shell", APPS);
    let mut doc = keyword_workflow("Gt", "qa.gt", ">");
    doc["objects"][0]["config"]["withspace"] = json!(false);
    env.install_workflow("user.workflow.gt", &doc);
    let e = env.engine();
    let r = e.query(">ls");
    assert_eq!(r[0].title, "Gt");
    assert!(!matches!(r[0].action, Action::Shell(_)));
}

#[test]
fn disabled_workflow_does_not_shadow_builtins() {
    let env = Env::new("prec-off", APPS);
    let mut doc = keyword_workflow("Off", "qa.off", "g");
    doc["disabled"] = json!(true);
    env.install_workflow("user.workflow.off", &doc);
    let e = env.engine();
    let r = e.query("g hello");
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].action,
        Action::OpenUrl("https://www.google.com/search?q=hello".into())
    );
    assert_eq!(e.workflow_summaries(), vec![("Off".into(), "qa.off".into(), true)]);
}

#[test]
fn partial_workflow_keyword_is_offered_alongside_apps() {
    let env = Env::new("partial", APPS);
    env.install_workflow("user.workflow.w", &keyword_workflow("Weather", "qa.w", "weather"));
    let e = env.engine();
    let r = e.query("wea");
    let w = r.iter().find(|i| i.title == "Weather").expect("partial keyword hint");
    assert!(!w.valid);
    assert_eq!(w.autocomplete.as_deref(), Some("weather "));
    // still shows fallbacks => not exclusive
    assert!(r.iter().any(is_fallback));
}

#[test]
fn exclusive_output_respects_max_results() {
    let env = Env::new("exmax", APPS);
    std::fs::create_dir_all(env.paths.data.clone()).unwrap();
    let snippets: Vec<_> = (0..12)
        .map(|i| json!({"name": format!("snippet {i}"), "keyword": format!("k{i}"), "text": "t"}))
        .collect();
    std::fs::write(env.paths.snippets_file(), serde_json::to_vec(&snippets).unwrap()).unwrap();
    let mut cfg = env.config();
    cfg.max_results = 4;
    let e = Engine::new(env.paths.clone(), cfg);
    assert_eq!(e.query("snip snippet").len(), 4);
}

#[test]
fn path_browsing_is_exclusive_when_it_matches() {
    let env = Env::new("browse", APPS);
    let dir = env.root.join("browse-me");
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join("alps")).unwrap();
    let e = env.engine();
    let r = e.query(&format!("{}/al", dir.display()));
    assert_eq!(r.len(), 2, "{:?}", titles(&r));
    assert!(!r.iter().any(is_fallback));
    // a path that matches nothing falls through to the normal pipeline
    let r = e.query(&format!("{}/zzzz", dir.display()));
    assert!(r.iter().any(is_fallback));
}

// ---------------------------------------------------------------- usage learning

fn neutral(i: &Item) -> Item {
    // Same id (what usage learning keys on) but no side effect when executed.
    Item {
        action: Action::None,
        cmd_action: None,
        ..i.clone()
    }
}

#[test]
fn usage_learning_persists_across_engine_instances() {
    let env = Env::new("usage", APPS);
    let baseline: Vec<Item> = env
        .engine()
        .query("a")
        .into_iter()
        .filter(|i| !is_fallback(i))
        .collect();
    assert!(baseline.len() >= 4, "{:?}", titles(&baseline));
    // Pick a mid-ranked entry: repeated picks for this exact query must lift it to the top.
    let target = baseline[3].clone();
    assert!(baseline[0].score - target.score < 500.0, "{baseline:?}");
    assert_ne!(baseline[0].id, target.id);

    {
        let e = env.engine();
        for _ in 0..10 {
            assert_eq!(e.execute(&neutral(&target), "a", Mods::NONE, false), Outcome::Done);
        }
    } // engine dropped

    assert!(env.paths.usage_file().is_file(), "usage.json must be written");
    let e2 = env.engine();
    let r = e2.query("a");
    assert_eq!(r[0].id, target.id, "after learning: {:?}", titles(&r));

    // a different query is only mildly affected: exact-query boost does not leak
    let other = e2.query("zzz");
    assert!(other.iter().all(|i| i.id != target.id));
}

#[test]
fn invalid_items_are_not_executed_or_learned() {
    let env = Env::new("usage-invalid", APPS);
    let e = env.engine();
    let mut it = Item::new("app:ghost", "Ghost").action(Action::None);
    it.valid = false;
    assert_eq!(e.execute(&it, "gh", Mods::NONE, false), Outcome::Done);
    assert!(!env.paths.usage_file().exists());
}

#[test]
fn corrupt_state_files_do_not_stop_the_engine() {
    let env = Env::new("corrupt", APPS);
    std::fs::create_dir_all(&env.paths.data).unwrap();
    std::fs::write(env.paths.usage_file(), b"\xff\xfenot json").unwrap();
    std::fs::write(env.paths.clipboard_file(), b"{{{").unwrap();
    std::fs::write(env.paths.snippets_file(), b"[1,2,3]").unwrap();
    let e = env.engine();
    assert_eq!(e.query("saf")[0].title, "Safari");
    assert_eq!(e.query("clip")[0].title, "Clipboard history is empty");
    assert_eq!(e.query("snip x")[0].title, "No snippets");
}

#[test]
fn usage_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("usage-perm", APPS);
    let e = env.engine();
    let it = neutral(&e.query("saf")[0]);
    e.execute(&it, "saf", Mods::NONE, false);
    let mode = std::fs::metadata(env.paths.usage_file()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "usage.json mode is {mode:o}");
}

// ---------------------------------------------------------------- destructive system commands

#[test]
fn exactly_the_destructive_commands_need_confirmation() {
    let need: Vec<_> = SystemCommand::ALL
        .iter()
        .filter(|c| c.needs_confirmation())
        .map(|c| c.title())
        .collect();
    assert_eq!(need, vec!["Log Out", "Restart", "Shut Down", "Empty Trash"]);
}

/// SAFETY: this test only ever calls `execute(.., confirmed = false)` on destructive commands.
#[test]
fn destructive_commands_ask_before_running_and_are_not_learned() {
    let env = Env::new("confirm", APPS);
    let e = env.engine();
    for (query, title) in [
        ("shut down", "Shut Down"),
        ("restart", "Restart"),
        ("log out", "Log Out"),
        ("empty trash", "Empty Trash"),
    ] {
        let item = e
            .query(query)
            .into_iter()
            .find(|i| i.title == title)
            .unwrap_or_else(|| panic!("no '{title}' item for query '{query}'"));
        assert!(matches!(item.action, Action::System(_)));
        match e.execute(&item, query, Mods::NONE, false) {
            Outcome::Confirm(msg) => assert!(msg.contains(title), "{msg}"),
            other => panic!("{title}: expected Confirm, got {other:?}"),
        }
        // repeating the unconfirmed call keeps asking
        assert!(matches!(
            e.execute(&item, query, Mods::NONE, false),
            Outcome::Confirm(_)
        ));
    }
    // asking is not picking: nothing was recorded
    assert!(!env.paths.usage_file().exists());
}

/// The Cmd-modifier alternative action must go through the same gate.
#[test]
fn cmd_modifier_cannot_bypass_confirmation() {
    let env = Env::new("confirm-cmd", APPS);
    let e = env.engine();
    let mut it = Item::new("app:innocent", "Innocent").action(Action::None);
    it.cmd_action = Some(Action::System(SystemCommand::ShutDown));
    let cmd = Mods {
        cmd: true,
        ..Mods::NONE
    };
    assert!(matches!(e.execute(&it, "x", cmd, false), Outcome::Confirm(_)));
}

#[test]
fn invalid_destructive_item_does_nothing() {
    let env = Env::new("confirm-invalid", APPS);
    let e = env.engine();
    let mut it = Item::new("system:Shut Down", "Shut Down").action(Action::System(SystemCommand::ShutDown));
    it.valid = false;
    // not even a prompt
    assert_eq!(e.execute(&it, "x", Mods::NONE, false), Outcome::Done);
}

// ---------------------------------------------------------------- workflows through the engine

#[test]
fn workflow_keyword_executes_to_large_type_without_os_side_effects() {
    let env = Env::new("wf", APPS);
    env.install_workflow("user.workflow.shout", &keyword_workflow("Shout", "qa.shout", "shout"));
    let e = env.engine();
    let r = e.query("shout hello");
    assert_eq!(r[0].subtitle, "sub hello");
    assert_eq!(
        e.execute(&r[0], "shout hello", Mods::NONE, false),
        Outcome::LargeType("hello!".into())
    );
}

#[test]
fn script_filter_items_flow_through_engine() {
    let env = Env::new("sf", APPS);
    let script = r#"printf '{"items":[{"title":"first %s","arg":"A"},{"title":"second","subtitle":"s"}]}' "$1""#;
    env.install_workflow("user.workflow.sf", &script_filter_workflow("sf", script));
    let e = env.engine();
    let r = e.query("sf zed");
    assert_eq!(titles(&r), vec!["first zed", "second"]);
    assert!(!r.iter().any(is_fallback));
}

#[test]
fn external_trigger_reports_errors_for_unknown_targets() {
    let env = Env::new("trigger", APPS);
    let e = env.engine();
    assert!(matches!(e.run_trigger("no.such.bundle", "t", ""), Outcome::Error(_)));
    env.install_workflow("user.workflow.k", &keyword_workflow("K", "qa.k", "kk"));
    e.reload_blocking();
    assert!(matches!(e.run_trigger("qa.k", "missing", ""), Outcome::Error(_)));
}

#[test]
fn external_trigger_runs_to_large_type() {
    let env = Env::new("trigger-ok", APPS);
    let doc = json!({
        "name": "Trig", "bundleid": "qa.trig",
        "objects": [
            {"uid": "T", "type": "alfred.workflow.trigger.external", "config": {"triggerid": "go"}},
            {"uid": "L", "type": "alfred.workflow.output.largetype", "config": {"largetypetext": "got {query}"}}
        ],
        "connections": {"T": [{"destinationuid": "L", "modifiers": 0}]}
    });
    env.install_workflow("user.workflow.trig", &doc);
    let e = env.engine();
    assert_eq!(
        e.run_trigger("qa.trig", "go", "xyz"),
        Outcome::LargeType("got xyz".into())
    );
}

// ---------------------------------------------------------------- run_node (workflow hotkeys)

fn hotkey_like_workflow(bundle: &str) -> serde_json::Value {
    json!({
        "name": "Hk", "bundleid": bundle,
        "objects": [
            {"uid": "H", "type": "alfred.workflow.trigger.hotkey", "config": {"hotkey": 0, "hotmod": 0}},
            {"uid": "L", "type": "alfred.workflow.output.largetype", "config": {"largetypetext": "{var:who} got {query}"}}
        ],
        "connections": {"H": [{"destinationuid": "L", "modifiers": 0}]}
    })
}

#[test]
fn run_node_starts_at_the_node_with_arg_and_vars_and_records_no_usage() {
    let env = Env::new("run-node", APPS);
    env.install_workflow("user.workflow.hk", &hotkey_like_workflow("qa.hk"));
    let e = env.engine();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("who".to_string(), "me".to_string());
    assert_eq!(
        e.run_node("user.workflow.hk", "H", "payload", vars),
        Outcome::LargeType("me got payload".into())
    );
    let usage = std::fs::read_to_string(env.paths.usage_file()).unwrap_or_default();
    assert!(!usage.contains("hotkey") && !usage.contains("qa.hk"), "usage: {usage}");
}

#[test]
fn run_node_reports_unknown_bundle_node_and_disabled_workflows() {
    let env = Env::new("run-node-err", APPS);
    let mut doc = hotkey_like_workflow("qa.off");
    env.install_workflow("user.workflow.hk", &hotkey_like_workflow("qa.hk"));
    doc["disabled"] = json!(true);
    env.install_workflow("user.workflow.off", &doc);
    let e = env.engine();
    let none = std::collections::BTreeMap::new;
    assert!(matches!(e.run_node("no.such", "H", "", none()), Outcome::Error(_)));
    assert!(matches!(e.run_node("user.workflow.hk", "NOPE", "", none()), Outcome::Error(_)));
    assert!(matches!(e.run_node("user.workflow.off", "H", "", none()), Outcome::Error(_)));
}

/// A hotkey workflow whose output text names the copy that ran.
fn marked_hotkey_workflow(bundle: &str, mark: &str) -> serde_json::Value {
    let mut doc = hotkey_like_workflow(bundle);
    doc["objects"][1]["config"]["largetypetext"] = json!(format!("{mark}:{{query}}"));
    doc
}

#[test]
fn run_node_works_for_a_workflow_without_a_bundle_id() {
    let env = Env::new("run-node-noid", APPS);
    env.install_workflow("user.workflow.noid", &marked_hotkey_workflow("", "solo"));
    let e = env.engine();
    assert_eq!(
        e.run_node("user.workflow.noid", "H", "x", Default::default()),
        Outcome::LargeType("solo:x".into())
    );
}

#[test]
fn run_node_runs_the_copy_that_owns_the_hotkey_when_bundle_ids_collide() {
    let env = Env::new("run-node-dup", APPS);
    env.install_workflow("user.workflow.one", &marked_hotkey_workflow("qa.same", "first"));
    let second_root = env.root.join("more-workflows");
    std::fs::create_dir_all(second_root.join("copy-two")).unwrap();
    common::write_info_plist(&second_root.join("copy-two"), &marked_hotkey_workflow("qa.same", "second"));
    let mut cfg = env.config();
    cfg.workflow_dirs = vec![second_root];
    let e = Engine::new(env.paths.clone(), cfg);
    assert_eq!(
        e.run_node("copy-two", "H", "x", Default::default()),
        Outcome::LargeType("second:x".into())
    );
    assert_eq!(
        e.run_node("user.workflow.one", "H", "x", Default::default()),
        Outcome::LargeType("first:x".into())
    );
}
