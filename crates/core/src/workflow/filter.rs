//! Keyword inputs, Script Filters, List Filters and File Filters: keyword
//! matching, running them, turning their output into result items, icon
//! resolution.
//!
//! Script Filter output may be JSON or the legacy XML; both are reduced to
//! the same value and handled once. Supported extras: `variables` (session
//! variables fed back to the next run), `rerun`, `cache` (+ `loosereload`),
//! `skipknowledge` (accepted; workflow results are never reordered by usage
//! knowledge, so results always keep the script's order), per-item `match`, `type: file`, `mods` (arg, valid,
//! subtitle, variables) and `runningsubtext` placeholders.
//!
//! Variable precedence when a result is actioned (lowest first): workflow
//! variables < script-level `variables` < item `variables` < the pressed
//! mod's `variables`. A mod that has its own `variables` object replaces the
//! item's (script-level ones remain), as the workflow format defines.

use crate::format_ids::FILTER_FLAG_KEY;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::filefilter::{self, Searcher};
use super::model::{str_map, stringify};
use super::script::run_node_script;
use super::utility::strip_diacritics;
use super::xml;
use super::{
    KIND_EXTERNAL_TRIGGER, KIND_FILE_FILTER, KIND_KEYWORD, KIND_LIST_FILTER, KIND_SCRIPT_FILTER, SCRIPT_FILTER_TIMEOUT,
};
use super::{Node, Workflow, render};
use crate::actions::{Os, RealOs};
use crate::config::Paths;
use crate::files::{SearchBudget, expand_tilde};
use crate::fuzzy;
use crate::item::{Action, Icon, Item, Mods};

/// Per-modifier override carried by a Script Filter item (`mods`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModOverride {
    pub arg: Option<String>,
    pub valid: Option<bool>,
    /// Subtitle to show while the modifier is held (for the UI).
    pub subtitle: Option<String>,
    /// Variables applied on top of the action's own when this mod is pressed.
    pub vars: BTreeMap<String, String>,
    /// The mod carried its own `variables` object: `vars` then holds the full
    /// script-level + mod set and replaces the item's variables.
    pub replaces_vars: bool,
}

/// Everything needed to continue a workflow once the user actions a result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkflowAction {
    pub workflow: String,
    pub from_uid: String,
    pub arg: String,
    /// Script-level + item-level variables (item wins).
    pub vars: BTreeMap<String, String>,
    pub mods: Vec<(Mods, ModOverride)>,
    /// Seconds after which the Script Filter that produced this result wants
    /// to run again (`rerun`, 0.1..=5). See [`rerun_interval`].
    pub rerun: Option<f64>,
}

/// Result of asking one workflow about a query.
#[derive(Debug, Clone, Default)]
pub struct KeywordResults {
    pub items: Vec<Item>,
    /// A keyword fully matched: show only these items.
    pub exclusive: bool,
    /// The shortest `rerun` requested by a Script Filter that ran. The UI
    /// should repeat the query after this long if the query is unchanged.
    pub rerun: Option<Duration>,
}

/// The shortest `rerun` any of `items` asks for, read from the metadata the
/// Script Filter attached to its results. Lets a caller that only has the
/// engine's item list (not [`KeywordResults`]) drive re-queries.
pub fn rerun_interval(items: &[Item]) -> Option<Duration> {
    items
        .iter()
        .filter_map(|i| match &i.action {
            Action::Workflow(a) => a.rerun,
            _ => None,
        })
        .reduce(f64::min)
        .map(Duration::from_secs_f64)
}

// ---------------------------------------------------------------- keywords & script filters

enum KeywordMatch {
    No,
    /// The query is a leading fragment of the keyword.
    Partial,
    /// Keyword typed in full; the payload is the argument text and whether a
    /// separating space has been typed.
    Full {
        arg: String,
        spaced: bool,
    },
}

fn match_keyword(query: &str, keyword: &str, withspace: bool) -> KeywordMatch {
    let q = query.trim_start();
    if keyword.is_empty() || q.is_empty() {
        return KeywordMatch::No;
    }
    let n = keyword.chars().count();
    let idx = q.char_indices().nth(n).map(|(i, _)| i).unwrap_or(q.len());
    let (head, rest) = q.split_at(idx);
    if head.to_lowercase() == keyword.to_lowercase() && head.chars().count() == n {
        if rest.is_empty() {
            return KeywordMatch::Full {
                arg: String::new(),
                spaced: false,
            };
        }
        if withspace && !rest.starts_with(' ') {
            return KeywordMatch::No;
        }
        let spaced = rest.starts_with(' ');
        return KeywordMatch::Full {
            arg: rest.trim_start().to_string(),
            spaced,
        };
    }
    if q.chars().count() >= 2 && keyword.to_lowercase().starts_with(&q.to_lowercase()) {
        return KeywordMatch::Partial;
    }
    KeywordMatch::No
}

