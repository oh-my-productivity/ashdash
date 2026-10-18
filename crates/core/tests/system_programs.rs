//! Regression guard (re-review N5, tightened in the Wave 4 fixes): macOS tools the launcher runs
//! are started by their absolute path (`actions::system_program` / `actions::system_command`),
//! never by a bare name that `PATH` would resolve.
//!
//! The guard reads the sources of **both crates** (`crates/core/src` and `crates/app/src`) and
//! looks at every `Command::new(<argument>)`, wherever the call sits (single line or broken over
//! several, `std::process::Command::new`, with or without `&`), except in `actions.rs`, which
//! defines the helpers:
//!
//! * a string literal starting with `/` is an absolute path: fine;
//! * a string literal that is one of the tools in `SYSTEM_PROGRAMS` (`"osascript"`, `"open"`...)
//!   fails;
//! * any other bare literal (`"zip"`, `"kill"`) and any expression that is not a call of
//!   `system_program(...)` must be listed in `KNOWN_SPAWNS` below, with the reason.
//!   A new spawn site therefore fails this test until someone has looked at it.
//!
//! **Limits** (what this cannot see): it is a text scan, not a parse. It does not follow a program
//! name through a variable or a constant (`let p = "osascript"; Command::new(p)` is only caught
//! because a non-literal argument has to be in `KNOWN_SPAWNS`), it ignores macros, code generated
//! at build time, `crates/*/benches`, `crates/*/tests` and `build.rs`, and it cannot tell test
//! code from production code. It says nothing about programs that run *scripts* (workflow
//! interpreters are fixed paths in `workflow/script.rs`; what a script calls inside is the
//! script's business). `Cmd::new("name")` is not a spawn: the `Os` implementation resolves it
//! through `system_program` in one place (`actions.rs::build`, tested there).

use std::path::{Path, PathBuf};

use regex::Regex;

