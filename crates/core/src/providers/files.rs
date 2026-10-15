//! `open`/`find`/`in`/`'` file search and `/` `~` path browsing.
//!
//! * `open <name>`, `find <name>`, `' <name>`: file names (Spotlight)
//! * `in <text>`: file contents (Spotlight full-text)
//! * `kind:pdf`, `kind:image`, `kind:folder`, `kind:rs`, ... narrow any of them
//!
//! Recently used files rank higher. Where to search and what to skip is a
//! [`FileSearchScope`] handed to the provider's constructor.

use std::path::Path;
use std::sync::Arc;

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword};
use crate::actions::{Os, RealOs};
use crate::files::{self, Completeness, FileQuery, FileSearchScope, SearchMode};
use crate::item::{Action, Icon, Item};

pub struct FilesProvider {
    scope: FileSearchScope,
    os: Arc<dyn Os>,
}

impl FilesProvider {
    pub fn new(scope: FileSearchScope) -> FilesProvider {
        FilesProvider::with_os(scope, Arc::new(RealOs))
    }

    pub fn with_os(scope: FileSearchScope, os: Arc<dyn Os>) -> FilesProvider {
        FilesProvider { scope, os }
    }
}

impl Default for FilesProvider {
    /// Searches the home folder, skipping `~/Library` and development noise.
    fn default() -> Self {
        FilesProvider::new(FileSearchScope::default())
    }
}

/// The keyword that triggered the provider, and what follows it.
fn trigger(q: &str) -> Option<(&'static str, SearchMode, &str)> {
    for (kw, mode) in [
        ("open", SearchMode::Name),
        ("find", SearchMode::Name),
        ("in", SearchMode::Content),
    ] {
        if let Some(rest) = strip_keyword(q, kw) {
            return Some((kw, mode, rest));
        }
    }
    q.strip_prefix('\'')
        .map(|rest| ("'", SearchMode::Name, rest.trim_start()))
}

impl Provider for FilesProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    /// Results are ordered by relevance and recency here, not by launch counts.
    fn learns(&self) -> bool {
        false
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        match trigger(ctx.query) {
            Some((kw, mode, rest)) => ProviderOutput::exclusive(file_items(
                self.os.as_ref(),
                &self.scope,
                kw,
                mode,
                rest,
                ctx.engine_state.config.max_results,
            )),
            None => ProviderOutput::none(),
        }
    }
}

fn file_items(os: &dyn Os, scope: &FileSearchScope, kw: &str, mode: SearchMode, rest: &str, max: usize) -> Vec<Item> {
    let q = FileQuery::parse(mode, rest);
    if q.is_empty() {
        let (title, sub) = match mode {
            SearchMode::Name => (
                format!("{kw} <file name>"),
                "Search files by name. Add kind:pdf, kind:image, kind:folder ... to filter",
            ),
            SearchMode::Content => (
                format!("{kw} <text>"),
                "Search inside files. Add kind:pdf, kind:md ... to filter",
            ),
        };
        return vec![
            Item::new(format!("kw:{kw}"), title)
                .subtitle(sub)
                .icon(Icon::Glyph("▤"))
                .valid(false),
        ];
    }
    let found = files::search_files_outcome(os, scope, &q, max);
    if found.paths.is_empty() {
        let (id, title, sub) = match found.completeness {
            Completeness::Complete => ("nofiles", "No files found", rest.to_string()),
            Completeness::Partial => (
                "nofiles-partial",
                "No files found yet",
                format!("The search hit its time or size limit; try a more specific query ({rest})"),
            ),
        };
        return vec![Item::new(id, title).subtitle(sub).valid(false)];
    }
    found.paths.iter().map(|p| path_item(p)).collect()
}

/// Typing `/…` or `~…` browses the file system; exclusive only if it matched.
pub struct PathBrowseProvider;

impl Provider for PathBrowseProvider {
    fn id(&self) -> &'static str {
        "path"
    }

    fn learns(&self) -> bool {
        false
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query;
        if q.starts_with('/') || q.starts_with('~') {
            let paths = files::browse(q, ctx.engine_state.config.max_results);
            if !paths.is_empty() {
                return ProviderOutput::exclusive(paths.iter().map(|p| path_item(p)).collect());
            }
        }
        ProviderOutput::none()
    }
}

