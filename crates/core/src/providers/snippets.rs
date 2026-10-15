//! `snip` snippet search: by name, keyword or collection.

use super::clipboard::clip_action;
use super::{Provider, ProviderOutput, QueryCtx, strip_keyword, truncate};
use crate::clipboard::placeholders::ExpandCtx;
use crate::clipboard::{ClipCommand, Snippet, expand};
use crate::fuzzy;
use crate::item::{Action, Icon, Item};

/// How many previous clipboard entries `{clipboard:N}` can reach.
const CLIPBOARD_DEPTH: usize = 50;

pub struct SnippetsProvider;

/// Matching snippets, best first. An empty query lists everything grouped by
/// collection. Collection matches rank below name/keyword matches.
fn rank<'a>(snippets: &'a [Snippet], query: &str) -> Vec<(f64, &'a Snippet)> {
    let mut v: Vec<(f64, &Snippet)> = snippets
        .iter()
        .filter_map(|s| {
            let name = fuzzy::score(query, &s.name);
            let kw = fuzzy::score(query, &s.resolved_keyword()).filter(|_| !s.keyword.is_empty());
            let coll = fuzzy::score(query, &s.collection)
                .filter(|_| !s.collection.is_empty())
                .map(|x| x * 0.5);
            [name, kw, coll]
                .into_iter()
                .flatten()
                .reduce(f64::max)
                .map(|sc| (sc, s))
        })
        .collect();
    v.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.collection.to_lowercase().cmp(&b.1.collection.to_lowercase()))
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    v
}

impl Provider for SnippetsProvider {
    fn id(&self) -> &'static str {
        "snippets"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let Some(rest) = strip_keyword(ctx.query, "snip") else {
            return ProviderOutput::none();
        };
        let st = ctx.engine_state;
        let clips = st.clipboard.lock().unwrap().recent_texts(CLIPBOARD_DEPTH);
        let snippets = st.snippets.read().unwrap();
        let ranked = rank(&snippets, rest);
        if ranked.is_empty() {
            let (title, sub) = if snippets.is_empty() {
                (
                    "No snippets",
                    format!(
                        "Add some to {} or import a .alfredsnippets package",
                        st.paths.snippets_file().display()
                    ),
                )
            } else {
                ("No matching snippets", "Search by name, keyword or collection".into())
            };
            return ProviderOutput::exclusive(vec![Item::new("nosnip", title).subtitle(sub).valid(false)]);
        }

