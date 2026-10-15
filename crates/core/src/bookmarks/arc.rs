//! Arc (The Browser Company): best-effort reader for
//! `~/Library/Application Support/Arc/StorableSidebar.json`.
//!
//! Arc has no bookmarks file; the closest thing is its sidebar of *pinned*
//! tabs and folders. The format is private and undocumented, so this reader
//! is deliberately tolerant and was written from the commonly observed shape,
//! NOT verified against a current Arc build:
//!
//! * somewhere below the root (we walk the whole JSON instead of trusting an
//!   exact path such as `sidebar.containers[].items`) there are node objects
//!   `{ "id", "parentID", "title", "data": {...} }`;
//! * a saved tab has `data.tab.savedURL` (+ `savedTitle`); a folder has
//!   `data.list` and a `title`;
//! * "space" objects carry `containerIDs`, an array alternating a label and an
//!   id (`["pinned", "<id>", "unpinned", "<id>"]`). Tabs whose ancestry
//!   reaches an `unpinned` container are today's open tabs, not bookmarks,
//!   and are skipped.
//!
//! If the file is missing, empty, unparsable or has an unrecognised shape
//! (Arc changes this format between releases, and newer builds may keep the
//! sidebar elsewhere or in a different structure) the source yields an empty
//! list rather than an error: a private format that changed is not something
//! the user can fix, so it must not produce a visible warning.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::Value;

use super::{Bookmark, BookmarkSource, FOLDER_SEP, Fingerprint, SourceError, read_optional, stamp};

const MAX_DEPTH: usize = 64;

pub struct ArcSource {
    path: PathBuf,
}

impl ArcSource {
    pub fn new(path: PathBuf) -> Self {
        ArcSource { path }
    }
}

impl BookmarkSource for ArcSource {
    fn id(&self) -> &'static str {
        "arc"
    }

    fn name(&self) -> &'static str {
        "Arc"
    }

    fn fingerprint(&self) -> Fingerprint {
        vec![stamp(&self.path)]
    }

    fn load(&self) -> Result<Vec<Bookmark>, SourceError> {
        // Permission problems are real and actionable; format problems are not.
        let Some(bytes) = read_optional(&self.path)? else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_slice::<Value>(&bytes)
            .map(|v| parse(&v))
            .unwrap_or_default())
    }
}

struct Node<'a> {
    parent: Option<&'a str>,
    title: &'a str,
    url: Option<&'a str>,
    is_folder: bool,
}

fn collect<'a>(v: &'a Value, nodes: &mut Vec<(&'a str, Node<'a>)>, unpinned: &mut HashSet<&'a str>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Object(o) => {
            if let Some(ids) = o.get("containerIDs").and_then(Value::as_array) {
                let ids: Vec<&str> = ids.iter().filter_map(Value::as_str).collect();
                for pair in ids.windows(2) {
                    if pair[0] == "unpinned" {
                        unpinned.insert(pair[1]);
                    }
                }
            }
            if let (Some(id), Some(data)) = (
                o.get("id").and_then(Value::as_str),
                o.get("data").and_then(Value::as_object),
            ) {
                let tab = data.get("tab").and_then(Value::as_object);
                let url = tab.and_then(|t| t.get("savedURL")).and_then(Value::as_str);
                let saved_title = tab.and_then(|t| t.get("savedTitle")).and_then(Value::as_str);
                let title = o.get("title").and_then(Value::as_str).or(saved_title).unwrap_or("");
                nodes.push((
                    id,
                    Node {
                        parent: o.get("parentID").and_then(Value::as_str),
                        title,
                        url,
                        is_folder: data.contains_key("list"),
                    },
                ));
            }
            for child in o.values() {
                collect(child, nodes, unpinned, depth + 1);
            }
        }
        Value::Array(a) => {
            for child in a {
                collect(child, nodes, unpinned, depth + 1);
            }
        }
        _ => {}
    }
}

pub(crate) fn parse(json: &Value) -> Vec<Bookmark> {
    let mut nodes = Vec::new();
    let mut unpinned = HashSet::new();
    collect(json, &mut nodes, &mut unpinned, 0);
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|(id, n)| (*id, n)).collect();

    let mut out = Vec::new();
    for (_, n) in &nodes {
        let Some(url) = n.url else { continue };
        let mut folders = Vec::new();
        let mut cur = n.parent;
        let mut is_unpinned = false;
        for _ in 0..MAX_DEPTH {
            let Some(pid) = cur else { break };
            if unpinned.contains(pid) {
                is_unpinned = true;
                break;
            }
            match by_id.get(pid) {
                Some(p) if p.is_folder => {
                    if !p.title.is_empty() {
                        folders.push(p.title);
                    }
                    cur = p.parent;
                }
                Some(p) => cur = p.parent, // a tab nesting tabs: keep climbing
                None => break,
            }
        }
        if is_unpinned {
            continue;
        }
        folders.reverse();
        out.push(Bookmark {
            title: n.title.to_string(),
            url: url.to_string(),
            folder: folders.join(FOLDER_SEP),
            source: "Arc".to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "sidebar": {"containers": [
        {"global": {}},
        {"spaces": ["s1", {"id":"s1","title":"Work","containerIDs":["pinned","P1","unpinned","U1"]}],
         "items": [
           "P1", {"id":"P1","parentID":null,"title":null,"data":{"itemContainer":{}}},
           "F1", {"id":"F1","parentID":"P1","title":"Docs","data":{"list":{}}},
           "F2", {"id":"F2","parentID":"F1","title":"Rust","data":{"list":{}}},
           "T1", {"id":"T1","parentID":"F2","title":null,"data":{"tab":{"savedURL":"https://doc.rust-lang.org/","savedTitle":"Rust docs"}}},
           "T2", {"id":"T2","parentID":"P1","title":"Pinned","data":{"tab":{"savedURL":"https://pinned.example/","savedTitle":"ignored"}}},
           "T3", {"id":"T3","parentID":"U1","title":null,"data":{"tab":{"savedURL":"https://open-today.example/","savedTitle":"Today"}}}
         ]}
      ]}
    }"#;

    #[test]
    fn pinned_tabs_become_bookmarks_and_unpinned_are_skipped() {
        let got = parse(&serde_json::from_str(FIXTURE).unwrap());
        let mut rows: Vec<_> = got.iter().map(|b| (b.title.as_str(), b.folder.as_str())).collect();
        rows.sort();
        assert_eq!(rows, vec![("Pinned", ""), ("Rust docs", "Docs › Rust")]);
    }

    #[test]
    fn unknown_shapes_and_bad_files_yield_empty_not_errors() {
        assert!(parse(&serde_json::json!({"hello": [1, 2, {"x": null}]})).is_empty());
        let dir = std::env::temp_dir().join(format!("oa-bm-arc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("StorableSidebar.json");
        let src = ArcSource::new(f.clone());
        assert!(src.load().unwrap().is_empty()); // missing
        std::fs::write(&f, "{ definitely not json").unwrap();
        assert!(src.load().unwrap().is_empty()); // malformed
        std::fs::write(&f, FIXTURE).unwrap();
        assert_eq!(src.load().unwrap().len(), 2); // real
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parent_cycles_terminate() {
        let j = serde_json::json!([
          {"id":"a","parentID":"b","title":"A","data":{"list":{}}},
          {"id":"b","parentID":"a","title":"B","data":{"list":{}}},
          {"id":"t","parentID":"a","title":"T","data":{"tab":{"savedURL":"https://x.example/"}}}
        ]);
        assert_eq!(parse(&j).len(), 1);
    }
}
