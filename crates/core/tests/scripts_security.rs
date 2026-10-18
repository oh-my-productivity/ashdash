//! Script execution behaviour that matters for security: argument passing,
//! `{query}` substitution, environment, timeouts, output volume, graph limits
//! and argument handling of the OS helpers. See docs/SECURITY.md.
//!
//! Tests marked `#[ignore = "known: SEC-nn ..."]` demonstrate an open finding
//! and should be un-ignored when it is fixed. Tests named `documents_*`
//! pin *intentional* (workflow-format) behaviour that callers must know about.
//!
//! Everything runs in a temp dir; the scripts are authored here.

mod common;

use ashdash_core::format_ids::{ENV_WORKFLOW_BUNDLEID, ENV_WORKFLOW_DATA, kind};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ashdash_core::item::{Action, Mods};
use ashdash_core::workflow::{Effect, Workflow};
use ashdash_core::{Item, Paths};
use common::*;
use serde_json::{Value, json};

struct Fx {
    root: TempDir,
    paths: Paths,
}

impl Fx {
    fn new(tag: &str) -> Fx {
        let root = TempDir::new(tag);
        let paths = Paths::under(&root.join("env"));
        Fx { root, paths }
    }
    fn wf(&self, doc: &Value) -> Workflow {
        let dir = self.root.join("wf");
        std::fs::create_dir_all(&dir).unwrap();
        Workflow::from_json(&dir, "wf", doc).unwrap()
    }
    fn items(&self, wf: &Workflow, q: &str) -> Vec<Item> {
        wf.keyword_items(q, &self.paths).0
    }
    fn effects(&self, wf: &Workflow, item: &Item) -> Vec<Effect> {
        let Action::Workflow(act) = &item.action else {
            panic!("not a workflow item: {item:?}")
        };
        wf.run(act, Mods::NONE, &self.paths)
    }
}

/// keyword `kw` (arg required) -> Run Script (`script_type`, `argtype`, `escaping`) -> notification "{query}".
fn keyword_script(kw: &str, script: &str, argtype: i64, escaping: i64) -> Value {
    json!({
        "name": "KS", "bundleid": "qa.ks",
        "objects": [
            {"uid": "K", "type": kind::INPUT_KEYWORD, "config": {
                "keyword": kw, "withspace": true, "argumenttype": 0, "text": "KS", "subtext": ""}},
            {"uid": "S", "type": kind::ACTION_SCRIPT, "config": {
                "type": 0, "scriptargtype": argtype, "escaping": escaping, "script": script}},
            {"uid": "N", "type": kind::OUTPUT_NOTIFICATION, "config": {"title": "T", "text": "{query}"}}
        ],
        "connections": {
            "K": [{"destinationuid": "S", "modifiers": 0}],
            "S": [{"destinationuid": "N", "modifiers": 0}]
        }
    })
}

