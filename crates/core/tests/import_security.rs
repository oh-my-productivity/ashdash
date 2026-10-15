//! `import_workflow` / `import_package` with synthetic (and hostile) archives.
//! The archives are built in the test; nothing is read from the user's system.
//!
//! Invariant under test: nothing is ever written outside the workflows
//! directory, and the installed tree contains nothing that points outside
//! itself. See docs/SECURITY.md (SEC-07, SEC-10, SEC-11).

mod common;

use std::path::{Path, PathBuf};

use ashdash_core::workflow::import_package;
use common::*;
use serde_json::json;

struct Sandbox {
    root: TempDir,
    /// Target directory handed to the importer.
    dest_root: PathBuf,
    /// A directory next to it that must never change.
    outside: PathBuf,
    pkg: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Sandbox {
        let root = TempDir::new(tag);
        let dest_root = root.join("workflows");
        let outside = root.join("outside");
        std::fs::create_dir_all(&dest_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("canary.txt"), b"keep me").unwrap();
        let pkg = root.join("pkg.alfredworkflow");
        Sandbox {
            root,
            dest_root,
            outside,
            pkg,
        }
    }

    fn plist(&self) -> Vec<u8> {
        plist_bytes(&keyword_workflow("Imported", "qa.imported", "imp"))
    }

    /// Everything under `root` except the (expected-to-change) workflows dir and the package.
    fn untouched_snapshot(&self) -> Vec<String> {
        snapshot(self.root.path())
            .into_iter()
            .filter(|p| !p.starts_with("workflows") && p != "pkg.alfredworkflow")
            .collect()
    }

    fn assert_nothing_escaped(&self, before: &[String]) {
        assert_eq!(&self.untouched_snapshot(), before, "files appeared outside the workflows dir");
        assert_eq!(
            std::fs::read(self.outside.join("canary.txt")).unwrap(),
            b"keep me",
            "canary modified"
        );
        // the workflows dir itself holds only installed workflow dirs (no stray files
        // from entries that climbed out of the staging dir)
        for e in std::fs::read_dir(&self.dest_root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            assert!(
                name.starts_with("user.workflow.") || name.starts_with(".import-"),
                "stray entry in workflows dir: {name}"
            );
        }
    }
}

/// Every symlink inside `dir` must resolve to something inside `dir`.
fn escaping_symlinks(dir: &Path) -> Vec<String> {
    let canon = dir.canonicalize().unwrap();
    let mut bad = Vec::new();
    for rel in snapshot(dir) {
        let p = dir.join(&rel);
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            let ok = p.canonicalize().map(|c| c.starts_with(&canon)).unwrap_or(true);
            if !ok {
                bad.push(format!("{rel} -> {:?}", std::fs::read_link(&p).unwrap()));
            }
        }
    }
    bad
}

// ---------------------------------------------------------------- happy path

#[test]
fn well_formed_package_installs_and_is_found_by_the_engine() {
    let env = Env::new("imp-ok", &["Safari"]);
    let pkg = env.root.join("good.alfredworkflow");
    write_zip(
        &pkg,
        &[
            Entry::File("info.plist", plist_bytes(&keyword_workflow("Imported", "qa.imported", "imp"))),
            Entry::File("icon.png", vec![0x89, b'P', b'N', b'G']),
            Entry::Dir("sub/"),
            Entry::File("sub/helper.sh", b"#!/bin/sh\necho hi\n".to_vec()),
        ],
    );
    let e = env.engine();
    let dest = e.import_workflow(&pkg).expect("import");
    assert_eq!(dest, env.paths.workflows_dir().join("user.workflow.qa.imported"));
    assert!(dest.join("icon.png").is_file() && dest.join("sub/helper.sh").is_file());
    assert_eq!(e.workflow_summaries(), vec![("Imported".into(), "qa.imported".into(), false)]);
    assert_eq!(e.query("imp x")[0].title, "Imported");
    // staging dir is gone
    assert!(
        std::fs::read_dir(env.paths.workflows_dir())
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().starts_with(".import-"))
    );
}

