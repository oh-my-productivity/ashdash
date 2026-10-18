//! AppleScript / JXA bridge: `crates/app/resources/Ashdash.sdef` names one
//! Cocoa class, [`AshdashScriptCommand`], for all of its commands. Cocoa
//! Scripting instantiates it on the main thread; it translates the command
//! into the same [`ipc::Request`] the CLI socket uses and answers with the
//! [`ipc::Handler`] registered by [`install`].
//!
//! The commands only exist when the app runs from a bundle whose Info.plist
//! has `NSAppleScriptEnabled` and `OSAScriptingDefinition = Ashdash.sdef`
//! (packaging, WP10). `run trigger` waits at most [`TRIGGER_WAIT`] so a slow
//! workflow cannot freeze the main thread; after that the script receives an
//! empty result and the workflow keeps running.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, define_class, msg_send};
use objc2_foundation::{NSScriptCommand, NSString};

use crate::ipc::{Handler, MAX_SEARCH, Request, Response};
use crate::urlscheme::MAX_ARG;

/// How long a script command waits for a trigger to finish.
pub const TRIGGER_WAIT: Duration = Duration::from_millis(1500);

/// `errAEEventFailed`-style generic failure reported to the script.
const SCRIPT_ERROR: isize = -10000;

static HANDLER: OnceLock<Arc<dyn Handler>> = OnceLock::new();

/// Register the handler and make sure the Cocoa class exists before Cocoa
/// Scripting looks it up by name while parsing the sdef.
pub fn install(handler: Arc<dyn Handler>) {
    let _ = HANDLER.set(handler);
    let _ = AshdashScriptCommand::class();
}

