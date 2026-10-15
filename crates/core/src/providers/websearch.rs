//! Web searches: keyword searches (`g rust`) and trailing fallbacks.

use super::{Placement, Provider, ProviderOutput, QueryCtx, strip_keyword};
use crate::item::{Action, Icon, Item};

/// `<keyword> <terms>` opens the search directly. Exclusive once terms exist.
pub struct WebKeywordProvider;

impl Provider for WebKeywordProvider {
    fn id(&self) -> &'static str {
        "web-keyword"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        for ws in &ctx.engine_state.config.web_searches {
            if let Some(rest) = strip_keyword(ctx.query, &ws.keyword)
                && !rest.is_empty()
            {
                return ProviderOutput::exclusive(vec![web_item(&ws.title, &ws.url_for(rest), rest)]);
            }
        }
        ProviderOutput::none()
    }
}

/// Searches flagged `fallback` close the list. This is a registry provider
/// with [`Placement::Trailing`]: the engine appends its output after ranking.
pub struct FallbackProvider;

impl Provider for FallbackProvider {
    fn id(&self) -> &'static str {
        "web-fallback"
    }

    fn placement(&self) -> Placement {
        Placement::Trailing
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query.trim();
        let items = ctx
            .engine_state
            .config
            .web_searches
            .iter()
            .filter(|w| w.fallback)
            .enumerate()
            .map(|(i, ws)| {
                let mut it = web_item(&ws.title, &ws.url_for(q), q);
                it.id = format!("fallback:{}", ws.keyword);
                it.score = -1000.0 - i as f64;
                it
            })
            .collect();
        ProviderOutput::ranked(items)
    }
}

fn web_item(engine: &str, url: &str, query: &str) -> Item {
    Item::new(
        format!("web:{engine}:{query}"),
        format!("Search {engine} for '{query}'"),
    )
    .subtitle(url.to_string())
    .icon(Icon::Glyph("🔍"))
    .action(Action::OpenUrl(url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

    #[test]
    fn keyword_with_terms_is_exclusive_without_fallbacks() {
        let (e, root) = engine("web-kw");
        let r = e.query("g rust ownership");
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].action,
            Action::OpenUrl("https://www.google.com/search?q=rust%20ownership".into())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_keyword_is_not_exclusive_and_fallbacks_come_last() {
        let (e, root) = engine("web-fb");
        let r = e.query("g");
        assert!(r.len() > 1, "bare keyword must fall through to general search");
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let tail: Vec<_> = r.iter().rev().take_while(|i| i.id.starts_with("fallback:")).collect();
        let fb = e.config.web_searches.iter().filter(|w| w.fallback).count();
        assert_eq!(tail.len(), fb);
        let _ = std::fs::remove_dir_all(root);
    }
}