/// Spawn sites that are neither an absolute literal nor `system_program(...)`: (file suffix,
/// start of the argument as written, why it is fine).
const KNOWN_SPAWNS: &[(&str, &str, &str)] = &[
    (
        "core/src/workflow/script.rs",
        "&program",
        "the interpreter of a Run Script object: a fixed absolute path from the interpreter table, or the script file the workflow ships",
    ),
    (
        "core/src/workflow/script.rs",
        "\"kill\"",
        "test code: checks that a timed-out script's grandchild was killed",
    ),
    (
        "core/src/bookmarks/firefox.rs",
        "&self.sqlite3",
        "defaults to /usr/bin/sqlite3; a test double can point it elsewhere",
    ),
    (
        "core/src/config.rs",
        "std::env::current_exe()",
        "test code: runs this very test binary",
    ),
    (
        "app/src/gui/prefs/mod.rs",
        "std::env::current_exe()",
        "starts the preferences window as a child of this executable",
    ),
    (
        "app/src/main.rs",
        "std::env::current_exe()",
        "starts this executable again (daemon / child)",
    ),
    (
        "app/src/mac/opendocs.rs",
        "\"zip\"",
        "test code: builds a workflow package fixture with the zip CLI",
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// What a spawn site's argument is.
#[derive(Debug, PartialEq, Eq)]
enum Spawn {
    /// 1-based line, the argument text (start of it, one line).
    Bare(usize, String),
    Unlisted(usize, String),
}

/// Every `Command::new(...)` in `text` that is neither an absolute literal nor a `system_program`
/// call. The caller decides which `Unlisted` ones are accepted (`KNOWN_SPAWNS`).
fn suspicious_spawns(text: &str, tools: &[&str]) -> Vec<Spawn> {
    // Whole-line comments are not code (they quote the forbidden form on purpose).
    let code: String = text
        .lines()
        .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n");
    let call = Regex::new(r"Command\s*::\s*new\s*\(\s*").unwrap();
    let mut out = Vec::new();
    for m in call.find_iter(&code) {
        // `ClipCommand::new(` is somebody else's type.
        if code[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let line = code[..m.start()].matches('\n').count() + 1;
        let rest = &code[m.end()..];
        let rest_one_line: String = rest.chars().take(80).collect::<String>().replace('\n', " ");
        if let Some(lit) = rest.strip_prefix('"') {
            let lit = &lit[..lit.find('"').unwrap_or(lit.len())];
            if lit.starts_with('/') {
                continue;
            }
            if tools.contains(&lit) {
                out.push(Spawn::Bare(line, rest_one_line));
                continue;
            }
        } else if rest.starts_with("system_program(")
            || rest.starts_with("actions::system_program(")
            || rest.starts_with("crate::actions::system_program(")
        {
            continue;
        }
        out.push(Spawn::Unlisted(line, rest_one_line));
    }
    out
}

#[test]
fn no_bare_system_tool_names_are_spawned() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let names = ashdash_core::actions::system_program_names();
    assert!(names.contains(&"osascript") && names.contains(&"pbcopy"));
    let mut files = Vec::new();
    rust_files(&crates.join("core/src"), &mut files);
    let core_count = files.len();
    rust_files(&crates.join("app/src"), &mut files);
    assert!(
        core_count > 20 && files.len() > core_count + 5,
        "the scan found too few files"
    );
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for f in &files {
        if f.ends_with("core/src/actions.rs") {
            continue;
        }
        let path = f.to_string_lossy().replace('\\', "/");
        let known: Vec<&str> = KNOWN_SPAWNS
            .iter()
            .filter(|(suffix, _, _)| path.ends_with(suffix))
            .map(|(_, arg, _)| *arg)
            .collect();
        let text = std::fs::read_to_string(f).unwrap();
        for s in suspicious_spawns(&text, &names) {
            let (line, arg) = match &s {
                Spawn::Bare(l, a) | Spawn::Unlisted(l, a) => (*l, a.clone()),
            };
            if let Spawn::Unlisted(..) = s
                && let Some(k) = known.iter().find(|k| arg.starts_with(**k))
            {
                used.insert(((*k).to_string(), path.clone()));
                continue;
            }
            bad.push(format!("{}:{line}: Command::new({arg}", f.display()));
        }
    }
    assert!(
        bad.is_empty(),
        "process spawned by bare system tool name, or a spawn site that is not in KNOWN_SPAWNS \
         (use actions::system_command, or add it there with the reason):\n{}",
        bad.join("\n")
    );
    // An allowlist entry that matches nothing is stale: remove it.
    for (suffix, arg, _) in KNOWN_SPAWNS {
        assert!(
            used.iter().any(|(k, p)| k == arg && p.ends_with(suffix)),
            "KNOWN_SPAWNS entry {suffix} / {arg} matches no spawn site any more"
        );
    }
}

#[test]
fn the_scan_sees_calls_the_old_line_match_missed() {
    let tools = ["osascript", "open", "pbcopy"];
    let kinds = |t: &str| suspicious_spawns(t, &tools);
    // the old guard matched exactly `Command::new("name")` on one line
    assert!(matches!(kinds("Command::new(\"osascript\")")[..], [Spawn::Bare(1, _)]));
    // broken over lines, extra spaces, full path to the type
    assert!(matches!(
        kinds("let c = std::process::Command::new(\n        \"open\",\n    );")[..],
        [Spawn::Bare(1, _)]
    ));
    assert!(matches!(
        kinds("x;\nCommand :: new ( \"pbcopy\" )")[..],
        [Spawn::Bare(2, _)]
    ));
    // an expression or an unlisted bare name is reported until it is listed
    assert!(matches!(kinds("Command::new(&prog)")[..], [Spawn::Unlisted(1, _)]));
    assert!(matches!(kinds("Command::new(\"zip\")")[..], [Spawn::Unlisted(1, _)]));
    // fine: absolute literal, the helper, comments, other types called ...Command
    assert!(kinds("Command::new(\"/usr/bin/osascript\")").is_empty());
    assert!(kinds("Command::new(system_program(OsStr::new(\"open\")))").is_empty());
    assert!(kinds("Command::new(crate::actions::system_program(x))").is_empty());
    assert!(kinds("// Command::new(\"osascript\") is forbidden\n/// Command::new(\"open\")").is_empty());
    assert!(kinds("ClipCommand::new(\"open\")").is_empty());
}

#[test]
fn system_command_uses_the_absolute_path() {
    let c = ashdash_core::actions::system_command("osascript");
    assert_eq!(c.get_program(), "/usr/bin/osascript");
    let c = ashdash_core::actions::system_command("not-one-of-ours");
    assert_eq!(c.get_program(), "not-one-of-ours");
}

#[test]
fn every_known_tool_has_one_absolute_path_and_never_falls_back_to_path() {
    use std::ffi::OsStr;
    let names = ashdash_core::actions::system_program_names();
    let mut seen = std::collections::BTreeSet::new();
    for n in &names {
        assert!(seen.insert(*n), "{n} listed twice");
        let p = ashdash_core::actions::system_program(OsStr::new(n));
        let p = Path::new(&p);
        assert!(p.is_absolute(), "{n} -> {p:?}");
        assert_eq!(p.file_name().unwrap(), OsStr::new(n), "{n} -> {p:?}");
        // These are the macOS system locations; a Mac without one of them gets an explicit
        // "starting ..." error from the spawn, not a different program from PATH.
        if cfg!(target_os = "macos") {
            assert!(p.exists(), "{n}: {p:?} does not exist on this Mac");
        }
    }
    // a path is passed through, an unknown name stays a bare name (PATH lookup, documented)
    let abs = ashdash_core::actions::system_program(OsStr::new("/opt/x/osascript"));
    assert_eq!(abs, OsStr::new("/opt/x/osascript"));
    let other = ashdash_core::actions::system_program(OsStr::new("zip"));
    assert_eq!(other, OsStr::new("zip"));
}
