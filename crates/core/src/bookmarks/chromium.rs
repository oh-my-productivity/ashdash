//! Chromium-family browsers: Chrome (+Beta/Canary), Chromium, Brave, Edge,
//! Vivaldi, Opera. All store a JSON file named `Bookmarks`.
//!
//! Chrome-like browsers keep one `Bookmarks` file per profile directory
//! (`Default`, `Profile 1`, ...) directly below the vendor directory; Opera
//! keeps a single one in the vendor directory itself. We check the vendor
//! directory and each of its direct sub-directories, so every profile is
//! found. The file's `roots` object holds `bookmark_bar`, `other`, `synced`
//! (and `trash` in some forks, which we skip); nodes are
//! `{"type": "url"|"folder", "name", "url", "children"}`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Bookmark, BookmarkSource, FOLDER_SEP, Fingerprint, SourceError, read_optional, stamp};

const MAX_DEPTH: usize = 64;

pub struct ChromiumSource {
    id: &'static str,
    name: &'static str,
    vendor_dirs: Vec<PathBuf>,
}

impl ChromiumSource {
    pub fn new(id: &'static str, name: &'static str, vendor_dirs: Vec<PathBuf>) -> Self {
        ChromiumSource { id, name, vendor_dirs }
    }

    /// Every `Bookmarks` file with its profile label (empty for the vendor
    /// directory itself), sorted for determinism.
    fn files(&self) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        for dir in &self.vendor_dirs {
            if dir.join("Bookmarks").is_file() {
                found.push((dir.join("Bookmarks"), String::new()));
            }
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                let f = p.join("Bookmarks");
                if p.is_dir() && f.is_file() {
                    found.push((f, entry.file_name().to_string_lossy().into_owned()));
                }
            }
        }
        found.sort();
        found
    }

    fn label(&self, profile: &str) -> String {
        if profile.is_empty() || profile == "Default" {
            self.name.to_string()
        } else {
            format!("{} ({profile})", self.name)
        }
    }
}

impl BookmarkSource for ChromiumSource {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn fingerprint(&self) -> Fingerprint {
        self.files().iter().map(|(p, _)| stamp(p)).collect()
    }

