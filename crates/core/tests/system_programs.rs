//! Regression guard (re-review N5): macOS tools the launcher runs are started by their absolute
//! path (`actions::system_program` / `actions::system_command`), never by a bare name that `PATH`
//! would resolve. Reads the sources of both crates and fails on `Command::new("osascript")` and
//! friends outside `actions.rs`.

use std::path::{Path, PathBuf};

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

#[test]
fn no_bare_system_tool_names_are_spawned() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let names = ashdash_core::actions::system_program_names();
    assert!(names.contains(&"osascript") && names.contains(&"pbcopy"));
    let mut files = Vec::new();
    rust_files(&crates.join("core/src"), &mut files);
    rust_files(&crates.join("app/src"), &mut files);
    let mut bad = Vec::new();
    for f in files {
        if f.ends_with("core/src/actions.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            for name in &names {
                if line.contains(&format!("Command::new(\"{name}\")")) {
                    bad.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "bare system tool names:\n{}", bad.join("\n"));
}

#[test]
fn system_command_uses_the_absolute_path() {
    let c = ashdash_core::actions::system_command("osascript");
    assert_eq!(c.get_program(), "/usr/bin/osascript");
    let c = ashdash_core::actions::system_command("not-one-of-ours");
    assert_eq!(c.get_program(), "not-one-of-ours");
}
