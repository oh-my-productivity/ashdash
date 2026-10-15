//! Executing a workflow graph when a result is actioned, plus `{query}` /
//! `{var:name}` template rendering.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::model::{str_map, stringify};
use super::script::run_node_script;
use super::{MAX_GRAPH_STEPS, SCRIPT_ACTION_TIMEOUT};
use super::{Node, Workflow, WorkflowAction};
use crate::config::Paths;
use crate::files::expand_tilde;
use crate::item::Mods;
use crate::websearch::percent_encode;

/// Side effects requested by a workflow run. The caller performs them.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    OpenUrl { url: String, browser: Option<String> },
    OpenPath { path: PathBuf, with: Option<String> },
    Reveal(PathBuf),
    Copy(String),
    Notify { title: String, text: String },
    LargeType(String),
    Terminal(String),
    Warn(String),
}

// ---------------------------------------------------------------- templating

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{(query|var:[^}]+)\}").expect("valid regex"))
}

/// Substitute `{query}` and `{var:name}` in a single pass (substituted text is
/// never re-expanded).
pub fn render(template: &str, query: &str, vars: &BTreeMap<String, String>) -> String {
    template_re()
        .replace_all(template, |c: &regex::Captures| {
            let key = &c[1];
            if key == "query" {
                query.to_string()
            } else {
                vars.get(&key[4..]).cloned().unwrap_or_default()
            }
        })
        .into_owned()
}

#[derive(Clone)]
struct State {
    arg: String,
    vars: BTreeMap<String, String>,
}

impl Workflow {
    /// Walk the graph from the object that produced the result.
    pub fn run(&self, act: &WorkflowAction, pressed: Mods, paths: &Paths) -> Vec<Effect> {
        let mut state = State {
            arg: act.arg.clone(),
            vars: self.variables.clone(),
        };
        state.vars.extend(act.vars.clone());
        if let Some((_, ov)) = act.mods.iter().find(|(m, _)| *m == pressed) {
            if ov.valid == Some(false) {
                return vec![];
            }
            if let Some(a) = &ov.arg {
                state.arg = a.clone();
            }
            state.vars.extend(ov.vars.clone());
        }
        let mut effects = Vec::new();
        let mut budget = MAX_GRAPH_STEPS;
        self.follow(&act.from_uid, pressed.flags(), &state, paths, &mut effects, &mut budget);
        effects
    }

    fn follow(&self, from: &str, flags: u64, state: &State, paths: &Paths, fx: &mut Vec<Effect>, budget: &mut usize) {
        for conn in self.connections.get(from).into_iter().flatten() {
            if conn.modifiers != flags {
                continue;
            }
            if *budget == 0 {
                fx.push(Effect::Warn("workflow graph too deep or cyclic; stopped".into()));
                return;
            }
            *budget -= 1;
            if let Some(node) = self.node(&conn.dest) {
                self.process(node, state.clone(), paths, fx, budget);
            }
        }
    }

    fn process(&self, node: &Node, mut st: State, paths: &Paths, fx: &mut Vec<Effect>, budget: &mut usize) {
        let r = |t: &str| render(t, &st.arg, &st.vars);
        match node.kind.as_str() {
            "alfred.workflow.action.script" => {
                match run_node_script(node, self, paths, &st.arg, &st.vars, SCRIPT_ACTION_TIMEOUT) {
                    Ok(out) => {
                        if !out.success {
                            fx.push(Effect::Warn(format!(
                                "{}: script exited with an error: {}",
                                self.name,
                                out.stderr.trim()
                            )));
                        }
                        apply_script_output(&mut st, &out.stdout);
                    }
                    Err(e) => {
                        fx.push(Effect::Warn(format!("{}: {e:#}", self.name)));
                        return;
                    }
                }
            }
            "alfred.workflow.action.openurl" => {
                let utf8 = node.b("utf8");
                let spaces = node.s("spaces");
                let q = if utf8 { percent_encode(&st.arg) } else { st.arg.clone() };
                let q = if spaces.is_empty() {
                    q
                } else {
                    q.replace("%20", &spaces)
                };
                let url = render(&node.s("url"), &q, &st.vars);
                let browser = Some(node.s("browser")).filter(|b| !b.is_empty());
                fx.push(Effect::OpenUrl { url, browser });
            }
            "alfred.workflow.action.openfile" => {
                let path = expand_tilde(&r(&node.s("sourcefile")));
                let with = Some(node.s("openwith")).filter(|w| !w.is_empty());
                fx.push(Effect::OpenPath { path, with });
            }
            "alfred.workflow.action.launchfiles" => {
                for p in node.config.get("paths").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(s) = p.as_str() {
                        fx.push(Effect::OpenPath {
                            path: expand_tilde(&r(s)),
                            with: None,
                        });
                    }
                }
            }
            "alfred.workflow.action.revealfile" => {
                fx.push(Effect::Reveal(expand_tilde(&st.arg)));
            }
            "alfred.workflow.action.terminalcommand" => {
                fx.push(Effect::Terminal(r(&node.s("text"))));
            }
            "alfred.workflow.output.notification" => {
                fx.push(Effect::Notify {
                    title: r(&node.s("title")),
                    text: r(&node.s("text")),
                });
            }
            "alfred.workflow.output.clipboard" => {
                fx.push(Effect::Copy(r(&node.s("clipboardtext"))));
            }
            "alfred.workflow.output.largetype" => {
                fx.push(Effect::LargeType(r(&node.s("largetypetext"))));
            }
            "alfred.workflow.utility.argument" => {
                let a = r(&node.s("argument"));
                if !a.is_empty() {
                    st.arg = a;
                }
                let current = st.clone();
                for (k, v) in node
                    .config
                    .get("variables")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    st.vars
                        .insert(k.clone(), render(&stringify(v), &current.arg, &current.vars));
                }
            }
            "alfred.workflow.utility.junction" => {}
            other => {
                fx.push(Effect::Warn(format!(
                    "{}: unsupported workflow object '{other}'",
                    self.name
                )));
                return;
            }
        }
        self.follow(&node.uid, 0, &st, paths, fx, budget);
    }
}

