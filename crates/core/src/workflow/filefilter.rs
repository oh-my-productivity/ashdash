//! The File Filter input: a Spotlight (`mdfind`) search whose predicate is
//! built from the object's `fields`, `types` and `scopes`.
//!
//! Layout observed in the bundled "File filter ..." templates: `fields` is a
//! list of `{field, value, split, words, not}` (the default searches
//! `kMDItemDisplayName` and `kMDItemFinderComment` for `{query}`), `types` a
//! list of UTIs, `scopes` a list of folders, plus `includesystem`,
//! `anchorfields`, `daterange`. How the pieces combine is inferred:
//!
//! * `split`: one clause per whitespace-separated word, all of which must match;
//! * `words`: the clause matches at the start of a word, otherwise anywhere;
//! * `not` negates the field's clause;
//! * a file matches when any field matches, and (if `types` is non-empty) is
//!   one of the types;
//! * results under `/System`, `/Library`, `/private`, `/usr`, `/bin`, `/sbin`,
//!   `/opt` are dropped unless `includesystem` is set.
//!
//! `anchorfields` and `daterange` are read but not applied yet.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::{Node, Workflow, WorkflowAction, render};
use crate::files::{expand_tilde, tilde};
use crate::fuzzy;
use crate::item::{Action, Icon, Item};

const MAX_RESULTS: usize = 50;

/// Runs a Spotlight predicate inside the given scopes and returns paths.
pub(super) type Searcher<'a> = &'a dyn Fn(&str, &[PathBuf]) -> Vec<PathBuf>;

