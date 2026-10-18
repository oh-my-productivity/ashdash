//! Workflow keyword triggers (workflows from existing workflow packages).
//!
//! First in the registry: a fully typed workflow keyword takes over the list
//! (exclusive, sorted and truncated here because the engine does not touch
//! exclusive output). Otherwise workflow items join the general pool; ids
//! starting with `wf` are exempt from the engine's usage boost.

use std::sync::Arc;

use super::{Provider, ProviderOutput, QueryCtx, sort_items};
use crate::actions::Os;
use crate::item::Item;

/// File Filter objects run `mdfind` through `os` with the configured file-search budget.
pub struct WorkflowProvider {
    os: Arc<dyn Os>,
}

impl WorkflowProvider {
    pub fn new(os: Arc<dyn Os>) -> WorkflowProvider {
        WorkflowProvider { os }
    }
}

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
        let budget = st.config.file_search.budget();
        for wf in st.workflows.read().unwrap().iter() {
            let (found, ex) = wf.keyword_items_os(ctx.query, st.paths, &*self.os, &budget);
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
    use crate::format_ids::kind;
    use serde_json::json;

    fn install(e: &crate::engine::Engine, keyword: &str, title: &str, with_space: bool) {
        let dir = e.paths.workflows_dir().join("user.workflow.t");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({
            "name": "T", "bundleid": "t.t", "objects": [
                {"uid": "K", "type": kind::INPUT_KEYWORD, "config": {
                    "keyword": keyword, "withspace": with_space, "argumenttype": 0, "text": title, "subtext": "s"}},
                {"uid": "L", "type": kind::OUTPUT_LARGETYPE, "config": {"largetypetext": "x"}}
            ],
            "connections": {"K": [{"destinationuid": "L", "modifiers": 0}]}
        });
        let xml = to_plist(&doc);
        xml.to_file_xml(dir.join("info.plist")).unwrap();
        e.reload_blocking();
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

    #[test]
    fn a_file_filter_runs_through_the_engines_os_with_the_configured_budget() {
        use crate::actions::testing::FakeOs;
        use crate::config::{Config, Paths};
        use std::sync::Arc;
        let root = std::env::temp_dir().join(format!("oa-wf-ffos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fake = Arc::new(FakeOs::with_output("/Users/x/a.txt\n"));
        let mut config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        config.file_search.mdfind_timeout_ms = 1500;
        let e = crate::engine::Engine::with_env(Paths::under(&root), config, Box::new(Arc::clone(&fake)), root.clone());
        let dir = e.paths.workflows_dir().join("user.workflow.ff");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({"name": "FF", "bundleid": "ff.t", "objects": [
            {"uid": "F", "type": kind::INPUT_FILEFILTER, "config": {
                "keyword": "ffx", "withspace": true, "argumenttype": 1, "title": "Find",
                "types": ["public.item"], "scopes": [], "fields": []}}], "connections": {}});
        to_plist(&doc).to_file_xml(dir.join("info.plist")).unwrap();
        e.reload_blocking();
        let r = e.query("ffx ");
        assert!(r.iter().any(|i| i.title == "a.txt"), "{r:?}");
        let cmds = fake.cmds();
        assert!(cmds.iter().any(|c| c.program == "mdfind"), "{cmds:?}");
        let b = fake.budgets.lock().unwrap();
        assert_eq!(b[0].timeout, std::time::Duration::from_millis(1500));
        let _ = std::fs::remove_dir_all(root);
    }
}
