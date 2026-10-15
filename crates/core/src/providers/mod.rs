//! Query providers: every searchable feature is a small, independent plug-in.
//!
//! The engine asks each provider in [`registry`] order. A provider that
//! returns `exclusive: true` takes over the whole result list: later
//! providers are not consulted and nothing is merged, ranked or truncated by
//! the engine (the provider returns the final list). All other output is
//! pooled, usage-boosted, sorted and truncated by the engine.
//!
//! # Adding a provider
//! 1. Create `providers/<name>.rs` with a type implementing [`Provider`].
//! 2. Add `mod <name>;` below and ONE line to [`registry`].
//!
//! # Placement
//! [`Provider::placement`] defaults to [`Placement::Ranked`]. Fallback web
//! searches use [`Placement::Trailing`]: their output is appended after the
//! ranked list has been sorted and truncated, so it always closes the list.
//! Trailing providers are skipped when an exclusive provider short-circuits.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use crate::actions::{Os, RealOs};
use crate::apps::AppEntry;
use crate::clipboard::{ClipboardHistory, Snippet};
use crate::config::{Config, Paths};
use crate::item::Item;
use crate::workflow::Workflow;

mod actions;
mod apps;
mod bookmarks;
mod calc;
mod clipboard;
mod files;
mod shell;
mod snippets;
mod system;
mod websearch;
mod workflow;

/// Read-only handles to the engine's shared state that providers may need.
#[derive(Clone, Copy)]
pub struct EngineState<'a> {
    pub config: &'a Config,
    pub paths: &'a Paths,
    pub apps: &'a RwLock<Vec<AppEntry>>,
    pub workflows: &'a RwLock<Vec<Workflow>>,
    pub snippets: &'a RwLock<Vec<Snippet>>,
    pub clipboard: &'a Mutex<ClipboardHistory>,
}

/// One query as seen by a provider.
pub struct QueryCtx<'a> {
    /// The input with leading whitespace removed (never blank).
    pub query: &'a str,
    pub engine_state: &'a EngineState<'a>,
}

#[derive(Default)]
pub struct ProviderOutput {
    pub items: Vec<Item>,
    /// When true, `items` is the complete, final answer to the query: no other
    /// provider is asked. The engine still truncates it to `max_results`, so
    /// providers do not need to (but should avoid producing huge lists).
    pub exclusive: bool,
}

