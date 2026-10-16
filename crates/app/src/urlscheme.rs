//! The `ashdash://` URL scheme.
//!
//! macOS hands URLs to the running app as `kAEGetURL` Apple Events
//! ([`install_apple_event_handler`]). A URL is **untrusted input**: any web
//! page can fire one. Everything therefore passes through, in order:
//!
//! 1. a rate limiter ([`RateLimiter`]);
//! 2. a strict, allow-listed parser ([`parse`]): fixed verbs, fixed parameter
//!    names, no userinfo/port/path, no repeated keys, percent-decoding exactly
//!    once, UTF-8 only, length caps, no control characters;
//! 3. verb-specific policy ([`UrlDispatcher::dispatch`]): `trigger` only fires
//!    an *External Trigger* object of an enabled workflow (the workflow author
//!    opted in by placing the object) and only after the user consented once
//!    per bundle id ([`TrustStore`], `<data>/url-trust.json`); `theme` only
//!    loads files from `<data>/themes/` by plain name.
//!
//! There is deliberately no verb that imports, runs scripts, reads the
//! clipboard, opens files or runs system commands (docs/SECURITY.md section 6).
//! `search` only *fills the field* ([`AppEvent::Prefill`]): a query that matches a workflow
//! keyword would otherwise start that workflow's Script Filter on text chosen by a web page,
//! so nothing is searched until the user edits the text.
//! Invocations are logged without arguments.
//!
//! Only `ashdash://` is handled. `reference://` belongs to a real reference launcher install
//! and is rejected here.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ashdash_core::Outcome;
use ashdash_core::brand::URL_SCHEME;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

use crate::events::{AppEvent, EventSink};

pub const MAX_QUERY: usize = 1024;
pub const MAX_ARG: usize = 4096;
const MAX_NAME: usize = 128;
const MAX_URL: usize = 16 * 1024;