#[test]
fn reimport_replaces_the_previous_version_completely() {
    let sb = Sandbox::new("imp-re");
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::File("old.txt", b"old".to_vec()),
        ],
    );
    let d1 = import_package(&sb.pkg, &sb.dest_root).unwrap();
    assert!(d1.join("old.txt").exists());
    write_zip(&sb.pkg, &[Entry::File("info.plist", sb.plist()), Entry::File("new.txt", b"new".to_vec())]);
    let d2 = import_package(&sb.pkg, &sb.dest_root).unwrap();
    assert_eq!(d1, d2);
    assert!(!d2.join("old.txt").exists() && d2.join("new.txt").exists());
}

// ---------------------------------------------------------------- path traversal

fn attempt(tag: &str, entries: &[Entry]) -> (Sandbox, Vec<String>, anyhow::Result<PathBuf>) {
    let sb = Sandbox::new(tag);
    write_zip(&sb.pkg, entries);
    let before = sb.untouched_snapshot();
    let r = import_package(&sb.pkg, &sb.dest_root);
    (sb, before, r)
}

#[test]
fn dotdot_entries_do_not_escape() {
    let plist = Sandbox::new("x").plist();
    for name in [
        "../escape.txt",
        "../../escape.txt",
        "a/../../escape.txt",
        "a/b/../../../escape.txt",
        "./../escape.txt",
        "..\\escape.txt",
        "a/..\\..\\escape.txt",
    ] {
        let (sb, before, r) = attempt(
            "trav",
            &[Entry::File("info.plist", plist.clone()), Entry::File(name, b"pwn".to_vec())],
        );
        // Either outcome is acceptable (reject or neutralise); escaping is not.
        let _ = r;
        sb.assert_nothing_escaped(&before);
        assert!(!sb.root.join("escape.txt").exists(), "{name}");
        assert!(!sb.dest_root.join("escape.txt").exists(), "{name} landed in workflows dir");
    }
}

#[test]
fn absolute_path_entries_do_not_escape() {
    let sb = Sandbox::new("abs");
    let target = sb.outside.join("abs-escape.txt");
    let target_s = target.display().to_string();
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::File(&target_s, b"pwn".to_vec()),
            Entry::File(&format!("/{}", sb.root.join("abs2.txt").display()), b"pwn".to_vec()),
        ],
    );
    let before = sb.untouched_snapshot();
    let _ = import_package(&sb.pkg, &sb.dest_root);
    assert!(!target.exists(), "absolute entry written to {}", target.display());
    sb.assert_nothing_escaped(&before);
}

#[test]
fn symlink_entry_cannot_be_used_to_write_outside() {
    let sb = Sandbox::new("symlink-write");
    let outside = sb.outside.display().to_string();
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::Symlink("link", &outside),
            Entry::File("link/pwned.txt", b"pwn".to_vec()),
        ],
    );
    let before = sb.untouched_snapshot();
    let _ = import_package(&sb.pkg, &sb.dest_root);
    assert!(!sb.outside.join("pwned.txt").exists(), "wrote through a symlink");
    sb.assert_nothing_escaped(&before);
}

#[test]
fn relative_symlink_chain_cannot_be_used_to_write_outside() {
    let sb = Sandbox::new("symlink-rel");
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::Symlink("up", "../.."),
            Entry::File("up/outside/pwned.txt", b"pwn".to_vec()),
        ],
    );
    let before = sb.untouched_snapshot();
    let _ = import_package(&sb.pkg, &sb.dest_root);
    assert!(!sb.outside.join("pwned.txt").exists());
    sb.assert_nothing_escaped(&before);
}

/// Chains of in-bounds-looking symlinks that only escape once resolved
/// (`up -> .`, `x -> up/..`, `y -> x/..`) must not let a later entry be
/// written outside the staging dir. (The zip library resolves these; this
/// pins that behaviour.)
#[test]
fn symlink_chains_that_resolve_outside_are_rejected() {
    let sb = Sandbox::new("symlink-chain");
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::Symlink("up", "."),
            Entry::Symlink("x", "up/.."),
            Entry::Symlink("y", "x/.."),
            Entry::File("y/outside/pwned.txt", b"pwn".to_vec()),
        ],
    );
    let before = sb.untouched_snapshot();
    let _ = import_package(&sb.pkg, &sb.dest_root);
    assert!(!sb.outside.join("pwned.txt").exists());
    sb.assert_nothing_escaped(&before);
}