impl ProviderOutput {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn ranked(items: Vec<Item>) -> Self {
        ProviderOutput {
            items,
            exclusive: false,
        }
    }
    pub fn exclusive(items: Vec<Item>) -> Self {
        ProviderOutput { items, exclusive: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Merged with other non-exclusive output, then boosted/sorted/truncated.
    Ranked,
    /// Appended after ranking; never sorted or truncated.
    Trailing,
}

pub trait Provider: Send + Sync {
    /// Unique, stable identifier (used for future per-provider settings).
    fn id(&self) -> &'static str;
    /// Whether picks from this provider feed usage-based ranking. Providers
    /// that control their own order (workflows) return false.
    fn learns(&self) -> bool {
        true
    }
    fn query(&self, ctx: &QueryCtx) -> ProviderOutput;
    fn placement(&self) -> Placement {
        Placement::Ranked
    }
}

/// What the providers need from the machine they run on. Production uses
/// [`RegistryDeps::ambient`]; tests inject a fake [`Os`] and a temp home so
/// nothing touches the real machine.
#[derive(Clone)]
pub struct RegistryDeps {
    /// Runs `mdfind`, Finder scripts, `open` ... for providers that need them.
    pub os: Arc<dyn Os>,
    /// The home folder: the file-search root and where browser bookmarks live.
    pub home: PathBuf,
}

impl RegistryDeps {
    /// The real OS and the current user's home.
    pub fn ambient() -> RegistryDeps {
        RegistryDeps {
            os: Arc::new(RealOs),
            home: ambient_home(),
        }
    }
}

/// The current user's home folder. Unit tests of this crate get a folder that
/// does not exist, so they can never read real browser data or search the real home.
pub fn ambient_home() -> PathBuf {
    #[cfg(test)]
    {
        PathBuf::from("/nonexistent-ashdash-test-home")
    }
    #[cfg(not(test))]
    {
        dirs::home_dir().unwrap_or_default()
    }
}

/// Ids of every built-in provider in precedence order (the values accepted by
/// `Config::providers_disabled`).
pub const PROVIDER_IDS: &[&str] = &[
    "workflow",
    "shell",
    "files",
    "actions",
    "clipboard",
    "snippets",
    "bookmarks",
    "web-keyword",
    "path",
    "calc",
    "apps",
    "system",
    "web-fallback",
];

/// All providers in precedence order, for the real machine, with default settings.
pub fn registry() -> Vec<Box<dyn Provider>> {
    registry_for(&Config::default(), &RegistryDeps::ambient())
}

/// Default settings, wired to `deps` (tests inject a fake OS and a temp home).
pub fn registry_with(deps: &RegistryDeps) -> Vec<Box<dyn Provider>> {
    registry_for(&Config::default(), deps)
}

/// The providers for `config`, wired to `deps`, in precedence order: providers
/// named in `config.providers_disabled` are not created at all, and the file
/// search and bookmark providers take their settings from the configuration.
pub fn registry_for(config: &Config, deps: &RegistryDeps) -> Vec<Box<dyn Provider>> {
    let mut v: Vec<Box<dyn Provider>> = Vec::new();
    // One line per provider; the id literal must equal `Provider::id` (tested).
    macro_rules! add {
        ($id:literal, $p:expr) => {
            if !config.providers_disabled.iter().any(|d| d == $id) {
                v.push(Box::new($p));
            }
        };
    }
    add!("workflow", workflow::WorkflowProvider);
    add!("shell", shell::ShellProvider);
    add!(
        "files",
        files::FilesProvider::with_os(config.file_search.to_scope_in(&deps.home), Arc::clone(&deps.os))
    );
    add!("actions", actions::ActionsProvider);
    add!("clipboard", clipboard::ClipboardProvider);
    add!("snippets", snippets::SnippetsProvider);
    add!(
        "bookmarks",
        bookmarks::BookmarksProvider::with_settings(&deps.home, config.bookmarks.clone())
    );
    add!("web-keyword", websearch::WebKeywordProvider);
    add!("path", files::PathBrowseProvider);
    add!("calc", calc::CalcProvider);
    add!("apps", apps::AppsProvider);
    add!("system", system::SystemProvider);
    add!("web-fallback", websearch::FallbackProvider);
    v
}

/// If `q` is `keyword` or `keyword <rest>`, return the (left-trimmed) rest.
pub(crate) fn strip_keyword<'a>(q: &'a str, keyword: &str) -> Option<&'a str> {
    let head = q.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &q[keyword.len()..];
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(' ') {
        Some(rest.trim_start())
    } else {
        None
    }
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

/// Order by score (high first), then title.
pub(crate) fn sort_items(items: &mut [Item]) {
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Shared fixture for provider tests: a throwaway engine rooted in a temp dir.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    use crate::config::{Config, Paths};
    use crate::engine::Engine;

    pub fn engine(tag: &str) -> (Engine, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-prov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("apps");
        std::fs::create_dir_all(apps.join("Safari.app")).unwrap();
        std::fs::create_dir_all(apps.join("Clipper.app")).unwrap();
        let config = Config {
            app_dirs: vec![apps],
            ..Config::default()
        };
        (Engine::new(Paths::under(&root), config), root)
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn provider_id_list_matches_the_registry() {
        let ids: Vec<_> = registry().iter().map(|p| p.id()).collect();
        assert_eq!(ids, PROVIDER_IDS);
    }

    #[test]
    fn disabled_providers_are_not_created() {
        let config = Config {
            providers_disabled: vec!["calc".into(), "bookmarks".into(), "no-such-provider".into()],
            ..Config::default()
        };
        let ids: Vec<_> = registry_for(&config, &RegistryDeps::ambient())
            .iter()
            .map(|p| p.id())
            .collect();
        assert!(!ids.contains(&"calc") && !ids.contains(&"bookmarks"));
        assert_eq!(ids.len(), PROVIDER_IDS.len() - 2);
    }

    #[test]
    fn registry_ids_are_unique() {
        let ids: Vec<_> = registry().iter().map(|p| p.id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "duplicate provider id in {ids:?}");
    }
}
