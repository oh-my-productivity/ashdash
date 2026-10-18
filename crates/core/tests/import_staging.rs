//! Staged imports: the package is opened once, the user is asked about that copy, and exactly
//! that copy is installed (docs/reviews/wave3.md item 3; docs/SECURITY.md SEC-21).

mod common;

use ashdash_core::format_ids::{kind, workflow_file};
use std::path::{Path, PathBuf};

use common::{Entry, Env, plist_bytes, write_zip};
use serde_json::{Value, json};

/// A workflow whose single script prints `marker`, so the installed bytes can be told apart.
fn doc(name: &str, bundle: &str, marker: &str) -> Value {
    json!({
        "name": name, "bundleid": bundle, "version": "1.0", "createdby": "Ann Author",
        "description": "Prints a marker",
        "objects": [
            {"uid": "S", "type": kind::ACTION_SCRIPT,
             "config": {"type": 0, "scriptargtype": 1, "script": format!("echo {marker}")}},
        ],
        "connections": {}
    })
}

fn package(env: &Env, file: &str, d: &Value) -> PathBuf {
    let p = env.root.join(file);
    write_zip(&p, &[Entry::File("info.plist", plist_bytes(d))]);
    p
}

fn installed_plist(env: &Env, dest: &Path) -> String {
    let _ = env;
    String::from_utf8(std::fs::read(dest.join("info.plist")).unwrap()).unwrap()
}

#[test]
fn the_installed_bytes_are_the_previewed_bytes_even_if_the_file_is_swapped() {
    let env = Env::new("stage-swap", &[]);
    let engine = env.engine();
    let pkg = package(&env, &workflow_file("w"), &doc("Harmless", "stage.swap", "PREVIEWED"));

    let staged = engine.stage_workflow(&pkg).unwrap();
    assert_eq!(staged.preview().name, "Harmless");

    // between the question and the answer the file is replaced by a different workflow
    write_zip(
        &pkg,
        &[Entry::File(
            "info.plist",
            plist_bytes(&doc("Evil", "stage.evil", "SWAPPED")),
        )],
    );

    let dest = engine.install_staged(staged).unwrap();
    let text = installed_plist(&env, &dest);
    assert!(text.contains("PREVIEWED"), "{text}");
    assert!(!text.contains("SWAPPED") && !text.contains("Evil"), "{text}");
    assert!(dest.ends_with("user.workflow.stage.swap"), "{}", dest.display());
    assert!(
        !env.paths.workflows_dir().join("user.workflow.stage.evil").exists(),
        "the swapped package was never read"
    );
}

#[test]
fn the_installed_bytes_survive_the_file_being_deleted() {
    let env = Env::new("stage-delete", &[]);
    let engine = env.engine();
    let pkg = package(&env, &workflow_file("w"), &doc("W", "stage.del", "KEPT"));
    let staged = engine.stage_workflow(&pkg).unwrap();
    std::fs::remove_file(&pkg).unwrap();
    let dest = engine.install_staged(staged).unwrap();
    assert!(installed_plist(&env, &dest).contains("KEPT"));
}

#[test]
fn staging_is_private_installs_nothing_and_cleans_up_when_dropped() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("stage-private", &[]);
    let engine = env.engine();
    let pkg = package(&env, &workflow_file("w"), &doc("W", "stage.priv", "x"));
    let staged = engine.stage_workflow(&pkg).unwrap();

    let root = env.paths.data.join("import-staging");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700, "staging root is private");
    let kids: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().collect();
    assert_eq!(kids.len(), 1);
    assert_eq!(mode(&kids[0].path()), 0o700, "the staged tree is private");
    assert!(
        !env.paths.workflows_dir().join("user.workflow.stage.priv").exists(),
        "not installed yet"
    );
    assert!(engine.workflow_summaries().is_empty());

    drop(staged);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "cancel deletes the copy");
}

#[test]
fn a_vanished_staging_copy_is_an_error_not_a_partial_install() {
    let env = Env::new("stage-gone", &[]);
    let engine = env.engine();
    let pkg = package(&env, &workflow_file("w"), &doc("W", "stage.gone", "x"));
    let staged = engine.stage_workflow(&pkg).unwrap();
    std::fs::remove_dir_all(env.paths.data.join("import-staging")).unwrap();
    let err = engine.install_staged(staged).unwrap_err();
    assert!(format!("{err:#}").contains("open the file again"), "{err:#}");
    assert!(!env.paths.workflows_dir().join("user.workflow.stage.gone").exists());
}

