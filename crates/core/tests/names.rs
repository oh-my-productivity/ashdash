//! Name guard: the project names that must not appear in the repository.
//!
//! Walks `git ls-files` from the repository root and fails when
//!
//! * the retired working name appears anywhere, in a file's contents or in its path;
//! * the name of the product whose file formats Ashdash reads appears outside the three
//!   files that list the format identifiers (`ALLOWED_FILES`);
//! * a token in one of those three files is not a format identifier: the allowed tokens are
//!   the object types, file extensions, dictionary keys and script variables that
//!   `format_ids.rs` defines (see `is_format_identifier`).
//!
//! The words themselves are derived from `format_ids::ENV_PREFIX`, so this file does not
//! contain them either. The test is skipped when `git` or the work tree is not available.

use std::path::{Path, PathBuf};
use std::process::Command;

use ashdash_core::format_ids::ENV_PREFIX;
use regex::Regex;

/// The only files that may contain format identifiers.
const ALLOWED_FILES: [&str; 3] = [
    "crates/core/src/format_ids.rs",
    "crates/app/resources/Info.plist.template",
    "docs/WORKFLOWS.md",
];

/// The name that appears in the format identifiers (lower case).
fn name() -> String {
    ENV_PREFIX.trim_end_matches('_').to_ascii_lowercase()
}

/// The retired working name: the name plus a suffix.
fn working_name() -> String {
    format!("{}o", name())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repository root")
}

/// Tracked files, relative to the root; `None` when this is not a git work tree.
fn tracked_files(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        out.stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
    )
}

/// 1-based line numbers of the lines of `bytes` that contain `needle` (ASCII case-insensitive).
fn lines_with(bytes: &[u8], needle: &str) -> Vec<usize> {
    let needle = needle.as_bytes();
    bytes
        .split(|b| *b == b'\n')
        .enumerate()
        .filter(|(_, line)| {
            let lower = line.to_ascii_lowercase();
            lower.windows(needle.len()).any(|w| w == needle)
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// Whether `token` (a maximal run of letters, digits, `_` and `.` around the name) is one of
/// the identifiers the workflow package, appearance and snippet formats define.
fn is_format_identifier(token: &str) -> bool {
    let n = regex::escape(&name());
    let token = token.trim_end_matches('.').to_ascii_lowercase();
    let patterns = [
        // object types: <name>.workflow.<kind>.<object>
        format!(r"^{n}\.workflow(\.[a-z]+)*$"),
        // file extensions, with or without the dot: workflow packages (any version),
        // appearance files, snippet collections
        format!(r"^\.?{n}[2-5]?workflow$"),
        format!(r"^\.?{n}appearance$"),
        format!(r"^\.?{n}snippets$"),
        // dictionary keys
        format!(r"^{n}(theme|snippet|filtersresults)$"),
        // script variables and handler names: <name>_<words>
        format!(r"^{n}_[a-z_]*$"),
    ];
    patterns.iter().any(|p| Regex::new(p).unwrap().is_match(&token))
}

/// Tokens around the name in one line.
fn tokens_in(line: &str) -> Vec<String> {
    let n = regex::escape(&name());
    let re = Regex::new(&format!(r"(?i)[a-z0-9_.\-]*{n}[a-z0-9_.\-]*")).unwrap();
    re.find_iter(line).map(|m| m.as_str().to_string()).collect()
}

fn scan(root: &Path, files: &[String]) -> Vec<String> {
    let name = name();
    let working = working_name();
    let mut problems = Vec::new();
    for rel in files {
        let allowed = ALLOWED_FILES.contains(&rel.as_str());
        let lower_path = rel.to_ascii_lowercase();
        if lower_path.contains(&working) {
            problems.push(format!("{rel}: the path contains the retired working name"));
        }
        if lower_path.contains(&name) {
            problems.push(format!("{rel}: the path contains the name"));
        }
        let Ok(bytes) = std::fs::read(root.join(rel)) else {
            continue; // deleted in the work tree, or a directory entry such as a submodule
        };
        for line in lines_with(&bytes, &working) {
            problems.push(format!("{rel}:{line}: contains the retired working name"));
        }
        let hits = lines_with(&bytes, &name);
        if hits.is_empty() {
            continue;
        }
        if !allowed {
            let shown: Vec<String> = hits.iter().take(5).map(|l| l.to_string()).collect();
            problems.push(format!(
                "{rel}: contains the name on {} line(s) (first: {})",
                hits.len(),
                shown.join(", ")
            ));
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            for token in tokens_in(line) {
                if !is_format_identifier(&token) {
                    problems.push(format!("{rel}:{}: `{token}` is not a format identifier", i + 1));
                }
            }
        }
    }
    problems
}

#[test]
fn the_names_appear_only_where_the_format_identifiers_live() {
    let root = repo_root();
    let Some(files) = tracked_files(&root) else {
        eprintln!("names: skipped, no git work tree at {}", root.display());
        return;
    };
    let problems = scan(&root, &files);
    assert!(
        problems.is_empty(),
        "{} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn the_allowed_files_exist_and_are_tracked() {
    let root = repo_root();
    let Some(files) = tracked_files(&root) else {
        return;
    };
    for f in [
        "crates/core/src/format_ids.rs",
        "crates/app/resources/Info.plist.template",
    ] {
        assert!(files.iter().any(|x| x == f), "{f} must be tracked");
    }
}

#[test]
fn the_classifier_accepts_identifiers_and_rejects_prose() {
    let n = name();
    for ok in [
        format!("{n}.workflow.input.keyword"),
        format!("{n}.workflow."),
        format!(".{n}workflow"),
        format!("{n}3workflow"),
        format!(".{n}5workflow"),
        format!("{n}appearance"),
        format!(".{n}snippets"),
        format!("{n}theme"),
        format!("{n}snippet"),
        format!("{n}filtersresults"),
        format!("{n}_workflow_data"),
        format!("{n}_"),
        format!("{n}_script"),
    ] {
        assert!(is_format_identifier(&ok), "{ok}");
    }
    for bad in [
        n.clone(),
        format!("{n}s"),
        format!("{n}-compatible"),
        format!("{n}.app"),
        format!("{n}1workflow"),
        format!("{n}.com"),
        working_name(),
    ] {
        assert!(!is_format_identifier(&bad), "{bad}");
    }
}

#[test]
fn tokens_are_cut_at_punctuation_around_the_identifier() {
    let n = name();
    let line = format!("| `{n}.workflow.action.script` | see the {n} docs, and `.{n}appearance`. |");
    assert_eq!(
        tokens_in(&line),
        vec![
            format!("{n}.workflow.action.script"),
            n.clone(),
            format!(".{n}appearance")
        ]
    );
}

#[test]
fn line_search_is_case_insensitive_and_reports_line_numbers() {
    let n = name();
    let text = format!("a\nb {}\nc\n{n}\n", n.to_ascii_uppercase());
    assert_eq!(lines_with(text.as_bytes(), &n), vec![2, 4]);
}