// ------------------------------------------------------------------ parsing

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlCommand {
    Show,
    Search {
        query: String,
    },
    Trigger {
        bundle: String,
        name: String,
        arg: String,
    },
    Reload,
    /// Open the preferences window.
    Prefs,
    /// A theme file name inside the themes directory (no path separators).
    Theme {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    WrongScheme,
    TooLong,
    ControlCharacter,
    /// userinfo, port, extra path segments or other structure we never accept.
    BadStructure(&'static str),
    UnknownVerb(String),
    UnknownParameter(String),
    DuplicateParameter(String),
    MissingParameter(&'static str),
    BadEscape,
    InvalidUtf8,
    ValueTooLong(&'static str),
    BadValue(&'static str),
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use UrlError::*;
        match self {
            WrongScheme => write!(f, "not an {URL_SCHEME}:// URL"),
            TooLong => write!(f, "URL too long"),
            ControlCharacter => write!(f, "control character in URL"),
            BadStructure(w) => write!(f, "unsupported URL structure: {w}"),
            UnknownVerb(v) => write!(f, "unknown action '{v}'"),
            UnknownParameter(p) => write!(f, "unknown parameter '{p}'"),
            DuplicateParameter(p) => write!(f, "repeated parameter '{p}'"),
            MissingParameter(p) => write!(f, "missing parameter '{p}'"),
            BadEscape => write!(f, "invalid percent-escape"),
            InvalidUtf8 => write!(f, "parameter is not valid UTF-8"),
            ValueTooLong(p) => write!(f, "parameter '{p}' is too long"),
            BadValue(p) => write!(f, "parameter '{p}' has an invalid value"),
        }
    }
}

impl std::error::Error for UrlError {}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode exactly once. Every `%` must be followed by two hex digits;
/// `+` stays a plus. The result must be UTF-8.
fn decode(s: &str) -> Result<String, UrlError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let (Some(h), Some(l)) = (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) else {
                return Err(UrlError::BadEscape);
            };
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| UrlError::InvalidUtf8)
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Identifier-like values: bundle ids and trigger ids.
fn check_ident(v: String, what: &'static str) -> Result<String, UrlError> {
    if v.is_empty() || v.len() > MAX_NAME || has_control(&v) {
        return Err(UrlError::BadValue(what));
    }
    Ok(v)
}

/// Parse and validate an `ashdash://` URL.
pub fn parse(url: &str) -> Result<UrlCommand, UrlError> {
    if url.len() > MAX_URL {
        return Err(UrlError::TooLong);
    }
    if has_control(url) {
        return Err(UrlError::ControlCharacter);
    }
    let prefix = format!("{URL_SCHEME}://");
    // `get`, not a slice: byte `prefix.len()` may sit inside a multibyte character.
    if !url.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)) {
        return Err(UrlError::WrongScheme);
    }
    let rest = &url[prefix.len()..];
    // Fragments carry nothing for us.
    let rest = rest.split('#').next().unwrap_or("");
    let (verb_part, query) = match rest.split_once('?') {
        Some((v, q)) => (v, q),
        None => (rest, ""),
    };
    let verb = verb_part.strip_suffix('/').unwrap_or(verb_part);
    if verb.contains(['@', ':']) {
        return Err(UrlError::BadStructure("userinfo or port"));
    }
    if verb.contains('/') || verb.contains('%') {
        return Err(UrlError::BadStructure("path"));
    }
    let verb = verb.to_ascii_lowercase();

    let mut params: BTreeMap<String, String> = BTreeMap::new();
    if !query.is_empty() {
        for pair in query.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                return Err(UrlError::BadStructure("parameter without '='"));
            };
            let key = decode(k)?;
            let value = decode(v)?;
            if params.insert(key.clone(), value).is_some() {
                return Err(UrlError::DuplicateParameter(key));
            }
        }
    }
    let allowed: &[&str] = match verb.as_str() {
        "show" | "reload" | "prefs" => &[],
        "search" => &["query"],
        "trigger" => &["bundle", "name", "arg"],
        "theme" => &["name"],
        _ => return Err(UrlError::UnknownVerb(verb)),
    };
    if let Some(k) = params.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(UrlError::UnknownParameter(k.clone()));
    }
    let take =
        |params: &mut BTreeMap<String, String>, k: &'static str| params.remove(k).ok_or(UrlError::MissingParameter(k));

    match verb.as_str() {
        "show" => Ok(UrlCommand::Show),
        "reload" => Ok(UrlCommand::Reload),
        "prefs" => Ok(UrlCommand::Prefs),
        "search" => {
            let query = take(&mut params, "query")?;
            if query.len() > MAX_QUERY {
                return Err(UrlError::ValueTooLong("query"));
            }
            if has_control(&query) {
                return Err(UrlError::BadValue("query"));
            }
            Ok(UrlCommand::Search { query })
        }
        "trigger" => {
            let bundle = check_ident(take(&mut params, "bundle")?, "bundle")?;
            let name = check_ident(take(&mut params, "name")?, "name")?;
            let arg = params.remove("arg").unwrap_or_default();
            if arg.len() > MAX_ARG {
                return Err(UrlError::ValueTooLong("arg"));
            }
            // Tabs and newlines are legitimate in text arguments; other controls are not.
            if arg.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                return Err(UrlError::BadValue("arg"));
            }
            Ok(UrlCommand::Trigger { bundle, name, arg })
        }
        "theme" => {
            let name = take(&mut params, "name")?;
            let ok = !name.is_empty()
                && name.len() <= 64
                && !name.starts_with('.')
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'));
            if !ok {
                return Err(UrlError::BadValue("name"));
            }
            Ok(UrlCommand::Theme { name })
        }
        _ => unreachable!("verb checked above"),
    }
}

// ------------------------------------------------------------------ rate limit

/// At most `max` events per `window` (sliding).
pub struct RateLimiter {
    max: usize,
    window: Duration,
    seen: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        RateLimiter {
            max,
            window,
            seen: VecDeque::new(),
        }
    }

    pub fn allow(&mut self, now: Instant) -> bool {
        while let Some(&t) = self.seen.front() {
            if now.saturating_duration_since(t) >= self.window {
                self.seen.pop_front();
            } else {
                break;
            }
        }
        if self.seen.len() >= self.max {
            return false;
        }
        self.seen.push_back(now);
        true
    }
}