        let lookup = |kw: &str| {
            snippets
                .iter()
                .find(|s| !s.keyword.is_empty() && s.resolved_keyword() == kw)
                .map(|s| s.text.clone())
        };
        let ectx = ExpandCtx::system(&clips).with_snippets(&lookup);
        ProviderOutput::exclusive(
            ranked
                .into_iter()
                .map(|(sc, s)| {
                    let ex = expand(&s.text, &ectx);
                    let kw = s.resolved_keyword();
                    let id = match (s.collection.is_empty(), kw.is_empty()) {
                        (true, false) => format!("snip:{kw}"),
                        (true, true) => format!("snip:{}", s.name),
                        (false, false) => format!("snip:{}/{kw}", s.collection),
                        (false, true) => format!("snip:{}/{}", s.collection, s.name),
                    };
                    let preview = truncate(ex.text.lines().next().unwrap_or(""), 100);
                    let sub = [s.collection.as_str(), kw.as_str(), preview.as_str()]
                        .into_iter()
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let action = clip_action(ClipCommand::PasteText {
                        text: ex.text.clone(),
                        cursor_back: ex.cursor_back,
                    });
                    let mut it = Item::new(id, &s.name)
                        .subtitle(sub)
                        .icon(Icon::Glyph("✂"))
                        .action(action)
                        .score(sc);
                    // Cmd+Return only copies the expanded text.
                    it.cmd_action = Some(Action::Copy(ex.text.clone()));
                    it.copy_text = Some(ex.text.clone());
                    it.large_type = Some(ex.text);
                    it
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use super::*;
    use crate::clipboard::snippets::{Collection, collections_dir, save_collection};
    use crate::item::Action;

    fn paste(text: &str, cursor_back: Option<usize>) -> Action {
        Action::Clipboard(ClipCommand::PasteText {
            text: text.into(),
            cursor_back,
        })
    }

    fn snip(name: &str, kw: &str, text: &str, coll: &str) -> Snippet {
        Snippet {
            name: name.into(),
            keyword: kw.into(),
            text: text.into(),
            collection: coll.into(),
            ..Snippet::default()
        }
    }

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
        e.reload_blocking();
        let r = e.query("snip sig");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "snip:sig");
        assert_eq!(r[0].action, paste("Regards", None));
        assert_eq!(r[0].cmd_action, Some(Action::Copy("Regards".into())));
        assert_eq!(r[0].subtitle, "sig · Regards");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn placeholders_expand_from_clipboard_history() {
        let (e, root) = engine("snipclip");
        std::fs::create_dir_all(&e.paths.data).unwrap();
        std::fs::write(
            e.paths.snippets_file(),
            r#"[{"name":"Reply","keyword":"re","text":"Hi {clipboard:1}, re: {clipboard}{cursor}!"}]"#,
        )
        .unwrap();
        e.reload_blocking();
        {
            let clip = e.clipboard();
            let mut h = clip.lock().unwrap();
            h.push("Andrew");
            h.push("hedgehogs");
        }
        let r = e.query("snip re");
        assert_eq!(r[0].action, paste("Hi Andrew, re: hedgehogs!", Some(1)));
        assert_eq!(r[0].large_type.as_deref(), Some("Hi Andrew, re: hedgehogs!"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn collections_prefixes_and_search_by_collection() {
        let (e, root) = engine("snipcoll");
        let dir = collections_dir(&e.paths.snippets_file());
        save_collection(
            &dir,
            &Collection {
                name: "Work".into(),
                keyword_prefix: ";".into(),
                keyword_suffix: String::new(),
                snippets: vec![
                    snip("Signature", "sig", "Best,\nMe", ""),
                    snip("Address", "addr", "1 Main St", ""),
                ],
            },
        )
        .unwrap();
        save_collection(
            &dir,
            &Collection {
                name: "Fun".into(),
                snippets: vec![snip("Shrug", "shrug", "¯\\_(ツ)_/¯", "")],
                ..Collection::default()
            },
        )
        .unwrap();
        e.reload_blocking();

        // By resolved keyword.
        let r = e.query("snip ;sig");
        assert_eq!(r[0].title, "Signature");
        assert_eq!(r[0].id, "snip:Work/;sig");
        assert_eq!(r[0].subtitle, "Work · ;sig · Best,");

        // By collection name: everything in it, and nothing from other collections.
        let r = e.query("snip work");
        assert_eq!(
            r.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            vec!["Address", "Signature"]
        );

        // Empty query lists all, grouped by collection.
        let r = e.query("snip");
        assert_eq!(
            r.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            vec!["Shrug", "Address", "Signature"]
        );

        // Nothing matches.
        let r = e.query("snip zzzqqq");
        assert_eq!(r[0].id, "nosnip");
        assert_eq!(r[0].title, "No matching snippets");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn name_and_keyword_matches_outrank_collection_matches() {
        let all = vec![
            snip("Other", "o", "x", "Mail"),
            snip("Mail merge", "mm", "y", ""),
            snip("Zed", "mail", "z", ""),
        ];
        let r = rank(&all, "mail");
        let names: Vec<&str> = r.iter().map(|(_, s)| s.name.as_str()).collect();
        assert_eq!(names.last(), Some(&"Other"), "collection-only match is last: {names:?}");
        assert_eq!(r.len(), 3);
        assert!(rank(&all, "qqqq").is_empty());
    }

    #[test]
    fn snippet_includes_resolve_through_the_provider() {
        let (e, root) = engine("snipinc");
        std::fs::create_dir_all(&e.paths.data).unwrap();
        std::fs::write(
            e.paths.snippets_file(),
            r#"[{"name":"Sig","keyword":"sig","text":"Regards"},
                {"name":"Mail","keyword":"mail","text":"Hello\n{snippet:sig}"}]"#,
        )
        .unwrap();
        e.reload_blocking();
        let r = e.query("snip mail");
        assert_eq!(r[0].action, paste("Hello\nRegards", None));
        let _ = std::fs::remove_dir_all(root);
    }
}
