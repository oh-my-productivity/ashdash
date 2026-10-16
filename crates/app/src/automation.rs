//! Production wiring of the automation surface: the engine-backed
//! implementations of the traits in `ipc`, `urlscheme` and `hotkeys`, and the
//! one function the launcher process calls at startup.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use ashdash_core::theme::Theme;
use ashdash_core::workflow::{self, KIND_EXTERNAL_TRIGGER};
use ashdash_core::{Engine, Outcome};
use global_hotkey::hotkey::HotKey;

use crate::events::{self, Automation, EventSink};
use crate::hotkeys::{AppHost, WorkflowHotkeys};
use crate::ipc::{self, AppHandler, Backend};
use crate::urlscheme::{DialogPrompter, TrustStore, UrlDispatcher, UrlHost};

/// Engine-backed [`Backend`] and [`UrlHost`].
pub struct EngineHost {
    pub engine: Arc<Engine>,
}

impl Backend for EngineHost {
    fn run_trigger(&self, bundle: &str, id: &str, arg: &str) -> Outcome {
        self.engine.run_trigger(bundle, id, arg)
    }

    fn validate_theme(&self, file: &Path) -> Result<(), String> {
        Theme::load(file).map(|_| ()).map_err(|e| format!("{e:#}"))
    }

    fn clear_usage(&self) {
        self.engine.clear_usage();
    }
}

impl UrlHost for EngineHost {
    fn external_trigger(&self, bundle: &str, name: &str) -> Option<String> {
        let mut roots = vec![self.engine.paths.workflows_dir()];
        roots.extend(self.engine.config.workflow_dirs.iter().cloned());
        workflow::load_all(&roots)
            .into_iter()
            .find(|w| {
                !w.disabled
                    && w.bundle_id == bundle
                    && w.nodes.iter().any(|n| {
                        n.kind == KIND_EXTERNAL_TRIGGER
                            && n.config.get("triggerid").and_then(|v| v.as_str()) == Some(name)
                    })
            })
            .map(|w| w.name)
    }

    fn run_trigger(&self, bundle: &str, name: &str, arg: &str) -> Outcome {
        self.engine.run_trigger(bundle, name, arg)
    }
}

/// What the launcher process keeps alive for its whole lifetime.
pub struct Started {
    /// `None` when the control socket could not be created.
    pub _server: Option<ipc::Server>,
    pub automation: Automation,
}

/// Wire the control socket, URL handler and workflow hotkeys to `engine`.
/// `primary` is the already-acquired socket (see `ipc::acquire`).
pub fn start(engine: &Arc<Engine>, primary: Option<ipc::Primary>) -> Started {
    let (sender, inbox) = events::channel();
    let sink: Arc<dyn EventSink> = Arc::new(sender);
    let host = Arc::new(EngineHost {
        engine: Arc::clone(engine),
    });

    let handler: Arc<dyn ipc::Handler> = Arc::new(AppHandler {
        sink: Arc::clone(&sink),
        backend: host.clone(),
    });
    let server = primary.map(|p| ipc::serve(p, Arc::clone(&handler), ipc::current_uid()));
    crate::scripting::install(handler);

    let dispatcher = Arc::new(UrlDispatcher::new(
        Arc::clone(&sink),
        host,
        Arc::new(DialogPrompter),
        TrustStore::load(engine.paths.data.join("url-trust.json")),
        engine.paths.data.join("themes"),
    ));

    let hotkey_host = Arc::new(AppHost {
        engine: Arc::clone(engine),
        sink,
    });
    let main_hotkey = HotKey::from_str(&engine.config.hotkey).ok();
    let hotkeys = WorkflowHotkeys::new(hotkey_host, &engine.paths, &engine.config.workflow_dirs, main_hotkey);

    Started {
        _server: server,
        automation: Automation::new(inbox, hotkeys, Some(dispatcher)),
    }
}
