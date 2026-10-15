//! Snippets: collections, loading, and import of the reference launcher `.alfredsnippets`.
//!
//! # On-disk layout (under the data directory)
//! * `snippets.json` - flat array of `{name, keyword, text}` (the original
//!   format; entries may carry an optional `collection`).
//! * `snippets/<Collection>.json` - one file per collection:
//!   `{"name": "...", "keyword_prefix": "", "keyword_suffix": "", "snippets": [...]}`
//!   (a bare array is accepted too; the collection is then the file stem).
//!
//! # `.alfredsnippets` packages
//! A zip archive holding one `*.json` file per snippet,
//! `{"alfredsnippet": {"snippet": "...", "uid": "...", "name": "...", "keyword": "..."}}`,
//! plus (in collections that define them) an `info.plist` with
//! `snippetkeywordprefix` / `snippetkeywordsuffix`. This layout is taken from
//! public descriptions of the format, not from the reference launcher's code. The collection
//! name is the package's file name without extension. Archive entries are
//! only read in memory, never extracted, and size-limited.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::write_private;

const MAX_PACKAGE_ENTRIES: usize = 20_000;
const MAX_ENTRY_BYTES: u64 = 1 << 20;
const MAX_TOTAL_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snippet {
    pub name: String,
    pub keyword: String,
    pub text: String,
    /// Collection (folder) the snippet belongs to; empty for the flat file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub collection: String,
    /// Collection-level prefix/suffix added to `keyword` (see [`Snippet::resolved_keyword`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keyword_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keyword_suffix: String,
    /// Identifier from an imported package (used to dedupe on re-import).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uid: String,
}

impl Snippet {
    /// The keyword as the user types it: prefix + keyword + suffix. Empty when
    /// the snippet has no keyword.
    pub fn resolved_keyword(&self) -> String {
        if self.keyword.is_empty() {
            String::new()
        } else {
            format!("{}{}{}", self.keyword_prefix, self.keyword, self.keyword_suffix)
        }
    }
}

/// A named group of snippets, as stored in `snippets/<name>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Collection {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub keyword_prefix: String,
    #[serde(default)]
    pub keyword_suffix: String,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CollectionFile {
    Full(Collection),
    Bare(Vec<Snippet>),
}

/// Directory holding per-collection files for a given `snippets.json` path
/// (`.../snippets.json` -> `.../snippets`).
pub fn collections_dir(snippets_file: &Path) -> PathBuf {
    snippets_file.with_extension("")
}

/// Load the flat file and every collection file. Unreadable or malformed
/// files are skipped; the result is ordered flat-first, then by file name.
pub fn load_snippets(snippets_file: &Path) -> Vec<Snippet> {
    let mut all: Vec<Snippet> = std::fs::read(snippets_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let dir = collections_dir(snippets_file);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for f in files {
        let Some(parsed) = std::fs::read(&f)
            .ok()
            .and_then(|b| serde_json::from_slice::<CollectionFile>(&b).ok())
        else {
            continue;
        };
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut c = match parsed {
            CollectionFile::Full(c) => c,
            CollectionFile::Bare(snippets) => Collection {
                snippets,
                ..Collection::default()
            },
        };
        if c.name.trim().is_empty() {
            c.name = stem;
        }
        for mut s in c.snippets {
            s.collection = c.name.clone();
            if !c.keyword_prefix.is_empty() {
                s.keyword_prefix = c.keyword_prefix.clone();
            }
            if !c.keyword_suffix.is_empty() {
                s.keyword_suffix = c.keyword_suffix.clone();
            }
            all.push(s);
        }
    }
    all
}

/// Distinct non-empty collection names, sorted.
pub fn collection_names(snippets: &[Snippet]) -> Vec<String> {
    let mut v: Vec<String> = snippets
        .iter()
        .map(|s| s.collection.clone())
        .filter(|c| !c.is_empty())
        .collect();
    v.sort_by_key(|c| c.to_lowercase());
    v.dedup();
    v
}

/// Write (or replace) a collection file under `dir`; returns its path.
pub fn save_collection(dir: &Path, c: &Collection) -> Result<PathBuf> {
    let path = dir.join(format!("{}.json", safe_file_stem(&c.name)));
    write_private(&path, &serde_json::to_vec_pretty(c)?)?;
    Ok(path)
}

/// File-system-safe stem for a collection name (never empty, no separators,
/// no leading dot).
pub fn safe_file_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').trim();
    let s: String = s.chars().take(100).collect();
    if s.is_empty() { "Imported".into() } else { s }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub collection: String,
    pub imported: usize,
    pub skipped: usize,
    /// Where the collection file was written (`None` for [`parse_snippet_package`]).
    pub path: Option<PathBuf>,
}

