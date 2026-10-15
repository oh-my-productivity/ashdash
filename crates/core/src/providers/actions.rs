//! `actions <text or path>`: the Universal Actions for something you type.
//!
//! A path (`/…` or `~…`) that exists lists the file actions (Open, Reveal,
//! Copy Path, Move to Trash, ...); anything else lists the text actions
//! (Search Google, Copy, Large Type, Open URL for web addresses). The actions
//! for a selected result come from `Engine::actions_for`.

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword};
use crate::actions;
use crate::files;
use crate::item::{Icon, Item};

pub struct ActionsProvider;

impl Provider for ActionsProvider {
    fn id(&self) -> &'static str {
        "actions"
    }

    fn learns(&self) -> bool {
        false
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let Some(rest) = strip_keyword(ctx.query, "actions") else {
            return ProviderOutput::none();
        };
        let rest = rest.trim();
        if rest.is_empty() {
            return ProviderOutput::exclusive(vec![
                Item::new("kw:actions", "actions <text or path>")
                    .subtitle("Universal Actions for text, URLs, files and folders")
                    .icon(Icon::Glyph("⌘"))
                    .valid(false),
            ]);
        }
        if rest.starts_with('/') || rest.starts_with('~') {
            let p = files::expand_tilde(rest);
            if std::fs::symlink_metadata(&p).is_ok() {
                return ProviderOutput::exclusive(actions::path_actions(&p));
            }
        }
        ProviderOutput::exclusive(actions::text_actions(rest))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::engine::Outcome;
    use crate::item::{Action, Mods};

    fn titles(v: &[crate::item::Item]) -> Vec<&str> {
        v.iter().map(|i| i.title.as_str()).collect()
    }

    #[test]
    fn hint_text_and_url_and_path() {
        let (e, root) = engine("actions-kw");
        assert_eq!(e.query("actions")[0].id, "kw:actions");
        assert!(!e.query("actions")[0].valid);
        assert_eq!(
            titles(&e.query("actions hello world")),
            ["Search Google", "Copy", "Large Type"]
        );
        assert_eq!(
            titles(&e.query("actions https://example.com")),
            ["Open URL", "Search Google", "Copy", "Large Type"]
        );
        let f = root.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let r = e.query(&format!("actions {}", f.display()));
        assert_eq!(r[0].title, "Open");
        assert!(titles(&r).contains(&"Move to Trash"));
        // a path that does not exist is just text
        assert_eq!(titles(&e.query("actions /no/such/thing"))[0], "Search Google");
        // not the keyword
        assert!(e.query("actionsx").iter().all(|i| i.id != "kw:actions"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn engine_actions_for_and_execute_without_the_os() {
        let (e, root) = engine("actions-engine");
        let f = root.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let r = e.query(&format!("actions {}", f.display()));
        // actions_for on a real file result
        let file_result = crate::item::Item::new("file:x", "a.txt").action(Action::Open(f.clone()));
        let acts = e.actions_for(&file_result);
        assert_eq!(acts.len(), r.len());
        // Large Type runs without touching the OS
        let large = e.actions_for(&crate::item::Item::new("c", "14").action(Action::Copy("14".into())));
        let lt = large.iter().find(|i| i.title == "Large Type").unwrap();
        assert_eq!(e.execute(lt, "14", Mods::NONE, false), Outcome::LargeType("14".into()));
        // Move to Trash asks first: unconfirmed => Confirm, and nothing is deleted
        let trash = acts.iter().find(|i| i.title == "Move to Trash").unwrap();
        match e.execute(trash, "x", Mods::NONE, false) {
            Outcome::Confirm(m) => assert!(m.contains("a.txt") && m.contains("Trash"), "{m}"),
            other => panic!("expected Confirm, got {other:?}"),
        }
        assert!(f.exists());
        assert!(trash.action.confirmation().is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
