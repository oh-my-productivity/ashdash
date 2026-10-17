//! Safari: `~/Library/Safari/Bookmarks.plist` (binary or XML property list).
//!
//! Reading this file requires Full Disk Access; without it the open fails
//! with a permission error, which is reported as
//! [`SourceErrorKind::PermissionDenied`] so the provider can explain the
//! remedy once instead of failing noisily.
//!
//! Structure (observed from the format, not copied from any app): the root
//! dictionary is a `WebBookmarkTypeList` whose `Children` array holds nodes.
//! A node is `WebBookmarkTypeLeaf` (`URLString`, `URIDictionary.title`),
//! `WebBookmarkTypeList` (`Title`, `Children`) or `WebBookmarkTypeProxy`
//! (History; ignored). The Reading List is a list titled
//! `com.apple.ReadingList` and is skipped: its entries are not bookmarks.

use std::path::PathBuf;

use plist::{Dictionary, Value};

use super::{Bookmark, BookmarkSource, FOLDER_SEP, Fingerprint, SourceError, read_optional, stamp};

const MAX_DEPTH: usize = 64;

pub struct SafariSource {
    path: PathBuf,
}

impl SafariSource {
    pub fn new(path: PathBuf) -> Self {
        SafariSource { path }
    }
}

impl BookmarkSource for SafariSource {
    fn id(&self) -> &'static str {
        "safari"
    }

    fn name(&self) -> &'static str {
        "Safari"
    }

    fn fingerprint(&self) -> Fingerprint {
        vec![stamp(&self.path)]
    }

    fn load(&self) -> Result<Vec<Bookmark>, SourceError> {
        let Some(bytes) = read_optional(&self.path)? else {
            return Ok(Vec::new());
        };
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(Vec::new());
        }
        let root = crate::theme::parse_plist(&bytes)
            .map_err(|e| SourceError::other(format!("{}: not a valid property list ({e})", self.path.display())))?;
        let Some(root) = root.as_dictionary() else {
            return Err(SourceError::other(format!(
                "{}: unexpected plist root",
                self.path.display()
            )));
        };
        let mut out = Vec::new();
        // The root itself has no display name: start with an empty path.
        walk_children(root, &mut Vec::new(), &mut out, 0);
        Ok(out)
    }
}

fn walk_children(node: &Dictionary, path: &mut Vec<String>, out: &mut Vec<Bookmark>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(children) = node.get("Children").and_then(Value::as_array) else {
        return;
    };
    for child in children.iter().filter_map(Value::as_dictionary) {
        match child.get("WebBookmarkType").and_then(Value::as_string) {
            Some("WebBookmarkTypeLeaf") => {
                let Some(url) = child.get("URLString").and_then(Value::as_string) else {
                    continue;
                };
                let title = child
                    .get("URIDictionary")
                    .and_then(Value::as_dictionary)
                    .and_then(|d| d.get("title"))
                    .and_then(Value::as_string)
                    .unwrap_or("");
                out.push(Bookmark {
                    title: title.to_string(),
                    url: url.to_string(),
                    folder: path.join(FOLDER_SEP),
                    source: "Safari".to_string(),
                });
            }
            Some("WebBookmarkTypeList") => {
                let raw = child.get("Title").and_then(Value::as_string).unwrap_or("");
                if raw == "com.apple.ReadingList" {
                    continue;
                }
                path.push(folder_label(raw, depth).to_string());
                walk_children(child, path, out, depth + 1);
                path.pop();
            }
            _ => {}
        }
    }
}

/// Safari's two built-in top-level folders carry internal names.
fn folder_label(raw: &str, depth: usize) -> &str {
    match (depth, raw) {
        (0, "BookmarksBar") => "Favorites",
        (0, "BookmarksMenu") => "Bookmarks Menu",
        (_, "") => "Untitled",
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-bm-safari-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn leaf(title: &str, url: &str) -> Value {
        let mut uri = Dictionary::new();
        uri.insert("title".into(), title.into());
        let mut d = Dictionary::new();
        d.insert("WebBookmarkType".into(), "WebBookmarkTypeLeaf".into());
        d.insert("URLString".into(), url.into());
        d.insert("URIDictionary".into(), Value::Dictionary(uri));
        Value::Dictionary(d)
    }

    fn list(title: &str, children: Vec<Value>) -> Value {
        let mut d = Dictionary::new();
        d.insert("WebBookmarkType".into(), "WebBookmarkTypeList".into());
        d.insert("Title".into(), title.into());
        d.insert("Children".into(), Value::Array(children));
        Value::Dictionary(d)
    }

    fn fixture() -> Value {
        let mut proxy = Dictionary::new();
        proxy.insert("WebBookmarkType".into(), "WebBookmarkTypeProxy".into());
        proxy.insert("Title".into(), "History".into());
        let mut root = Dictionary::new();
        root.insert("WebBookmarkType".into(), "WebBookmarkTypeList".into());
        root.insert(
            "Children".into(),
            Value::Array(vec![
                Value::Dictionary(proxy),
                list(
                    "BookmarksBar",
                    vec![
                        leaf("Rust", "https://www.rust-lang.org/"),
                        list("Dev", vec![list("Tools", vec![leaf("Crates", "https://crates.io/")])]),
                    ],
                ),
                list("BookmarksMenu", vec![leaf("Apple", "https://apple.com")]),
                list(
                    "com.apple.ReadingList",
                    vec![leaf("Read later", "https://example.com/later")],
                ),
            ]),
        );
        Value::Dictionary(root)
    }

    #[test]
    fn parses_binary_plist_with_nested_folders() {
        let dir = tmp("bin");
        let f = dir.join("Bookmarks.plist");
        plist::to_file_binary(&f, &fixture()).unwrap();
        let got = SafariSource::new(f).load().unwrap();
        let rows: Vec<_> = got.iter().map(|b| (b.title.as_str(), b.folder.as_str())).collect();
        assert_eq!(
            rows,
            vec![
                ("Rust", "Favorites"),
                ("Crates", "Favorites › Dev › Tools"),
                ("Apple", "Bookmarks Menu"),
            ]
        );
        assert!(got.iter().all(|b| b.source == "Safari"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_xml_plist() {
        let dir = tmp("xml");
        let f = dir.join("Bookmarks.plist");
        plist::to_file_xml(&f, &fixture()).unwrap();
        assert_eq!(SafariSource::new(f).load().unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_and_empty_files_are_empty_lists() {
        let dir = tmp("empty");
        assert!(SafariSource::new(dir.join("absent.plist")).load().unwrap().is_empty());
        let f = dir.join("Bookmarks.plist");
        std::fs::write(&f, b"").unwrap();
        assert!(SafariSource::new(f).load().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_file_is_a_plain_error_not_a_panic() {
        let dir = tmp("bad");
        let f = dir.join("Bookmarks.plist");
        std::fs::write(&f, b"bplist00 garbage \x00\x01\x02").unwrap();
        let e = SafariSource::new(f).load().unwrap_err();
        assert_eq!(e.kind, super::super::SourceErrorKind::Other);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreadable_file_reports_permission_denied() {
        let dir = tmp("denied");
        let f = dir.join("Bookmarks.plist");
        plist::to_file_binary(&f, &fixture()).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Running as root would bypass the mode bits; nothing to assert then.
        if std::fs::read(&f).is_ok() {
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        let e = SafariSource::new(f).load().unwrap_err();
        assert_eq!(e.kind, super::super::SourceErrorKind::PermissionDenied);
        let _ = std::fs::remove_dir_all(dir);
    }
}
