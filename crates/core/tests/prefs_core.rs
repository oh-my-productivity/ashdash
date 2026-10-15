//! Core side of the preferences window: applying settings to a running engine, the file
//! watcher, workflow enable/remove/preview, theme import and the snippet library.
//! Everything runs in a temp directory.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use ashdash_core::clipboard::Snippet;
use ashdash_core::settings_view::{self as sv, Change, SettingsWatcher, SnippetLibrary};
use ashdash_core::websearch::WebSearch;
use ashdash_core::{Config, Mods};
use common::{Entry, Env, keyword_workflow, plist_bytes, write_zip};
use serde_json::json;

fn titles(e: &ashdash_core::Engine, q: &str) -> Vec<String> {
    e.query(q).into_iter().map(|i| i.title).collect()
}

// ---------------------------------------------------------------- apply_config

#[test]
fn apply_config_switches_providers_without_a_restart() {
    let env = Env::new("apply-providers", &["Safari"]);
    let engine = env.engine();
    assert!(titles(&engine, "saf").iter().any(|t| t == "Safari"));

    let mut c = engine.config_now();
    c.providers_disabled = vec!["apps".into()];
    let report = engine.apply_config(c);
    assert!(report.restart_required.is_empty());
    assert!(
        !titles(&engine, "saf").iter().any(|t| t == "Safari"),
        "apps provider is off"
    );

    let mut c = engine.config_now();
    c.providers_disabled.clear();
    engine.apply_config(c);
    assert!(titles(&engine, "saf").iter().any(|t| t == "Safari"));
}

#[test]
fn apply_config_applies_web_searches_and_result_count() {
    let env = Env::new("apply-web", &[]);
    let engine = env.engine();
    assert!(titles(&engine, "zork hello").iter().all(|t| !t.contains("Zork")));
    let mut c = engine.config_now();
    c.web_searches.push(WebSearch {
        keyword: "zork".into(),
        title: "Zork".into(),
        url: "https://example.test/?q={query}".into(),
        fallback: false,
    });
    engine.apply_config(c);
    assert!(titles(&engine, "zork hello").iter().any(|t| t.contains("Zork")));
}

#[test]
fn apply_config_reports_what_needs_a_restart_and_keeps_the_rest_live() {
    let env = Env::new("apply-restart", &[]);
    let engine = env.engine();
    let mut c = engine.config_now();
    c.app_dirs.push(PathBuf::from("/somewhere/else"));
    c.max_results = 7;
    let report = engine.apply_config(c);
    assert_eq!(report.restart_required, vec!["app_dirs"]);
    assert_eq!(engine.config_now().max_results, 7);
}

#[test]
fn usage_learning_switch_is_applied_and_clearable() {
    let env = Env::new("apply-usage", &["Alpha One", "Alpha Two"]);
    let engine = env.engine();
    assert!(engine.usage_enabled());
    let mut c = engine.config_now();
    c.learn_usage = false;
    engine.apply_config(c);
    assert!(!engine.usage_enabled());
    let item = engine
        .query("alpha")
        .into_iter()
        .find(|i| i.title == "Alpha Two")
        .unwrap();
    engine.execute(&item, "alpha", Mods::NONE, true);
    assert!(
        !env.paths.usage_file().exists()
            || !std::fs::read_to_string(env.paths.usage_file())
                .unwrap()
                .contains("Alpha Two"),
        "nothing is recorded while learning is off"
    );
    engine.clear_usage();
}

#[test]
fn clipboard_settings_reach_the_live_history() {
    let env = Env::new("apply-clip", &[]);
    let engine = env.engine();
    let mut c = engine.config_now();
    c.clipboard.ignored_apps = vec!["com.example.vault".into()];
    engine.apply_config(c);
    let hist = engine.clipboard();
    let hist = hist.lock().unwrap();
    assert!(hist.ignored_apps().iter().any(|a| a == "com.example.vault"));
}

// ---------------------------------------------------------------- watcher

fn wait_for(rx: &mpsc::Receiver<BTreeSet<Change>>, want: Change) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(8);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(set) if set.contains(&want) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