// ------------------------------------------------------------------ consent

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// Remembered per-bundle-id consent for URL-triggered workflows.
#[derive(Debug)]
pub struct TrustStore {
    path: PathBuf,
    decisions: BTreeMap<String, Decision>,
}

impl TrustStore {
    pub fn load(path: PathBuf) -> Self {
        let mut decisions = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(map) = v.get("bundles").and_then(|b| b.as_object())
        {
            for (k, v) in map {
                match v.as_str() {
                    Some("allow") => decisions.insert(k.clone(), Decision::Allow),
                    Some("deny") => decisions.insert(k.clone(), Decision::Deny),
                    _ => None,
                };
            }
        }
        TrustStore { path, decisions }
    }

    pub fn get(&self, bundle: &str) -> Option<Decision> {
        self.decisions.get(bundle).copied()
    }

    /// Remember a decision and write the file (0600, replaced atomically).
    pub fn set(&mut self, bundle: &str, d: Decision) -> std::io::Result<()> {
        self.decisions.insert(bundle.to_string(), d);
        let bundles: serde_json::Map<String, serde_json::Value> = self
            .decisions
            .iter()
            .map(|(k, d)| {
                let s = if *d == Decision::Allow { "allow" } else { "deny" };
                (k.clone(), serde_json::Value::from(s))
            })
            .collect();
        let doc = serde_json::json!({"version": 1, "bundles": bundles});
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&doc)?.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path)
    }
}

// ------------------------------------------------------------------ dispatch

/// What the dispatcher needs from the workflow engine.
pub trait UrlHost: Send + Sync {
    /// The workflow's display name when an *enabled* workflow with this bundle
    /// id has an External Trigger object with this trigger id.
    fn external_trigger(&self, bundle: &str, name: &str) -> Option<String>;
    fn run_trigger(&self, bundle: &str, name: &str, arg: &str) -> Outcome;
}

/// Asks the user whether a workflow may be started from URLs.
pub trait Prompter: Send + Sync {
    fn confirm(&self, workflow_name: &str, bundle: &str) -> bool;
}

pub struct UrlDispatcher {
    sink: Arc<dyn EventSink>,
    host: Arc<dyn UrlHost>,
    prompter: Arc<dyn Prompter>,
    trust: Mutex<TrustStore>,
    limiter: Mutex<RateLimiter>,
    themes_dir: PathBuf,
}

impl UrlDispatcher {
    pub fn new(
        sink: Arc<dyn EventSink>,
        host: Arc<dyn UrlHost>,
        prompter: Arc<dyn Prompter>,
        trust: TrustStore,
        themes_dir: PathBuf,
    ) -> Self {
        UrlDispatcher {
            sink,
            host,
            prompter,
            trust: Mutex::new(trust),
            limiter: Mutex::new(RateLimiter::new(6, Duration::from_secs(10))),
            themes_dir,
        }
    }

