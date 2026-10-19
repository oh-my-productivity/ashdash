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
        let workflows = st.workflows.read().unwrap();
        // Workflows that start a process (a Script Filter, a Spotlight search) run side by side: two matching filters cost the
        // time of the slower one, not the sum. Everything else is answered from memory, in order, on this thread.
        let slow: Vec<bool> = workflows
            .iter()
            .map(|wf| wf.would_run_process(ctx.query, st.paths))
            .collect();
        let token = crate::cancel::current();
        let mut answers: Vec<Option<(Vec<Item>, bool)>> = vec![None; workflows.len()];
        std::thread::scope(|scope| {
            let mut running = Vec::new();
            for (i, wf) in workflows.iter().enumerate().filter(|(i, _)| slow[*i]) {
                let (os, budget, token) = (&*self.os, &budget, token.clone());
                let run = move || wf.keyword_items_os(ctx.query, st.paths, os, budget);
                running.push((
                    i,
                    scope.spawn(move || {
                        // the query thread re-raises a panic of this one (below), where it is handled
                        crate::recovery::mark_thread();
                        match token {
                            Some(t) => crate::cancel::scope(&t, run),
                            None => run(),
                        }
                    }),
                ));
            }
            for (i, wf) in workflows.iter().enumerate().filter(|(i, _)| !slow[*i]) {
                answers[i] = Some(wf.keyword_items_os(ctx.query, st.paths, &*self.os, &budget));
            }
            for (i, handle) in running {
                // a panic in the helper surfaces on the query thread, as it did when the workflow ran there
                answers[i] = Some(handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)));
            }
        });
        for (found, ex) in answers.into_iter().flatten() {
            exclusive |= ex;
            items.extend(found);
        }
        if exclusive {
            sort_items(&mut items);
            items.truncate(st.config.max_results);
        }
        ProviderOutput { items, exclusive }
    }

    fn is_slow(&self, ctx: &QueryCtx) -> bool {
        let st = ctx.engine_state;
        st.workflows
            .read()
            .unwrap()
            .iter()
            .any(|wf| wf.would_run_process(ctx.query, st.paths))
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

    /// A Script Filter workflow: `keyword`, argument optional, bash `script`.
    fn install_script_filter(e: &crate::engine::Engine, keyword: &str, with_space: bool, script: &str) {
        let dir = e.paths.workflows_dir().join("user.workflow.sf");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({
            "name": "SF", "bundleid": "t.sf", "objects": [
                {"uid": "S", "type": kind::INPUT_SCRIPTFILTER, "config": {
                    "keyword": keyword, "withspace": with_space, "argumenttype": 1, "type": 0, "scriptargtype": 1,
                    "script": script, "title": "sf", "subtext": "s"}}
            ],
            "connections": {}
        });
        to_plist(&doc).to_file_xml(dir.join("info.plist")).unwrap();
        e.reload_blocking();
    }

    #[test]
    fn only_a_fully_typed_script_filter_keyword_with_its_argument_is_slow() {
        let (e, root) = engine("wf-slow");
        install_script_filter(&e, "sf", true, "echo '{\"items\":[]}'");
        assert!(e.query_is_slow("sf rust"), "keyword and argument: a script would start");
        assert!(!e.query_is_slow("sf"), "the keyword alone shows the placeholder row");
        assert!(!e.query_is_slow("s"), "a partial keyword shows the placeholder row");
        assert!(!e.query_is_slow("pup"), "no keyword matches");
        assert!(!e.query_is_slow("   "), "a blank query asks nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_keyword_without_a_space_starts_a_script_on_every_character() {
        let (e, root) = engine("wf-nospace");
        install_script_filter(&e, "p", false, "echo '{\"items\":[]}'");
        // typing "pup" against the keyword "p": all three keystrokes would start the script
        for q in ["p", "pu", "pup"] {
            assert!(e.query_is_slow(q), "{q}");
        }
        assert!(!e.query_is_slow("x"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_script_filter_answered_from_its_live_cache_is_not_slow() {
        let (e, root) = engine("wf-cached");
        install_script_filter(
            &e,
            "sf",
            true,
            "echo '{\"cache\":{\"seconds\":60},\"items\":[{\"title\":\"A\"}]}'",
        );
        assert!(e.query_is_slow("sf a"));
        assert!(e.query("sf a").iter().any(|i| i.title == "A"));
        assert!(
            !e.query_is_slow("sf a"),
            "the script's own cache answers the same query"
        );
        assert!(e.query_is_slow("sf b"), "another argument is another script run");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_search_keywords_are_slow_once_they_have_something_to_search_for() {
        let (e, root) = engine("files-slow");
        assert!(e.query_is_slow("open report"));
        assert!(e.query_is_slow("in budget"));
        assert!(!e.query_is_slow("open"), "the hint row");
        assert!(!e.query_is_slow("opens"), "an ordinary search");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn script_filters_that_match_the_same_query_run_side_by_side() {
        let (e, root) = engine("wf-parallel");
        // each script waits (up to 5 s) for the other one's marker: they only meet when both run at the same time
        let script = |mine: &str, other: &str| {
            let dir = root.display();
            format!(
                "touch '{dir}/{mine}'; for i in $(seq 1 100); do [ -e '{dir}/{other}' ] && break; sleep 0.05; done; \
                 if [ -e '{dir}/{other}' ]; then echo '{{\"items\":[{{\"title\":\"met-{mine}\"}}]}}'; \
                 else echo '{{\"items\":[{{\"title\":\"alone-{mine}\"}}]}}'; fi"
            )
        };
        for (name, mine, other) in [("a", "ma", "mb"), ("b", "mb", "ma")] {
            let dir = e.paths.workflows_dir().join(format!("user.workflow.{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            let doc = json!({
                "name": name, "bundleid": format!("t.{name}"), "objects": [
                    {"uid": name.to_uppercase(), "type": kind::INPUT_SCRIPTFILTER, "config": {
                        "keyword": "sf", "withspace": true, "argumenttype": 1, "type": 0, "scriptargtype": 1,
                        "script": script(mine, other), "title": "sf", "subtext": "s"}}
                ],
                "connections": {}
            });
            to_plist(&doc).to_file_xml(dir.join("info.plist")).unwrap();
        }
        e.reload_blocking();
        let mut titles: Vec<String> = e.query("sf x").into_iter().map(|i| i.title).collect();
        titles.sort();
        assert_eq!(titles, ["met-ma", "met-mb"], "the two scripts did not overlap in time");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cancelled_script_filter_is_killed_instead_of_waited_for() {
        use crate::cancel::CancelToken;
        let (e, root) = engine("wf-cancel");
        let (started, finished) = (root.join("started"), root.join("finished"));
        let script = format!(
            "touch '{}'; sleep 20; touch '{}'; echo '{{\"items\":[]}}'",
            started.display(),
            finished.display()
        );
        install_script_filter(&e, "sf", true, &script);
        let token = CancelToken::new();
        let canceller = {
            let (t, started) = (token.clone(), started.clone());
            std::thread::spawn(move || {
                // cancel while the script is running
                while !started.exists() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                t.cancel();
            })
        };
        // returns when the script is killed; a query that waited for the 20 s script would have written `finished`
        let items = e.query_cancellable("sf rust", &token);
        canceller.join().unwrap();
        assert!(started.exists(), "it must have been running when cancelled");
        assert!(items.iter().all(|i| !i.title.is_empty()));
        assert!(!finished.exists(), "the script must not run on to its end");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_query_cancelled_before_it_starts_asks_no_provider() {
        use crate::cancel::CancelToken;
        let (e, root) = engine("wf-precancel");
        let token = CancelToken::new();
        token.cancel();
        assert!(e.query_cancellable("saf", &token).is_empty());
        assert!(!e.query("saf").is_empty(), "the same query without a token is answered");
        let _ = std::fs::remove_dir_all(root);
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