fn resolve_icon(wf: &Workflow, icon: Option<&Value>) -> Icon {
    let Some(icon) = icon else { return Icon::None };
    let path = icon.get("path").and_then(Value::as_str).unwrap_or("");
    let ty = icon.get("type").and_then(Value::as_str).unwrap_or("");
    if path.is_empty() {
        return Icon::None;
    }
    match ty {
        "fileicon" => {
            let p = expand_tilde(path);
            if p.extension().is_some_and(|e| e == "app") {
                Icon::App(p)
            } else {
                Icon::Glyph("▤")
            }
        }
        "filetype" => Icon::Glyph("▤"),
        _ => {
            // SEC-17: only regular files inside the workflow directory, no symlinks, no hard links.
            match crate::confine::confined_workflow_file(&wf.dir, Path::new(path)) {
                Some(p) => Icon::Image(p),
                None => Icon::None,
            }
        }
    }
}

fn workflow_icon(wf: &Workflow) -> Icon {
    match crate::confine::confined_workflow_file(&wf.dir, Path::new("icon.png")) {
        Some(p) => Icon::Image(p),
        None => Icon::Glyph("◆"),
    }
}

/// What `build_item` needs to know about the object an entry belongs to.
#[derive(Clone, Copy)]
struct ItemCtx<'a> {
    node: &'a Node,
    script_vars: &'a BTreeMap<String, String>,
    filter_query: Option<&'a str>,
    fixed_order: bool,
    rerun: Option<f64>,
}

fn is_filter_kind(kind: &str) -> bool {
    kind == KIND_SCRIPT_FILTER || kind == KIND_LIST_FILTER || kind == KIND_FILE_FILTER
}

/// Diacritic-insensitive matching unless the query itself uses diacritics.
fn fold_for(query: &str, text: &str) -> String {
    if strip_diacritics(query) == query {
        strip_diacritics(text)
    } else {
        text.to_string()
    }
}