/// What a script command asks for, independent of Cocoa. `None` for an
/// unknown command name or missing parameters (with a message).
pub fn request_for(
    name: &str,
    direct: Option<&str>,
    bundle: Option<&str>,
    argument: Option<&str>,
) -> Result<Request, String> {
    let need = |v: Option<&str>, what: &str| v.map(str::to_string).ok_or_else(|| format!("missing {what}"));
    match name {
        "show" => Ok(Request::Show),
        "reload" => Ok(Request::Reload),
        "search" => {
            // the same cap as the CLI socket; AppleScript is trusted by TCC, not unbounded
            let text = need(direct, "search text")?;
            if text.len() > MAX_SEARCH {
                return Err(format!("search text longer than {MAX_SEARCH} bytes"));
            }
            Ok(Request::Search { text })
        }
        "run trigger" => {
            let bundle = need(bundle, "workflow (in workflow ...)")?;
            let id = need(direct, "trigger id")?;
            if bundle.is_empty() || id.is_empty() {
                return Err("'run trigger' needs a workflow bundle id and a trigger id".into());
            }
            // the same cap as an `ashdash://trigger` argument
            let arg = argument.unwrap_or("").to_string();
            if arg.len() > MAX_ARG {
                return Err(format!("argument longer than {MAX_ARG} bytes"));
            }
            Ok(Request::Trigger { bundle, id, arg })
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

/// Run `req` on `handler`; triggers are given at most `wait` to answer.
pub fn run_request(handler: &Arc<dyn Handler>, req: Request, wait: Duration) -> Response {
    if !matches!(req, Request::Trigger { .. }) {
        return handler.handle(req);
    }
    let (tx, rx) = channel();
    let h = Arc::clone(handler);
    std::thread::spawn(move || {
        let _ = tx.send(h.handle(req));
    });
    rx.recv_timeout(wait).unwrap_or_else(|_| Response::ok(""))
}

fn string_of(obj: Option<Retained<AnyObject>>) -> Option<String> {
    obj?.downcast_ref::<NSString>().map(|s| s.to_string())
}

define_class!(
    #[unsafe(super(NSScriptCommand))]
    #[name = "AshdashScriptCommand"]
    struct AshdashScriptCommand;

    impl AshdashScriptCommand {
        #[unsafe(method(performDefaultImplementation))]
        fn perform(&self) -> *mut AnyObject {
            let name = self.commandDescription().commandName().to_string();
            let direct = string_of(self.directParameter());
            let args = self.evaluatedArguments();
            let arg = |k: &str| {
                args.as_ref()
                    .and_then(|d| d.objectForKey(&NSString::from_str(k)))
                    .and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()))
            };
            let fail = |msg: &str| {
                self.setScriptErrorNumber(SCRIPT_ERROR);
                self.setScriptErrorString(Some(&NSString::from_str(msg)));
                std::ptr::null_mut()
            };
            let Some(handler) = HANDLER.get() else {
                return fail("Ashdash is not ready");
            };
            let req = match request_for(&name, direct.as_deref(), arg("bundle").as_deref(), arg("argument").as_deref()) {
                Ok(r) => r,
                Err(e) => return fail(&e),
            };
            let resp = run_request(handler, req, TRIGGER_WAIT);
            if !resp.ok {
                return fail(&resp.error);
            }
            if resp.message.is_empty() {
                return std::ptr::null_mut();
            }
            let s = NSString::from_str(&resp.message);
            let obj: Retained<AnyObject> = unsafe { msg_send![&*s, self] };
            Retained::autorelease_ptr(obj)
        }
    }
);

// Directory of the sdef source, for tests and packaging scripts.
#[allow(dead_code)]
pub fn sdef_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Ashdash.sdef")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn commands_map_to_requests() {
        assert_eq!(request_for("show", None, None, None), Ok(Request::Show));
        assert_eq!(request_for("reload", None, None, None), Ok(Request::Reload));
        assert_eq!(
            request_for("search", Some("saf"), None, None),
            Ok(Request::Search { text: "saf".into() })
        );
        assert_eq!(
            request_for("run trigger", Some("go"), Some("b.c"), Some("x")),
            Ok(Request::Trigger {
                bundle: "b.c".into(),
                id: "go".into(),
                arg: "x".into()
            })
        );
        assert_eq!(
            request_for("run trigger", Some("go"), Some("b.c"), None),
            Ok(Request::Trigger {
                bundle: "b.c".into(),
                id: "go".into(),
                arg: "".into()
            })
        );
        assert!(request_for("search", None, None, None).is_err());
        assert!(request_for("run trigger", Some("go"), None, None).is_err());
        assert!(request_for("run trigger", None, Some("b"), None).is_err());
        assert!(request_for("quit-everything", None, None, None).is_err());
    }

    #[test]
    fn script_commands_apply_the_same_length_caps_as_the_other_entry_points() {
        let ok_search = "a".repeat(MAX_SEARCH);
        assert!(request_for("search", Some(&ok_search), None, None).is_ok());
        let long = "a".repeat(MAX_SEARCH + 1);
        assert!(
            request_for("search", Some(&long), None, None)
                .unwrap_err()
                .contains("longer")
        );

        let ok_arg = "a".repeat(MAX_ARG);
        assert!(request_for("run trigger", Some("go"), Some("b"), Some(&ok_arg)).is_ok());
        let long = "a".repeat(MAX_ARG + 1);
        assert!(
            request_for("run trigger", Some("go"), Some("b"), Some(&long))
                .unwrap_err()
                .contains("longer")
        );
        // multibyte text is counted in bytes, like the socket does
        let wide = "é".repeat(MAX_ARG / 2 + 1);
        assert!(request_for("run trigger", Some("go"), Some("b"), Some(&wide)).is_err());
        // empty ids are refused here as on the socket
        assert!(request_for("run trigger", Some(""), Some("b"), None).is_err());
        assert!(request_for("run trigger", Some("go"), Some(""), None).is_err());
    }

    struct Slow(Mutex<Vec<Request>>, Duration);
    impl Handler for Slow {
        fn handle(&self, req: Request) -> Response {
            std::thread::sleep(self.1);
            self.0.lock().unwrap().push(req);
            Response::ok("done")
        }
    }

    #[test]
    fn slow_triggers_do_not_block_the_caller() {
        let slow: Arc<dyn Handler> = Arc::new(Slow(Mutex::new(vec![]), Duration::from_millis(400)));
        let trig = Request::Trigger {
            bundle: "b".into(),
            id: "t".into(),
            arg: "".into(),
        };
        let t = std::time::Instant::now();
        let r = run_request(&slow, trig.clone(), Duration::from_millis(50));
        assert!(t.elapsed() < Duration::from_millis(300));
        assert!(
            r.ok && r.message.is_empty(),
            "timed-out triggers report an empty result"
        );
        // Within the wait the real answer comes back.
        let r = run_request(&slow, trig, Duration::from_secs(3));
        assert_eq!(r.message, "done");
    }

    #[test]
    fn the_sdef_names_every_command_and_one_class() {
        let sdef = std::fs::read_to_string(sdef_path()).unwrap();
        for c in ["show", "search", "run trigger", "reload"] {
            assert!(sdef.contains(&format!("<command name=\"{c}\"")), "{c}");
            assert!(
                request_for(c, Some("x"), Some("y"), None).is_ok(),
                "{c} has a request mapping"
            );
        }
        assert_eq!(sdef.matches("<cocoa class=\"AshdashScriptCommand\"/>").count(), 4);
    }
}