/// Whatever is accepted, the installed tree must not contain links that leave
/// the workflow directory.
#[test]
fn installed_workflow_contains_no_escaping_symlinks() {
    let sb = Sandbox::new("symlink-keep");
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::Symlink("to-root", "/"),
            Entry::Symlink("to-parent", "../../.."),
            Entry::Symlink("icon.png", "/etc/hosts"),
        ],
    );
    // The current extractor rejects the whole archive; accepting it with the links removed would also be fine.
    if let Ok(dest) = import_package(&sb.pkg, &sb.dest_root) {
        let bad = escaping_symlinks(&dest);
        assert!(bad.is_empty(), "escaping symlinks installed: {bad:?}");
    }
}

/// In-archive links that stay inside are preserved and harmless.
#[test]
fn internal_symlinks_are_allowed() {
    let sb = Sandbox::new("symlink-ok");
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::File("real.txt", b"r".to_vec()),
            Entry::Symlink("alias.txt", "real.txt"),
        ],
    );
    let dest = import_package(&sb.pkg, &sb.dest_root).unwrap();
    assert!(escaping_symlinks(&dest).is_empty());
}

#[test]
fn symlinked_info_plist_cannot_smuggle_an_outside_file_into_loading() {
    let sb = Sandbox::new("symlink-plist");
    // `info.plist` is a symlink to a file outside; loading must not succeed with that content.
    let secret = sb.outside.join("secret.plist");
    std::fs::write(&secret, plist_bytes(&keyword_workflow("Stolen", "qa.stolen", "st"))).unwrap();
    write_zip(&sb.pkg, &[Entry::Symlink("info.plist", &secret.display().to_string())]);
    let before = sb.untouched_snapshot();
    let r = import_package(&sb.pkg, &sb.dest_root);
    sb.assert_nothing_escaped(&before);
    if let Ok(d) = r {
        // if it was accepted at all, it must not have loaded the outside document
        let wf = ashdash_core::workflow::Workflow::load(&d).unwrap();
        assert_ne!(wf.name, "Stolen");
    }
}

// ---------------------------------------------------------------- naming

#[test]
fn bundle_id_cannot_steer_the_destination() {
    for bundle in ["../../evil", "/etc/passwd", "..", ".", "a/b/c", "x\u{0}y", "name with spaces & $(rm -rf)"] {
        let sb = Sandbox::new("bundle");
        let doc = keyword_workflow("N", bundle, "k");
        write_zip(&sb.pkg, &[Entry::File("info.plist", plist_bytes(&doc))]);
        let before = sb.untouched_snapshot();
        let r = import_package(&sb.pkg, &sb.dest_root);
        let dest = r.unwrap_or_else(|e| panic!("{bundle:?}: {e:#}"));
        assert_eq!(dest.parent().unwrap(), sb.dest_root, "{bundle:?} -> {dest:?}");
        let name = dest.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("user.workflow.") && !name.contains('/'), "{name}");
        sb.assert_nothing_escaped(&before);
    }
}

#[test]
fn empty_bundle_id_falls_back_to_name_and_rejects_when_both_empty() {
    let sb = Sandbox::new("noname");
    let doc = json!({"name": "", "bundleid": "", "objects": [], "connections": {}});
    write_zip(&sb.pkg, &[Entry::File("info.plist", plist_bytes(&doc))]);
    assert!(import_package(&sb.pkg, &sb.dest_root).is_err());
    let doc = json!({"name": "Only Name", "bundleid": "", "objects": [], "connections": {}});
    write_zip(&sb.pkg, &[Entry::File("info.plist", plist_bytes(&doc))]);
    let d = import_package(&sb.pkg, &sb.dest_root).unwrap();
    assert_eq!(d.file_name().unwrap(), "user.workflow.Only_Name");
}

// ---------------------------------------------------------------- malformed input

#[test]
fn malformed_archives_are_errors_not_panics() {
    let sb = Sandbox::new("malformed");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("text", b"definitely not a zip".to_vec()),
        ("pk-only", b"PK\x03\x04".to_vec()),
        ("eocd-only", b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec()),
    ];
    for (what, bytes) in cases {
        std::fs::write(&sb.pkg, bytes).unwrap();
        assert!(import_package(&sb.pkg, &sb.dest_root).is_err(), "{what}");
    }
    // truncated real archive
    write_zip(&sb.pkg, &[Entry::File("info.plist", sb.plist())]);
    let bytes = std::fs::read(&sb.pkg).unwrap();
    for cut in [bytes.len() / 2, bytes.len() - 10, 30] {
        std::fs::write(&sb.pkg, &bytes[..cut]).unwrap();
        assert!(import_package(&sb.pkg, &sb.dest_root).is_err(), "truncated at {cut}");
    }
    // missing file
    assert!(import_package(&sb.root.join("nope.alfredworkflow"), &sb.dest_root).is_err());
}