#[test]
fn stale_staging_leftovers_are_swept_but_fresh_ones_are_not() {
    let env = Env::new("stage-sweep", &[]);
    let engine = env.engine();
    let root = env.paths.data.join("import-staging");
    std::fs::create_dir_all(&root).unwrap();
    let (old, fresh, other) = (
        root.join(".import-old"),
        root.join(".import-fresh"),
        root.join("keep-me"),
    );
    for d in [&old, &fresh, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
    std::fs::File::open(&old).unwrap().set_modified(long_ago).unwrap();
    std::fs::File::open(&other).unwrap().set_modified(long_ago).unwrap();

    let pkg = package(&env, &workflow_file("w"), &doc("W", "stage.sweep", "x"));
    let _staged = engine.stage_workflow(&pkg).unwrap();
    assert!(!old.exists(), "a day-old leftover is removed");
    assert!(fresh.exists(), "a recent one may belong to a prompt that is still open");
    assert!(other.exists(), "only .import-* entries are ever touched");
}

#[test]
fn the_preview_reports_what_would_be_replaced_and_inherited() {
    let env = Env::new("stage-replace", &[]);
    let engine = env.engine();
    let first = package(&env, &workflow_file("v1"), &doc("Weather", "stage.rep", "ONE"));
    let staged = engine.stage_workflow(&first).unwrap();
    assert!(staged.preview().replaces.is_none());
    assert!(!staged.preview().saved_settings);
    assert_eq!(staged.preview().author, "Ann Author");
    assert_eq!(staged.preview().description, "Prints a marker");
    engine.install_staged(staged).unwrap();

    // the user saved a value (could be an API token) for the installed workflow
    let cfg = env.paths.workflow_config_dir();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("stage.rep.json"), r#"{"token": "secret"}"#).unwrap();

    let mut v2 = doc("Weather Pro", "stage.rep", "TWO");
    v2["version"] = json!("2.0");
    let second = package(&env, &workflow_file("v2"), &v2);
    let staged = engine.stage_workflow(&second).unwrap();
    let pv = staged.preview();
    assert!(pv.replaces_installed());
    let old = pv.replaces.as_ref().unwrap();
    assert_eq!((old.name.as_str(), old.version.as_str()), ("Weather", "1.0"));
    assert_eq!(pv.version, "2.0");
    assert!(pv.saved_settings, "the saved values stay in force and are disclosed");
    // declining leaves the installed one untouched
    drop(staged);
    let dest = env.paths.workflows_dir().join("user.workflow.stage.rep");
    assert!(installed_plist(&env, &dest).contains("ONE"));
}

#[test]
fn a_package_without_a_bundle_id_is_installed_where_it_was_previewed() {
    let env = Env::new("stage-nobundle", &[]);
    let engine = env.engine();
    let d = json!({"name": "No Id", "objects": [], "connections": {}});
    let pkg = package(&env, &workflow_file("n"), &d);
    let staged = engine.stage_workflow(&pkg).unwrap();
    let planned = staged.destination().to_path_buf();
    assert_eq!(engine.install_staged(staged).unwrap(), planned);
    assert!(planned.join("info.plist").is_file());
}

#[test]
fn import_package_still_installs_in_one_step_and_leaves_no_staging_behind() {
    let env = Env::new("stage-oneshot", &[]);
    let pkg = package(&env, &workflow_file("w"), &doc("W", "stage.one", "x"));
    let root = env.paths.workflows_dir();
    let dest = ashdash_core::workflow::import_package(&pkg, &root).unwrap();
    assert!(dest.join("info.plist").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".import-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn saved_settings_of_an_id_less_workflow_are_found_by_its_folder_name() {
    let env = Env::new("stage-idless-settings", &[]);
    let engine = env.engine();
    let d = json!({"name": "No Id", "objects": [], "connections": {}});
    let pkg = package(&env, &workflow_file("n"), &d);
    let cfg = env.paths.workflow_config_dir();
    std::fs::create_dir_all(&cfg).unwrap();
    assert!(!engine.stage_workflow(&pkg).unwrap().preview().saved_settings);
    std::fs::write(cfg.join("user.workflow.No_Id.json"), "{}").unwrap();
    let staged = engine.stage_workflow(&pkg).unwrap();
    let name = staged.destination().file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name, "user.workflow.No_Id");
    assert!(staged.preview().saved_settings);
}
