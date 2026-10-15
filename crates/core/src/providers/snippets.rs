//! `snip` snippet search.

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword, truncate};
use crate::clipboard::{self, Snippet};
use crate::fuzzy;
use crate::item::{Action, Icon, Item};

pub struct SnippetsProvider;

impl Provider for SnippetsProvider {
    fn id(&self) -> &'static str {
        "snippets"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let Some(rest) = strip_keyword(ctx.query, "snip") else {
            return ProviderOutput::none();
        };
        let st = ctx.engine_state;
        let clip = st.clipboard.lock().unwrap().latest().unwrap_or("").to_string();
        let snippets = st.snippets.read().unwrap();
        let mut v: Vec<(f64, &Snippet)> = snippets
            .iter()
            .filter_map(|s| {
                let a = fuzzy::score(rest, &s.name);
                let b = fuzzy::score(rest, &s.keyword);
                match (a, b) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (x, y) => x.or(y),
                }
                .map(|sc| (sc, s))
            })
            .collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        if v.is_empty() {
            return ProviderOutput::exclusive(vec![
                Item::new("nosnip", "No snippets")
                    .subtitle(format!("Add some to {}", st.paths.snippets_file().display()))
                    .valid(false),
            ]);
        }
        ProviderOutput::exclusive(
            v.into_iter()
                .map(|(sc, s)| {
                    let text = clipboard::expand_snippet(&s.text, &clip);
                    let mut it = Item::new(format!("snip:{}", s.keyword), &s.name)
                        .subtitle(truncate(text.lines().next().unwrap_or(""), 100))
                        .icon(Icon::Glyph("✂"))
                        .action(Action::Copy(text.clone()))
                        .score(sc);
                    it.large_type = Some(text);
                    it
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

    #[test]
    fn snip_keyword_is_exclusive_and_expands() {
        let (e, root) = engine("snip");
        let r = e.query("snip x");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "nosnip");
        std::fs::create_dir_all(&e.paths.data).unwrap();
        std::fs::write(
            e.paths.snippets_file(),
            r#"[{"name":"Sign off","keyword":"sig","text":"Regards"}]"#,
        )
        .unwrap();
        e.reload();
        let r = e.query("snip sig");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "snip:sig");
        assert_eq!(r[0].action, Action::Copy("Regards".into()));
        let _ = std::fs::remove_dir_all(root);
    }
}
