//! Keyword inputs and Script Filters: keyword matching, running filters,
//! parsing their JSON output into result items, and icon resolution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::model::{str_map, stringify};
use super::script::run_node_script;
use super::{KIND_EXTERNAL_TRIGGER, KIND_KEYWORD, KIND_SCRIPT_FILTER, SCRIPT_FILTER_TIMEOUT};
use super::{Node, Workflow, render};
use crate::config::Paths;
use crate::files::expand_tilde;
use crate::fuzzy;
use crate::item::{Action, Icon, Item, Mods};

/// Per-modifier override carried by a Script Filter item (`mods`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModOverride {
    pub arg: Option<String>,
    pub valid: Option<bool>,
    pub vars: BTreeMap<String, String>,
}

/// Everything needed to continue a workflow once the user actions a result.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowAction {
    pub workflow: String,
    pub from_uid: String,
    pub arg: String,
    pub vars: BTreeMap<String, String>,
    pub mods: Vec<(Mods, ModOverride)>,
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
            let p = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                wf.dir.join(path)
            };
            if p.is_file() { Icon::Image(p) } else { Icon::None }
        }
    }
}

fn workflow_icon(wf: &Workflow) -> Icon {
    let p = wf.dir.join("icon.png");
    if p.is_file() {
        Icon::Image(p)
    } else {
        Icon::Glyph("◆")
    }
}

impl Workflow {
    /// Result items contributed by this workflow's keyword inputs for `query`.
    /// The second value is true when a keyword fully matched, in which case
    /// the caller should show only these items.
    pub fn keyword_items(&self, query: &str, paths: &Paths) -> (Vec<Item>, bool) {
        let mut items = Vec::new();
        let mut exclusive = false;
        if self.disabled {
            return (items, exclusive);
        }
        for node in &self.nodes {
            if node.kind != KIND_KEYWORD && node.kind != KIND_SCRIPT_FILTER {
                continue;
            }
            let keyword = render(&node.s("keyword"), "", &self.variables);
            let withspace = node.b("withspace");
            let argtype = node.i("argumenttype"); // 0 required, 1 optional, 2 none
            let title = {
                let t = node.s("title");
                if t.is_empty() { node.s("text") } else { t }
            };
            let title = if title.is_empty() { keyword.clone() } else { title };
            let sub = node.s("subtext");
            let placeholder_id = format!("wfkw:{}:{}", self.id, node.uid);

            match match_keyword(query, &keyword, withspace) {
                KeywordMatch::No => {}
                KeywordMatch::Partial => {
                    items.push(
                        Item::new(placeholder_id, title)
                            .subtitle(sub)
                            .icon(workflow_icon(self))
                            .autocomplete(format!("{keyword}{}", if withspace { " " } else { "" }))
                            .valid(false)
                            .score(700.0),
                    );
                }
                KeywordMatch::Full { arg, spaced } => {
                    exclusive = true;
                    let needs_arg = argtype == 0;
                    let satisfied = !needs_arg || !arg.is_empty();
                    if node.kind == KIND_KEYWORD {
                        let subtitle = render(&sub, &arg, &self.variables);
                        let mut it = Item::new(placeholder_id, title)
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
                            vars: BTreeMap::new(),
                            mods: vec![],
                        }));
                        if !satisfied {
                            it.autocomplete = Some(format!("{keyword} "));
                        }
                        items.push(it);
                    } else if (withspace && !spaced) || (needs_arg && arg.is_empty()) {
                        // Script filter waiting for its argument.
                        items.push(
                            Item::new(placeholder_id, title)
                                .subtitle(sub)
                                .icon(workflow_icon(self))
                                .autocomplete(format!("{keyword} "))
                                .valid(false)
                                .score(1000.0),
                        );
                    } else {
                        items.extend(self.run_script_filter(node, &arg, paths));
                    }
                }
            }
        }
        (items, exclusive)
    }

    fn run_script_filter(&self, node: &Node, arg: &str, paths: &Paths) -> Vec<Item> {
        let out = match run_node_script(node, self, paths, arg, &BTreeMap::new(), SCRIPT_FILTER_TIMEOUT) {
            Ok(o) => o,
            Err(e) => return vec![error_item(self, node, &format!("{e:#}"))],
        };
        let parsed = match parse_script_filter(&out.stdout) {
            Ok(p) => p,
            Err(e) => {
                let detail = if out.stderr.trim().is_empty() {
                    format!("{e:#}")
                } else {
                    out.stderr.trim().to_string()
                };
                return vec![error_item(self, node, &detail)];
            }
        };
        let filter = node.b("alfredfiltersresults");
        let mut items = Vec::new();
        for (idx, raw) in parsed.items.iter().enumerate() {
            let title = raw.get("title").map(stringify).unwrap_or_default();
            let mut score = 10_000.0 - idx as f64;
            if filter {
                match fuzzy::score(arg, &title) {
                    Some(s) => score += s,
                    None => continue,
                }
            }
            let sub = raw.get("subtitle").map(stringify).unwrap_or_default();
            let item_arg = match raw.get("arg") {
                Some(Value::Array(a)) => a.first().map(stringify).unwrap_or_default(),
                Some(v) => stringify(v),
                None => String::new(),
            };
            let mut vars = parsed.variables.clone();
            vars.extend(str_map(raw.get("variables")));
            let mut mods = Vec::new();
            for (k, m) in raw.get("mods").and_then(Value::as_object).into_iter().flatten() {
                if let Some(mm) = Mods::parse(k) {
                    mods.push((
                        mm,
                        ModOverride {
                            arg: m.get("arg").map(stringify),
                            valid: m.get("valid").and_then(Value::as_bool),
                            vars: str_map(m.get("variables")),
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
            }));
            items.push(it);
        }
        if items.is_empty() {
            items.push(
                Item::new(format!("wfempty:{}:{}", self.id, node.uid), "No results")
                    .subtitle(node.s("subtext"))
                    .icon(workflow_icon(self))
                    .valid(false)
                    .score(1.0),
            );
        }
        items
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

struct ParsedFilter {
    items: Vec<Value>,
    variables: BTreeMap<String, String>,
}

fn parse_script_filter(stdout: &str) -> Result<ParsedFilter> {
    let v: Value = serde_json::from_str(stdout.trim()).context("script filter output is not valid JSON")?;
    let items = match v.get("items") {
        Some(Value::Array(a)) => a.clone(),
        _ => bail!("script filter JSON has no \"items\" array"),
    };
    Ok(ParsedFilter {
        items,
        variables: str_map(v.get("variables")),
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
