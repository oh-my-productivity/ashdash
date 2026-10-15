//! Browser bookmarks: `bm <query>` (exclusive list) and ranked matches in
//! general queries.
//!
//! The provider never reads bookmark files itself: it asks the
//! [`BookmarkCache`] for its current snapshot (an `Arc` clone) and nudges it
//! to re-check in the background. Until the first background load finishes,
//! general queries simply get no bookmark results and `bm` shows a
//! "Loading" row.

use std::path::Path;

use super::{Provider, ProviderOutput, QueryCtx, sort_items, strip_keyword};
use crate::bookmarks::url::display_host;
use crate::bookmarks::{Bookmark, BookmarkCache, BookmarkSettings, Issue, Snapshot, SourceErrorKind, build_sources};
use crate::brand;
use crate::fuzzy;
use crate::item::{Action, Icon, Item};

const KEYWORD: &str = "bm";
/// Deep link to Privacy & Security > Full Disk Access.
const FULL_DISK_ACCESS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
/// Bookmarks contribute at most this many rows to a general query.
const GENERAL_LIMIT: usize = 8;
/// Keep bookmarks just below an app/command with an identical title.
const GENERAL_PENALTY: f64 = 50.0;
/// General queries ignore weak "somewhere inside the title" matches
/// (`fuzzy::score` yields < 500 for those) and 1-character queries.
const GENERAL_MIN_SCORE: f64 = 500.0;
/// Host-only matches in `bm` mode rank below any title match.
const HOST_PENALTY: f64 = 350.0;

pub struct BookmarksProvider {
    settings: BookmarkSettings,
    cache: BookmarkCache,
}

impl BookmarksProvider {
    /// The browsers found under `home` (injected, never an ambient lookup, so a test
    /// points it at a temp folder); loading starts now, in the background.
    pub fn with_settings(home: &Path, settings: BookmarkSettings) -> Self {
        let sources = build_sources(home, &settings.enabled_sources);
        let p = Self::with_cache(settings, BookmarkCache::new(sources));
        p.cache.refresh_in_background();
        p
    }

    pub fn with_cache(settings: BookmarkSettings, cache: BookmarkCache) -> Self {
        BookmarksProvider { settings, cache }
    }
}

impl Provider for BookmarksProvider {
    fn id(&self) -> &'static str {
        "bookmarks"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        self.cache.poke();
        let snap = self.cache.snapshot();
        if let Some(rest) = strip_keyword(ctx.query, KEYWORD) {
            let max = ctx.engine_state.config.max_results;
            return ProviderOutput::exclusive(keyword_items(&snap, rest, max));
        }
        if !self.settings.include_in_general_search {
            return ProviderOutput::none();
        }
        ProviderOutput::ranked(general_items(&snap, ctx.query))
    }
}

fn item_for(b: &Bookmark, score: f64) -> Item {
    let host = display_host(&b.url);
    let subtitle = match (b.folder.is_empty(), host.is_empty()) {
        (false, false) => format!("{} · {host}", b.folder),
        (false, true) => b.folder.clone(),
        (true, false) => host,
        (true, true) => b.url.clone(),
    };
    let mut it = Item::new(format!("bm:{}", b.url), &b.title)
        .subtitle(subtitle)
        .icon(Icon::Glyph("🔖"))
        .action(Action::OpenUrl(b.url.clone()))
        .score(score)
        .autocomplete(format!("{KEYWORD} {}", b.title));
    it.cmd_action = Some(Action::Copy(b.url.clone()));
    it.copy_text = Some(b.url.clone());
    it.large_type = Some(b.title.clone());
    it
}

fn general_items(snap: &Snapshot, query: &str) -> Vec<Item> {
    if query.trim().chars().count() < 2 {
        return Vec::new();
    }
    // Normalised once per keystroke; every bookmark title was normalised when
    // the snapshot was built, so scoring allocates nothing per bookmark.
    let q = fuzzy::Query::new(query);
    let mut items: Vec<Item> = snap
        .scored()
        .filter_map(|(b, title, _)| {
            let s = q.score(title)?;
            (s >= GENERAL_MIN_SCORE).then(|| item_for(b, s - GENERAL_PENALTY))
        })
        .collect();
    sort_items(&mut items);
    items.truncate(GENERAL_LIMIT);
    items
}

