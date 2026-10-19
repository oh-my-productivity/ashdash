//! Web searches: keyword searches (`g rust`) and trailing fallbacks.

use super::{Placement, Provider, ProviderOutput, QueryCtx, strip_keyword};
use crate::item::{Action, FALLBACK_ID_PREFIX, Icon, Item};
use crate::websearch::WebSearch;

/// `<keyword> <terms>` opens the search directly. Exclusive once terms exist.
pub struct WebKeywordProvider {
    searches: Vec<WebSearch>,
}

impl WebKeywordProvider {
    /// `searches` must already have passed `websearch::checked`.
    pub fn new(searches: Vec<WebSearch>) -> Self {
        WebKeywordProvider { searches }
    }
}

impl Provider for WebKeywordProvider {
    fn id(&self) -> &'static str {
        "web-keyword"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        for ws in &self.searches {
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
pub struct FallbackProvider {
    searches: Vec<WebSearch>,
}

impl FallbackProvider {
    /// `searches` must already have passed `websearch::checked`.
    pub fn new(searches: Vec<WebSearch>) -> Self {
        FallbackProvider { searches }
    }
}

impl Provider for FallbackProvider {
    fn id(&self) -> &'static str {
        "web-fallback"
    }

    fn placement(&self) -> Placement {
        Placement::Trailing
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query.trim();
        let items = self
            .searches
            .iter()
            .filter(|w| w.fallback)
            .enumerate()
            .map(|(i, ws)| {
                let mut it = web_item(&ws.title, &ws.url_for(q), q);
                it.id = format!("{FALLBACK_ID_PREFIX}{}", ws.keyword);
                it.score = -1000.0 - i as f64;
                it
            })
            .collect();
        ProviderOutput::ranked(items)
    }
}

/// "Search Google for “saf”" (curly quotes: straight ones look like code in a result row).
fn search_title(engine: &str, query: &str) -> String {
    format!("Search {engine} for \u{201C}{query}\u{201D}")
}

fn web_item(engine: &str, url: &str, query: &str) -> Item {
    Item::new(format!("web:{engine}:{query}"), search_title(engine, query))
        .subtitle(url.to_string())
        .icon(Icon::Glyph("🔍"))
        .action(Action::OpenUrl(url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

    #[test]
    fn the_title_quotes_the_query_with_curly_quotes() {
        assert_eq!(
            super::search_title("Google", "saf"),
            "Search Google for \u{201C}saf\u{201D}"
        );
        let (e, root) = engine("web-title");
        let r = e.query("saf");
        let fb = r.iter().find(|i| i.id.starts_with("fallback:")).expect("a fallback");
        assert!(fb.title.starts_with("Search "), "{}", fb.title);
        assert!(fb.title.ends_with("for \u{201C}saf\u{201D}"), "{}", fb.title);
        assert!(!fb.title.contains('\''), "{}", fb.title);
        assert!(fb.subtitle.starts_with("https://"), "the URL stays in the subtitle");
        let _ = std::fs::remove_dir_all(root);
    }

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
