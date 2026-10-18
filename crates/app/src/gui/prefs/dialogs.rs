//! A native "choose file" dialog through `osascript` (arguments go in as argv, never into the
//! script text). Blocking: call it from a thread.

use std::path::PathBuf;
use std::process::Stdio;

/// Ask for one file whose name ends in one of `extensions`. `None` when cancelled or unavailable.
pub fn choose_file(prompt: &str, extensions: &[&str]) -> Option<PathBuf> {
    let out = ashdash_core::actions::system_command("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "set f to choose file with prompt (item 1 of argv) of type (items 2 thru -1 of argv)",
            "-e",
            "return POSIX path of f",
            "-e",
            "end run",
            "--",
            prompt,
        ])
        .args(extensions)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // user cancelled (-128) or no osascript
    }
    parse_choice(&String::from_utf8_lossy(&out.stdout))
}

/// The path line osascript prints, if it is an absolute path.
pub fn parse_choice(stdout: &str) -> Option<PathBuf> {
    let line = stdout.trim_end_matches(['\n', '\r']);
    line.starts_with('/').then(|| PathBuf::from(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashdash_core::format_ids::workflow_file;

    #[test]
    fn only_absolute_paths_are_accepted() {
        assert_eq!(
            parse_choice(&format!("{}\n", workflow_file("/Users/a/x"))),
            Some(workflow_file("/Users/a/x").into())
        );
        assert_eq!(parse_choice(""), None);
        assert_eq!(parse_choice("relative/x\n"), None);
        assert_eq!(parse_choice("execution error: User canceled. (-128)"), None);
    }
}