fn keyword_items(snap: &Snapshot, rest: &str, max: usize) -> Vec<Item> {
    let notice = access_notice(&snap.issues);
    let room = max.saturating_sub(usize::from(notice.is_some())).max(1);

    let q = fuzzy::Query::new(rest);
    let mut items: Vec<Item> = snap
        .scored()
        .filter_map(|(b, title, host)| {
            let by_host = || q.score(host).map(|s| s - HOST_PENALTY);
            q.score(title).or_else(by_host).map(|s| item_for(b, s))
        })
        .collect();
    sort_items(&mut items);
    items.truncate(room);

    if items.is_empty() {
        if !snap.loaded {
            items.push(
                Item::new("bm:loading", "Loading bookmarks…")
                    .subtitle("Reading your browsers' bookmark files; try again in a moment")
                    .icon(Icon::Glyph("🔖"))
                    .valid(false),
            );
        } else if notice.is_none() {
            let sub = if rest.trim().is_empty() {
                "No bookmarks found in the enabled browsers"
            } else {
                "Nothing matches"
            };
            items.push(
                Item::new("bm:none", "No bookmarks")
                    .subtitle(sub)
                    .icon(Icon::Glyph("🔖"))
                    .valid(false),
            );
        }
    }
    items.extend(notice);
    items
}