    /// Entry point for the Apple Event handler (main thread): never blocks it.
    pub fn handle_async(self: &Arc<Self>, url: String) {
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(e) = this.dispatch(&url) {
                eprintln!("ashdash: ignored URL: {e}");
            }
        });
    }

    pub fn dispatch(&self, url: &str) -> Result<(), String> {
        self.dispatch_at(url, Instant::now())
    }

    pub fn dispatch_at(&self, url: &str, now: Instant) -> Result<(), String> {
        if !self.limiter.lock().unwrap().allow(now) {
            return Err("rate limited".into());
        }
        let cmd = parse(url).map_err(|e| e.to_string())?;
        match cmd {
            UrlCommand::Show => {
                log_invocation("show", "-");
                self.sink.emit(AppEvent::Show);
            }
            UrlCommand::Search { query } => {
                log_invocation("search", "-");
                // A web page can send this with one click: the text is put in the field but
                // nothing runs on it (no workflow keyword, no Script Filter) until the user edits
                // it. The CLI and AppleScript `search` are local and keep running the query.
                self.sink.emit(AppEvent::Prefill(query));
            }
            UrlCommand::Reload => {
                log_invocation("reload", "-");
                self.sink.emit(AppEvent::Reload);
            }
            UrlCommand::Prefs => {
                log_invocation("prefs", "-");
                self.sink.emit(AppEvent::Preferences);
            }
            UrlCommand::Theme { name } => {
                log_invocation("theme", &name);
                let file = self.theme_file(&name)?;
                self.sink.emit(AppEvent::Theme(file));
            }
            UrlCommand::Trigger { bundle, name, arg } => {
                log_invocation("trigger", &bundle);
                self.trigger(&bundle, &name, &arg)?;
            }
        }
        Ok(())
    }

    fn theme_file(&self, name: &str) -> Result<PathBuf, String> {
        let file = if name.ends_with(".alfredappearance") {
            name.to_string()
        } else {
            format!("{name}.alfredappearance")
        };
        let path = self.themes_dir.join(file);
        match fs::symlink_metadata(&path) {
            Ok(md) if md.is_file() => Ok(path),
            _ => Err(format!("no theme named '{name}' in the themes folder")),
        }
    }

    fn trigger(&self, bundle: &str, name: &str, arg: &str) -> Result<(), String> {
        let Some(workflow) = self.host.external_trigger(bundle, name) else {
            return Err(format!(
                "no enabled workflow '{bundle}' with an External Trigger '{name}'"
            ));
        };
        let known = self.trust.lock().unwrap().get(bundle);
        let allowed = match known {
            Some(Decision::Allow) => true,
            Some(Decision::Deny) => false,
            None => {
                // Asked outside the lock: the dialog can stay open for a minute.
                let yes = self.prompter.confirm(&workflow, bundle);
                let d = if yes { Decision::Allow } else { Decision::Deny };
                if let Err(e) = self.trust.lock().unwrap().set(bundle, d) {
                    eprintln!("ashdash: could not save URL consent: {e}");
                }
                yes
            }
        };
        if !allowed {
            return Err(format!("workflow '{bundle}' is not allowed to be started from URLs"));
        }
        let outcome = self.host.run_trigger(bundle, name, arg);
        if outcome != Outcome::Done {
            self.sink.emit(AppEvent::Outcome(outcome));
        }
        Ok(())
    }
}

/// Verb and workflow only; arguments can be private.
fn log_invocation(verb: &str, subject: &str) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("ashdash: url verb={verb} subject={subject} t={t}");
}

// ------------------------------------------------------------------ real prompter

/// Native dialog through `osascript`; the texts are passed as `argv`, never
/// spliced into the script.
pub struct DialogPrompter;

impl Prompter for DialogPrompter {
    fn confirm(&self, workflow_name: &str, bundle: &str) -> bool {
        let message = format!(
            "A link or app wants to run the workflow \"{workflow_name}\" ({bundle}) in Ashdash.\n\n\
             Workflows can run scripts, open files and URLs and change your data. Only allow this if you trust the workflow and where the link came from."
        );
        let out = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "set r to display dialog (item 1 of argv) with title \"Ashdash\" buttons {\"Don't Allow\", \"Allow\"} default button \"Don't Allow\" with icon caution giving up after 60",
                "-e",
                "return button returned of r",
                "-e",
                "end run",
                "--",
            ])
            .arg(message)
            .stdin(std::process::Stdio::null())
            .output();
        matches!(out, Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "Allow")
    }
}

// ------------------------------------------------------------------ Apple Events

/// Apple Event class and id of "get URL" (`'GURL'`, kInternetEventClass / kAEGetURL).
const GURL: u32 = u32::from_be_bytes(*b"GURL");
/// `keyDirectObject` (`'----'`).
const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

struct HandlerIvars {
    dispatcher: Arc<UrlDispatcher>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AshdashUrlHandler"]
    #[ivars = HandlerIvars]
    struct UrlTarget;

    impl UrlTarget {
        #[unsafe(method(handleGetURL:withReplyEvent:))]
        fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let direct: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
            let url = direct.and_then(|d| d.stringValue()).map(|s| s.to_string());
            if let Some(url) = url {
                self.ivars().dispatcher.handle_async(url);
            }
        }
    }
);