fn merge_rerun(a: Option<Duration>, b: Option<f64>) -> Option<Duration> {
    let b = b.map(Duration::from_secs_f64);
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

impl Workflow {
    /// Result items contributed by this workflow's keyword inputs for `query`.
    /// The second value is true when a keyword fully matched, in which case
    /// the caller should show only these items. See [`Workflow::keyword_results`]
    /// for the variant that also reports `rerun`.
    pub fn keyword_items(&self, query: &str, paths: &Paths) -> (Vec<Item>, bool) {
        let r = self.keyword_results(query, paths);
        (r.items, r.exclusive)
    }

    /// With the real OS and the default file-search budget; the engine uses
    /// [`Workflow::keyword_results_os`] with its own handle and the configured budget.
    pub fn keyword_results(&self, query: &str, paths: &Paths) -> KeywordResults {
        self.keyword_results_os(query, paths, &RealOs, &SearchBudget::default())
    }

    /// [`Workflow::keyword_items`] through `os`: File Filter objects run `mdfind` with `budget`
    /// (time and output bytes), like the built-in file search.
    pub fn keyword_items_os(
        &self,
        query: &str,
        paths: &Paths,
        os: &dyn Os,
        budget: &SearchBudget,
    ) -> (Vec<Item>, bool) {
        let r = self.keyword_results_os(query, paths, os, budget);
        (r.items, r.exclusive)
    }

    /// [`Workflow::keyword_results`] through `os` with an explicit file-search `budget`.
    pub fn keyword_results_os(&self, query: &str, paths: &Paths, os: &dyn Os, budget: &SearchBudget) -> KeywordResults {
        self.keyword_results_with(query, paths, &|pred: &str, scopes: &[PathBuf]| {
            filefilter::mdfind(os, budget, pred, scopes)
        })
    }

    /// Like [`Workflow::keyword_results`] with an injectable Spotlight search
    /// (used by File Filter objects), so tests never touch the real index.
    pub(super) fn keyword_results_with(&self, query: &str, paths: &Paths, search: Searcher) -> KeywordResults {
        let mut out = KeywordResults::default();
        if self.disabled {
            return out;
        }
        let vars = self.base_vars(paths);
        for node in &self.nodes {
            let is_keyword = node.kind == KIND_KEYWORD;
            if !is_keyword && !is_filter_kind(&node.kind) {
                continue;
            }
            let keyword = render(&node.s("keyword"), "", &vars);
            let withspace = node.b("withspace");
            let argtype = node.i("argumenttype"); // 0 required, 1 optional, 2 none
            let raw_title = {
                let t = node.s("title");
                if t.is_empty() { node.s("text") } else { t }
            };
            let sub = node.s("subtext");
            let placeholder_id = format!("wfkw:{}:{}", self.id, node.uid);
            let shown_title = |arg: &str| {
                let t = render(&raw_title, arg, &vars);
                if t.is_empty() { keyword.clone() } else { t }
            };

            match match_keyword(query, &keyword, withspace) {
                KeywordMatch::No => {}
                KeywordMatch::Partial => {
                    out.items.push(
                        Item::new(placeholder_id, shown_title(""))
                            .subtitle(sub)
                            .icon(workflow_icon(self))
                            .autocomplete(format!("{keyword}{}", if withspace { " " } else { "" }))
                            .valid(false)
                            .score(700.0),
                    );
                }
                KeywordMatch::Full { arg, spaced } => {
                    out.exclusive = true;
                    let needs_arg = argtype == 0;
                    let satisfied = !needs_arg || !arg.is_empty();
                    if is_keyword {
                        let subtitle = render(&sub, &arg, &vars);
                        let mut it = Item::new(placeholder_id, shown_title(&arg))
                            .subtitle(subtitle)
                            .icon(workflow_icon(self))
                            .valid(satisfied)
                            .score(1000.0);
                        if argtype == 2 && !arg.is_empty() {
                            it = it.valid(false);
                        }
                        it.action = Action::Workflow(Box::new(WorkflowAction {
                            workflow: self.id.clone(),
                            from_uid: node.uid.clone(),
                            arg: arg.clone(),
                            ..Default::default()
                        }));
                        if !satisfied {
                            it.autocomplete = Some(format!("{keyword} "));
                        }
                        out.items.push(it);
                    } else if (withspace && !spaced) || (needs_arg && arg.is_empty()) {
                        // Filter waiting for its argument.
                        out.items.push(
                            Item::new(placeholder_id, shown_title(""))
                                .subtitle(sub)
                                .icon(workflow_icon(self))
                                .autocomplete(format!("{keyword} "))
                                .valid(false)
                                .score(1000.0),
                        );
                    } else if node.kind == KIND_SCRIPT_FILTER {
                        let (items, rerun) = self.run_script_filter(node, &arg, paths);
                        out.items.extend(items);
                        out.rerun = merge_rerun(out.rerun, rerun);
                    } else if node.kind == KIND_LIST_FILTER {
                        out.items.extend(self.list_filter_items(node, &arg));
                    } else {
                        let found = self.file_filter_items(node, &arg, &vars, search);
                        if found.is_empty() {
                            out.items.push(self.no_results_item(node, None));
                        }
                        out.items.extend(found);
                    }
                }
            }
        }
        out
    }

    /// Whether answering `query` would start a process: a Script Filter (not answered from a live cache entry) or a File Filter
    /// (Spotlight) whose keyword is fully typed and whose argument is there. Reads the loaded workflow only; runs nothing. The
    /// query worker uses it to wait a few milliseconds for the next keystroke before it spawns something that is slow and that
    /// the next keystroke would make pointless.
    pub fn would_run_process(&self, query: &str, paths: &Paths) -> bool {
        if self.disabled {
            return false;
        }
        let vars = self.base_vars(paths);
        self.nodes.iter().filter(|n| is_filter_kind(&n.kind)).any(|node| {
            let keyword = render(&node.s("keyword"), "", &vars);
            let withspace = node.b("withspace");
            let KeywordMatch::Full { arg, spaced } = match_keyword(query, &keyword, withspace) else {
                return false;
            };
            let needs_arg = node.i("argumenttype") == 0;
            if (withspace && !spaced) || (needs_arg && arg.is_empty()) {
                return false; // the placeholder row, no process
            }
            match node.kind.as_str() {
                KIND_SCRIPT_FILTER => !matches!(self.runtime.cached(&node.uid, &arg), Some((_, true))),
                KIND_FILE_FILTER => true,
                _ => false,
            }
        })
    }

    /// Placeholders for filters that would run for `query` and declare a
    /// `runningsubtext`: a UI shows them while the real (slow) query is in
    /// flight. Empty when no filter applies.
    pub fn pending_items(&self, query: &str, paths: &Paths) -> Vec<Item> {
        let mut items = Vec::new();
        if self.disabled {
            return items;
        }
        let vars = self.base_vars(paths);
        for node in self.nodes.iter().filter(|n| is_filter_kind(&n.kind)) {
            let running = node.s("runningsubtext");
            if running.is_empty() {
                continue;
            }
            let keyword = render(&node.s("keyword"), "", &vars);
            let argtype = node.i("argumenttype");
            if let KeywordMatch::Full { arg, spaced } = match_keyword(query, &keyword, node.b("withspace")) {
                let waiting = (node.b("withspace") && !spaced) || (argtype == 0 && arg.is_empty());
                if waiting {
                    continue;
                }
                let title = render(&node.s("title"), &arg, &vars);
                items.push(
                    Item::new(
                        format!("wfrun:{}:{}", self.id, node.uid),
                        if title.is_empty() { keyword.clone() } else { title },
                    )
                    .subtitle(render(&running, &arg, &vars))
                    .icon(workflow_icon(self))
                    .valid(false)
                    .score(1000.0),
                );
            }
        }
        items
    }

    fn run_script_filter(&self, node: &Node, arg: &str, paths: &Paths) -> (Vec<Item>, Option<f64>) {
        let rt = &self.runtime;
        let cached = rt.cached(&node.uid, arg);
        let parsed = match cached {
            // A live cache entry answers without running the script.
            Some((p, true)) => p,
            stale => {
                let session = rt.session_vars(&node.uid);
                match self.exec_script_filter(node, arg, paths, &session) {
                    Ok(p) => {
                        rt.merge_session_vars(&node.uid, &p.variables);
                        if let Some((secs, _)) = p.cache {
                            rt.store_cache(&node.uid, arg, Duration::from_secs(secs), p.clone());
                        }
                        p
                    }
                    // `loosereload`: prefer slightly old data over an error.
                    Err(_) if stale.as_ref().is_some_and(|(p, _)| p.cache.is_some_and(|c| c.1)) => {
                        stale.map(|(p, _)| p).unwrap_or_default()
                    }
                    Err(detail) => return (vec![error_item(self, node, &detail)], None),
                }
            }
        };
        let script_vars = {
            let mut v = rt.session_vars(&node.uid);
            v.extend(parsed.variables.clone());
            v
        };
        let filter = node.b(FILTER_FLAG_KEY);
        let mut items: Vec<Item> = parsed
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, raw)| {
                let cx = ItemCtx {
                    node,
                    script_vars: &script_vars,
                    filter_query: filter.then_some(arg),
                    fixed_order: false,
                    rerun: parsed.rerun,
                };
                self.build_item(&cx, raw, idx)
            })
            .collect();
        if items.is_empty() {
            items.push(self.no_results_item(node, parsed.rerun));
        }
        (items, parsed.rerun)
    }

    fn no_results_item(&self, node: &Node, rerun: Option<f64>) -> Item {
        let mut it = Item::new(format!("wfempty:{}:{}", self.id, node.uid), "No results")
            .subtitle(node.s("subtext"))
            .icon(workflow_icon(self))
            .valid(false)
            .score(1.0);
        if rerun.is_some() {
            // Carries the rerun request even when there is nothing to action.
            it.action = Action::Workflow(Box::new(WorkflowAction {
                workflow: self.id.clone(),
                from_uid: node.uid.clone(),
                rerun,
                ..Default::default()
            }));
        }
        it
    }

    fn exec_script_filter(
        &self,
        node: &Node,
        arg: &str,
        paths: &Paths,
        session: &BTreeMap<String, String>,
    ) -> std::result::Result<ParsedFilter, String> {
        let out =
            run_node_script(node, self, paths, arg, session, SCRIPT_FILTER_TIMEOUT).map_err(|e| format!("{e:#}"))?;
        parse_script_filter(&out.stdout).map_err(|e| {
            if out.stderr.trim().is_empty() {
                format!("{e:#}")
            } else {
                out.stderr.trim().to_string()
            }
        })
    }

    /// Turn one script-filter / list-filter entry into a result item.
    /// `filter_query` is set when the launcher (not the script) filters the results.
    /// `fixed_order` keeps the given order instead of ranking by match quality.
    fn build_item(&self, cx: &ItemCtx, raw: &Value, idx: usize) -> Option<Item> {
        let ItemCtx {
            node,
            script_vars,
            filter_query,
            fixed_order,
            rerun,
        } = *cx;
        let title = raw.get("title").map(stringify).unwrap_or_default();
        let mut score = 10_000.0 - idx as f64;
        if let Some(q) = filter_query {
            // `match` fully replaces the title as the text to match against.
            let text = raw.get("match").map(stringify).unwrap_or_else(|| title.clone());
            let s = fuzzy::score(q, &fold_for(q, &text))?;
            if !fixed_order {
                score += s;
            }
        }
        let sub = raw.get("subtitle").map(stringify).unwrap_or_default();
        let item_arg = match raw.get("arg") {
            Some(Value::Array(a)) => a.first().map(stringify).unwrap_or_default(),
            Some(v) => stringify(v),
            None => String::new(),
        };
        if raw.get("type").and_then(Value::as_str) == Some("file")
            && !item_arg.is_empty()
            && !expand_tilde(&item_arg).exists()
        {
            return None;
        }
        let mut vars = script_vars.clone();
        let item_vars = str_map(raw.get("variables"));
        vars.extend(item_vars.clone());
        let mut mods = Vec::new();
        for (k, m) in raw.get("mods").and_then(Value::as_object).into_iter().flatten() {
            if let Some(mm) = Mods::parse(k) {
                let own = m.get("variables").map(|v| str_map(Some(v)));
                let (mod_vars, replaces) = match own {
                    Some(own) => {
                        let mut full = script_vars.clone();
                        full.extend(own);
                        (full, true)
                    }
                    None => (BTreeMap::new(), false),
                };
                mods.push((
                    mm,
                    ModOverride {
                        arg: m.get("arg").map(stringify),
                        valid: m.get("valid").and_then(Value::as_bool),
                        subtitle: m.get("subtitle").map(stringify),
                        vars: mod_vars,
                        replaces_vars: replaces,
                    },
                ));
            }
        }
        let uid = raw
            .get("uid")
            .map(stringify)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| title.clone());
        let text = raw.get("text");
        let mut it = Item::new(format!("wf:{}:{}:{}", self.id, node.uid, uid), title)
            .subtitle(sub)
            .icon(resolve_icon(self, raw.get("icon")))
            .valid(raw.get("valid").and_then(Value::as_bool).unwrap_or(true))
            .score(score);
        it.autocomplete = raw.get("autocomplete").map(stringify);
        it.copy_text = text.and_then(|t| t.get("copy")).map(stringify);
        it.large_type = text.and_then(|t| t.get("largetype")).map(stringify);
        it.action = Action::Workflow(Box::new(WorkflowAction {
            workflow: self.id.clone(),
            from_uid: node.uid.clone(),
            arg: item_arg,
            vars,
            mods,
            rerun,
        }));
        Some(it)
    }

    /// Items of a List Filter: a static JSON list in the object itself,
    /// filtered by the typed argument (kept in order when `fixedorder`).
    fn list_filter_items(&self, node: &Node, arg: &str) -> Vec<Item> {
        let list: Vec<Value> = serde_json::from_str::<Value>(&node.s("items"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let fixed = node.b("fixedorder");
        let mut items: Vec<Item> = list
            .iter()
            .enumerate()
            .filter_map(|(idx, raw)| {
                // A list entry without `arg` passes its title on.
                let mut raw = raw.clone();
                if raw.get("arg").is_none()
                    && let Some(o) = raw.as_object_mut()
                {
                    o.insert("arg".into(), o.get("title").cloned().unwrap_or(Value::Null));
                }
                let cx = ItemCtx {
                    node,
                    script_vars: &BTreeMap::new(),
                    filter_query: Some(arg),
                    fixed_order: fixed,
                    rerun: None,
                };
                self.build_item(&cx, &raw, idx)
            })
            .collect();
        if items.is_empty() {
            items.push(self.no_results_item(node, None));
        }
        items
    }

    /// Whether the graph has an object with this uid (any kind).
    pub fn has_node(&self, uid: &str) -> bool {
        self.node(uid).is_some()
    }

    /// Find an External Trigger object by its trigger id.
    pub fn external_trigger(&self, trigger_id: &str) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|n| n.kind == KIND_EXTERNAL_TRIGGER && n.s("triggerid") == trigger_id)
    }
}