/// Import a `.alfredsnippets` package into `dir` (see [`collections_dir`]),
/// replacing a collection of the same name.
pub fn import_snippet_package(package: &Path, dir: &Path) -> Result<ImportReport> {
    let name = package
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = std::fs::File::open(package).with_context(|| format!("opening {}", package.display()))?;
    let (collection, mut report) = parse_snippet_package(file, &name)?;
    report.path = Some(save_collection(dir, &collection)?);
    Ok(report)
}

/// Parse a package from any reader. `name` becomes the collection name.
pub fn parse_snippet_package<R: Read + Seek>(reader: R, name: &str) -> Result<(Collection, ImportReport)> {
    let mut zip = zip::ZipArchive::new(reader).context("not a valid .alfredsnippets archive")?;
    if zip.len() > MAX_PACKAGE_ENTRIES {
        bail!("archive has too many entries ({})", zip.len());
    }
    let mut total = 0u64;
    let mut coll = Collection {
        name: if name.trim().is_empty() {
            "Imported".into()
        } else {
            name.trim().to_string()
        },
        ..Collection::default()
    };
    let mut skipped = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("reading archive entry")?;
        if entry.is_dir() {
            continue;
        }
        let Ok(path) = entry.name().map(|n| n.into_owned()) else {
            skipped += 1;
            continue;
        };
        let file_name = path.rsplit('/').next().unwrap_or(&path).to_string();
        if path.starts_with("__MACOSX/") || file_name.starts_with("._") || file_name == ".DS_Store" {
            continue;
        }
        let is_json = file_name.to_ascii_lowercase().ends_with(".json");
        let is_info = file_name == "info.plist";
        if !is_json && !is_info {
            continue;
        }
        if entry.size() > MAX_ENTRY_BYTES {
            skipped += 1;
            continue;
        }
        let mut buf = Vec::new();
        (&mut entry).take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
        total += buf.len() as u64;
        if buf.len() as u64 > MAX_ENTRY_BYTES {
            skipped += 1;
            continue;
        }
        if total > MAX_TOTAL_BYTES {
            bail!("archive is too large when unpacked");
        }
        if is_info {
            if let Ok(plist::Value::Dictionary(d)) = plist::Value::from_reader(std::io::Cursor::new(&buf)) {
                let get = |k: &str| d.get(k).and_then(|v| v.as_string()).unwrap_or("").to_string();
                coll.keyword_prefix = get("snippetkeywordprefix");
                coll.keyword_suffix = get("snippetkeywordsuffix");
            }
            continue;
        }
        match parse_snippet_json(&buf) {
            Some(s) => {
                // The same uid appearing twice keeps the later entry.
                if !s.uid.is_empty()
                    && let Some(old) = coll.snippets.iter_mut().find(|o| o.uid == s.uid)
                {
                    *old = s;
                } else {
                    coll.snippets.push(s);
                }
            }
            None => skipped += 1,
        }
    }
    if coll.snippets.is_empty() && skipped == 0 {
        bail!("no snippets found in the archive");
    }
    coll.snippets.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.keyword.cmp(&b.keyword))
    });
    let report = ImportReport {
        collection: coll.name.clone(),
        imported: coll.snippets.len(),
        skipped,
        path: None,
    };
    Ok((coll, report))
}