#[test]
fn watcher_reports_config_and_workflow_edits_but_not_noise() {
    let env = Env::new("watch", &[]);
    let (tx, rx) = mpsc::channel();
    let _w = SettingsWatcher::start(&env.paths, &[], Duration::from_millis(100), move |set| {
        let _ = tx.send(set);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300)); // let FSEvents arm

    // noise first: history and usage writes must not report anything
    std::fs::write(env.paths.clipboard_file(), "[]").unwrap();
    std::fs::write(env.paths.usage_file(), "{}").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(800)).is_err(), "noise reported");

    // `ashdash config set` style: atomic replace
    Config::default().save(&env.paths).unwrap();
    assert!(wait_for(&rx, Change::Config), "config.json write not seen");

    // a workflow dropped into the folder
    env.install_workflow("user.workflow.w", &keyword_workflow("W", "w.b", "w"));
    assert!(wait_for(&rx, Change::Content), "new workflow not seen");

    // hand-edited snippets
    std::fs::write(env.paths.snippets_file(), "[]").unwrap();
    assert!(wait_for(&rx, Change::Content), "snippets.json write not seen");
}

#[test]
fn watcher_follows_extra_workflow_folders() {
    let env = Env::new("watch-extra", &[]);
    let extra = env.root.join("reference-workflows");
    std::fs::create_dir_all(&extra).unwrap();
    let (tx, rx) = mpsc::channel();
    let w = SettingsWatcher::start(&env.paths, &[], Duration::from_millis(100), move |set| {
        let _ = tx.send(set);
    })
    .unwrap();
    w.set_extra_roots(&env.paths, std::slice::from_ref(&extra));
    std::thread::sleep(Duration::from_millis(300));
    let dir = extra.join("theirs");
    std::fs::create_dir_all(&dir).unwrap();
    common::write_info_plist(&dir, &keyword_workflow("T", "t.b", "t"));
    assert!(wait_for(&rx, Change::Content));
}

// ---------------------------------------------------------------- workflows

#[test]
fn workflows_are_listed_disabled_and_removed_only_inside_the_managed_folder() {
    let env = Env::new("wf-manage", &[]);
    let own = env.install_workflow("user.workflow.one", &keyword_workflow("One", "one.b", "one"));
    let extra_root = env.root.join("foreign");
    let foreign = extra_root.join("theirs");
    std::fs::create_dir_all(&foreign).unwrap();
    common::write_info_plist(&foreign, &keyword_workflow("Theirs", "t.b", "t"));

    let list = sv::list_workflows(&env.paths, std::slice::from_ref(&extra_root));
    let one = list.iter().find(|w| w.bundle_id == "one.b").unwrap();
    assert!(one.managed && !one.disabled);
    let theirs = list.iter().find(|w| w.bundle_id == "t.b").unwrap();
    assert!(!theirs.managed);

    sv::set_workflow_enabled(&env.paths, &own, false).unwrap();
    let list = sv::list_workflows(&env.paths, &[]);
    assert!(list.iter().find(|w| w.bundle_id == "one.b").unwrap().disabled);
    // other keys survive the rewrite
    let doc = plist::Value::from_file(own.join("info.plist")).unwrap();
    assert_eq!(
        doc.as_dictionary().unwrap().get("name").unwrap().as_string(),
        Some("One")
    );
    sv::set_workflow_enabled(&env.paths, &own, true).unwrap();
    assert!(!sv::list_workflows(&env.paths, &[])[0].disabled);

    // foreign folders and paths that merely look like the managed one are refused
    assert!(sv::set_workflow_enabled(&env.paths, &foreign, false).is_err());
    assert!(sv::remove_workflow(&env.paths, &foreign).is_err());
    assert!(foreign.join("info.plist").exists());
    let sneaky = env
        .paths
        .workflows_dir()
        .join("..")
        .join("..")
        .join("foreign")
        .join("theirs");
    assert!(sv::remove_workflow(&env.paths, &sneaky).is_err());
    assert!(sv::remove_workflow(&env.paths, &env.paths.workflows_dir()).is_err());
    assert!(foreign.join("info.plist").exists());

    // a symlink in the managed folder pointing outside is not followed
    let link = env.paths.workflows_dir().join("user.workflow.link");
    std::os::unix::fs::symlink(&foreign, &link).unwrap();
    assert!(sv::remove_workflow(&env.paths, &link).is_err());
    assert!(sv::set_workflow_enabled(&env.paths, &link, false).is_err());
    assert!(foreign.join("info.plist").exists());
    std::fs::remove_file(&link).unwrap();

    sv::remove_workflow(&env.paths, &own).unwrap();
    assert!(!own.exists());
}