/// ONE item for all permission-denied sources, explaining the remedy.
fn access_notice(issues: &[Issue]) -> Option<Item> {
    let names: Vec<&str> = issues
        .iter()
        .filter(|i| i.kind == SourceErrorKind::PermissionDenied)
        .map(|i| i.source_name)
        .collect();
    if names.is_empty() {
        return None;
    }
    Some(
        Item::new(
            "bm:needs-full-disk-access",
            format!("{} bookmarks need Full Disk Access", names.join(", ")),
        )
        .subtitle(format!(
            "System Settings › Privacy & Security › Full Disk Access: enable {}, then restart it. ↩ opens that pane.",
            brand::NAME
        ))
        .icon(Icon::Glyph("🔒"))
        .action(Action::OpenUrl(FULL_DISK_ACCESS_URL.to_string()))
        .score(-1.0),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, RwLock};

    use super::*;
    use crate::bookmarks::cache::testsupport::{FakeSource, bm};
    use crate::bookmarks::{BookmarkSource, SourceError};
    use crate::config::{Config, Paths};
    use crate::providers::{EngineState, testutil};

    fn provider(sources: Vec<Box<dyn BookmarkSource>>, settings: BookmarkSettings) -> BookmarksProvider {
        let cache = BookmarkCache::new(sources);
        cache.refresh_blocking();
        BookmarksProvider::with_cache(settings, cache)
    }

    fn sample() -> Vec<Bookmark> {
        vec![
            bm(
                "Rust Programming Language",
                "https://www.rust-lang.org/",
                "Bookmarks Bar › Dev",
            ),
            bm(
                "Crates.io: Rust Package Registry",
                "https://crates.io/",
                "Bookmarks Bar › Dev",
            ),
            bm("Rustacean Station", "https://rustacean-station.org/", "Other Bookmarks"),
            bm("Hacker News", "https://news.ycombinator.com/", ""),
            bm("Bookmarklet", "javascript:alert(1)", ""),
            bm("Duplicate Crates", "https://CRATES.io", "Elsewhere"),
        ]
    }

    fn run(p: &BookmarksProvider, q: &str) -> ProviderOutput {
        let config = Config::default();
        let paths = Paths::under(&std::env::temp_dir().join("oa-bm-prov-unused"));
        let apps = RwLock::new(Vec::new());
        let wfs = RwLock::new(Vec::new());
        let snips = RwLock::new(Vec::new());
        let clip = Mutex::new(crate::clipboard::ClipboardHistory::load(paths.clipboard_file(), 10));
        let st = EngineState {
            config: &config,
            paths: &paths,
            apps: &apps,
            workflows: &wfs,
            snippets: &snips,
            clipboard: &clip,
        };
        p.query(&QueryCtx {
            query: q,
            engine_state: &st,
        })
    }

    fn fake(id: &'static str, r: Result<Vec<Bookmark>, SourceError>) -> Vec<Box<dyn BookmarkSource>> {
        vec![Box::new(FakeSource::new(id, r))]
    }

    #[test]
    fn keyword_is_exclusive_and_lists_matches_with_folder_and_host() {
        let p = provider(fake("a", Ok(sample())), BookmarkSettings::default());
        let out = run(&p, "bm rust prog");
        assert!(out.exclusive);
        let titles: Vec<_> = out.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Rust Programming Language"]);
        let wide: Vec<_> = run(&p, "bm rust").items.iter().map(|i| i.title.clone()).collect();
        assert!(wide.contains(&"Crates.io: Rust Package Registry".to_string()));
        assert!(!wide.contains(&"Hacker News".to_string()));
        let first = &out.items[0];
        assert_eq!(first.subtitle, "Bookmarks Bar › Dev · rust-lang.org");
        assert_eq!(first.action, Action::OpenUrl("https://www.rust-lang.org/".into()));
        assert_eq!(
            first.cmd_action,
            Some(Action::Copy("https://www.rust-lang.org/".into()))
        );
        assert_eq!(first.id, "bm:https://www.rust-lang.org/");
        assert!(p.learns());
    }

    #[test]
    fn keyword_variants() {
        let p = provider(fake("a", Ok(sample())), BookmarkSettings::default());
        // Bare keyword lists everything (still exclusive), alphabetical.
        let all = run(&p, "bm");
        assert!(all.exclusive);
        assert_eq!(all.items.len(), 4); // javascript: dropped, duplicate URL dropped
        // Case-insensitive keyword; a longer word is not the keyword.
        assert!(run(&p, "BM hacker").exclusive);
        let other = run(&p, "bmw rust");
        assert!(!other.exclusive);
        // Host-only match still finds it in keyword mode.
        let by_host = run(&p, "bm ycombinator");
        assert_eq!(by_host.items.len(), 1);
        assert_eq!(by_host.items[0].title, "Hacker News");
        // No match -> one informative, non-actionable row.
        let none = run(&p, "bm zzzzqqq");
        assert_eq!(none.items.len(), 1);
        assert!(!none.items[0].valid);
    }

    #[test]
    fn keyword_respects_max_results() {
        let many: Vec<_> = (0..200)
            .map(|i| bm(&format!("Site {i}"), &format!("https://s{i}.example/"), ""))
            .collect();
        let p = provider(fake("a", Ok(many)), BookmarkSettings::default());
        let cfg_max = Config::default().max_results;
        assert!(run(&p, "bm site").items.len() <= cfg_max);
    }

    #[test]
    fn general_queries_get_ranked_non_exclusive_results() {
        let p = provider(fake("a", Ok(sample())), BookmarkSettings::default());
        let out = run(&p, "rust");
        assert!(!out.exclusive);
        let titles: Vec<_> = out.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles.len(), 3);
        assert!(titles.contains(&"Rust Programming Language"));
        assert!(!titles.contains(&"Hacker News"));
        assert!(run(&p, "duplicate").items.is_empty(), "duplicate URL must be dropped");
        // initials match: "hn" -> Hacker News
        let hn = run(&p, "hn");
        assert_eq!(hn.items[0].title, "Hacker News");
        // scores are below an app with the identical title would score
        assert!(hn.items[0].score < 1000.0);
    }

    #[test]
    fn general_search_ignores_one_char_and_weak_substring_matches() {
        let p = provider(fake("a", Ok(sample())), BookmarkSettings::default());
        assert!(run(&p, "r").items.is_empty());
        // "acker" is only a mid-word substring of "Hacker".
        assert!(run(&p, "acker").items.is_empty());
        // ...but is still found with the keyword.
        assert_eq!(run(&p, "bm acker").items.len(), 1);
    }

    #[test]
    fn ranking_prefers_exact_then_prefix_then_word() {
        let list = vec![
            bm("My Rust Notes", "https://c.example/", ""),
            bm("Rust Book", "https://b.example/", ""),
            bm("Rust", "https://a.example/", ""),
        ];
        let p = provider(fake("a", Ok(list)), BookmarkSettings::default());
        let titles: Vec<_> = run(&p, "rust").items.iter().map(|i| i.title.clone()).collect();
        assert_eq!(titles, vec!["Rust", "Rust Book", "My Rust Notes"]);
    }

    #[test]
    fn general_search_can_be_disabled_but_keyword_still_works() {
        let settings = BookmarkSettings {
            include_in_general_search: false,
            ..BookmarkSettings::default()
        };
        let p = provider(fake("a", Ok(sample())), settings);
        let out = run(&p, "rust");
        assert!(!out.exclusive && out.items.is_empty());
        assert!(!run(&p, "bm rust").items.is_empty());
    }

    #[test]
    fn bookmarklets_never_surface() {
        let p = provider(fake("a", Ok(sample())), BookmarkSettings::default());
        for q in ["bm bookmarklet", "bookmarklet", "bm alert"] {
            for it in run(&p, q).items {
                if let Action::OpenUrl(u) = &it.action {
                    assert!(!u.starts_with("javascript:"), "{q}");
                }
            }
        }
    }

    #[test]
    fn file_bookmarks_are_dropped() {
        // `open file:///...` would launch an app or run a .command; a browser only displays it
        let list = vec![
            bm("Calculator", "file:///System/Applications/Calculator.app", ""),
            bm("Local Notes", "file:///Users/me/notes.html", ""),
            bm("Calc Site", "https://calc.example/", ""),
        ];
        let p = provider(fake("a", Ok(list)), BookmarkSettings::default());
        for q in ["bm calc", "calc", "bm local", "bm"] {
            for it in run(&p, q).items {
                assert!(
                    !matches!(&it.action, Action::OpenUrl(u) if u.starts_with("file:")),
                    "{q}: {:?}",
                    it.action
                );
            }
        }
        assert_eq!(run(&p, "bm").items.len(), 1);
    }

    #[test]
    fn scoring_thousands_of_bookmarks_per_keystroke_stays_cheap() {
        let many: Vec<_> = (0..5000)
            .map(|i| {
                bm(
                    &format!("Some Reasonably Long Bookmark Title Number {i} About Topic {}", i % 97),
                    &format!("https://site{i}.example/page"),
                    "",
                )
            })
            .collect();
        let cache = BookmarkCache::new(fake("a", Ok(many)));
        cache.refresh_blocking();
        let snap = cache.snapshot();
        let mut times: Vec<std::time::Duration> = Vec::new();
        for q in ["so", "som", "bookmark", "topic 4", "srbt", "zzzz", "number 12", "about"]
            .iter()
            .cycle()
            .take(40)
        {
            let t = std::time::Instant::now();
            let items = general_items(&snap, q);
            times.push(t.elapsed());
            assert!(items.len() <= GENERAL_LIMIT);
        }
        times.sort();
        let p95 = times[times.len() * 95 / 100];
        // generous: an unoptimised build on a loaded machine (measured here with 5 000 bookmarks: about 17 ms debug, 4.5 ms release)
        assert!(p95 < std::time::Duration::from_millis(150), "p95 {p95:?}");
    }

    #[test]
    fn full_disk_access_denial_yields_one_explanatory_item() {
        let denied = SourceError {
            kind: SourceErrorKind::PermissionDenied,
            message: "Bookmarks.plist: Operation not permitted".into(),
        };
        let sources: Vec<Box<dyn BookmarkSource>> = vec![
            Box::new(FakeSource::new("safari", Err(denied.clone()))),
            Box::new(FakeSource::new("chrome", Ok(sample()))),
        ];
        let p = provider(sources, BookmarkSettings::default());

        let out = run(&p, "bm rust");
        let notices: Vec<_> = out
            .items
            .iter()
            .filter(|i| i.id == "bm:needs-full-disk-access")
            .collect();
        assert_eq!(notices.len(), 1);
        let n = notices[0];
        assert!(n.title.contains("safari") && n.title.contains("Full Disk Access"));
        assert!(n.subtitle.contains("Full Disk Access") && n.subtitle.contains("restart"));
        assert!(matches!(&n.action, Action::OpenUrl(u) if u.starts_with("x-apple.systempreferences:")));
        assert!(n.valid);
        // Other browsers still work, and the notice comes last.
        assert!(out.items.len() > 1);
        assert_eq!(out.items.last().unwrap().id, "bm:needs-full-disk-access");

        // It is not injected into ordinary queries.
        assert!(run(&p, "rust").items.iter().all(|i| i.id.starts_with("bm:https")));

        // Only denied sources yield only the notice (no "No bookmarks" row).
        let only: Vec<Box<dyn BookmarkSource>> = vec![Box::new(FakeSource::new("safari", Err(denied)))];
        let p = provider(only, BookmarkSettings::default());
        let out = run(&p, "bm");
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].id, "bm:needs-full-disk-access");
    }

    #[test]
    fn other_errors_do_not_produce_items() {
        let p = provider(
            fake("a", Err(SourceError::other("malformed"))),
            BookmarkSettings::default(),
        );
        let out = run(&p, "bm");
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].id, "bm:none");
    }

    #[test]
    fn unloaded_cache_shows_loading_and_never_blocks() {
        let src = FakeSource::new("a", Ok(sample()));
        let cache = BookmarkCache::new(vec![Box::new(std::sync::Arc::clone(&src))]);
        // Not refreshed: the provider must not read anything itself.
        let p = BookmarksProvider::with_cache(BookmarkSettings::default(), cache);
        // The first query only *starts* a background load.
        let out = run(&p, "bm");
        assert!(out.exclusive);
        assert!(!out.items.is_empty());
        // Wait for the background thread, then results appear.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !p.cache.snapshot().loaded && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(run(&p, "bm").items.len(), 4);
    }

    #[test]
    fn end_to_end_through_the_engine_keyword_is_exclusive() {
        let (e, root) = testutil::engine("bm-e2e");
        // Test builds have no browser sources: the list is a single info row,
        // with no app results or web-search fallbacks mixed in.
        let r = e.query("bm anything");
        assert_eq!(r.len(), 1);
        assert!(r[0].id.starts_with("bm:"));
        // "bmx" is not the keyword.
        assert!(e.query("bmx").iter().all(|i| !i.id.starts_with("bm:")));
        let _ = std::fs::remove_dir_all(root);
    }
}
