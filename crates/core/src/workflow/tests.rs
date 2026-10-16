//! Golden tests per workflow object type. Workflows are written as JSON,
//! converted to a real `info.plist` and loaded back, so the plist path is
//! exercised too. Nothing here touches the user's data or the network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::test_support::*;
use super::*;
use crate::config::Paths;
use crate::item::{Action, Mods};

const NOTIFY: &str = "alfred.workflow.output.notification";
const JUNCTION: &str = "alfred.workflow.utility.junction";

fn notif(uid: &str, text: &str) -> Value {
    json!({"uid": uid, "type": NOTIFY, "config": {"title": uid, "text": text}})
}

fn obj(uid: &str, kind: &str, config: Value) -> Value {
    json!({"uid": uid, "type": kind, "config": config})
}

fn wire(dest: &str) -> Value {
    json!({"destinationuid": dest, "modifiers": 0})
}

fn wire_out(dest: &str, output: &str) -> Value {
    json!({"destinationuid": dest, "modifiers": 0, "sourceoutputuid": output})
}

fn load(tag: &str, doc: Value) -> (Workflow, Paths, PathBuf) {
    let (paths, root) = temp_paths(tag);
    let mut doc = doc;
    doc["name"] = json!(tag);
    doc["bundleid"] = json!(format!("test.{tag}"));
    let wf = load_via_plist(&root, tag, &doc);
    (wf, paths, root)
}

fn start(wf: &Workflow, from: &str, arg: &str) -> WorkflowAction {
    WorkflowAction {
        workflow: wf.id.clone(),
        from_uid: from.into(),
        arg: arg.into(),
        ..Default::default()
    }
}

fn run(wf: &Workflow, paths: &Paths, from: &str, arg: &str) -> Vec<Effect> {
    wf.run(&start(wf, from, arg), Mods::NONE, paths)
}

/// Run the stream starting AT object `uid` (a plain `run` starts below it)
/// by hanging a synthetic junction in front of it.
fn run_at(wf: &Workflow, paths: &Paths, uid: &str, arg: &str) -> Vec<Effect> {
    let mut wf = wf.clone();
    wf.nodes.push(super::Node {
        uid: "HEAD".into(),
        kind: JUNCTION.into(),
        config: Value::Null,
    });
    wf.connections.insert(
        "HEAD".into(),
        vec![super::Connection {
            dest: uid.into(),
            modifiers: 0,
            source_output: None,
        }],
    );
    run(&wf, paths, "HEAD", arg)
}

/// `title:text` of every notification, in order; warnings as `!msg`.
fn notes(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Notify { title, text } => Some(format!("{title}:{text}")),
            Effect::Warn(w) => Some(format!("!{w}")),
            _ => None,
        })
        .collect()
}