fn notification_text(fx: &[Effect]) -> String {
    fx.iter()
        .find_map(|e| match e {
            Effect::Notify { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no notification in {fx:?}"))
}

fn run_keyword_script(fx: &Fx, script: &str, argtype: i64, escaping: i64, arg: &str) -> Vec<Effect> {
    let wf = fx.wf(&keyword_script("ks", script, argtype, escaping));
    let items = fx.items(&wf, &format!("ks {arg}"));
    assert_eq!(items.len(), 1, "{items:?}");
    fx.effects(&wf, &items[0])
}

// ---------------------------------------------------------------- argument passing

const NASTY: &str = "a'b\"c$d`e\\f;g|h&i<j>k(l)m{n}o\np q*?~#!%^";

#[test]
fn argv_mode_passes_hostile_text_literally() {
    let fx = Fx::new("argv");
    let marker = fx.root.join("argv-marker");
    let evil = format!("x; touch {} #", marker.display());
    let out = run_keyword_script(&fx, r#"printf '%s' "$1""#, 1, 102, &evil);
    assert_eq!(notification_text(&out), evil);
    assert!(!marker.exists());
    // every metacharacter survives untouched
    let out = run_keyword_script(&fx, r#"printf '%s' "$1""#, 1, 102, NASTY);
    assert_eq!(notification_text(&out), NASTY);
}

#[test]
fn stdin_mode_passes_hostile_text_literally() {
    let fx = Fx::new("stdin");
    let out = run_keyword_script(&fx, "cat", 2, 102, NASTY);
    assert_eq!(notification_text(&out), NASTY);
}

/// Dollar, backtick, double quote and backslash are escaped by the default mask (102).
#[test]
fn default_mask_blocks_dollar_backtick_and_quote_breakout() {
    let fx = Fx::new("mask-ok");
    let marker = fx.root.join("mask-marker");
    let m = marker.display();
    for arg in [
        format!("$(touch {m})"),
        format!("`touch {m}`"),
        format!("\"; touch {m}; echo \""),
        format!("\\\"; touch {m}; echo \\\""),
    ] {
        // the common shape: {query} inside double quotes
        let _ = run_keyword_script(&fx, r#"echo -n "{query}""#, 0, 102, &arg);
        assert!(!marker.exists(), "injection through {arg:?}");
    }
}

/// SEC-03 (documented behaviour): the default escaping mask does NOT neutralise `;`, `|`, `&`,
/// `'`, `<`, `>` or newlines. `{query}` substitution is therefore code injection whenever the
/// value can be influenced by someone other than the user (Script Filter `arg`, clipboard,
/// file names). Pinned so that a change (better escaping, or a different default) is noticed
/// and SECURITY.md is updated.
#[test]
fn documents_default_mask_leaves_shell_metacharacters_live() {
    let fx = Fx::new("mask-gap");
    let m = fx.root.join("m");
    let ms = m.display();
    for (what, script, arg) in [
        (
            "semicolon, bare",
            "echo {query}".to_string(),
            format!("x; touch {ms}-semi"),
        ),
        (
            "single-quote breakout",
            "echo '{query}'".to_string(),
            format!("'; touch {ms}-squote; echo '"),
        ),
        ("pipe/amp", "echo {query}".to_string(), format!("x & touch {ms}-amp")),
        ("newline", "echo {query}".to_string(), format!("x\ntouch {ms}-nl")),
    ] {
        let _ = run_keyword_script(&fx, &script, 0, 102, &arg);
        let suffix = match what {
            "semicolon, bare" => "semi",
            "single-quote breakout" => "squote",
            "pipe/amp" => "amp",
            _ => "nl",
        };
        // `&` backgrounds the command; give it a moment
        std::thread::sleep(Duration::from_millis(if what == "pipe/amp" { 200 } else { 0 }));
        assert!(
            Path::new(&format!("{ms}-{suffix}")).exists(),
            "{what}: expected the documented injection to work (if this fails, escaping improved: update SEC-03)"
        );
    }
}

// ---------------------------------------------------------------- environment

#[test]
fn workflow_variables_reach_scripts_and_data_dirs_are_private_to_the_env() {
    let fx = Fx::new("env");
    let doc = json!({
        "name": "E", "bundleid": "qa.env", "variables": {"greeting": "hello"},
        "objects": [
            {"uid": "K", "type": kind::INPUT_KEYWORD, "config": {
                "keyword": "e", "withspace": true, "argumenttype": 1, "text": "E"}},
            {"uid": "S", "type": kind::ACTION_SCRIPT, "config": {
                "type": 0, "scriptargtype": 1,
                "script": format!("printf '%s|%s|%s' \"$greeting\" \"${ENV_WORKFLOW_DATA}\" \"${ENV_WORKFLOW_BUNDLEID}\"")}},
            {"uid": "N", "type": kind::OUTPUT_NOTIFICATION, "config": {"title": "T", "text": "{query}"}}
        ],
        "connections": {"K": [{"destinationuid": "S", "modifiers": 0}], "S": [{"destinationuid": "N", "modifiers": 0}]}
    });
    let wf = fx.wf(&doc);
    let items = fx.items(&wf, "e x");
    let text = notification_text(&fx.effects(&wf, &items[0]));
    let parts: Vec<&str> = text.split('|').collect();
    assert_eq!(parts[0], "hello");
    assert!(
        Path::new(parts[1]).starts_with(fx.paths.data.join("workflow-data")),
        "data dir {} not under the data root",
        parts[1]
    );
    assert!(Path::new(parts[1]).is_dir());
    assert_eq!(parts[2], "qa.env");
}

/// Script Filter output (`variables`) is untrusted data, yet it becomes process environment of
/// the next Run Script. Loader/interpreter variables turn that into code execution.
#[test]
fn script_filter_variables_cannot_inject_loader_env() {
    let fx = Fx::new("env-inject");
    let marker = fx.root.join("bash-env-marker");
    let rc = fx.root.join("evil-rc");
    std::fs::write(&rc, format!("touch '{}'\n", marker.display())).unwrap();
    let sf = format!(
        r#"printf '{{"items":[{{"title":"t","arg":"a","variables":{{"BASH_ENV":"{}"}}}}]}}'"#,
        rc.display()
    );
    let doc = json!({
        "name": "I", "bundleid": "qa.inject",
        "objects": [
            {"uid": "SF", "type": kind::INPUT_SCRIPTFILTER, "config": {
                "keyword": "inj", "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1, "title": "I", "script": sf}},
            {"uid": "S", "type": kind::ACTION_SCRIPT, "config": {
                "type": 0, "scriptargtype": 1, "script": "echo -n ok"}},
        ],
        "connections": {"SF": [{"destinationuid": "S", "modifiers": 0}]}
    });
    let wf = fx.wf(&doc);
    let items = fx.items(&wf, "inj x");
    assert_eq!(items[0].title, "t");
    let _ = fx.effects(&wf, &items[0]);
    assert!(!marker.exists(), "BASH_ENV from Script Filter output was honoured");
}

/// Same vector through PATH: a Script Filter result can choose which `ls` the next script runs.
#[test]
fn script_filter_variables_cannot_replace_path() {
    let fx = Fx::new("env-path");
    let bin = fx.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = fx.root.join("path-marker");
    let fake = bin.join("date");
    std::fs::write(&fake, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let sf = format!(
        r#"printf '{{"items":[{{"title":"t","arg":"a","variables":{{"PATH":"{}:/usr/bin:/bin"}}}}]}}'"#,
        bin.display()
    );
    let mut doc = keyword_script("zz", "date", 1, 102);
    doc["objects"][0] = json!({"uid": "K", "type": kind::INPUT_SCRIPTFILTER, "config": {
        "keyword": "p", "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1, "title": "P", "script": sf}});
    let wf = fx.wf(&doc);
    let items = fx.items(&wf, "p x");
    let _ = fx.effects(&wf, &items[0]);
    assert!(!marker.exists(), "PATH from Script Filter output was honoured");
}

/// SEC-14 (documented): scripts see the parent's whole environment.
#[test]
fn documents_scripts_inherit_parent_environment() {
    let fx = Fx::new("env-inherit");
    let out = run_keyword_script(&fx, r#"printf '%s' "${HOME:-unset}""#, 1, 102, "x");
    let home = std::env::var("HOME").unwrap_or_default();
    assert_eq!(
        notification_text(&out),
        if home.is_empty() { "unset".into() } else { home }
    );
}

/// SEC-12: the per-workflow data directory is keyed on the raw bundle id, so a bundle id with
/// `..` creates directories outside the data root when any script of that workflow runs.
#[test]
fn bundle_id_cannot_steer_workflow_data_dir() {
    let fx = Fx::new("datakey");
    let escape = fx.root.join("escaped-data-dir");
    // data root = <root>/env/data ; data/workflow-data/<key> ; key climbs 3 levels to <root>
    let doc = {
        let mut d = keyword_script("ks", "echo -n hi", 1, 102);
        d["bundleid"] = json!("../../../escaped-data-dir");
        d
    };
    let wf = fx.wf(&doc);
    let items = fx.items(&wf, "ks x");
    let _ = fx.effects(&wf, &items[0]);
    assert!(
        !escape.exists(),
        "directory created outside the data root: {}",
        escape.display()
    );
}

/// SEC-15: the temporary script file holds the substituted source (which may contain
/// clipboard/query data) and should not be world-readable while it exists.
#[test]
fn temp_script_file_is_private() {
    let fx = Fx::new("tmpmode");
    // the script prints its own file mode
    let out = run_keyword_script(&fx, r#"stat -f %Lp "$0""#, 1, 102, "x");
    assert_eq!(notification_text(&out).trim(), "600");
}

#[test]
fn temp_script_file_is_removed_after_the_run() {
    let fx = Fx::new("tmpclean");
    let _ = run_keyword_script(&fx, "echo -n hi", 1, 102, "x");
    let dir = fx.paths.cache.join("scripts");
    let left: Vec<_> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "left: {left:?}");
}

// ---------------------------------------------------------------- process lifecycle

fn sf_workflow(script: &str) -> Value {
    script_filter_workflow("sf", script)
}

/// A script that exits while a background child still holds its stdout must not stall the launcher.
#[test]
fn backgrounded_grandchild_does_not_stall_the_script_filter() {
    let fx = Fx::new("pipehang");
    let wf = fx.wf(&sf_workflow(r#"sleep 6 & echo '{"items":[{"title":"fast"}]}'"#));
    let start = Instant::now();
    let items = fx.items(&wf, "sf x");
    let took = start.elapsed();
    assert_eq!(items[0].title, "fast");
    assert!(took < Duration::from_secs(3), "returned after {took:?}");
}

/// On timeout the whole process group should die, not just the interpreter.
#[test]
fn timeout_kills_grandchildren() {
    let fx = Fx::new("timeout");
    let pidfile = fx.root.join("pid");
    let script = format!("sleep 300 & echo $! > '{}'; wait", pidfile.display());
    let wf = fx.wf(&sf_workflow(&script));
    let start = Instant::now();
    let items = fx.items(&wf, "sf x");
    assert!(items[0].title.contains("script error"), "{items:?}");
    assert!(start.elapsed() < Duration::from_secs(15));
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .expect("script wrote its child's pid")
        .trim()
        .parse()
        .unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success();
    if alive {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    assert!(!alive, "grandchild {pid} survived the timeout");
}

/// Output volume: a Script Filter that prints 64 MiB must be rejected or truncated, not held in memory.
#[test]
fn huge_script_output_is_bounded() {
    let fx = Fx::new("hugeout");
    let script = r#"printf '{"items":[{"title":"'; head -c 67108864 /dev/zero | tr '\0' 'a'; printf '"}]}'"#;
    let wf = fx.wf(&sf_workflow(script));
    let items = fx.items(&wf, "sf x");
    assert!(
        items
            .iter()
            .all(|i| i.title.len() < 1_000_000 && i.subtitle.len() < 1_000_000),
        "a {} byte title came through",
        items.iter().map(|i| i.title.len()).max().unwrap_or(0)
    );
}

/// Moderate output must keep working (guard for whatever limit gets added).
#[test]
fn large_but_reasonable_output_works() {
    let fx = Fx::new("midout");
    let script = r#"printf '{"items":['; for i in $(seq 1 2000); do [ $i -gt 1 ] && printf ','; printf '{"title":"item %s"}' $i; done; printf ']}'"#;
    let wf = fx.wf(&sf_workflow(script));
    let items = fx.items(&wf, "sf x");
    assert_eq!(items.len(), 2000);
}

#[test]
fn script_failures_surface_as_error_items_not_panics() {
    let fx = Fx::new("fail");
    for script in [
        "exit 3",
        "echo oops >&2; exit 1",
        "kill -9 $$",
        "printf '\\377\\376{'",
        "exec 1>&-; sleep 0.1",
        "/nonexistent/binary",
    ] {
        let wf = fx.wf(&sf_workflow(script));
        let items = fx.items(&wf, "sf x");
        assert!(!items.is_empty(), "{script}");
        assert!(
            items[0].title.contains("script error") || items[0].title == "No results",
            "{script}: {items:?}"
        );
    }
}

#[test]
fn unknown_script_type_is_an_error_item() {
    let fx = Fx::new("badtype");
    let mut doc = sf_workflow("echo hi");
    doc["objects"][0]["config"]["type"] = json!(99);
    let wf = fx.wf(&doc);
    let items = fx.items(&wf, "sf x");
    assert!(items[0].title.contains("script error"), "{items:?}");
}

// ---------------------------------------------------------------- graph limits

#[test]
fn long_chain_is_cut_off_with_a_warning_and_finishes_fast() {
    let fx = Fx::new("chain");
    let n = 20_000;
    let objects: Vec<Value> = (0..n)
        .map(|i| json!({"uid": format!("J{i}"), "type": kind::UTILITY_JUNCTION, "config": {}}))
        .collect();
    let mut conns = serde_json::Map::new();
    for i in 0..n - 1 {
        conns.insert(
            format!("J{i}"),
            json!([{"destinationuid": format!("J{}", i + 1), "modifiers": 0}]),
        );
    }
    let doc = json!({"name": "Long", "bundleid": "qa.long", "objects": objects, "connections": conns});
    let start = Instant::now();
    let wf = fx.wf(&doc);
    let act = ashdash_core::workflow::WorkflowAction {
        workflow: "wf".into(),
        from_uid: "J0".into(),
        arg: String::new(),
        vars: BTreeMap::new(),
        mods: vec![],
        ..Default::default()
    };
    let fx_out = wf.run(&act, Mods::NONE, &fx.paths);
    assert!(
        matches!(fx_out.last(), Some(Effect::Warn(w)) if w.contains("too deep")),
        "{fx_out:?}"
    );
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
}

#[test]
fn wide_fanout_and_self_loops_are_bounded() {
    let fx = Fx::new("fanout");
    let mut conns = serde_json::Map::new();
    let dests: Vec<Value> = (0..5000)
        .map(|_| json!({"destinationuid": "A", "modifiers": 0}))
        .collect();
    conns.insert("A".into(), Value::Array(dests));
    let doc = json!({"name": "Fan", "bundleid": "qa.fan",
        "objects": [{"uid": "A", "type": kind::UTILITY_JUNCTION, "config": {}}],
        "connections": conns});
    let wf = fx.wf(&doc);
    let act = ashdash_core::workflow::WorkflowAction {
        workflow: "wf".into(),
        from_uid: "A".into(),
        arg: String::new(),
        vars: BTreeMap::new(),
        mods: vec![],
        ..Default::default()
    };
    let start = Instant::now();
    let out = wf.run(&act, Mods::NONE, &fx.paths);
    assert!(out.iter().any(|e| matches!(e, Effect::Warn(_))));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn connections_to_missing_objects_and_odd_shapes_are_ignored() {
    let fx = Fx::new("odd");
    let doc = json!({"name": "Odd", "bundleid": "qa.odd",
    "objects": [{"uid": "A", "type": kind::UTILITY_JUNCTION, "config": null}],
    "connections": {
        "A": [{"destinationuid": "NOPE", "modifiers": 0}, {"destinationuid": 5}, "str", null, {"modifiers": 1}],
        "B": "not-a-list",
        "C": 7
    }});
    let wf = fx.wf(&doc);
    let act = ashdash_core::workflow::WorkflowAction {
        workflow: "wf".into(),
        from_uid: "A".into(),
        arg: String::new(),
        vars: BTreeMap::new(),
        mods: vec![],
        ..Default::default()
    };
    assert!(wf.run(&act, Mods::NONE, &fx.paths).is_empty());
}

// ---------------------------------------------------------------- OS helper argument handling

#[test]
fn open_helpers_refuse_option_like_arguments() {
    use ashdash_core::engine::{open_path, open_url};
    // refused before any process is spawned
    for bad in ["-a", "--help", "-R", "--args", "-n", ""] {
        assert!(open_url(bad, None).is_err(), "open_url({bad:?})");
        assert!(open_path(Path::new(bad), None).is_err(), "open_path({bad:?})");
    }
    assert!(open_url("https://example.com", Some("-b")).is_err());
    assert!(open_path(Path::new("/tmp"), Some("--args")).is_err());
}

/// `reveal` runs `open -R <path>`; a path starting with `-` is read by `open` as an option.
#[test]
fn reveal_refuses_option_like_paths() {
    // "-@" is not an `open` option, so on a build without the guard `open` itself rejects it
    // ("illegal option"); the point is that *we* must refuse before spawning it.
    match ashdash_core::engine::reveal(Path::new("-@invalid")) {
        Ok(()) => panic!("reveal accepted an option-like path"),
        Err(e) => assert!(format!("{e:#}").contains("refusing"), "{e:#}"),
    }
}

/// SEC-01: `osascript -e ... <args>`: a first argument beginning with `-e` is parsed as another
/// script fragment. `property x : (do shell script ...)` is evaluated while the script compiles,
/// so notification text controlled by an attacker runs a shell command.
/// (The test shows a harmless `touch` of a temp file; on a fixed build the text is just displayed.)
#[test]
fn notification_text_cannot_inject_applescript() {
    let t = TempDir::new("osa-inject");
    let marker = t.join("pwned");
    let text = format!("-eproperty zz : (do shell script \"touch '{}'\")", marker.display());
    let _ = ashdash_core::engine::notify("QA test", &text);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!marker.exists(), "AppleScript from the notification text was executed");
}

/// Stdin mode writes the whole query to the child synchronously, *before* the timeout loop
/// starts. A script that never reads stdin and a query larger than the pipe buffer therefore
/// blocks the caller for as long as the script lives, defeating the timeout.
#[test]
fn unread_stdin_cannot_defeat_the_timeout() {
    let fx = Fx::new("stdin-block");
    let mut doc = script_filter_workflow("sf", "sleep 14");
    doc["objects"][0]["config"]["scriptargtype"] = json!(2);
    let wf = fx.wf(&doc);
    let big = "x".repeat(4 * 1024 * 1024);
    let start = Instant::now();
    let items = fx.items(&wf, &format!("sf {big}"));
    let took = start.elapsed();
    assert!(!items.is_empty());
    assert!(
        took < Duration::from_secs(12),
        "returned after {took:?} (timeout is 10 s)"
    );
}