#[test]
fn disabling_a_workflow_takes_effect_after_a_content_reload() {
    let env = Env::new("wf-disable-live", &[]);
    let dir = env.install_workflow("user.workflow.k", &keyword_workflow("K", "k.b", "kwk"));
    let engine = env.engine();
    assert!(titles(&engine, "kwk x").iter().any(|t| t == "K"));
    sv::set_workflow_enabled(&env.paths, &dir, false).unwrap();
    engine.reload_content();
    assert!(!titles(&engine, "kwk x").iter().any(|t| t == "K"));
}

fn package(env: &Env, name: &str, doc: &serde_json::Value) -> PathBuf {
    let p = env.root.join(name);
    write_zip(&p, &[Entry::File("info.plist", plist_bytes(doc))]);
    p
}

#[test]
fn preview_describes_a_package_without_installing_it() {
    let env = Env::new("wf-preview", &[]);
    let doc = json!({
        "name": "Risky", "bundleid": "risky.b", "version": "2.0",
        "objects": [
            {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {"keyword": "risk", "withspace": true, "argumenttype": 1, "text": "R", "subtext": ""}},
            {"uid": "S", "type": "alfred.workflow.action.script", "config": {"type": 0, "scriptargtype": 1, "script": "echo hi"}},
            {"uid": "U", "type": "alfred.workflow.action.openurl", "config": {"url": "https://example.test/{query}", "utf8": true}},
        ],
        "connections": {"K": [{"destinationuid": "S", "modifiers": 0}, {"destinationuid": "U", "modifiers": 0}]}
    });
    let pkg = package(&env, "risky.alfredworkflow", &doc);
    let before = common::snapshot(&env.paths.data);
    let pv = sv::preview_package(&env.paths, &pkg).unwrap();
    assert_eq!(
        (pv.name.as_str(), pv.bundle_id.as_str(), pv.version.as_str()),
        ("Risky", "risky.b", "2.0")
    );
    assert_eq!(pv.trust.scripts, 1);
    assert_eq!(pv.trust.open_url_schemes, vec!["https"]);
    assert!(!pv.replaces_installed);
    assert_eq!(
        common::snapshot(&env.paths.data),
        before,
        "preview leaves no trace in the data folder"
    );

    // once installed, a second preview says it would replace it
    env.engine().import_workflow(&pkg).unwrap();
    assert!(sv::preview_package(&env.paths, &pkg).unwrap().replaces_installed);

    // garbage and traversal are rejected like an import
    let bad = env.root.join("bad.alfredworkflow");
    std::fs::write(&bad, b"not a zip").unwrap();
    assert!(sv::preview_package(&env.paths, &bad).is_err());
    let evil = env.root.join("evil.alfredworkflow");
    write_zip(&evil, &[Entry::File("../escape.txt", b"x".to_vec())]);
    assert!(sv::preview_package(&env.paths, &evil).is_err());
    assert!(!env.root.join("escape.txt").exists());
}

// ---------------------------------------------------------------- themes

fn theme_file(dir: &std::path::Path, name: &str) -> PathBuf {
    let doc = json!({"alfredtheme": {"name": name, "window": {"color": "#112233ff"}}});
    let p = dir.join(format!("{name}-src.alfredappearance"));
    common::json_to_plist(&doc).to_file_xml(&p).unwrap();
    p
}

