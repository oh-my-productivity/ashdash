//! Installed applications, fuzzy matched by name.

use super::{Provider, ProviderOutput, QueryCtx};
use crate::files;
use crate::fuzzy;
use crate::item::{Action, Icon, Item};

pub struct AppsProvider;

impl Provider for AppsProvider {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let mut items = Vec::new();
        // Normalize the query once; each app's name was normalized when it was indexed.
        let query = fuzzy::Query::new(ctx.query);
        for app in ctx.engine_state.apps.read().unwrap_or_else(|e| e.into_inner()).iter() {
            if let Some(s) = query.score(&app.candidate) {
                let mut it = Item::new(format!("app:{}", app.path.display()), &app.name)
                    .subtitle(files::tilde(&app.path))
                    .icon(Icon::App(app.path.clone()))
                    .action(Action::Open(app.path.clone()))
                    .autocomplete(&app.name)
                    .score(s);
                it.cmd_action = Some(Action::Reveal(app.path.clone()));
                items.push(it);
            }
        }
        ProviderOutput::ranked(items)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

    #[test]
    fn matches_by_prefix_and_offers_reveal() {
        let (e, root) = engine("apps");
        let r = e.query("saf");
        assert_eq!(r[0].title, "Safari");
        assert!(matches!(r[0].cmd_action, Some(Action::Reveal(_))));
        // ranked, not exclusive: fallbacks still follow
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Tab on an application row completes the field with the app's name (docs/QA.md 3.3.3b; it did nothing).
    #[test]
    fn application_rows_tab_complete_to_their_name() {
        let (e, root) = engine("apps");
        let r = e.query("saf");
        assert_eq!(r[0].title, "Safari");
        assert_eq!(r[0].autocomplete.as_deref(), Some("Safari"));
        let _ = std::fs::remove_dir_all(root);
    }
}