/// A Run Script object may print plain text (which becomes the next
/// argument) or the reference launcher's `{"alfredworkflow": {"arg": ..., "variables": {...}}}`.
fn apply_script_output(st: &mut State, stdout: &str) {
    let trimmed = stdout.trim_end_matches(['\n', '\r']);
    if trimmed.trim_start().starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && let Some(w) = v.get("alfredworkflow")
    {
        if let Some(a) = w.get("arg") {
            st.arg = stringify(a);
        }
        st.vars.extend(str_map(w.get("variables")));
        return;
    }
    st.arg = trimmed.to_string();
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::item::Action;
    use serde_json::json;

    #[test]
    fn render_templates_once() {
        let mut v = BTreeMap::new();
        v.insert("a".to_string(), "{query}".to_string());
        assert_eq!(render("x {query} {var:a} {var:missing}.", "Q", &v), "x Q {query} .");
    }

    #[test]
    fn script_filter_to_notification_chain() {
        let (paths, root) = temp_paths("chain");
        let wf = sample(&root);
        let (items, exclusive) = wf.keyword_items("demo hello", &paths);
        assert!(exclusive);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "one: hello");
        assert!(!items[1].valid);
        assert!(items[0].score > items[1].score);

        let Action::Workflow(act) = &items[0].action else {
            panic!()
        };
        let fx = wf.run(act, Mods::NONE, &paths);
        assert_eq!(
            fx,
            vec![Effect::Notify {
                title: "T".into(),
                text: "got:A hello:hi/hi".into()
            }]
        );

        // cmd modifier: override arg and follow the cmd-only connection
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        let fx = wf.run(act, cmd, &paths);
        assert_eq!(
            fx,
            vec![Effect::OpenUrl {
                url: "https://x.test/?q=CMD".into(),
                browser: None
            }]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_json_output_sets_vars() {
        let mut st = State {
            arg: "x".into(),
            vars: BTreeMap::new(),
        };
        apply_script_output(&mut st, r#"{"alfredworkflow":{"arg":"new","variables":{"k":1}}}"#);
        assert_eq!(st.arg, "new");
        assert_eq!(st.vars["k"], "1");
        apply_script_output(&mut st, "plain\n");
        assert_eq!(st.arg, "plain");
    }

    #[test]
    fn cycle_is_bounded() {
        let (paths, root) = temp_paths("cycle");
        let doc = json!({
            "name": "C", "bundleid": "c",
            "objects": [
                {"uid": "A", "type": "alfred.workflow.utility.junction", "config": {}},
                {"uid": "B", "type": "alfred.workflow.utility.junction", "config": {}},
            ],
            "connections": {"A": [{"destinationuid": "B", "modifiers": 0}], "B": [{"destinationuid": "A", "modifiers": 0}]}
        });
        let wf = Workflow::from_json(&root, "c", &doc).unwrap();
        let act = WorkflowAction {
            workflow: "c".into(),
            from_uid: "A".into(),
            arg: String::new(),
            vars: BTreeMap::new(),
            mods: vec![],
        };
        let fx = wf.run(&act, Mods::NONE, &paths);
        assert!(matches!(fx.last(), Some(Effect::Warn(_))));
        let _ = std::fs::remove_dir_all(root);
    }
}
