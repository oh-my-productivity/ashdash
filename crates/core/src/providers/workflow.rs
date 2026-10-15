//! Workflow keyword triggers (reference-compatible workflows).
//!
//! First in the registry: a fully typed workflow keyword takes over the list
//! (exclusive, sorted and truncated here because the engine does not touch
//! exclusive output). Otherwise workflow items join the general pool; ids
//! starting with `wf` are exempt from the engine's usage boost.

use super::{Provider, ProviderOutput, QueryCtx, sort_items};
use crate::item::Item;

pub struct WorkflowProvider;

impl Provider for WorkflowProvider {
    fn id(&self) -> &'static str {
        "workflow"
    }

    fn learns(&self) -> bool {
        false
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let st = ctx.engine_state;
        let mut items: Vec<Item> = Vec::new();
        let mut exclusive = false;
        for wf in st.workflows.read().unwrap().iter() {
            let (found, ex) = wf.keyword_items(ctx.query, st.paths);
            exclusive |= ex;
            items.extend(found);
        }
        if exclusive {
            sort_items(&mut items);
            items.truncate(st.config.max_results);
        }
        ProviderOutput { items, exclusive }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use serde_json::json;

    fn install(e: &crate::engine::Engine, keyword: &str, title: &str, with_space: bool) {
        let dir = e.paths.workflows_dir().join("user.workflow.t");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({
            "name": "T", "bundleid": "t.t", "objects": [
                {"uid": "K", "type": "alfred.workflow.input.keyword", "config": {
                    "keyword": keyword, "withspace": with_space, "argumenttype": 0, "text": title, "subtext": "s"}},
                {"uid": "L", "type": "alfred.workflow.output.largetype", "config": {"largetypetext": "x"}}
            ],
            "connections": {"K": [{"destinationuid": "L", "modifiers": 0}]}
        });
        let xml = to_plist(&doc);
        xml.to_file_xml(dir.join("info.plist")).unwrap();
        e.reload();
    }

    fn to_plist(j: &serde_json::Value) -> plist::Value {
        use serde_json::Value as J;
        match j {
            J::Null => plist::Value::String(String::new()),
            J::Bool(b) => plist::Value::Boolean(*b),
            J::Number(n) => n
                .as_i64()
                .map(plist::Value::from)
                .unwrap_or_else(|| plist::Value::Real(n.as_f64().unwrap())),
            J::String(s) => plist::Value::String(s.clone()),
            J::Array(a) => plist::Value::Array(a.iter().map(to_plist).collect()),
            J::Object(o) => plist::Value::Dictionary(o.iter().map(|(k, v)| (k.clone(), to_plist(v))).collect()),
        }
    }

    #[test]
    fn typed_keyword_is_exclusive_and_beats_builtin_prefixes() {
        let (e, root) = engine("wf-excl");
        // "g" is also a built-in web keyword; the workflow must win.
        install(&e, "g", "Workflow G", true);
        let r = e.query("g rust");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Workflow G");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_keyword_joins_general_results() {
        let (e, root) = engine("wf-partial");
        install(&e, "zebra", "Zebra Workflow", true);
        let r = e.query("zeb");
        assert!(r.iter().any(|i| i.title == "Zebra Workflow"));
        // not exclusive: fallbacks still close the list
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }
}
