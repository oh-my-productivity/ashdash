//! Putting a history entry back on the system pasteboard.
//!
//! Text goes through `pbcopy`. Images and file lists are written with a small
//! JavaScript-for-Automation script (`osascript -l JavaScript`) that calls
//! `NSPasteboard` directly, so the pasteboard gets the modern types
//! (`public.png`, `public.file-url`) and needs no extra permission and no
//! Objective-C bindings in this crate. (AppleScript's `set the clipboard to`
//! was tried first: it writes only legacy flavours.) The decision of *what*
//! to run is the pure [`plan`], so it is unit-tested without touching the
//! pasteboard; paths are embedded as JSON string literals, which are valid
//! JavaScript and escape quotes, backslashes and control characters.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};

use super::history::{ClipEntry, ClipKind, ClipboardHistory};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePlan {
    /// Pipe this text into `pbcopy`.
    Text(String),
    /// Run this JXA source with `osascript -l JavaScript -e`.
    Jxa(String),
}

fn js_string(s: &str) -> String {
    // Serialising a string cannot fail.
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Decide how to restore `entry`. `image_path` is the PNG for image entries
/// (see [`ClipboardHistory::image_path`]); `exists` reports whether a path
/// still exists (injected for tests).
pub fn plan(entry: &ClipEntry, image_path: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Result<RestorePlan> {
    match entry.kind {
        ClipKind::Text => Ok(RestorePlan::Text(entry.text.clone())),
        ClipKind::Image => {
            let p = image_path.context("the image file for this entry is not available")?;
            if !exists(p) {
                bail!("the image file for this entry is gone");
            }
            Ok(RestorePlan::Jxa(format!(
                "ObjC.import(\"AppKit\");\n\
                 const pb = $.NSPasteboard.generalPasteboard;\n\
                 const d = $.NSData.dataWithContentsOfFile({});\n\
                 if (!d) throw new Error(\"cannot read the image file\");\n\
                 pb.clearContents;\n\
                 if (!pb.setDataForType(d, \"public.png\")) throw new Error(\"pasteboard refused the image\");",
                js_string(&p.to_string_lossy())
            )))
        }
        ClipKind::Files => {
            let present: Vec<String> = entry
                .files()
                .into_iter()
                .filter(|f| exists(Path::new(f)))
                .map(|f| format!("$.NSURL.fileURLWithPath({})", js_string(f)))
                .collect();
            if present.is_empty() {
                bail!("none of the copied files exist any more");
            }
            Ok(RestorePlan::Jxa(format!(
                "ObjC.import(\"AppKit\");\n\
                 const pb = $.NSPasteboard.generalPasteboard;\n\
                 pb.clearContents;\n\
                 if (!pb.writeObjects($([{}]))) throw new Error(\"pasteboard refused the files\");",
                present.join(", ")
            )))
        }
    }
}

/// Run a plan against the real pasteboard.
pub fn apply(plan: &RestorePlan) -> Result<()> {
    match plan {
        RestorePlan::Text(t) => copy_text(t),
        RestorePlan::Jxa(src) => {
            let out = crate::actions::system_command("osascript")
                .args(["-l", "JavaScript", "-e"])
                .arg(src)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .context("running osascript")?;
            if !out.status.success() {
                bail!(
                    "could not set the clipboard: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(())
        }
    }
}

/// Copy text to the pasteboard via `pbcopy`, forcing a UTF-8 locale (apps
/// started from Finder have no `LANG`, which garbles non-ASCII text).
pub fn copy_text(text: &str) -> Result<()> {
    let mut child = crate::actions::system_command("pbcopy")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .spawn()
        .context("running pbcopy")?;
    child
        .stdin
        .take()
        .context("pbcopy stdin")?
        .write_all(text.as_bytes())
        .context("writing to pbcopy")?;
    if !child.wait()?.success() {
        bail!("pbcopy failed");
    }
    Ok(())
}

/// Put `entry` back on the pasteboard.
pub fn restore(history: &ClipboardHistory, entry: &ClipEntry) -> Result<()> {
    let path: Option<PathBuf> = history.image_path(entry);
    apply(&plan(entry, path.as_deref(), &|p| p.exists())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::history::tests::{fake_png, tmp};

    fn entry(kind: ClipKind, text: &str) -> ClipEntry {
        ClipEntry {
            text: text.into(),
            ts: 1,
            kind,
            image: None,
            pinned: false,
        }
    }

    fn jxa(p: Result<RestorePlan>) -> String {
        match p.expect("plan") {
            RestorePlan::Jxa(s) => s,
            other => panic!("expected a script, got {other:?}"),
        }
    }

    #[test]
    fn text_restores_as_text() {
        let e = entry(ClipKind::Text, "héllo\nworld");
        assert_eq!(
            plan(&e, None, &|_| false).unwrap(),
            RestorePlan::Text("héllo\nworld".into())
        );
    }

    #[test]
    fn image_plan_writes_public_png() {
        let e = entry(ClipKind::Image, "");
        let p = PathBuf::from("/tmp/dir/0123456789abcdef.png");
        let s = jxa(plan(&e, Some(&p), &|_| true));
        assert!(
            s.contains("dataWithContentsOfFile(\"/tmp/dir/0123456789abcdef.png\")"),
            "{s}"
        );
        assert!(s.contains("setDataForType(d, \"public.png\")"), "{s}");
        assert!(plan(&e, Some(&p), &|_| false).is_err(), "file gone");
        assert!(plan(&e, None, &|_| true).is_err(), "no path");
    }

    #[test]
    fn file_list_plan_skips_missing_files_and_escapes() {
        let e = entry(ClipKind::Files, "/a/one.txt\n/a/gone.txt\n/a/q\"uote\\.txt");
        let exists = |p: &Path| !p.to_string_lossy().contains("gone");
        let s = jxa(plan(&e, None, &exists));
        assert!(
            s.contains("[$.NSURL.fileURLWithPath(\"/a/one.txt\"), $.NSURL.fileURLWithPath(\"/a/q\\\"uote\\\\.txt\")]"),
            "{s}"
        );
        assert!(!s.contains("gone"));
        assert!(plan(&e, None, &|_| false).is_err(), "all files gone");
    }

    #[test]
    fn hostile_paths_stay_inside_their_string_literal() {
        for evil in [
            "/x\") ; $.system(\"id\") ; (\"",
            "/x\\\") ; evil() ; (\"",
            "/x\u{2028}y",
            "/x\u{7}y",
        ] {
            let e = entry(ClipKind::Files, evil);
            // `files()` splits on newlines, so a path is one line here.
            let s = jxa(plan(&e, None, &|_| true));
            let lit = js_string(evil);
            assert!(s.contains(&lit), "{s}");
            // The literal is a JSON string: no raw control characters, and every
            // quote inside it is escaped.
            let body = &lit[1..lit.len() - 1];
            assert!(!body.chars().any(|c| c.is_control()), "{lit}");
            let mut chars = body.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => panic!("unescaped quote in {lit}"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn image_entries_resolve_their_png_through_the_history() {
        let root = tmp("restore-img");
        let mut h = ClipboardHistory::load(root.join("clipboard.json"), 5);
        assert!(h.push_image(&fake_png(4, 4, 9)));
        let e = h.entries()[0].clone();
        let p = h.image_path(&e);
        let s = jxa(plan(&e, p.as_deref(), &|p| p.exists()));
        assert!(s.contains(".png") && s.contains("clipboard-images"), "{s}");
        let _ = std::fs::remove_dir_all(root);
    }
}