fn script_filter(keyword: &str, script: &str, extra: Value) -> Value {
    let mut c = json!({
        "keyword": keyword, "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1,
        "title": keyword, "script": script
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        c[k] = v;
    }
    obj("SF", KIND_SCRIPT_FILTER, c)
}

fn sf_workflow(tag: &str, script: &str, extra: Value) -> (Workflow, Paths, PathBuf) {
    load(
        tag,
        json!({"objects": [script_filter("demo", script, extra), notif("N", "{query}")],
               "connections": {"SF": [wire("N")]}}),
    )
}

fn action_of(item: &crate::item::Item) -> &WorkflowAction {
    match &item.action {
        Action::Workflow(a) => a,
        other => panic!("not a workflow action: {other:?}"),
    }
}

// ------------------------------------------------------------ utility.conditional

fn conditional_workflow(tag: &str) -> (Workflow, Paths, PathBuf) {
    let cond = |uid: &str, mode: i64, pat: &str, cs: bool| json!({"uid": uid, "inputstring": "", "matchmode": mode, "matchstring": pat, "matchcasesensitive": cs});
    load(
        tag,
        json!({
            "objects": [
                obj("J", JUNCTION, json!({})),
                obj("C", "alfred.workflow.utility.conditional", json!({"conditions": [
                    cond("eq", 0, "hello", false),
                    cond("ne", 1, "x", true),
                    cond("gt", 2, "10", false),
                    cond("lt", 3, "-5", false),
                    cond("re", 4, "^a+b$", true),
                ]})),
                notif("EQ", "eq"), notif("NE", "ne"), notif("GT", "gt"), notif("LT", "lt"),
                notif("RE", "re"), notif("ELSE", "else"),
            ],
            "connections": {"J": [wire("C")], "C": [
                wire_out("EQ", "eq"), wire_out("NE", "ne"), wire_out("GT", "gt"),
                wire_out("LT", "lt"), wire_out("RE", "re"), wire("ELSE"),
            ]}
        }),
    )
}

#[test]
fn conditional_picks_first_matching_output_and_else() {
    let (wf, paths, root) = conditional_workflow("cond");
    let t = |arg: &str| notes(&run(&wf, &paths, "J", arg));
    assert_eq!(t("HELLO"), ["EQ:eq"], "case-insensitive equal, first match wins");
    assert_eq!(
        t("y"),
        ["NE:ne"],
        "not equal (case-sensitive) matches before later rows"
    );
    // "x" fails ne; numeric rows compare as integers: 'x' -> 0, not > 10, not < -5.
    assert_eq!(t("x"), ["ELSE:else"]);
    assert_eq!(t("x "), ["NE:ne"]);
    let (wf2, _, _) = conditional_workflow("cond2");
    // Regex row is reachable once earlier rows are out of the way.
    let mut wf2 = wf2;
    wf2.nodes.iter_mut().find(|n| n.uid == "C").unwrap().config["conditions"]
        .as_array_mut()
        .unwrap()
        .drain(0..4);
    assert_eq!(notes(&run(&wf2, &paths, "J", "aaab")), ["RE:re"]);
    assert_eq!(
        notes(&run(&wf2, &paths, "J", "AAAB")),
        ["ELSE:else"],
        "regex honours case sensitivity"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn conditional_numeric_modes() {
    let (wf, paths, root) = conditional_workflow("cond-num");
    // Equal and not-equal rows are first; use values that skip them.
    let mut wf = wf;
    wf.nodes.iter_mut().find(|n| n.uid == "C").unwrap().config["conditions"]
        .as_array_mut()
        .unwrap()
        .drain(0..2);
    assert_eq!(notes(&run(&wf, &paths, "J", "11")), ["GT:gt"]);
    assert_eq!(notes(&run(&wf, &paths, "J", "10")), ["ELSE:else"]);
    assert_eq!(notes(&run(&wf, &paths, "J", "-6")), ["LT:lt"]);
    assert_eq!(notes(&run(&wf, &paths, "J", "abc")), ["ELSE:else"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn conditional_input_string_uses_variables() {
    let (wf, paths, root) = load(
        "cond-in",
        json!({
            "variables": {"mode": "fast"},
            "objects": [
                obj("J", JUNCTION, json!({})),
                obj("C", "alfred.workflow.utility.conditional", json!({"conditions": [
                    {"uid": "m", "inputstring": "{var:mode}", "matchmode": 0, "matchstring": "fast", "matchcasesensitive": true}
                ]})),
                notif("YES", "yes"), notif("NO", "no"),
            ],
            "connections": {"J": [wire("C")], "C": [wire_out("YES", "m"), wire("NO")]}
        }),
    );
    assert_eq!(notes(&run(&wf, &paths, "J", "ignored")), ["YES:yes"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ utility.filter

#[test]
fn legacy_filter_stops_the_stream_unless_matching() {
    let (wf, paths, root) = load(
        "filter",
        json!({
            "objects": [
                obj("F", "alfred.workflow.utility.filter",
                    json!({"matchmode": 2, "matchstring": "^\\d+$", "matchcasesensitive": false})),
                notif("N", "{query}"),
            ],
            "connections": {"F": [wire("N")]}
        }),
    );
    assert_eq!(notes(&run_at(&wf, &paths, "F", "123")), ["N:123"]);
    assert!(run_at(&wf, &paths, "F", "12a").is_empty());
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ replace / transform / argument

#[test]
fn replace_string_regex_and_empty_input() {
    let mk = |tag: &str, cfg: Value| {
        load(
            tag,
            json!({"objects": [obj("R", "alfred.workflow.utility.replace", cfg), notif("N", "{query}")],
                   "connections": {"R": [wire("N")]}}),
        )
    };
    let (wf, paths, root) = mk(
        "rep-str",
        json!({"matchmode": 0, "matchstring": "a.b", "replacestring": "-"}),
    );
    assert_eq!(
        notes(&run_at(&wf, &paths, "R", "xa.by aXb")),
        ["N:x-y aXb"],
        "literal, not a regex"
    );
    let (wf, paths, _) = mk(
        "rep-re",
        json!({"matchmode": 1, "matchstring": "(\\w+)@(\\w+)", "replacestring": "$2 at $1", "regexcaseinsensitive": true}),
    );
    assert_eq!(notes(&run_at(&wf, &paths, "R", "bob@HOST")), ["N:HOST at bob"]);
    let (wf, paths, _) = mk(
        "rep-empty",
        json!({"matchmode": 2, "matchstring": "", "replacestring": "fallback"}),
    );
    assert_eq!(notes(&run_at(&wf, &paths, "R", "")), ["N:fallback"]);
    assert_eq!(notes(&run_at(&wf, &paths, "R", "kept")), ["N:kept"]);
    let (wf, paths, _) = mk(
        "rep-bad",
        json!({"matchmode": 1, "matchstring": "(", "replacestring": ""}),
    );
    let out = notes(&run_at(&wf, &paths, "R", "x"));
    assert_eq!(out.len(), 1);
    assert!(out[0].starts_with("!") && out[0].contains("replace"), "{out:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn transform_and_argument_objects() {
    let (wf, paths, root) = load(
        "transform",
        json!({
            "objects": [
                obj("T", "alfred.workflow.utility.transform", json!({"type": 1})),
                obj("A", "alfred.workflow.utility.argument",
                    json!({"argument": "{query}!", "variables": {"k": "[{query}]"}})),
                notif("N", "{query}/{var:k}"),
            ],
            "connections": {"T": [wire("A")], "A": [wire("N")]}
        }),
    );
    assert_eq!(notes(&run_at(&wf, &paths, "T", "abc")), ["N:ABC!/[ABC!]"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ split / merge

#[test]
fn split_to_variables_and_to_arguments_then_merge() {
    let (wf, paths, root) = load(
        "split",
        json!({
            "objects": [
                obj("S1", "alfred.workflow.utility.split",
                    json!({"delimiter": ",", "outputas": 0, "trimarguments": true, "discardemptyarguments": true, "variableprefix": "p"})),
                notif("V", "{var:p1}|{var:p2}|{var:p3}"),
                obj("S2", "alfred.workflow.utility.split",
                    json!({"delimiter": ",", "outputas": 1, "trimarguments": true, "discardemptyarguments": true})),
                notif("EACH", "{query}"),
                obj("S3", "alfred.workflow.utility.split", json!({"delimiter": ",", "outputas": 1, "trimarguments": true})),
                obj("M", "alfred.workflow.utility.merge", json!({"delimiter": "+"})),
                notif("JOINED", "{query}"),
            ],
            "connections": {
                "S1": [wire("V")], "S2": [wire("EACH")], "S3": [wire("M")], "M": [wire("JOINED")],
            }
        }),
    );
    assert_eq!(notes(&run_at(&wf, &paths, "S1", " a, b ,,c")), ["V:a|b|c"]);
    assert_eq!(
        notes(&run_at(&wf, &paths, "S2", "a, b,,c")),
        ["EACH:a", "EACH:b", "EACH:c"]
    );
    assert_eq!(notes(&run_at(&wf, &paths, "S3", "a, b ,c")), ["JOINED:a+b+c"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ delay

#[test]
fn delay_yields_without_blocking_and_resumes_later() {
    let (wf, paths, root) = load(
        "delay",
        json!({
            "objects": [
                obj("J", JUNCTION, json!({})),
                obj("D", "alfred.workflow.utility.delay", json!({"seconds": "30"})),
                notif("LATE", "late {query}"),
                notif("NOW", "now {query}"),
            ],
            "connections": {"J": [wire("D"), wire("NOW")], "D": [wire("LATE")]},
            "uidata": {"D": {"ypos": 10}, "NOW": {"ypos": 20}}
        }),
    );
    let t0 = std::time::Instant::now();
    let fx = run(&wf, &paths, "J", "q");
    assert!(t0.elapsed() < Duration::from_secs(5), "a 30 s delay must not block");
    // The delay yields: the other stream runs first, the continuation is parked.
    assert!(matches!(fx[0], Effect::Notify { .. }) || matches!(fx[0], Effect::Delay { .. }));
    assert_eq!(notes(&fx), ["NOW:now q"]);
    let Some(Effect::Delay { seconds, resume }) = fx.into_iter().find(|e| matches!(e, Effect::Delay { .. })) else {
        panic!("no delay effect")
    };
    assert_eq!(seconds, 30.0);
    // Running the parked stream (what a timer would do) yields the rest.
    assert_eq!(notes(&resume.run()), ["LATE:late q"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ output.script / writefile / debug

#[test]
fn run_script_action_feeds_next_object_and_output_script_ends_stream() {
    let (wf, paths, root) = load(
        "scripts",
        json!({
            "objects": [
                obj("A", "alfred.workflow.action.script",
                    json!({"type": 0, "scriptargtype": 1, "script": "printf 'A<%s>' \"$1\""})),
                obj("O", "alfred.workflow.output.script",
                    json!({"type": 0, "scriptargtype": 1, "script": "printf '%s' \"$1\" > \"$out\""})),
                notif("AFTER", "never"),
                notif("SHOWN", "{query}"),
            ],
            "variables": {"out": "/dev/null"},
            "connections": {"A": [wire("O"), wire("SHOWN")], "O": [wire("AFTER")]},
            "uidata": {"O": {"ypos": 1}, "SHOWN": {"ypos": 2}}
        }),
    );
    let mut wf = wf;
    let target = root.join("out.txt");
    wf.variables.insert("out".into(), target.display().to_string());
    let fx = run_at(&wf, &paths, "A", "x");
    assert_eq!(notes(&fx), ["SHOWN:A<x>"], "nothing follows an output script");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "A<x>");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn write_file_output_resolves_path_and_modes() {
    let (wf, paths, root) = load(
        "write",
        json!({
            "objects": [
                obj("W", "alfred.workflow.output.writefile", json!({
                    "filename": "out/{query}.txt", "filetext": "line {query}\n", "type": 1,
                    "relativepathmode": 0, "createintermediatefolders": true, "adduuid": false})),
                notif("N", "{query}"),
            ],
            "connections": {"W": [wire("N")]}
        }),
    );
    let fx = run_at(&wf, &paths, "W", "a");
    let wrote: Vec<&WriteFile> = fx
        .iter()
        .filter_map(|e| if let Effect::WriteFile(w) = e { Some(w) } else { None })
        .collect();
    assert_eq!(wrote.len(), 1);
    let w = wrote[0];
    assert!(w.path.starts_with(paths.data.join("workflow-data")), "{:?}", w.path);
    assert!(w.path.ends_with("out/a.txt"));
    assert_eq!(w.mode, WriteMode::Append);
    w.perform().unwrap();
    w.perform().unwrap();
    assert_eq!(std::fs::read_to_string(&w.path).unwrap(), "line a\nline a\n");
    // The resolved path is the next object's argument.
    assert!(notes(&fx)[0].ends_with("out/a.txt"));
    let prepend = WriteFile {
        mode: WriteMode::Prepend,
        contents: "top\n".into(),
        ..w.clone()
    };
    prepend.perform().unwrap();
    assert!(std::fs::read_to_string(&w.path).unwrap().starts_with("top\nline a"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn write_file_never_rewrites_the_workflows_own_info_plist() {
    // Re-review N2: relative-to-the-workflow-folder writes may land beside the scripts, but a
    // package must not add objects to itself after the import prompt. info.plist is refused
    // however the path is spelled (relative, `./`, `sub/..`, any case, or absolute via a variable).
    let (wf, paths, root) = load(
        "selfplist",
        json!({
            "variables": {},
            "objects": [
                obj("W", "alfred.workflow.output.writefile", json!({
                    "filename": "{query}", "filetext": "<plist/>", "type": 0,
                    "relativepathmode": 2, "createintermediatefolders": true})),
            ],
            "connections": {}
        }),
    );
    let writes = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::WriteFile(_))).count();
    let abs = wf.dir.join("info.plist").display().to_string();
    for name in [
        "info.plist",
        "./info.plist",
        "sub/../info.plist",
        "INFO.plist",
        "Info.Plist",
        abs.as_str(),
    ] {
        let fx = run_at(&wf, &paths, "W", name);
        assert_eq!(writes(&fx), 0, "{name}: {fx:?}");
        assert!(
            notes(&fx).iter().any(|n| n.contains("info.plist")),
            "{name}: the refusal is reported: {fx:?}"
        );
    }
    // other files in the folder (scripts, settings of the workflow's own) keep working
    for name in ["run.sh", "sub/info.plist", "info.plist.bak"] {
        let fx = run_at(&wf, &paths, "W", name);
        assert_eq!(writes(&fx), 1, "{name}: {fx:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ triggers

fn trigger_workflow(tag: &str) -> (Workflow, Paths, PathBuf) {
    load(
        tag,
        json!({
            "variables": {"base": "B"},
            "objects": [
                obj("TR", KIND_EXTERNAL_TRIGGER, json!({"triggerid": "go"})),
                notif("N", "{query}/{var:base}/{var:extra}"),
                obj("CALL", "alfred.workflow.output.callexternaltrigger",
                    json!({"externaltriggerid": "go", "passinputasargument": true, "passvariables": true, "workflowbundleid": ""})),
                obj("START", KIND_KEYWORD, json!({"keyword": "k", "argumenttype": 1, "withspace": true})),
            ],
            "connections": {"TR": [wire("N")], "START": [wire("CALL")]}
        }),
    )
}

#[test]
fn external_trigger_takes_argument_and_variables() {
    let (wf, paths, root) = trigger_workflow("trigger");
    let mut vars = BTreeMap::new();
    vars.insert("extra".to_string(), "E".to_string());
    vars.insert("base".to_string(), "override".to_string());
    let fx = wf.run_trigger("go", "arg", vars, &paths).expect("trigger exists");
    assert_eq!(notes(&fx), ["N:arg/override/E"]);
    assert!(wf.run_trigger("missing", "", BTreeMap::new(), &paths).is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn call_external_trigger_jumps_within_the_workflow() {
    let (wf, paths, root) = trigger_workflow("calltrig");
    let mut act = start(&wf, "START", "payload");
    act.vars.insert("extra".into(), "X".into());
    let fx = wf.run(&act, Mods::NONE, &paths);
    assert_eq!(notes(&fx), ["N:payload/B/X"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ list filter

#[test]
fn list_filter_filters_fuzzily_and_passes_title_as_arg() {
    let items = json!([
        {"title": "Apple", "subtitle": "fruit", "arg": "a1"},
        {"title": "Banana", "subtitle": "yellow"},
        {"title": "Cherry", "match": "red stone fruit", "variables": {"kind": "stone"}},
    ])
    .to_string();
    let (wf, paths, root) = load(
        "listfilter",
        json!({"objects": [obj("LF", KIND_LIST_FILTER, json!({
            "keyword": "fr", "withspace": true, "argumenttype": 1, "title": "Fruit", "items": items}))]}),
    );
    let titles = |q: &str| -> Vec<String> {
        let r = wf.keyword_results(q, &paths);
        assert!(r.exclusive);
        r.items.iter().map(|i| i.title.clone()).collect()
    };
    assert_eq!(titles("fr ").len(), 3, "empty filter lists everything");
    assert_eq!(titles("fr ban"), ["Banana"]);
    assert_eq!(
        titles("fr stone"),
        ["Cherry"],
        "`match` replaces the title for matching"
    );
    assert_eq!(titles("fr zzz"), ["No results"]);
    let r = wf.keyword_results("fr ban", &paths);
    assert_eq!(action_of(&r.items[0]).arg, "Banana");
    let r = wf.keyword_results("fr app", &paths);
    assert_eq!(action_of(&r.items[0]).arg, "a1");
    let r = wf.keyword_results("fr stone", &paths);
    assert_eq!(action_of(&r.items[0]).vars["kind"], "stone");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn list_filter_fixed_order_is_kept() {
    let items = json!([{"title": "bb"}, {"title": "ab"}, {"title": "b"}]).to_string();
    let (wf, paths, root) = load(
        "listfilter-fixed",
        json!({"objects": [obj("LF", KIND_LIST_FILTER, json!({
            "keyword": "ls", "withspace": true, "argumenttype": 1, "fixedorder": true, "items": items}))]}),
    );
    let r = wf.keyword_results("ls b", &paths);
    let mut sorted = r.items.clone();
    sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let titles: Vec<_> = sorted.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["bb", "ab", "b"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ file filter

#[test]
fn file_filter_uses_injected_search_and_drops_system_paths() {
    let (wf, paths, root) = load(
        "filefilter",
        json!({"objects": [obj("FF", KIND_FILE_FILTER, json!({
            "keyword": "ff", "withspace": true, "argumenttype": 0, "title": "Find", "scopes": ["~/Documents"],
            "types": ["public.plain-text"], "includesystem": false,
            "fields": [{"field": "kMDItemDisplayName", "value": "{query}", "split": true, "words": true, "not": false}]}))]}),
    );
    let seen = std::sync::Mutex::new(Vec::<(String, Vec<PathBuf>)>::new());
    let search = |pred: &str, scopes: &[PathBuf]| -> Vec<PathBuf> {
        seen.lock().unwrap().push((pred.to_string(), scopes.to_vec()));
        vec![
            PathBuf::from("/Users/x/Documents/notes.txt"),
            PathBuf::from("/System/Library/hidden.txt"),
            PathBuf::from("/Users/x/Documents/sub/notes2.txt"),
        ]
    };
    let r = wf.keyword_results_with("ff note", &paths, &search);
    assert!(r.exclusive);
    let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["notes.txt", "notes2.txt"]);
    assert_eq!(action_of(&r.items[0]).arg, "/Users/x/Documents/notes.txt");
    let (pred, scopes) = seen.lock().unwrap()[0].clone();
    assert!(pred.contains("kMDItemDisplayName") && pred.contains("note*"), "{pred}");
    assert!(pred.contains("public.plain-text"), "{pred}");
    assert!(scopes[0].ends_with("Documents"));
    // Argument required: nothing is searched without one.
    let before = seen.lock().unwrap().len();
    let r = wf.keyword_results_with("ff ", &paths, &search);
    assert_eq!(seen.lock().unwrap().len(), before);
    assert!(!r.items[0].valid);
    // No hits -> a "No results" row.
    let r = wf.keyword_results_with("ff zzz", &paths, &|_: &str, _: &[PathBuf]| vec![]);
    assert_eq!(r.items[0].title, "No results");
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ script filter extras

#[test]
fn script_filter_rerun_is_surfaced_and_clamped() {
    let (wf, paths, root) = sf_workflow("rerun", r#"echo '{"rerun":0.5,"items":[{"title":"a"}]}'"#, json!({}));
    let r = wf.keyword_results("demo x", &paths);
    assert_eq!(r.rerun, Some(Duration::from_millis(500)));
    assert_eq!(action_of(&r.items[0]).rerun, Some(0.5));
    assert_eq!(rerun_interval(&r.items), Some(Duration::from_millis(500)));
    // Same items still produce the same effects (rerun is metadata only).
    assert_eq!(notes(&wf.run(action_of(&r.items[0]), Mods::NONE, &paths)), ["N:"]);

    let (wf, paths, _) = sf_workflow("rerun2", r#"echo '{"rerun":0.01,"items":[]}'"#, json!({}));
    let r = wf.keyword_results("demo x", &paths);
    assert_eq!(r.rerun, Some(Duration::from_millis(100)));
    assert_eq!(r.items[0].title, "No results");
    assert_eq!(rerun_interval(&r.items), Some(Duration::from_millis(100)));

    let (wf, paths, _) = sf_workflow("rerun3", r#"echo '{"items":[{"title":"a"}]}'"#, json!({}));
    let r = wf.keyword_results("demo x", &paths);
    assert_eq!(r.rerun, None);
    assert_eq!(rerun_interval(&r.items), None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_cache_serves_repeat_queries_and_loosereload() {
    let (paths, root) = temp_paths("cache");
    let count = root.join("count");
    let flag = root.join("fail");
    let script = format!(
        r#"if [ -f '{flag}' ]; then echo boom >&2; exit 1; fi; echo run >> '{count}'; echo '{{"cache":{{"seconds":60,"loosereload":true}},"items":[{{"title":"cached"}}]}}'"#,
        flag = flag.display(),
        count = count.display()
    );
    let doc = json!({"name": "c", "bundleid": "test.cache",
        "objects": [script_filter("demo", &script, json!({}))], "connections": {}});
    let wf = load_via_plist(&root, "cache", &doc);
    let runs = || std::fs::read_to_string(&count).map(|s| s.lines().count()).unwrap_or(0);

    assert_eq!(wf.keyword_results("demo q", &paths).items[0].title, "cached");
    assert_eq!(wf.keyword_results("demo q", &paths).items[0].title, "cached");
    assert_eq!(runs(), 1, "second query is served from the cache");
    wf.keyword_results("demo other", &paths);
    assert_eq!(runs(), 2, "cache is per argument");

    // Expired entry + failing script + loosereload: old data beats an error.
    wf.runtime.age_cache(Duration::from_secs(3600));
    std::fs::write(&flag, "").unwrap();
    assert_eq!(wf.keyword_results("demo q", &paths).items[0].title, "cached");
    // Without loosereload (or without cache) the failure is shown.
    wf.reset_session();
    let r = wf.keyword_results("demo q", &paths);
    assert!(r.items[0].title.contains("script error"), "{:?}", r.items[0].title);
    std::fs::remove_file(&flag).unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_skipknowledge_keeps_script_order() {
    let script = r#"echo '{"skipknowledge":true,"items":[{"title":"zeta"},{"title":"alpha"},{"title":"mid"}]}'"#;
    let (wf, paths, root) = sf_workflow("skipk", script, json!({}));
    let r = wf.keyword_results("demo ", &paths);
    let mut v = r.items.clone();
    v.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    assert_eq!(
        v.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        ["zeta", "alpha", "mid"]
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_reference_filters_results_with_match_field() {
    let script = r#"echo '{"items":[{"title":"Safari"},{"title":"Notes","match":"scribble"},{"title":"Mail"}]}'"#;
    let (wf, paths, root) = sf_workflow("alfilter", script, json!({"alfredfiltersresults": true}));
    let titles = |q: &str| -> Vec<String> {
        wf.keyword_results(q, &paths)
            .items
            .iter()
            .map(|i| i.title.clone())
            .collect()
    };
    assert_eq!(titles("demo saf"), ["Safari"]);
    assert_eq!(titles("demo scrib"), ["Notes"]);
    assert_eq!(titles("demo notes"), ["No results"], "match replaces the title");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_runningsubtext_placeholder() {
    let (wf, paths, root) = sf_workflow(
        "running",
        r#"echo '{"items":[]}'"#,
        json!({"runningsubtext": "Searching for {query}...", "title": "Look"}),
    );
    let p = wf.pending_items("demo cats", &paths);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].subtitle, "Searching for cats...");
    assert!(!p[0].valid);
    assert!(wf.pending_items("other", &paths).is_empty());
    assert!(wf.pending_items("demo ", &paths).len() <= 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_legacy_xml_output() {
    let script = r#"cat <<'EOF'
<?xml version="1.0"?>
<items>
  <item uid="u1" arg="A&amp;B" valid="no" autocomplete="ac">
    <title>Caf&#233;</title>
    <subtitle>sub</subtitle>
    <variables><variable name="item">i</variable></variables>
  </item>
  <item arg="second"><title>Two</title></item>
</items>
EOF"#;
    let (wf, paths, root) = sf_workflow("xml", script, json!({}));
    let r = wf.keyword_results("demo x", &paths);
    assert_eq!(r.items.len(), 2);
    let first = &r.items[0];
    assert_eq!(first.title, "Café");
    assert_eq!(first.subtitle, "sub");
    assert!(!first.valid);
    assert_eq!(first.autocomplete.as_deref(), Some("ac"));
    let a = action_of(first);
    assert_eq!(a.arg, "A&B");
    assert_eq!(a.vars["item"], "i");
    assert!(r.items[1].valid);

    // Document-level variables and rerun in XML.
    let script = r#"echo '<output><rerun>2</rerun><variables><variable name="g">1</variable></variables><items><item><title>T</title></item></items></output>'"#;
    let (wf, paths, _) = sf_workflow("xml2", script, json!({}));
    let r = wf.keyword_results("demo x", &paths);
    assert_eq!(r.rerun, Some(Duration::from_secs(2)));
    assert_eq!(action_of(&r.items[0]).vars["g"], "1");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn variable_precedence_workflow_script_item_mod() {
    let script = r#"echo '{
      "variables": {"a": "script", "b": "script", "c": "script", "d": "script"},
      "items": [{"title": "t", "arg": "x",
        "variables": {"b": "item", "c": "item", "d": "item"},
        "mods": {
          "cmd": {"variables": {"c": "mod"}},
          "alt": {"arg": "alt-arg"}
        }}]}'"#;
    let (wf, paths, root) = load(
        "prec",
        json!({
            "variables": {"a": "wf", "z": "wf", "b": "wf", "c": "wf", "d": "wf"},
            "objects": [script_filter("demo", script, json!({})), notif("N", "{var:a}{var:b}{var:c}{var:d}{var:z}")],
            "connections": {"SF": [
                wire("N"),
                {"destinationuid": "N", "modifiers": 1048576},
                {"destinationuid": "N", "modifiers": 524288}
            ]}
        }),
    );
    let r = wf.keyword_results("demo x", &paths);
    let act = action_of(&r.items[0]);
    let text = |m: Mods| notes(&wf.run(act, m, &paths));
    // item > script > workflow
    assert_eq!(text(Mods::NONE), ["N:scriptitemitemitemwf"]);
    // alt only changes the argument; variables are untouched
    let alt = Mods {
        alt: true,
        ..Mods::NONE
    };
    assert_eq!(text(alt), ["N:scriptitemitemitemwf"]);
    // cmd mod has its own variables: it replaces the item's, script-level stay
    let cmd = Mods {
        cmd: true,
        ..Mods::NONE
    };
    assert_eq!(text(cmd), ["N:scriptscriptmodscriptwf"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn script_filter_session_variables_reach_the_next_run() {
    let script = r#"printf '{"variables":{"n":"%s"},"items":[{"title":"n=%s"}]}' "${n:-0}x" "${n:-none}""#;
    let (wf, paths, root) = sf_workflow("session", script, json!({}));
    assert_eq!(wf.keyword_results("demo a", &paths).items[0].title, "n=none");
    assert_eq!(wf.keyword_results("demo b", &paths).items[0].title, "n=0x");
    wf.reset_session();
    assert_eq!(wf.keyword_results("demo c", &paths).items[0].title, "n=none");
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ user configuration

#[test]
fn user_configuration_defaults_reach_scripts_templates_and_overrides() {
    let doc = json!({
        "name": "cfg", "bundleid": "test.cfg",
        "variables": {"plain": "p"},
        "variablesdontexport": ["secret"],
        "userconfigurationconfig": [
            {"variable": "name", "type": "textfield", "label": "Name", "config": {"default": "Ann"}},
            {"variable": "loud", "type": "checkbox", "config": {"default": true}},
            {"variable": "plain", "type": "textfield", "config": {"default": "ignored: plist variable wins"}},
            {"variable": "secret", "type": "textfield", "config": {"default": "s"}}
        ],
        "objects": [
            obj("SC", "alfred.workflow.action.script",
                json!({"type": 0, "scriptargtype": 1, "script": "printf '%s-%s-%s' \"$name\" \"$loud\" \"$plain\""})),
            notif("N", "{query}|{var:name}|{var:loud}"),
        ],
        "connections": {"SC": [wire("N")]}
    });
    let (paths, root) = temp_paths("userconfig");
    let wf = load_via_plist(&root, "cfg", &doc);
    assert_eq!(wf.user_config.len(), 4);
    assert_eq!(wf.user_config[0].label, "Name");
    assert_eq!(notes(&run_at(&wf, &paths, "SC", "")), ["N:Ann-1-p|Ann|1"]);
    assert!(!wf.exportable_variables().contains_key("secret"));
    assert!(wf.exportable_variables().contains_key("name"));

    // A value saved by a preferences UI overrides the default.
    let file = wf.user_values_file(&paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"name": "Bob", "loud": "0"}"#).unwrap();
    wf.reset_session();
    assert_eq!(notes(&run_at(&wf, &paths, "SC", "")), ["N:Bob-0-p|Bob|0"]);
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ misc object types

#[test]
fn every_flow_kind_is_handled_by_the_executor() {
    let (paths, root) = temp_paths("flowkinds");
    for kind in FLOW_KINDS {
        let doc = json!({"name": "k", "bundleid": "k", "objects": [obj("X", kind, json!({}))], "connections": {}});
        let wf = Workflow::from_json(&root, "k", &doc).unwrap();
        // Run only objects without external side effects; the rest are checked
        // for "not reported as unsupported" through the compat table.
        assert!(is_supported(kind));
        let safe =
            kind.contains("utility.") || kind.ends_with("output.largetype") || kind.ends_with("output.clipboard");
        if safe {
            let fx = run(&wf, &paths, "X", "q");
            assert!(
                !fx.iter()
                    .any(|e| matches!(e, Effect::Warn(w) if w.contains("unsupported workflow object"))),
                "{kind}: {fx:?}"
            );
        }
    }
    let doc = json!({"name": "k", "bundleid": "k", "objects": [obj("X", "alfred.workflow.trigger.hotkey", json!({}))]});
    let wf = Workflow::from_json(&root, "k", &doc).unwrap();
    assert_eq!(wf.unsupported_kinds(), ["alfred.workflow.trigger.hotkey"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn effect_objects_produce_expected_effects() {
    let (wf, paths, root) = load(
        "effects",
        json!({
            "objects": [
                obj("J", JUNCTION, json!({})),
                obj("CB", "alfred.workflow.output.clipboard", json!({"clipboardtext": "c:{query}"})),
                obj("LT", "alfred.workflow.output.largetype", json!({"largetypetext": "l:{query}"})),
                obj("DB", "alfred.workflow.utility.debug", json!({"argument": "d:{query}"})),
                obj("REV", "alfred.workflow.action.revealfile", json!({})),
                obj("URL", "alfred.workflow.action.openurl", json!({"url": "https://e.test/?q={query}", "utf8": true})),
            ],
            "connections": {"J": [wire("CB"), wire("LT"), wire("DB"), wire("REV"), wire("URL")]}
        }),
    );
    let fx = run(&wf, &paths, "J", "a b");
    assert_eq!(
        fx,
        vec![
            Effect::Copy("c:a b".into()),
            Effect::LargeType("l:a b".into()),
            Effect::Debug("d:a b".into()),
            Effect::Reveal(PathBuf::from("a b")),
            Effect::OpenUrl {
                url: "https://e.test/?q=a%20b".into(),
                browser: None
            },
        ]
    );
    let _ = std::fs::remove_dir_all(root);
}

// ------------------------------------------------------------ The reference launcher's bundled templates

fn bundled_templates() -> Vec<PathBuf> {
    let dir =
        Path::new("/Applications/Reference.app/Contents/Resources");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "alfredworkflow"))
        .collect();
    v.sort();
    v
}

#[test]
fn bundled_reference_templates_load_and_report_unsupported_objects() {
    let templates = bundled_templates();
    if templates.is_empty() {
        eprintln!("skipping: the reference launcher is not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("oa-wf-bundled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut loaded = Vec::new();
    let mut failed = Vec::new();
    for t in &templates {
        match import_package(t, &root) {
            Ok(dir) => match Workflow::load(&dir) {
                Ok(w) => loaded.push(w),
                Err(e) => failed.push(format!("{}: {e:#}", t.display())),
            },
            Err(e) => failed.push(format!("{}: {e:#}", t.display())),
        }
    }
    assert!(failed.is_empty(), "templates that failed to import: {failed:?}");
    assert!(!loaded.is_empty());

    let unsupported = unsupported_kinds(&loaded);
    let mut seen = std::collections::BTreeSet::new();
    for w in &loaded {
        seen.extend(w.nodes.iter().map(|n| n.kind.clone()));
    }
    eprintln!(
        "bundled templates: {} workflows, {} object types seen",
        loaded.len(),
        seen.len()
    );
    eprintln!("unsupported object types: {unsupported:?}");
    // Keyword, script filter, action, output and utility objects must all run.
    let missing: Vec<&String> = unsupported
        .iter()
        .filter(|k| {
            ["input.", "action.", "output.", "utility."]
                .iter()
                .any(|p| k.starts_with(&format!("alfred.workflow.{p}")))
        })
        .collect();
    assert!(missing.is_empty(), "unsupported object types: {missing:?}");

    // Keyword items must generate without panicking for every template
    // (scripts are not run for a query that matches no keyword).
    let (paths, r) = temp_paths("bundled-run");
    for w in &loaded {
        let _ = w.keyword_items("zzzzqqqq", &paths);
    }
    let _ = std::fs::remove_dir_all(r);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------- SEC-17 icon confinement

#[test]
fn item_icons_are_confined_to_the_workflow_directory() {
    use crate::item::Icon;
    use std::os::unix::fs::symlink;

    let script = r#"printf '{"items":[
        {"title":"own","icon":{"path":"img/ok.png"}},
        {"title":"dotdot","icon":{"path":"../../outside.png"}},
        {"title":"abs-outside","icon":{"path":"%s"}},
        {"title":"abs-inside","icon":{"path":"%s"}},
        {"title":"link","icon":{"path":"link.png"}},
        {"title":"dirlink","icon":{"path":"dirlink/outside.png"}}
    ]}' "$OUTSIDE" "$INSIDE""#;
    let (wf, paths, root) = load(
        "icon-confine",
        json!({"objects": [obj("SF", KIND_SCRIPT_FILTER, json!({
            "keyword": "ic", "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1,
            "title": "I", "subtext": "", "script": script}))], "connections": {}}),
    );
    let outside = root.join("outside.png");
    std::fs::write(&outside, b"png").unwrap();
    std::fs::create_dir_all(wf.dir.join("img")).unwrap();
    std::fs::write(wf.dir.join("img/ok.png"), b"png").unwrap();
    symlink(&outside, wf.dir.join("link.png")).unwrap();
    symlink(&root, wf.dir.join("dirlink")).unwrap();
    // the script gets the two absolute paths through the process environment
    let mut wf = wf;
    wf.variables.insert("OUTSIDE".into(), outside.display().to_string());
    wf.variables
        .insert("INSIDE".into(), wf.dir.join("img/ok.png").display().to_string());

    let (items, _) = wf.keyword_items("ic x", &paths);
    let icon = |t: &str| {
        items
            .iter()
            .find(|i| i.title == t)
            .unwrap_or_else(|| panic!("{t}: {items:?}"))
            .icon
            .clone()
    };
    assert_eq!(icon("own"), Icon::Image(wf.dir.join("img/ok.png")));
    assert_eq!(icon("abs-inside"), Icon::Image(wf.dir.join("img/ok.png")));
    for t in ["dotdot", "abs-outside", "link", "dirlink"] {
        assert_eq!(icon(t), Icon::None, "{t} must not resolve to an image");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workflow_icon_png_symlink_falls_back_to_the_glyph() {
    use crate::item::Icon;
    let (wf, paths, root) = load(
        "icon-link",
        json!({"objects": [obj("K", KIND_KEYWORD, json!({
            "keyword": "ik", "withspace": true, "argumenttype": 1, "text": "T", "subtext": ""}))],
            "connections": {}}),
    );
    let outside = root.join("evil.png");
    std::fs::write(&outside, b"png").unwrap();
    std::os::unix::fs::symlink(&outside, wf.dir.join("icon.png")).unwrap();
    let (items, _) = wf.keyword_items("ik x", &paths);
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i.icon == Icon::Glyph("◆")), "{items:?}");
    std::fs::remove_file(wf.dir.join("icon.png")).unwrap();
    std::fs::write(wf.dir.join("icon.png"), b"png").unwrap();
    let (items, _) = wf.keyword_items("ik x", &paths);
    assert!(items.iter().all(|i| i.icon == Icon::Image(wf.dir.join("icon.png"))));
    let _ = std::fs::remove_dir_all(root);
}