#[test]
fn missing_nested_or_broken_plist_is_rejected() {
    for (what, entries) in [
        ("no plist", vec![Entry::File("readme.txt", b"x".to_vec())]),
        ("nested plist", vec![Entry::File("Folder/info.plist", Sandbox::new("y").plist())]),
        ("garbage plist", vec![Entry::File("info.plist", b"<<<not a plist".to_vec())]),
        ("plist is a dir", vec![Entry::Dir("info.plist/")]),
        (
            "object without uid",
            vec![Entry::File(
                "info.plist",
                plist_bytes(&json!({"name":"n","bundleid":"b","objects":[{"type":"x"}]})),
            )],
        ),
    ] {
        let (sb, before, r) = attempt("bad-plist", &entries);
        assert!(r.is_err(), "{what} was accepted");
        // nothing installed
        assert!(
            std::fs::read_dir(&sb.dest_root)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with("user.workflow.")),
            "{what}"
        );
        sb.assert_nothing_escaped(&before);
    }
}

#[test]
fn hostile_entry_names_do_not_panic() {
    let plist = Sandbox::new("z").plist();
    let long = format!("{}/file", "d/".repeat(1000));
    let very_long_component = "x".repeat(5000);
    for name in [
        long.as_str(),
        very_long_component.as_str(),
        "con:/aux",
        " ",
        "a//b///c",
        "\u{202e}rtl.txt",
        "emoji-😀/ü.txt",
    ] {
        let (sb, before, r) = attempt(
            "names",
            &[Entry::File("info.plist", plist.clone()), Entry::File(name, b"x".to_vec())],
        );
        let _ = r; // reject or accept; only escaping/panicking is wrong
        sb.assert_nothing_escaped(&before);
    }
}

/// A rejected import must not leave its staging directory behind.
#[test]
#[ignore = "known: SEC-10 (failed import leaks .import-<pid> staging dir)"]
fn failed_import_cleans_up_staging_dir() {
    let (sb, _before, r) = attempt("leak", &[Entry::File("readme.txt", b"x".to_vec())]);
    assert!(r.is_err());
    let leaked: Vec<_> = std::fs::read_dir(&sb.dest_root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(leaked.is_empty(), "left behind: {leaked:?}");
}

/// A pre-planted staging path (predictable name) must not redirect extraction.
#[test]
fn preplanted_staging_symlink_is_not_followed() {
    let sb = Sandbox::new("staging-link");
    let staging = sb.dest_root.join(format!(".import-{}", std::process::id()));
    std::os::unix::fs::symlink(&sb.outside, &staging).unwrap();
    write_zip(&sb.pkg, &[Entry::File("info.plist", sb.plist()), Entry::File("marker.txt", b"m".to_vec())]);
    let r = import_package(&sb.pkg, &sb.dest_root);
    assert!(sb.outside.join("canary.txt").exists(), "canary deleted via staging symlink");
    assert!(!sb.outside.join("marker.txt").exists(), "extracted through staging symlink");
    assert!(!sb.outside.join("info.plist").exists());
    let _ = r;
}

/// Decompression bombs: an archive that expands to hundreds of MB is accepted without limits.
#[test]
#[ignore = "known: SEC-07 (no size/entry-count limit on extraction); writes ~300 MB to a temp dir"]
fn zip_bomb_is_rejected() {
    let sb = Sandbox::new("bomb");
    let zeros = vec![0u8; 100 * 1024 * 1024];
    write_zip(
        &sb.pkg,
        &[
            Entry::File("info.plist", sb.plist()),
            Entry::File("a.bin", zeros.clone()),
            Entry::File("b.bin", zeros.clone()),
            Entry::File("c.bin", zeros),
        ],
    );
    assert!(import_package(&sb.pkg, &sb.dest_root).is_err(), "300 MB of zeros was extracted");
}