fn mdq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn valid_attribute(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// UTIs additionally contain hyphens (`public.plain-text`).
fn valid_uti(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Build the Spotlight predicate for `query`; `None` when there is nothing to
/// search for (no query words and no type restriction).
pub(super) fn predicate(node: &Node, query: &str, vars: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.replace(['*', '?'], ""))
        .filter(|w| !w.is_empty())
        .collect();

    let default_fields = [serde_json::json!({
        "field": "kMDItemDisplayName", "value": "{query}", "split": true, "words": true, "not": false
    })];
    let fields: Vec<&Value> = match node.config.get("fields").and_then(Value::as_array) {
        Some(f) if !f.is_empty() => f.iter().collect(),
        _ => default_fields.iter().collect(),
    };

    let mut field_clauses = Vec::new();
    if !words.is_empty() {
        for f in fields {
            let name = f.get("field").and_then(Value::as_str).unwrap_or("");
            if !valid_attribute(name) {
                continue;
            }
            let tpl = f.get("value").and_then(Value::as_str).unwrap_or("{query}");
            let split = f.get("split").and_then(Value::as_bool).unwrap_or(true);
            let by_word = f.get("words").and_then(Value::as_bool).unwrap_or(false);
            let negate = f.get("not").and_then(Value::as_bool).unwrap_or(false);
            let pieces: Vec<String> = if split { words.clone() } else { vec![words.join(" ")] };
            let clauses: Vec<String> = pieces
                .iter()
                .map(|p| {
                    let v = render(tpl, p, vars);
                    let v = mdq(&v);
                    let pat = if v.contains('*') {
                        v
                    } else if by_word {
                        format!("{v}*")
                    } else {
                        format!("*{v}*")
                    };
                    let c = format!("{name} == \"{pat}\"{}", if by_word { "wcd" } else { "cd" });
                    if negate { format!("!({c})") } else { c }
                })
                .collect();
            if !clauses.is_empty() {
                field_clauses.push(format!("({})", clauses.join(" && ")));
            }
        }
    }

    let types: Vec<String> = node
        .config
        .get("types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|t| valid_uti(t))
        .map(|t| format!("kMDItemContentTypeTree == \"{t}\""))
        .collect();

    let mut parts = Vec::new();
    if !field_clauses.is_empty() {
        parts.push(format!("({})", field_clauses.join(" || ")));
    } else if types.is_empty() || !words.is_empty() {
        return None;
    }
    if !types.is_empty() {
        parts.push(format!("({})", types.join(" || ")));
    }
    Some(parts.join(" && "))
}

pub(super) fn scopes(node: &Node) -> Vec<PathBuf> {
    node.config
        .get("scopes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(expand_tilde)
        .collect()
}

/// The real Spotlight search.
pub(super) fn mdfind(predicate: &str, scopes: &[PathBuf]) -> Vec<PathBuf> {
    let mut cmd = Command::new("mdfind");
    for s in scopes {
        cmd.arg("-onlyin").arg(s);
    }
    let Ok(out) = cmd.arg(predicate).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(MAX_RESULTS * 4)
        .map(PathBuf::from)
        .collect()
}

fn is_system_path(p: &Path) -> bool {
    ["/System", "/Library", "/private", "/usr", "/bin", "/sbin", "/opt"]
        .iter()
        .any(|root| p.starts_with(root))
}

impl Workflow {
    pub(super) fn file_filter_items(
        &self,
        node: &Node,
        arg: &str,
        vars: &std::collections::BTreeMap<String, String>,
        search: Searcher,
    ) -> Vec<Item> {
        let Some(pred) = predicate(node, arg, vars) else {
            return Vec::new();
        };
        let scopes = scopes(node);
        let include_system = node.b("includesystem");
        let mut found: Vec<(f64, PathBuf)> = search(&pred, &scopes)
            .into_iter()
            .filter(|p| include_system || !is_system_path(p) || scopes.iter().any(|s| p.starts_with(s)))
            .map(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                // Better name matches first; shallower paths break ties.
                let s = fuzzy::score(arg, &name).unwrap_or(0.0) - p.components().count() as f64 * 0.1;
                (s, p)
            })
            .collect();
        found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        found
            .into_iter()
            .take(MAX_RESULTS)
            .enumerate()
            .map(|(i, (_, p))| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let icon = if p.extension().is_some_and(|e| e == "app") {
                    Icon::App(p.clone())
                } else {
                    Icon::Glyph("▤")
                };
                let mut it = Item::new(format!("wf:{}:{}:{}", self.id, node.uid, p.display()), name)
                    .subtitle(tilde(&p))
                    .icon(icon)
                    .score(10_000.0 - i as f64);
                it.copy_text = Some(p.display().to_string());
                it.cmd_action = Some(Action::Reveal(p.clone()));
                it.action = Action::Workflow(Box::new(WorkflowAction {
                    workflow: self.id.clone(),
                    from_uid: node.uid.clone(),
                    arg: p.display().to_string(),
                    ..Default::default()
                }));
                it
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn node(config: Value) -> Node {
        Node {
            uid: "FF".into(),
            kind: super::super::KIND_FILE_FILTER.into(),
            config,
        }
    }

    fn template_config() -> Value {
        json!({
            "fields": [
                {"split": true, "value": "{query}", "words": true, "not": false, "field": "kMDItemDisplayName"},
                {"split": true, "value": "{query}", "words": true, "not": false, "field": "kMDItemFinderComment"}
            ],
            "scopes": [], "types": [], "includesystem": false, "anchorfields": true
        })
    }

    #[test]
    fn predicate_from_template_fields() {
        let n = node(template_config());
        let p = predicate(&n, "my doc", &BTreeMap::new()).unwrap();
        assert_eq!(
            p,
            "((kMDItemDisplayName == \"my*\"wcd && kMDItemDisplayName == \"doc*\"wcd) || \
             (kMDItemFinderComment == \"my*\"wcd && kMDItemFinderComment == \"doc*\"wcd))"
        );
        assert_eq!(predicate(&n, "   ", &BTreeMap::new()), None);
    }

    #[test]
    fn predicate_escapes_and_restricts_types() {
        let mut c = template_config();
        c["types"] = json!(["public.folder", "bad\" || true"]);
        c["fields"] = json!([{"field": "kMDItemFSName", "value": "{query}", "split": false, "words": false, "not": true},
                              {"field": "bad name", "value": "x"}]);
        let p = predicate(&node(c), "a \"q*\" b", &BTreeMap::new()).unwrap();
        assert!(p.contains("!(kMDItemFSName == \"*a \\\"q\\\" b*\"cd)"), "{p}");
        assert!(p.ends_with("&& (kMDItemContentTypeTree == \"public.folder\")"), "{p}");
        assert!(!p.contains("bad name") && !p.contains("true"));
    }

    #[test]
    fn types_alone_can_list_everything() {
        let mut c = template_config();
        c["types"] = json!(["public.folder"]);
        assert_eq!(
            predicate(&node(c), "", &BTreeMap::new()).as_deref(),
            Some("(kMDItemContentTypeTree == \"public.folder\")")
        );
    }

    #[test]
    fn scopes_expand_tilde_and_results_drop_system_paths() {
        let mut c = template_config();
        c["scopes"] = json!(["/tmp/scope", ""]);
        let n = node(c);
        assert_eq!(scopes(&n), vec![PathBuf::from("/tmp/scope")]);
        assert!(is_system_path(Path::new("/System/Library/x")));
        assert!(!is_system_path(Path::new("/Users/me/x")));
    }
}
