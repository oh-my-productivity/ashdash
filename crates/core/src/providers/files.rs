//! `open`/`find` file search and `/` `~` path browsing.

use std::path::Path;

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword};
use crate::files;
use crate::item::{Action, Icon, Item};

/// `open <name>` / `find <name>`.
pub struct FilesProvider;

impl Provider for FilesProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        for kw in ["open", "find"] {
            if let Some(rest) = strip_keyword(ctx.query, kw) {
                return ProviderOutput::exclusive(file_items(kw, rest, ctx.engine_state.config.max_results));
            }
        }
        ProviderOutput::none()
    }
}

fn file_items(kw: &str, rest: &str, max: usize) -> Vec<Item> {
    if rest.is_empty() {
        return vec![
            Item::new(format!("kw:{kw}"), format!("{kw} <file name>"))
                .subtitle("Search files in your home folder by name")
                .icon(Icon::Glyph("▤"))
                .valid(false),
        ];
    }
    let found = files::search(rest, max);
    if found.is_empty() {
        return vec![
            Item::new("nofiles", "No files found")
                .subtitle(rest.to_string())
                .valid(false),
        ];
    }
    found.iter().map(|p| path_item(p)).collect()
}

/// Typing `/…` or `~…` browses the file system; exclusive only if it matched.
pub struct PathBrowseProvider;

impl Provider for PathBrowseProvider {
    fn id(&self) -> &'static str {
        "path"
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

fn path_item(p: &Path) -> Item {
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
    if p.is_dir() {
        it.autocomplete = Some(format!("{}/", files::tilde(p)));
    }
    it
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

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
    fn path_browse_is_exclusive_only_when_it_matches() {
        let (e, root) = engine("files-path");
        let dir = root.join("browse");
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        let r = e.query(&format!("{}/", dir.display()));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "alpha");
        assert!(matches!(r[0].action, Action::Open(_)));
        // no match: falls through to the general search + fallbacks
        let r = e.query("/definitely/not/a/real/path");
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }
}