/// Keeps the Apple Event target alive.
pub struct AppleEventHandler {
    _target: Retained<UrlTarget>,
}

/// Register for `kAEGetURL` on the shared Apple Event manager. Must be called
/// after AppKit installed its own default handler (i.e. once the app has
/// launched), because registering replaces any earlier handler.
pub fn install_apple_event_handler(dispatcher: Arc<UrlDispatcher>) -> Option<AppleEventHandler> {
    let target: Retained<UrlTarget> = {
        let this = UrlTarget::alloc().set_ivars(HandlerIvars { dispatcher });
        unsafe { msg_send![super(this), init] }
    };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    unsafe {
        let _: () = msg_send![
            &*manager,
            setEventHandler: &*target,
            andSelector: sel!(handleGetURL:withReplyEvent:),
            forEventClass: GURL,
            andEventID: GURL
        ];
    }
    Some(AppleEventHandler { _target: target })
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::VecSink;

    fn q(s: &str) -> UrlCommand {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn parses_the_documented_forms() {
        assert_eq!(
            q("ashdash://search?query=saf"),
            UrlCommand::Search { query: "saf".into() }
        );
        assert_eq!(
            q("ashdash://trigger?bundle=com.x.y&name=go&arg=a%20b"),
            UrlCommand::Trigger {
                bundle: "com.x.y".into(),
                name: "go".into(),
                arg: "a b".into()
            }
        );
        assert_eq!(
            q("ashdash://trigger?name=go&bundle=b"),
            UrlCommand::Trigger {
                bundle: "b".into(),
                name: "go".into(),
                arg: "".into()
            }
        );
        assert_eq!(q("ashdash://reload"), UrlCommand::Reload);
        assert_eq!(q("ashdash://prefs"), UrlCommand::Prefs);
        assert_eq!(q("ashdash://show"), UrlCommand::Show);
        assert_eq!(
            q("ashdash://theme?name=Dark%20Mode"),
            UrlCommand::Theme {
                name: "Dark Mode".into()
            }
        );
        // Case-insensitive scheme/verb, trailing slash, ignored fragment.
        assert_eq!(q("ASHDASH://Reload/"), UrlCommand::Reload);
        assert_eq!(
            q("ashdash://search?query=x#frag"),
            UrlCommand::Search { query: "x".into() }
        );
    }

    #[test]
    fn decoding_happens_exactly_once_and_plus_stays_plus() {
        assert_eq!(
            q("ashdash://search?query=a%2Bb+c"),
            UrlCommand::Search { query: "a+b+c".into() }
        );
        // %2520 decodes once to the text "%20", not to a space.
        assert_eq!(
            q("ashdash://search?query=%2520"),
            UrlCommand::Search { query: "%20".into() }
        );
        assert_eq!(
            q("ashdash://search?query=%C3%A9%F0%9F%98%80"),
            UrlCommand::Search { query: "é😀".into() }
        );
        // Encoded separators stay inside the value.
        assert_eq!(
            q("ashdash://search?query=a%26b%3Dc"),
            UrlCommand::Search { query: "a&b=c".into() }
        );
    }

    #[test]
    fn rejects_hostile_or_malformed_urls() {
        use UrlError::*;
        let cases: Vec<(String, UrlError)> = vec![
            ("reference://search?query=x".into(), WrongScheme),
            ("ashdash:search?query=x".into(), WrongScheme),
            ("http://search?query=x".into(), WrongScheme),
            ("".into(), WrongScheme),
            ("ashdash://user@search?query=x".into(), BadStructure("userinfo or port")),
            ("ashdash://user:pw@search".into(), BadStructure("userinfo or port")),
            ("ashdash://search:80?query=x".into(), BadStructure("userinfo or port")),
            ("ashdash://search/extra?query=x".into(), BadStructure("path")),
            ("ashdash://%73earch?query=x".into(), BadStructure("path")),
            ("ashdash://search?query".into(), BadStructure("parameter without '='")),
            (
                "ashdash://search?query=a&&x=1".into(),
                BadStructure("parameter without '='"),
            ),
            ("ashdash://import?url=http://evil".into(), UnknownVerb("import".into())),
            ("ashdash://run?cmd=rm".into(), UnknownVerb("run".into())),
            ("ashdash://search".into(), MissingParameter("query")),
            ("ashdash://search?q=x".into(), UnknownParameter("q".into())),
            ("ashdash://reload?force=1".into(), UnknownParameter("force".into())),
            (
                "ashdash://search?query=a&query=b".into(),
                DuplicateParameter("query".into()),
            ),
            ("ashdash://search?query=%zz".into(), BadEscape),
            ("ashdash://search?query=%4".into(), BadEscape),
            ("ashdash://search?query=%ff%fe".into(), InvalidUtf8),
            ("ashdash://search?query=a%00b".into(), BadValue("query")),
            ("ashdash://search?query=a%0Ab".into(), BadValue("query")),
            ("ashdash://search?query=a\nb".into(), ControlCharacter),
            ("ashdash://search?query=x#a\u{7}b".into(), ControlCharacter),
            (
                format!("ashdash://search?query={}", "a".repeat(MAX_QUERY + 1)),
                ValueTooLong("query"),
            ),
            ("ashdash://trigger?bundle=b".into(), MissingParameter("name")),
            ("ashdash://trigger?bundle=&name=x".into(), BadValue("bundle")),
            ("ashdash://trigger?bundle=b&name=x&arg=%07".into(), BadValue("arg")),
            (
                format!("ashdash://trigger?bundle=b&name=x&arg={}", "a".repeat(MAX_ARG + 1)),
                ValueTooLong("arg"),
            ),
            ("ashdash://theme?name=..%2F..%2Fetc%2Fpasswd".into(), BadValue("name")),
            ("ashdash://theme?name=.hidden".into(), BadValue("name")),
            ("ashdash://theme?name=a%2Fb".into(), BadValue("name")),
            ("ashdash://theme?name=".into(), BadValue("name")),
            (format!("ashdash://search?query={}", "a".repeat(MAX_URL)), TooLong),
        ];
        for (url, want) in cases {
            let short: String = url.chars().take(60).collect();
            assert_eq!(parse(&url), Err(want), "for {short}");
        }
    }

    #[test]
    fn multibyte_text_anywhere_never_panics_the_parser() {
        // `ashdash:/é` is 11 bytes and byte 10 is inside the `é`: the scheme check used to slice there.
        assert_eq!(parse("ashdash:/é"), Err(UrlError::WrongScheme));
        assert_eq!(parse("ashdash:é"), Err(UrlError::WrongScheme));
        assert_eq!(parse("é"), Err(UrlError::WrongScheme));
        // Fuzz-style: every multibyte shape inserted at every position of valid and almost-valid URLs.
        let seeds = [
            "ashdash://search?query=saf",
            "ashdash://trigger?bundle=b&name=x&arg=y",
            "ashdash://reload/",
            "ashdash://",
            "ashdash:/",
            "",
        ];
        let chars = ["é", "€", "😀", "\u{200e}", "\u{0301}", "日本"];
        for seed in seeds {
            for (i, _) in seed.char_indices().chain(std::iter::once((seed.len(), ' '))) {
                for ch in chars {
                    let mut url = seed.to_string();
                    url.insert_str(i, ch);
                    let _ = parse(&url);
                    // and replacing a run of bytes with the character, so a char straddles byte 9/10
                    let cut = (i + 2).min(seed.len());
                    if seed.is_char_boundary(cut) {
                        let _ = parse(&format!("{}{ch}{}", &seed[..i], &seed[cut..]));
                    }
                }
            }
        }
        // a multibyte scheme prefix of exactly the right byte length is a wrong scheme, not a match
        assert_eq!(parse("alfréd://x"), Err(UrlError::WrongScheme));
        assert_eq!(parse("ASHDASH://show"), Ok(UrlCommand::Show));
    }

    #[test]
    fn trigger_argument_may_contain_newline_and_tab() {
        assert_eq!(
            q("ashdash://trigger?bundle=b&name=x&arg=a%0Ab%09c"),
            UrlCommand::Trigger {
                bundle: "b".into(),
                name: "x".into(),
                arg: "a\nb\tc".into()
            }
        );
    }

    #[test]
    fn rate_limiter_is_a_sliding_window() {
        let mut r = RateLimiter::new(3, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(r.allow(t0));
        assert!(r.allow(t0 + Duration::from_secs(1)));
        assert!(r.allow(t0 + Duration::from_secs(2)));
        assert!(!r.allow(t0 + Duration::from_secs(3)));
        assert!(!r.allow(t0 + Duration::from_secs(9)));
        // The first event left the window.
        assert!(r.allow(t0 + Duration::from_secs(10)));
        assert!(!r.allow(t0 + Duration::from_secs(10)));
    }

    // ---- dispatcher with fakes

    struct FakeHost {
        known: Vec<(&'static str, &'static str, &'static str)>,
        runs: Mutex<Vec<(String, String, String)>>,
        outcome: Outcome,
    }
    impl UrlHost for FakeHost {
        fn external_trigger(&self, bundle: &str, name: &str) -> Option<String> {
            self.known
                .iter()
                .find(|(b, n, _)| *b == bundle && *n == name)
                .map(|(_, _, w)| w.to_string())
        }
        fn run_trigger(&self, bundle: &str, name: &str, arg: &str) -> Outcome {
            self.runs.lock().unwrap().push((bundle.into(), name.into(), arg.into()));
            self.outcome.clone()
        }
    }

    struct FakePrompter {
        answer: bool,
        asked: Mutex<Vec<String>>,
    }
    impl Prompter for FakePrompter {
        fn confirm(&self, _: &str, bundle: &str) -> bool {
            self.asked.lock().unwrap().push(bundle.into());
            self.answer
        }
    }

    struct Rig {
        d: UrlDispatcher,
        sink: Arc<VecSink>,
        host: Arc<FakeHost>,
        prompter: Arc<FakePrompter>,
        dir: PathBuf,
    }

    fn rig(tag: &str, answer: bool, outcome: Outcome) -> Rig {
        let dir = std::env::temp_dir().join(format!("al-url-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("themes")).unwrap();
        let sink = Arc::new(VecSink::default());
        let host = Arc::new(FakeHost {
            known: vec![("com.ok", "go", "OK Workflow")],
            runs: Mutex::new(vec![]),
            outcome,
        });
        let prompter = Arc::new(FakePrompter {
            answer,
            asked: Mutex::new(vec![]),
        });
        let d = UrlDispatcher::new(
            sink.clone(),
            host.clone(),
            prompter.clone(),
            TrustStore::load(dir.join("url-trust.json")),
            dir.join("themes"),
        );
        Rig {
            d,
            sink,
            host,
            prompter,
            dir,
        }
    }

    #[test]
    fn simple_verbs_become_events() {
        let r = rig("simple", true, Outcome::Done);
        r.d.dispatch("ashdash://search?query=saf").unwrap();
        r.d.dispatch("ashdash://reload").unwrap();
        r.d.dispatch("ashdash://show").unwrap();
        r.d.dispatch("ashdash://prefs").unwrap();
        assert_eq!(
            r.sink.take(),
            vec![
                AppEvent::Prefill("saf".into()),
                AppEvent::Reload,
                AppEvent::Show,
                AppEvent::Preferences
            ]
        );
        assert!(r.d.dispatch("ashdash://prefs?x=1").is_err());
        assert!(r.d.dispatch("reference://search?query=saf").is_err());
        assert!(r.sink.take().is_empty());
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn search_from_a_url_never_becomes_a_query() {
        let r = rig("prefill", true, Outcome::Done);
        // a text that would hit a workflow keyword (and so run its Script Filter)
        r.d.dispatch("ashdash://search?query=deploy%20--prod").unwrap();
        let events = r.sink.take();
        assert_eq!(events, vec![AppEvent::Prefill("deploy --prod".into())]);
        assert!(!events.iter().any(|e| matches!(e, AppEvent::Search(_))));
        assert!(r.host.runs.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn trigger_requires_an_external_trigger_and_first_use_consent() {
        let r = rig("consent", true, Outcome::LargeType("hi".into()));
        // Unknown workflow / trigger: nothing runs, nobody is asked.
        assert!(r.d.dispatch("ashdash://trigger?bundle=com.evil&name=go").is_err());
        assert!(r.d.dispatch("ashdash://trigger?bundle=com.ok&name=other").is_err());
        assert!(r.prompter.asked.lock().unwrap().is_empty());
        assert!(r.host.runs.lock().unwrap().is_empty());

        // First use asks once and remembers.
        r.d.dispatch("ashdash://trigger?bundle=com.ok&name=go&arg=x").unwrap();
        r.d.dispatch("ashdash://trigger?bundle=com.ok&name=go&arg=y").unwrap();
        assert_eq!(r.prompter.asked.lock().unwrap().len(), 1);
        assert_eq!(
            r.host.runs.lock().unwrap().clone(),
            vec![
                ("com.ok".into(), "go".into(), "x".into()),
                ("com.ok".into(), "go".into(), "y".into())
            ]
        );
        assert_eq!(
            r.sink.take(),
            vec![
                AppEvent::Outcome(Outcome::LargeType("hi".into())),
                AppEvent::Outcome(Outcome::LargeType("hi".into()))
            ]
        );

        // The decision survives a restart and the file is private.
        let again = TrustStore::load(r.dir.join("url-trust.json"));
        assert_eq!(again.get("com.ok"), Some(Decision::Allow));
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(r.dir.join("url-trust.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn denial_is_remembered_and_nothing_runs() {
        let r = rig("deny", false, Outcome::Done);
        assert!(r.d.dispatch("ashdash://trigger?bundle=com.ok&name=go").is_err());
        assert!(r.d.dispatch("ashdash://trigger?bundle=com.ok&name=go").is_err());
        assert_eq!(r.prompter.asked.lock().unwrap().len(), 1, "no prompt spam");
        assert!(r.host.runs.lock().unwrap().is_empty());
        assert_eq!(
            TrustStore::load(r.dir.join("url-trust.json")).get("com.ok"),
            Some(Decision::Deny)
        );
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn theme_loads_only_plain_names_from_the_themes_folder() {
        let r = rig("theme", true, Outcome::Done);
        fs::write(r.dir.join("themes/Dark.alfredappearance"), "x").unwrap();
        fs::write(r.dir.join("secret.alfredappearance"), "x").unwrap();
        r.d.dispatch("ashdash://theme?name=Dark").unwrap();
        assert_eq!(
            r.sink.take(),
            vec![AppEvent::Theme(r.dir.join("themes/Dark.alfredappearance"))]
        );
        assert!(r.d.dispatch("ashdash://theme?name=Missing").is_err());
        assert!(r.d.dispatch("ashdash://theme?name=..%2Fsecret").is_err());
        // A symlink inside the folder is not followed.
        std::os::unix::fs::symlink(
            r.dir.join("secret.alfredappearance"),
            r.dir.join("themes/Link.alfredappearance"),
        )
        .unwrap();
        assert!(r.d.dispatch("ashdash://theme?name=Link").is_err());
        assert!(r.sink.take().is_empty());
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn dispatch_is_rate_limited() {
        let r = rig("rate", true, Outcome::Done);
        let t0 = Instant::now();
        for _ in 0..6 {
            r.d.dispatch_at("ashdash://reload", t0).unwrap();
        }
        assert_eq!(r.d.dispatch_at("ashdash://reload", t0).unwrap_err(), "rate limited");
        assert!(
            r.d.dispatch_at("ashdash://reload", t0 + Duration::from_secs(11))
                .is_ok()
        );
        let _ = fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn trust_store_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("al-trust-garbage-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("url-trust.json");
        fs::write(&p, "{{{ not json").unwrap();
        assert_eq!(TrustStore::load(p.clone()).get("x"), None);
        fs::write(&p, r#"{"bundles":{"a":"allow","b":"maybe","c":5}}"#).unwrap();
        let t = TrustStore::load(p);
        assert_eq!(t.get("a"), Some(Decision::Allow));
        assert_eq!(t.get("b"), None);
        assert_eq!(t.get("c"), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
