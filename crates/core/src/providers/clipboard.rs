//! `clip` / `clipboard` history search.

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword, truncate};
use crate::item::{Action, Icon, Item};

pub struct ClipboardProvider;

impl Provider for ClipboardProvider {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query;
        let Some(rest) = strip_keyword(q, "clipboard").or_else(|| strip_keyword(q, "clip")) else {
            return ProviderOutput::none();
        };
        let st = ctx.engine_state;
        let hits = st.clipboard.lock().unwrap().search(rest, st.config.max_results);
        if hits.is_empty() {
            return ProviderOutput::exclusive(vec![Item::new("noclip", "Clipboard history is empty").valid(false)]);
        }
        ProviderOutput::exclusive(
            hits.iter()
                .enumerate()
                .map(|(i, e)| {
                    let first = e.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                    let lines = e.text.lines().count();
                    let mut it = Item::new(format!("clip:{i}:{}", e.ts), truncate(first, 120))
                        .subtitle(format!(
                            "{} chars, {} line(s) — ↩ to copy",
                            e.text.chars().count(),
                            lines
                        ))
                        .icon(Icon::Glyph("⎘"))
                        .action(Action::Copy(e.text.clone()))
                        .score(10_000.0 - i as f64);
                    it.large_type = Some(e.text.clone());
                    it
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;

    #[test]
    fn clip_keyword_is_exclusive_even_when_empty() {
        let (e, root) = engine("clip");
        let r = e.query("clip");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "noclip");
        // "Clipper.app" would match a general search for "clip"; it must not here.
        assert!(r.iter().all(|i| !i.id.starts_with("app:")));
        e.clipboard().lock().unwrap().push("hello world");
        let r = e.query("clipboard hello");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "hello world");
        let _ = std::fs::remove_dir_all(root);
    }
}