    fn load(&self) -> Result<Vec<Bookmark>, SourceError> {
        let files = self.files();
        let mut out = Vec::new();
        let mut first_err = None;
        let mut ok = 0usize;
        for (path, profile) in &files {
            match parse_file(path, &self.label(profile)) {
                Ok(mut v) => {
                    ok += 1;
                    out.append(&mut v);
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        // One broken profile must not hide the others; fail only if all did.
        match first_err {
            Some(e) if ok == 0 => Err(e),
            _ => Ok(out),
        }
    }
}

fn parse_file(path: &Path, source: &str) -> Result<Vec<Bookmark>, SourceError> {
    let Some(bytes) = read_optional(path)? else {
        return Ok(Vec::new());
    };
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Vec::new());
    }
    let json: Value = serde_json::from_slice(&bytes)
        .map_err(|e| SourceError::other(format!("{}: invalid JSON ({e})", path.display())))?;
    Ok(parse_json(&json, source))
}

pub(crate) fn parse_json(json: &Value, source: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let Some(roots) = json.get("roots").and_then(Value::as_object) else {
        return out;
    };
    // Fixed order for the well-known roots, then anything else foldery.
    let known = ["bookmark_bar", "other", "synced"];
    let keys = known.iter().copied().filter(|k| roots.contains_key(*k)).chain(
        roots
            .keys()
            .map(String::as_str)
            .filter(|k| !known.contains(k) && *k != "trash"),
    );
    for key in keys {
        let node = &roots[key];
        if node.get("type").and_then(Value::as_str) != Some("folder") {
            continue;
        }
        let label = match key {
            "bookmark_bar" => "Bookmarks Bar".to_string(),
            "other" => "Other Bookmarks".to_string(),
            "synced" => "Mobile Bookmarks".to_string(),
            _ => node.get("name").and_then(Value::as_str).unwrap_or(key).to_string(),
        };
        walk(node, &mut vec![label], source, &mut out, 0);
    }
    out
}

fn walk(folder: &Value, path: &mut Vec<String>, source: &str, out: &mut Vec<Bookmark>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(children) = folder.get("children").and_then(Value::as_array) else {
        return;
    };
    for node in children {
        match node.get("type").and_then(Value::as_str) {
            Some("url") => {
                let Some(url) = node.get("url").and_then(Value::as_str) else {
                    continue;
                };
                out.push(Bookmark {
                    title: node.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    url: url.to_string(),
                    folder: path.join(FOLDER_SEP),
                    source: source.to_string(),
                });
            }
            Some("folder") => {
                let name = node.get("name").and_then(Value::as_str).unwrap_or("");
                path.push(if name.is_empty() { "Untitled" } else { name }.to_string());
                walk(node, path, source, out, depth + 1);
                path.pop();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::super::SourceErrorKind;
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-bm-chromium-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const NESTED: &str = r#"{
      "checksum": "x", "version": 1,
      "roots": {
        "bookmark_bar": {"type":"folder","name":"Bookmarks bar","children":[
          {"type":"url","name":"Rust","url":"https://www.rust-lang.org/"},
          {"type":"folder","name":"Dev","children":[
            {"type":"folder","name":"Tools","children":[
              {"type":"url","name":"Crates","url":"https://crates.io/"}]},
            {"type":"url","name":"Bookmarklet","url":"javascript:alert(1)"}]}]},
        "other": {"type":"folder","name":"Other bookmarks","children":[
          {"type":"url","name":"Docs","url":"https://docs.rs/"}]},
        "synced": {"type":"folder","name":"Mobile bookmarks","children":[
          {"type":"url","name":"Phone","url":"http://m.example.com/"}]},
        "trash": {"type":"folder","name":"Trash","children":[
          {"type":"url","name":"Deleted","url":"https://deleted.example/"}]},
        "sync_transaction_version": "3"
      }
    }"#;

    #[test]
    fn nested_folders_and_all_roots() {
        let got = parse_json(&serde_json::from_str(NESTED).unwrap(), "Chrome");
        let rows: Vec<_> = got.iter().map(|b| (b.title.as_str(), b.folder.as_str())).collect();
        assert_eq!(
            rows,
            vec![
                ("Rust", "Bookmarks Bar"),
                ("Crates", "Bookmarks Bar › Dev › Tools"),
                ("Bookmarklet", "Bookmarks Bar › Dev"), // scheme filtering happens in the cache
                ("Docs", "Other Bookmarks"),
                ("Phone", "Mobile Bookmarks"),
            ]
        );
    }

    #[test]
    fn every_profile_directory_is_read() {
        let home = tmp("profiles");
        let vendor = home.join("Google/Chrome");
        for (profile, name, url) in [
            ("Default", "A", "https://a.example/"),
            ("Profile 1", "B", "https://b.example/"),
            ("Profile 2", "C", "https://c.example/"),
        ] {
            let d = vendor.join(profile);
            std::fs::create_dir_all(&d).unwrap();
            let json = format!(
                r#"{{"roots":{{"bookmark_bar":{{"type":"folder","name":"x","children":[{{"type":"url","name":"{name}","url":"{url}"}}]}}}}}}"#
            );
            std::fs::write(d.join("Bookmarks"), json).unwrap();
        }
        // Noise that must be ignored.
        std::fs::create_dir_all(vendor.join("Crashpad")).unwrap();
        std::fs::write(vendor.join("Local State"), "{}").unwrap();

        let src = ChromiumSource::new("chrome", "Chrome", vec![vendor]);
        let got = src.load().unwrap();
        let mut names: Vec<_> = got.iter().map(|b| (b.title.clone(), b.source.clone())).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ("A".into(), "Chrome".into()),
                ("B".into(), "Chrome (Profile 1)".into()),
                ("C".into(), "Chrome (Profile 2)".into()),
            ]
        );
        assert_eq!(src.fingerprint().len(), 3);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn opera_style_file_in_vendor_dir() {
        let home = tmp("opera");
        let vendor = home.join("com.operasoftware.Opera");
        std::fs::create_dir_all(&vendor).unwrap();
        std::fs::write(vendor.join("Bookmarks"), NESTED).unwrap();
        let src = ChromiumSource::new("opera", "Opera", vec![vendor]);
        assert_eq!(src.load().unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn missing_dirs_empty_and_malformed_files() {
        let home = tmp("bad");
        let src = ChromiumSource::new("brave", "Brave", vec![home.join("nothing-here")]);
        assert!(src.load().unwrap().is_empty());
        assert!(src.fingerprint().is_empty());

        let vendor = home.join("v");
        std::fs::create_dir_all(vendor.join("Default")).unwrap();
        std::fs::create_dir_all(vendor.join("Profile 1")).unwrap();
        std::fs::write(vendor.join("Default/Bookmarks"), "").unwrap();
        std::fs::write(vendor.join("Profile 1/Bookmarks"), "{ not json").unwrap();
        let src = ChromiumSource::new("brave", "Brave", vec![vendor.clone()]);
        // One empty (ok) + one malformed: partial success is not an error.
        assert!(src.load().unwrap().is_empty());
        // Only the malformed one: error.
        std::fs::remove_file(vendor.join("Default/Bookmarks")).unwrap();
        assert_eq!(src.load().unwrap_err().kind, SourceErrorKind::Other);
        // Valid JSON of the wrong shape: no bookmarks, no error.
        std::fs::write(vendor.join("Profile 1/Bookmarks"), "[1,2,3]").unwrap();
        assert!(src.load().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn unreadable_file_reports_permission_denied() {
        let home = tmp("denied");
        let vendor = home.join("v");
        std::fs::create_dir_all(&vendor).unwrap();
        let f = vendor.join("Bookmarks");
        std::fs::write(&f, NESTED).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&f).is_ok() {
            let _ = std::fs::remove_dir_all(home);
            return;
        }
        let src = ChromiumSource::new("chrome", "Chrome", vec![vendor]);
        assert_eq!(src.load().unwrap_err().kind, SourceErrorKind::PermissionDenied);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn deeply_nested_input_does_not_overflow() {
        let mut s = String::from(r#"{"roots":{"bookmark_bar":"#);
        let depth = 100;
        for _ in 0..depth {
            s.push_str(r#"{"type":"folder","name":"f","children":["#);
        }
        s.push_str(r#"{"type":"url","name":"deep","url":"https://deep.example/"}"#);
        for _ in 0..depth {
            s.push_str("]}");
        }
        s.push_str("}}");
        // serde_json's own recursion limit is 128; both outcomes are fine as
        // long as nothing panics and the over-deep entry is not returned.
        if let Ok(json) = serde_json::from_str::<Value>(&s) {
            assert!(parse_json(&json, "x").is_empty());
        }
    }
}