/// A result row for a file or folder. Folders autocomplete to `~/dir/` so
/// Tab keeps navigating into them.
pub(crate) fn path_item(p: &Path) -> Item {
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string());
    let is_app = p.extension().is_some_and(|e| e == "app");
    let mut it = Item::new(format!("file:{}", p.display()), name)
        .subtitle(files::tilde(p))
        .icon(if is_app {
            Icon::App(p.to_path_buf())
        } else if p.is_dir() {
            Icon::Glyph("▸")
        } else {
            Icon::Glyph("▤")
        })
        .action(Action::Open(p.to_path_buf()));
    it.cmd_action = Some(Action::Reveal(p.to_path_buf()));
    it.copy_text = Some(p.display().to_string());
    if p.is_dir() {
        it.autocomplete = Some(format!("{}/", files::tilde(p)));
    }
    it
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::super::testutil::engine;
    use super::*;
    use crate::actions::Cmd;
    use crate::actions::testing::FakeOs;

    #[test]
    fn open_keyword_is_exclusive_hint() {
        let (e, root) = engine("files-kw");
        let r = e.query("open");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "kw:open");
        assert!(!r[0].valid);
        // not a keyword without a space: falls through to general search
        assert!(e.query("opens").iter().all(|i| i.id != "kw:open"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_trigger_has_a_hint_and_is_exclusive() {
        let (e, root) = engine("files-hints");
        for (q, id) in [("find", "kw:find"), ("in", "kw:in"), ("'", "kw:'"), ("' ", "kw:'")] {
            let r = e.query(q);
            assert_eq!(r.len(), 1, "{q}: {r:?}");
            assert_eq!(r[0].id, id, "{q}");
            assert!(!r[0].valid);
        }
        // "in" must be a whole word: "inbox" is an ordinary search
        assert!(e.query("inbox").iter().all(|i| i.id != "kw:in"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn triggers_map_to_modes() {
        assert_eq!(trigger("open a"), Some(("open", SearchMode::Name, "a")));
        assert_eq!(trigger("FIND   a b"), Some(("find", SearchMode::Name, "a b")));
        assert_eq!(trigger("in  needle"), Some(("in", SearchMode::Content, "needle")));
        assert_eq!(trigger("'  a"), Some(("'", SearchMode::Name, "a")));
        assert_eq!(trigger("'a"), Some(("'", SearchMode::Name, "a")));
        assert_eq!(trigger("opened"), None);
        assert_eq!(trigger("inside"), None);
    }

    fn scoped(tag: &str) -> (PathBuf, FileSearchScope) {
        let root = std::env::temp_dir().join(format!("oa-fprov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs/Reports")).unwrap();
        std::fs::write(root.join("docs/Reports/q3.pdf"), b"x").unwrap();
        let root = root.canonicalize().unwrap();
        let scope = FileSearchScope::new(vec![root.clone()], vec![]);
        (root, scope)
    }

    #[test]
    fn name_search_goes_through_spotlight_and_builds_items() {
        let (root, scope) = scoped("name");
        let hit = root.join("docs/Reports");
        let os = FakeOs::with_output(&format!(
            "{}\n{}\n",
            hit.display(),
            root.join("docs/Reports/q3.pdf").display()
        ));
        let items = file_items(&os, &scope, "find", SearchMode::Name, "reports", 10);
        assert_eq!(
            os.cmds(),
            vec![
                Cmd::new("mdfind")
                    .args(["-0", "-attr", "kMDItemLastUsedDate", "-onlyin"])
                    .arg(&root)
                    .arg("kMDItemFSName == \"*reports*\"cd")
            ]
        );
        assert_eq!(items[0].title, "Reports");
        // folders keep navigating: Tab completes to "<path>/"
        let dir_item = items.iter().find(|i| i.title == "Reports").unwrap();
        assert!(dir_item.autocomplete.as_deref().unwrap().ends_with("/Reports/"));
        assert_eq!(dir_item.copy_text.as_deref(), Some(hit.to_str().unwrap()));
        assert_eq!(items.len(), 1, "fuzzy matching drops names that do not match");
        let items = file_items(&os, &scope, "find", SearchMode::Name, "q3", 10);
        let file_item = &items[0];
        assert_eq!(file_item.title, "q3.pdf");
        assert_eq!(file_item.autocomplete, None);
        assert_eq!(file_item.action, Action::Open(root.join("docs/Reports/q3.pdf")));
        assert_eq!(
            file_item.cmd_action,
            Some(Action::Reveal(root.join("docs/Reports/q3.pdf")))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn content_search_uses_text_content_attribute() {
        let (root, scope) = scoped("content");
        let os = FakeOs::default();
        let items = file_items(&os, &scope, "in", SearchMode::Content, "invoice kind:pdf", 10);
        assert_eq!(items[0].id, "nofiles");
        let c = &os.cmds()[0];
        assert_eq!(
            c.args.last().unwrap().to_string_lossy(),
            "kMDItemTextContent == \"invoice*\"cd && (kMDItemContentTypeTree == \"com.adobe.pdf\")"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn provider_respects_its_scope_and_is_exclusive() {
        let (root, scope) = scoped("provider");
        let outside = "/elsewhere/Reports";
        let os = Arc::new(FakeOs::with_output(&format!("{outside}\n")));
        let p = FilesProvider::with_os(scope, os);
        assert!(!p.learns());
        // out-of-scope hits are dropped
        let items = file_items(p.os.as_ref(), &p.scope, "open", SearchMode::Name, "reports", 10);
        assert_eq!(items[0].id, "nofiles");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_browse_is_exclusive_only_when_it_matches() {
        let (e, root) = engine("files-path");
        let dir = root.join("browse");
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        let r = e.query(&format!("{}/", dir.display()));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "alpha");
        assert!(matches!(r[0].action, Action::Open(_)));
        assert!(r[0].autocomplete.as_deref().unwrap().ends_with("/alpha/"));
        // no match: falls through to the general search + fallbacks
        let r = e.query("/definitely/not/a/real/path");
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }
}