fn error_item(wf: &Workflow, node: &Node, detail: &str) -> Item {
    let first = detail.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
    Item::new(
        format!("wferr:{}:{}", wf.id, node.uid),
        format!("{}: script error", wf.name),
    )
    .subtitle(first.to_string())
    .icon(Icon::Glyph("!"))
    .valid(false)
    .score(1.0)
}

/// A parsed Script Filter response.
#[derive(Debug, Clone, Default)]
pub(super) struct ParsedFilter {
    pub items: Vec<Value>,
    pub variables: BTreeMap<String, String>,
    /// Seconds, clamped to 0.1..=5.
    pub rerun: Option<f64>,
    /// `(ttl seconds clamped to 5..=86400, loosereload)`.
    pub cache: Option<(u64, bool)>,
}

/// A Script Filter may return at most this many items; the rest are dropped (the output size cap
/// alone would still allow ~100k tiny items to be built on every keystroke).
pub(super) const MAX_FILTER_ITEMS: usize = 5_000;

pub(super) fn parse_script_filter(stdout: &str) -> Result<ParsedFilter> {
    let text = stdout.trim();
    let v: Value = if text.starts_with('<') {
        xml::to_json(text).context("script filter output is not valid XML")?
    } else {
        serde_json::from_str(text).context("script filter output is not valid JSON")?
    };
    let items = match v.get("items") {
        Some(Value::Array(a)) => a.iter().take(MAX_FILTER_ITEMS).cloned().collect(),
        _ => bail!("script filter JSON has no \"items\" array"),
    };
    let rerun = v
        .get("rerun")
        .and_then(|r| r.as_f64().or_else(|| r.as_str().and_then(|s| s.trim().parse().ok())))
        .filter(|r| r.is_finite() && *r > 0.0)
        .map(|r| r.clamp(0.1, 5.0));
    let cache = v.get("cache").and_then(|c| {
        let secs = c.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite())?;
        let loose = c.get("loosereload").and_then(Value::as_bool).unwrap_or(false);
        Some((secs.clamp(5.0, 86_400.0) as u64, loose))
    });
    Ok(ParsedFilter {
        items,
        variables: str_map(v.get("variables")),
        rerun,
        cache,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use serde_json::json;

    #[test]
    fn keyword_matching_modes() {
        assert!(matches!(
            match_keyword("demo", "demo", true),
            KeywordMatch::Full { spaced: false, .. }
        ));
        match match_keyword("Demo  hello", "demo", true) {
            KeywordMatch::Full { arg, spaced } => assert!(spaced && arg == "hello"),
            _ => panic!(),
        }
        assert!(matches!(match_keyword("demox", "demo", true), KeywordMatch::No));
        assert!(matches!(match_keyword("de", "demo", true), KeywordMatch::Partial));
        assert!(matches!(match_keyword("d", "demo", true), KeywordMatch::No));
    }

    #[test]
    fn script_filter_item_count_is_capped() {
        let big = format!(
            "{{\"items\":[{}]}}",
            (0..MAX_FILTER_ITEMS + 500)
                .map(|i| format!("{{\"title\":\"t{i}\"}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let parsed = parse_script_filter(&big).unwrap();
        assert_eq!(parsed.items.len(), MAX_FILTER_ITEMS);
        assert_eq!(parsed.items[0]["title"], "t0", "the first items are kept");
        assert_eq!(
            parse_script_filter(r#"{"items":[{"title":"a"}]}"#).unwrap().items.len(),
            1
        );
    }

    #[test]
    fn script_filter_errors_become_items() {
        let (paths, root) = temp_paths("err");
        let mut wf = sample(&root);
        wf.nodes[0].config["script"] = json!("echo not json");
        let (items, _) = wf.keyword_items("demo x", &paths);
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains("script error"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn placeholder_before_space() {
        let (paths, root) = temp_paths("ph");
        let wf = sample(&root);
        let (items, exclusive) = wf.keyword_items("demo", &paths);
        assert!(exclusive);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].autocomplete.as_deref(), Some("demo "));
        let _ = std::fs::remove_dir_all(root);
    }
}