fn parse_snippet_json(bytes: &[u8]) -> Option<Snippet> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let o = v.get("alfredsnippet")?;
    let text = o.get("snippet")?.as_str()?.to_string();
    if text.is_empty() {
        return None;
    }
    let s = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let keyword = s("keyword");
    let mut name = s("name");
    if name.trim().is_empty() {
        name = if keyword.is_empty() {
            text.lines().next().unwrap_or("").chars().take(40).collect()
        } else {
            keyword.clone()
        };
    }
    Some(Snippet {
        name,
        keyword,
        text,
        uid: s("uid"),
        ..Snippet::default()
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;
    use crate::clipboard::history::tests::tmp;

    /// Build a package in memory from (file name, content) pairs.
    fn package(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn snip_json(name: &str, kw: &str, text: &str, uid: &str) -> String {
        serde_json::json!({"alfredsnippet": {"snippet": text, "uid": uid, "name": name, "keyword": kw}}).to_string()
    }

    fn info_plist(prefix: &str, suffix: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>snippetkeywordprefix</key><string>{prefix}</string>
<key>snippetkeywordsuffix</key><string>{suffix}</string>
</dict></plist>"#
        )
    }

    #[test]
    fn imports_a_package_with_prefix_and_suffix() {
        let zip = package(&[
            ("info.plist", &info_plist(";", "!")),
            ("Sig [AAA].json", &snip_json("Signature", "sig", "Best,\nMe", "AAA")),
            (
                "Addr [BBB].json",
                &snip_json("Address", "addr", "221B Baker St.", "BBB"),
            ),
            ("notes.txt", "ignored"),
            ("__MACOSX/._Sig [AAA].json", "garbage"),
            (".DS_Store", "x"),
        ]);
        let (c, r) = parse_snippet_package(Cursor::new(zip), "Work").unwrap();
        assert_eq!((r.collection.as_str(), r.imported, r.skipped), ("Work", 2, 0));
        assert_eq!(c.keyword_prefix, ";");
        assert_eq!(c.keyword_suffix, "!");
        // sorted by name
        assert_eq!(c.snippets[0].name, "Address");
        assert_eq!(c.snippets[1].text, "Best,\nMe");
        assert_eq!(c.snippets[1].uid, "AAA");
    }

    #[test]
    fn package_without_info_plist_has_no_prefix() {
        let zip = package(&[("a.json", &snip_json("A", "a", "x", "1"))]);
        let (c, _) = parse_snippet_package(Cursor::new(zip), "Plain").unwrap();
        assert_eq!((c.keyword_prefix.as_str(), c.keyword_suffix.as_str()), ("", ""));
    }

    #[test]
    fn bad_entries_are_skipped_not_fatal() {
        let zip = package(&[
            ("ok.json", &snip_json("Ok", "ok", "text", "1")),
            ("broken.json", "{not json"),
            ("wrongshape.json", r#"{"something":"else"}"#),
            ("empty.json", &snip_json("Empty", "e", "", "2")),
        ]);
        let (c, r) = parse_snippet_package(Cursor::new(zip), "X").unwrap();
        assert_eq!((r.imported, r.skipped), (1, 3));
        assert_eq!(c.snippets[0].name, "Ok");
    }

    #[test]
    fn duplicate_uid_keeps_the_later_entry_and_name_falls_back() {
        let zip = package(&[
            ("a.json", &snip_json("Old", "k", "old text", "U1")),
            ("b.json", &snip_json("New", "k", "new text", "U1")),
            ("c.json", &snip_json("", "kw", "named by keyword", "U2")),
            ("d.json", &snip_json("", "", "first line only\nsecond", "U3")),
        ]);
        let (c, r) = parse_snippet_package(Cursor::new(zip), "D").unwrap();
        assert_eq!(r.imported, 3);
        assert!(c.snippets.iter().any(|s| s.name == "New" && s.text == "new text"));
        assert!(!c.snippets.iter().any(|s| s.name == "Old"));
        assert!(c.snippets.iter().any(|s| s.name == "kw"));
        assert!(c.snippets.iter().any(|s| s.name == "first line only"));
    }

    #[test]
    fn empty_or_invalid_archives_error() {
        assert!(parse_snippet_package(Cursor::new(package(&[("readme.txt", "x")])), "E").is_err());
        assert!(parse_snippet_package(Cursor::new(b"definitely not a zip".to_vec()), "E").is_err());
    }

    #[test]
    fn oversized_entries_are_skipped() {
        let big = snip_json("Big", "b", &"x".repeat(MAX_ENTRY_BYTES as usize + 10), "9");
        let zip = package(&[("big.json", &big), ("ok.json", &snip_json("Ok", "o", "t", "1"))]);
        let (_, r) = parse_snippet_package(Cursor::new(zip), "S").unwrap();
        assert_eq!((r.imported, r.skipped), (1, 1));
    }

    #[test]
    fn hostile_entry_names_never_touch_the_filesystem() {
        let root = tmp("snip-zipslip");
        let zip = package(&[("../../escape.json", &snip_json("Evil", "e", "t", "1"))]);
        let pkg = root.join("Evil.alfredsnippets");
        std::fs::write(&pkg, zip).unwrap();
        let dir = root.join("data").join("snippets");
        let r = import_snippet_package(&pkg, &dir).unwrap();
        assert_eq!(r.imported, 1);
        assert!(!root.join("escape.json").exists());
        assert!(!root.parent().unwrap().join("escape.json").exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn import_writes_private_file_and_load_resolves_keywords() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("snip-import");
        let pkg = root.join("My Work.alfredsnippets");
        std::fs::write(
            &pkg,
            package(&[
                ("info.plist", &info_plist("..", "")),
                ("a.json", &snip_json("Sig", "sig", "Regards", "1")),
                ("b.json", &snip_json("NoKeyword", "", "text only", "2")),
            ]),
        )
        .unwrap();
        let flat = root.join("data").join("snippets.json");
        std::fs::create_dir_all(flat.parent().unwrap()).unwrap();
        std::fs::write(&flat, r#"[{"name":"Flat","keyword":"fl","text":"flat text"}]"#).unwrap();
        let r = import_snippet_package(&pkg, &collections_dir(&flat)).unwrap();
        let path = r.path.unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(path.file_name().unwrap(), "My Work.json");

        let all = load_snippets(&flat);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "Flat");
        assert_eq!(all[0].collection, "");
        assert_eq!(all[0].resolved_keyword(), "fl");
        let sig = all.iter().find(|s| s.name == "Sig").unwrap();
        assert_eq!(sig.collection, "My Work");
        assert_eq!(sig.resolved_keyword(), "..sig");
        assert_eq!(
            all.iter().find(|s| s.name == "NoKeyword").unwrap().resolved_keyword(),
            ""
        );
        assert_eq!(collection_names(&all), vec!["My Work".to_string()]);

        // Re-importing replaces the collection instead of duplicating.
        import_snippet_package(&pkg, &collections_dir(&flat)).unwrap();
        assert_eq!(load_snippets(&flat).len(), 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn loads_bare_array_collections_and_ignores_junk() {
        let root = tmp("snip-load");
        let flat = root.join("snippets.json");
        let dir = collections_dir(&flat);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Emails.json"),
            r#"[{"name":"Hi","keyword":"hi","text":"Hello"}]"#,
        )
        .unwrap();
        std::fs::write(dir.join("Broken.json"), "{{{").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(
            dir.join("a-named.json"),
            r#"{"name":"Named","keyword_prefix":"//","snippets":[{"name":"N","keyword":"n","text":"t","collection":"ignored"}]}"#,
        )
        .unwrap();
        let all = load_snippets(&flat); // no flat file at all
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].collection, "Named");
        assert_eq!(all[0].resolved_keyword(), "//n");
        assert_eq!(all[1].collection, "Emails");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_flat_format_still_loads() {
        let root = tmp("snip-legacy");
        let flat = root.join("snippets.json");
        std::fs::write(&flat, r#"[{"name":"Sign off","keyword":"sig","text":"Regards"}]"#).unwrap();
        let all = load_snippets(&flat);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].resolved_keyword(), "sig");
        assert!(load_snippets(&root.join("missing.json")).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn safe_file_stems() {
        assert_eq!(safe_file_stem("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_file_stem("  .hidden"), "hidden");
        assert_eq!(safe_file_stem(""), "Imported");
        assert_eq!(safe_file_stem("a/b\\c"), "a_b_c");
        assert_eq!(safe_file_stem("Café Ünï"), "Café Ünï");
        assert!(safe_file_stem(&"x".repeat(500)).len() <= 100);
    }
}
