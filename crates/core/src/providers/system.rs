//! System commands (sleep, lock, shut down, ...).

use super::{Provider, ProviderOutput, QueryCtx};
use crate::fuzzy;
use crate::item::{Action, Icon, Item};
use crate::system::SystemCommand;

pub struct SystemProvider;

impl Provider for SystemProvider {
    fn id(&self) -> &'static str {
        "system"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let mut items = Vec::new();
        for sc in SystemCommand::ALL {
            if let Some(s) = fuzzy::score(ctx.query, sc.title()) {
                items.push(
                    Item::new(sc.id(), sc.title())
                        .subtitle(sc.subtitle())
                        .icon(Icon::Glyph("⚙"))
                        .action(Action::System(sc))
                        .score(s),
                );
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
    fn commands_are_ranked_alongside_other_results() {
        let (e, root) = engine("system");
        let r = e.query("shut down");
        let it = r.iter().find(|i| i.title == "Shut Down").unwrap();
        assert!(matches!(it.action, Action::System(_)));
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }
}
