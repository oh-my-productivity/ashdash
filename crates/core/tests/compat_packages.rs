//! Compatibility suite against the workflow templates that ship inside an installed
//! The reference launcher. Skips cleanly when the reference launcher is not installed.
//!
//! Reads the bundled `*.alfredworkflow` files (zip + info.plist) at test time and imports
//! them into a temp dir; nothing is copied into the repository and nothing outside the
//! temp dir is written. The bundled scripts are tiny, side-effect free templates, and
//! workflow *effects* (notifications, URLs, files) are returned, never performed.
//!
//! Run with `cargo test -p ashdash-core --test compat_packages -- --nocapture` to see the
//! node-type table.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ashdash_core::item::Mods;
use ashdash_core::workflow::{Effect, Workflow, WorkflowAction};
use common::*;

const RESOURCES: &str =
    "/Applications/Reference.app/Contents/Resources";

fn bundled() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(RESOURCES) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "alfredworkflow"))
        .collect();
    v.sort();
    v
}

/// Types the compat run reports as unsupported (from `Effect::Warn` text).
fn unsupported_type(msg: &str) -> Option<String> {
    let rest = msg.split("unsupported workflow object '").nth(1)?;
    Some(rest.split('\'').next()?.to_string())
}

/// Every object with outgoing connections is a place a run can start from.
fn start_nodes(wf: &Workflow) -> Vec<String> {
    wf.nodes
        .iter()
        .filter(|n| wf.connections.contains_key(&n.uid))
        .map(|n| n.uid.clone())
        .collect()
}

#[test]
fn every_bundled_reference_workflow_imports_loads_and_runs_without_panics() {
    let packages = bundled();
    if packages.is_empty() {
        eprintln!("compat_packages: the reference launcher not installed, skipping");
        return;
    }
    let env = Env::new("compat", &[]);
    let engine = env.engine();

    let mut import_failures = Vec::new();
    for p in &packages {
        if let Err(e) = engine.import_workflow(p) {
            import_failures.push(format!("{}: {e:#}", p.file_name().unwrap().to_string_lossy()));
        }
    }
    assert!(import_failures.is_empty(), "import failed: {import_failures:#?}");

    // Packages whose bundle id (or name) collide replace each other, so count by directory.
    let loaded = ashdash_core::workflow::load_all(&[env.paths.workflows_dir()]);
    assert!(!loaded.is_empty(), "nothing loaded");
    eprintln!(
        "compat_packages: {} packages imported, {} distinct workflows loaded",
        packages.len(),
        loaded.len()
    );

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut unsupported: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut keyword_items_total = 0usize;

    for wf in &loaded {
        for n in &wf.nodes {
            *seen.entry(n.kind.clone()).or_default() += 1;
        }

        // 1. keyword / Script Filter items for every configured keyword, typed three ways
        for n in &wf.nodes {
            let kw = n.config.get("keyword").and_then(|v| v.as_str()).unwrap_or("");
            if kw.is_empty() {
                continue;
            }
            for q in [kw.to_string(), format!("{kw} "), format!("{kw} hello world")] {
                let (items, _exclusive) = wf.keyword_items(&q, &env.paths);
                keyword_items_total += items.len();
                for it in &items {
                    assert!(!it.title.is_empty() || !it.subtitle.is_empty() || !it.id.is_empty());
                }
            }
        }

        // 2. run the graph from every connected object with a plain argument
        for uid in start_nodes(wf) {
            let act = WorkflowAction {
                workflow: wf.id.clone(),
                from_uid: uid,
                arg: "qa argument".into(),
                vars: BTreeMap::new(),
                mods: vec![],
                ..Default::default()
            };
            for e in wf.run(&act, Mods::NONE, &env.paths) {
                if let Effect::Warn(msg) = e
                    && let Some(t) = unsupported_type(&msg)
                {
                    unsupported.entry(t).or_default().insert(wf.name.clone());
                }
            }
        }
    }

    eprintln!("compat_packages: {keyword_items_total} keyword/script-filter items generated");
    eprintln!("compat_packages: node types seen across bundled workflows:");
    for (k, n) in &seen {
        let mark = if unsupported.contains_key(k) {
            "UNSUPPORTED"
        } else {
            "ok / not reached"
        };
        eprintln!("  {n:>3}  {k}  [{mark}]");
    }
    eprintln!("compat_packages: unsupported at run time (type -> workflows):");
    for (k, w) in &unsupported {
        eprintln!("  {k} -> {w:?}");
    }
    let never_run: Vec<&String> = seen
        .keys()
        .filter(|k| k.contains(".trigger.") || k.contains(".input."))
        .collect();
    eprintln!("compat_packages: entry-point types (not exercised by graph runs): {never_run:?}");
}

#[test]
fn bundled_info_plists_all_parse_with_the_loader() {
    let packages = bundled();
    if packages.is_empty() {
        eprintln!("compat_packages: the reference launcher not installed, skipping");
        return;
    }
    // Direct unzip of just info.plist into temp dirs, bypassing import_package, so a
    // loader regression is distinguishable from an extraction regression.
    let t = TempDir::new("compat-plist");
    for (i, p) in packages.iter().enumerate() {
        let f = std::fs::File::open(p).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut entry = z
            .by_name("info.plist")
            .unwrap_or_else(|_| panic!("{}: no info.plist", p.display()));
        let dir = t.join(format!("w{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = std::fs::File::create(dir.join("info.plist")).unwrap();
        std::io::copy(&mut entry, &mut out).unwrap();
        Workflow::load(&dir).unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
    }
}
