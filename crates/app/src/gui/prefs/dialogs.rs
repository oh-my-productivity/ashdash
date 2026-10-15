//! A native "choose file" dialog through `osascript` (arguments go in as argv, never into the
//! script text). Blocking: call it from a thread.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Ask for one file whose name ends in `extension`. `None` when cancelled or unavailable.
pub fn choose_file(prompt: &str, extension: &str) -> Option<PathBuf> {
    let out = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "set f to choose file with prompt (item 1 of argv) of type {item 2 of argv}",
            "-e",
            "return POSIX path of f",
            "-e",
            "end run",
            "--",
            prompt,
            extension,
        ])
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

    #[test]
    fn only_absolute_paths_are_accepted() {
        assert_eq!(
            parse_choice("/Users/a/x.alfredworkflow\n"),
            Some("/Users/a/x.alfredworkflow".into())
        );
        assert_eq!(parse_choice(""), None);
        assert_eq!(parse_choice("relative/x\n"), None);
        assert_eq!(parse_choice("execution error: User canceled. (-128)"), None);
    }
}