#[test]
fn themes_are_validated_copied_and_listed() {
    let env = Env::new("themes", &[]);
    assert!(sv::list_themes(&env.paths).is_empty());
    let src = theme_file(env.root.path(), "Midnight");
    let copy = sv::import_theme(&env.paths, &src).unwrap();
    assert!(copy.starts_with(env.paths.data.join("themes")));
    let list = sv::list_themes(&env.paths);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Midnight");
    assert_eq!(list[0].path, copy);

    let junk = env.root.join("junk.alfredappearance");
    std::fs::write(&junk, b"nope").unwrap();
    assert!(sv::import_theme(&env.paths, &junk).is_err());
    assert!(sv::import_theme(&env.paths, &env.root.join("missing")).is_err());
    assert_eq!(sv::list_themes(&env.paths).len(), 1);
}

// ---------------------------------------------------------------- snippets

fn snip(collection: &str, name: &str, kw: &str, text: &str) -> Snippet {
    Snippet {
        name: name.into(),
        keyword: kw.into(),
        text: text.into(),
        collection: collection.into(),
        ..Snippet::default()
    }
}

#[test]
fn snippet_library_round_trips_collections_and_flat_entries() {
    let env = Env::new("snippets", &[]);
    let file = env.paths.snippets_file();
    let mut lib = SnippetLibrary::load(&file);
    assert!(lib.entries().is_empty());

    lib.add(snip("", "Sig", "sig", "Best,\nMe"));
    lib.add_collection("Work").unwrap();
    assert!(lib.add_collection("work").is_err(), "names are unique ignoring case");
    lib.add(snip("Work", "Address", "addr", "1 Main St"));
    lib.add(snip("Fresh", "Hello", "hi", "hello"));
    lib.save(&file).unwrap();

    let again = SnippetLibrary::load(&file);
    let names: Vec<_> = again
        .entries()
        .iter()
        .map(|(_, s)| (s.collection.clone(), s.name.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("".into(), "Sig".into()),
            ("Fresh".into(), "Hello".into()),
            ("Work".into(), "Address".into())
        ]
    );
    assert_eq!(again.collection_names(), vec!["Fresh", "Work"]);
    // the engine's own loader sees exactly the same
    assert_eq!(ashdash_core::clipboard::load_snippets(&file).len(), 3);

    // edit, move between collections, delete
    let mut lib = again;
    let (loc, _) = lib.entries().into_iter().find(|(_, s)| s.name == "Address").unwrap();
    lib.replace(loc, snip("Work", "Address", "addr", "2 Side St"));
    let (loc, _) = lib.entries().into_iter().find(|(_, s)| s.name == "Hello").unwrap();
    lib.replace(loc, snip("Work", "Hello", "hi", "hello"));
    lib.remove_collection("Fresh");
    lib.save(&file).unwrap();
    let after = ashdash_core::clipboard::load_snippets(&file);
    assert_eq!(after.len(), 3);
    assert!(after.iter().any(|s| s.name == "Address" && s.text == "2 Side St"));
    assert!(after.iter().any(|s| s.name == "Hello" && s.collection == "Work"));
    assert!(
        !env.paths.data.join("snippets/Fresh.json").exists(),
        "removed collection file is deleted"
    );
}

#[test]
fn saving_snippets_keeps_imported_prefixes_and_ignores_unreadable_files() {
    let env = Env::new("snippets-keep", &[]);
    let file = env.paths.snippets_file();
    let dir = env.paths.data.join("snippets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Imported.json"),
        serde_json::to_vec(&json!({"name": "Imported", "keyword_prefix": ";", "keyword_suffix": "",
            "snippets": [{"name": "A", "keyword": "a", "text": "AAA", "uid": "u1"}]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("Broken.json"), b"{ not json").unwrap();

    let mut lib = SnippetLibrary::load(&file);
    assert_eq!(lib.entries()[0].1.resolved_keyword(), ";a");
    lib.add(snip("Imported", "B", "b", "BBB"));
    lib.save(&file).unwrap();

    assert_eq!(
        std::fs::read(dir.join("Broken.json")).unwrap(),
        b"{ not json",
        "unreadable file untouched"
    );
    let all = ashdash_core::clipboard::load_snippets(&file);
    let a = all.iter().find(|s| s.name == "A").unwrap();
    assert_eq!((a.resolved_keyword().as_str(), a.uid.as_str()), (";a", "u1"));
    assert_eq!(all.iter().find(|s| s.name == "B").unwrap().resolved_keyword(), ";b");
}
