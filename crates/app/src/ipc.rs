//! Single-instance control socket and the `ashdash` CLI client for it.
//!
//! One launcher process owns a unix domain socket (`<data>/run/ipc.sock`, the
//! directory 0700 and the socket 0600; a flock on a sibling file makes the
//! ownership race-free). Clients send one JSON line per request and read one
//! JSON line back:
//!
//! ```text
//! {"cmd":"search","text":"saf"}                       -> {"ok":true,"message":"","large":false}
//! {"cmd":"trigger","bundle":"b","id":"t","arg":"x"}   -> {"ok":false,"error":"..."}
//! ```
//!
//! Every connection must come from the same uid (`getpeereid`). The socket is
//! a *local* control channel, as trusted as the shell user: unlike the
//! `ashdash://` URL scheme it needs no per-workflow consent.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ashdash_core::{Outcome, Paths};
use serde_json::{Value, json};

use crate::events::{AppEvent, EventSink};

/// `sockaddr_un.sun_path` holds 104 bytes on macOS (including the NUL).
const MAX_SOCKET_PATH: usize = 100;
/// A request line longer than this is rejected.
const MAX_LINE: u64 = 64 * 1024;
/// Search text typed through the socket; the launcher field is short anyway.
pub const MAX_SEARCH: usize = 4096;
const MAX_CONNECTIONS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

// ------------------------------------------------------------------ protocol

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Ping,
    Show,
    Search {
        text: String,
    },
    Trigger {
        bundle: String,
        id: String,
        arg: String,
    },
    Reload,
    Quit,
    /// Open the preferences window.
    Prefs,
    /// Forget everything learned from earlier picks (the launcher holds it in memory).
    ClearUsage,
    /// Absolute path of an `.alfredappearance` file.
    Theme {
        file: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub ok: bool,
    /// Text for the caller (Large Type content, "pong", ...).
    pub message: String,
    /// `message` is Large Type text rather than a status.
    pub large: bool,
    pub error: String,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Response {
            ok: true,
            message: message.into(),
            large: false,
            error: String::new(),
        }
    }
    pub fn err(error: impl Into<String>) -> Self {
        Response {
            ok: false,
            message: String::new(),
            large: false,
            error: error.into(),
        }
    }
    pub fn from_outcome(o: Outcome) -> Self {
        match o {
            Outcome::Done | Outcome::ShowLauncher(_) => Response::ok(""),
            Outcome::LargeType(t) => Response {
                large: true,
                ..Response::ok(t)
            },
            Outcome::Confirm(m) | Outcome::Error(m) => Response::err(m),
        }
    }

    pub fn encode(&self) -> String {
        let v = if self.ok {
            json!({"ok": true, "message": self.message, "large": self.large})
        } else {
            json!({"ok": false, "error": self.error})
        };
        format!("{v}\n")
    }

    pub fn parse(line: &str) -> Result<Response, String> {
        let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))?;
        let ok = v.get("ok").and_then(Value::as_bool).ok_or("response without 'ok'")?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(Response {
            ok,
            message: text("message"),
            large: v.get("large").and_then(Value::as_bool).unwrap_or(false),
            error: text("error"),
        })
    }
}

impl Request {
    pub fn encode(&self) -> String {
        let v = match self {
            Request::Ping => json!({"cmd": "ping"}),
            Request::Show => json!({"cmd": "show"}),
            Request::Search { text } => json!({"cmd": "search", "text": text}),
            Request::Trigger { bundle, id, arg } => {
                json!({"cmd": "trigger", "bundle": bundle, "id": id, "arg": arg})
            }
            Request::Reload => json!({"cmd": "reload"}),
            Request::Quit => json!({"cmd": "quit"}),
            Request::Prefs => json!({"cmd": "prefs"}),
            Request::ClearUsage => json!({"cmd": "clear-usage"}),
            Request::Theme { file } => json!({"cmd": "theme", "file": file.to_string_lossy()}),
        };
        format!("{v}\n")
    }

    /// Strict parser: unknown commands, missing or mistyped fields and
    /// oversized values are errors (the message goes back to the client).
    pub fn parse(line: &str) -> Result<Request, String> {
        let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
        let obj = v.as_object().ok_or("request must be a JSON object")?;
        let cmd = obj
            .get("cmd")
            .and_then(Value::as_str)
            .ok_or("request without a string 'cmd'")?;
        let field = |name: &str, required: bool| -> Result<String, String> {
            match obj.get(name) {
                Some(Value::String(s)) => Ok(s.clone()),
                None | Some(Value::Null) if !required => Ok(String::new()),
                None | Some(Value::Null) => Err(format!("'{cmd}' needs '{name}'")),
                Some(_) => Err(format!("'{name}' must be a string")),
            }
        };
        let req = match cmd {
            "ping" => Request::Ping,
            "show" => Request::Show,
            "reload" => Request::Reload,
            "quit" => Request::Quit,
            "prefs" => Request::Prefs,
            "clear-usage" => Request::ClearUsage,
            "search" => {
                let text = field("text", true)?;
                if text.len() > MAX_SEARCH {
                    return Err(format!("search text longer than {MAX_SEARCH} bytes"));
                }
                Request::Search { text }
            }
            "trigger" => {
                let bundle = field("bundle", true)?;
                let id = field("id", true)?;
                if bundle.is_empty() || id.is_empty() {
                    return Err("'trigger' needs a bundle id and a trigger id".into());
                }
                Request::Trigger {
                    bundle,
                    id,
                    arg: field("arg", false)?,
                }
            }
            "theme" => {
                let file = PathBuf::from(field("file", true)?);
                if !file.is_absolute() {
                    return Err("'theme' needs an absolute path".into());
                }
                Request::Theme { file }
            }
            other => return Err(format!("unknown command '{other}'")),
        };
        Ok(req)
    }
}

// ------------------------------------------------------------------ handler

/// What the handler needs from the engine, so tests can run without one.
pub trait Backend: Send + Sync {
    fn run_trigger(&self, bundle: &str, id: &str, arg: &str) -> Outcome;
    fn validate_theme(&self, file: &Path) -> Result<(), String>;
    /// Forget what the launcher learned from earlier picks.
    fn clear_usage(&self) {}
}

pub trait Handler: Send + Sync {
    fn handle(&self, req: Request) -> Response;
}

/// Maps requests onto window events and engine calls.
pub struct AppHandler {
    pub sink: Arc<dyn EventSink>,
    pub backend: Arc<dyn Backend>,
}

impl Handler for AppHandler {
    fn handle(&self, req: Request) -> Response {
        match req {
            Request::Ping => Response::ok("pong"),
            Request::Show => {
                self.sink.emit(AppEvent::Show);
                Response::ok("")
            }
            Request::Search { text } => {
                self.sink.emit(AppEvent::Search(text));
                Response::ok("")
            }
            Request::Reload => {
                self.sink.emit(AppEvent::Reload);
                Response::ok("")
            }
            Request::Quit => {
                self.sink.emit(AppEvent::Quit);
                Response::ok("")
            }
            Request::Prefs => {
                self.sink.emit(AppEvent::Preferences);
                Response::ok("")
            }
            Request::ClearUsage => {
                self.backend.clear_usage();
                Response::ok("")
            }
            Request::Trigger { bundle, id, arg } => {
                let outcome = self.backend.run_trigger(&bundle, &id, &arg);
                // an Open Launcher object: show the window as `search` does
                if let Outcome::ShowLauncher(query) = &outcome {
                    self.sink.emit(AppEvent::Search(query.clone()));
                }
                Response::from_outcome(outcome)
            }
            Request::Theme { file } => match self.backend.validate_theme(&file) {
                Ok(()) => {
                    self.sink.emit(AppEvent::Theme(file));
                    Response::ok("")
                }
                Err(e) => Response::err(e),
            },
        }
    }
}

// ------------------------------------------------------------------ paths and locking

/// Socket location. Normally `<data>/run/ipc.sock`; when that would not fit in
/// `sun_path` (long user names, deep temp dirs) `/tmp/ashdash-<uid>/ipc.sock`.
/// Clients and server compute the same answer.
pub fn socket_path(paths: &Paths, uid: u32) -> PathBuf {
    let preferred = paths.data.join("run").join("ipc.sock");
    if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
        preferred
    } else {
        PathBuf::from(format!("/tmp/ashdash-{uid}/ipc.sock"))
    }
}

pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// Create `dir` (and parents) 0700 and make sure it is a real directory we own
/// that nobody else can enter. Refuses symlinks and foreign owners, which is
/// what protects the `/tmp` fallback.
fn prepare_dir(dir: &Path, uid: u32) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let md = fs::symlink_metadata(dir)?;
    if !md.is_dir() {
        return Err(io::Error::other(format!("{} is not a directory", dir.display())));
    }
    if md.uid() != uid {
        return Err(io::Error::other(format!("{} is owned by another user", dir.display())));
    }
    if md.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct Primary {
    _lock: File,
    listener: UnixListener,
    path: PathBuf,
}

pub enum Acquired {
    Primary(Primary),
    /// Another launcher holds the lock.
    AlreadyRunning,
}

/// Become the single instance: take the lock, replace a stale socket, listen.
pub fn acquire(path: &Path, uid: u32) -> io::Result<Acquired> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("socket path without a directory"))?;
    prepare_dir(dir, uid)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("lock"))?;
    // SAFETY: valid fd owned by `lock`, which outlives the lock (kept in Primary).
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::WouldBlock {
            Ok(Acquired::AlreadyRunning)
        } else {
            Err(e)
        };
    }
    // We hold the lock, so whatever socket file is left over is stale.
    match fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_socket() && md.uid() == uid => fs::remove_file(path)?,
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} exists and is not our socket",
                path.display()
            )));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(Acquired::Primary(Primary {
        _lock: lock,
        listener,
        path: path.to_path_buf(),
    }))
}

// ------------------------------------------------------------------ server

/// Whether a connection from `peer` may talk to a server running as `me`.
pub fn authorize(peer: u32, me: u32) -> bool {
    peer == me
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: valid fd, valid out pointers.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

pub struct Server {
    stop: Arc<AtomicBool>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
    _lock: File,
}

/// One of the `MAX_CONNECTIONS` connection slots; frees itself on drop (also while unwinding).
struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Take a slot, or `None` when `MAX_CONNECTIONS` connections are already being served.
fn admit(active: &Arc<AtomicUsize>) -> Option<ConnectionSlot> {
    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        active.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    Some(ConnectionSlot(Arc::clone(active)))
}

/// Start answering requests on a background thread.
pub fn serve(primary: Primary, handler: Arc<dyn Handler>, uid: u32) -> Server {
    let Primary { _lock, listener, path } = primary;
    let stop = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let thread = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let Some(slot) = admit(&active) else {
                    let mut s = stream;
                    let _ = s.write_all(Response::err("too many connections").encode().as_bytes());
                    continue;
                };
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || {
                    // Released when this closure ends, however it ends (a panicking handler
                    // included): sixteen panics must not lock the socket out until restart.
                    let _slot = slot;
                    if let Err(e) = handle_connection(stream, handler.as_ref(), uid) {
                        eprintln!("ashdash: ipc connection: {e}");
                    }
                });
            }
        })
    };
    Server {
        stop,
        path,
        thread: Some(thread),
        _lock,
    }
}

/// Read one request line (at most `MAX_LINE` bytes) from `r`.
fn read_line_capped(r: impl Read) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut limited = io::BufReader::new(r).take(MAX_LINE + 1);
    limited.read_until(b'\n', &mut buf).map_err(|e| format!("read: {e}"))?;
    if buf.len() as u64 > MAX_LINE {
        return Err("request too long".into());
    }
    String::from_utf8(buf).map_err(|_| "request is not valid UTF-8".to_string())
}

fn handle_connection(stream: UnixStream, handler: &dyn Handler, me: u32) -> io::Result<()> {
    let peer = peer_uid(&stream)?;
    let mut out = stream.try_clone()?;
    if !authorize(peer, me) {
        out.write_all(Response::err("not authorized").encode().as_bytes())?;
        return Ok(());
    }
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    let resp = match read_line_capped(&stream) {
        Err(e) => Response::err(e),
        Ok(line) if line.trim().is_empty() => Response::err("empty request"),
        Ok(line) => match Request::parse(&line) {
            Err(e) => Response::err(e),
            Ok(req) => handler.handle(req),
        },
    };
    out.write_all(resp.encode().as_bytes())
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept().
        let _ = UnixStream::connect(&self.path);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

// ------------------------------------------------------------------ client

#[derive(Debug)]
pub enum ClientError {
    /// No launcher is listening.
    NotRunning,
    Io(io::Error),
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::NotRunning => write!(f, "ashdash is not running"),
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::Protocol(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send one request and wait for the answer (at most `timeout` per I/O step).
pub fn send(path: &Path, req: &Request, timeout: Duration) -> Result<Response, ClientError> {
    let mut stream = UnixStream::connect(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => ClientError::NotRunning,
        _ => ClientError::Io(e),
    })?;
    stream.set_read_timeout(Some(timeout)).map_err(ClientError::Io)?;
    stream.set_write_timeout(Some(timeout)).map_err(ClientError::Io)?;
    stream.write_all(req.encode().as_bytes()).map_err(ClientError::Io)?;
    let mut line = String::new();
    io::BufReader::new(&stream)
        .take(8 * 1024 * 1024)
        .read_line(&mut line)
        .map_err(ClientError::Io)?;
    if line.is_empty() {
        return Err(ClientError::Protocol("connection closed without an answer".into()));
    }
    Response::parse(&line).map_err(ClientError::Protocol)
}

/// Wait until something answers `ping` on `path`.
pub fn wait_until_up(path: &Path, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if matches!(send(path, &Request::Ping, Duration::from_secs(2)), Ok(r) if r.ok) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::events::VecSink;

    struct FakeBackend {
        triggers: Mutex<Vec<(String, String, String)>>,
        theme_ok: bool,
    }

    impl Backend for FakeBackend {
        fn run_trigger(&self, bundle: &str, id: &str, arg: &str) -> Outcome {
            self.triggers
                .lock()
                .unwrap()
                .push((bundle.into(), id.into(), arg.into()));
            if id == "big" {
                Outcome::LargeType("HELLO".into())
            } else if id == "browse" {
                Outcome::ShowLauncher("open docs".into())
            } else if id == "missing" {
                Outcome::Error("no such trigger".into())
            } else {
                Outcome::Done
            }
        }
        fn validate_theme(&self, _: &Path) -> Result<(), String> {
            if self.theme_ok { Ok(()) } else { Err("bad theme".into()) }
        }
    }

    fn handler(theme_ok: bool) -> (AppHandler, Arc<VecSink>, Arc<FakeBackend>) {
        let sink = Arc::new(VecSink::default());
        let backend = Arc::new(FakeBackend {
            triggers: Mutex::new(vec![]),
            theme_ok,
        });
        (
            AppHandler {
                sink: sink.clone(),
                backend: backend.clone(),
            },
            sink,
            backend,
        )
    }

    fn temp(tag: &str) -> PathBuf {
        // Short path: sockets are limited to ~100 bytes.
        let p = std::env::temp_dir().join(format!("al-{tag}-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn requests_round_trip() {
        let all = [
            Request::Ping,
            Request::Show,
            Request::Reload,
            Request::Quit,
            Request::Prefs,
            Request::ClearUsage,
            Request::Search {
                text: "saf \"quoted\"\n é".into(),
            },
            Request::Trigger {
                bundle: "b.c".into(),
                id: "t 1".into(),
                arg: "x\ty".into(),
            },
            Request::Theme {
                file: PathBuf::from("/tmp/a b.alfredappearance"),
            },
        ];
        for r in all {
            let line = r.encode();
            assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
            assert_eq!(Request::parse(&line), Ok(r));
        }
    }

    #[test]
    fn parser_rejects_bad_requests() {
        for bad in [
            "",
            "not json",
            "[]",
            r#"{"cmd":5}"#,
            r#"{"cmd":"nope"}"#,
            r#"{"cmd":"search"}"#,
            r#"{"cmd":"search","text":5}"#,
            r#"{"cmd":"trigger","bundle":"b"}"#,
            r#"{"cmd":"trigger","bundle":"","id":"x"}"#,
            r#"{"cmd":"theme","file":"relative.alfredappearance"}"#,
        ] {
            assert!(Request::parse(bad).is_err(), "should reject {bad:?}");
        }
        let long = format!(r#"{{"cmd":"search","text":"{}"}}"#, "a".repeat(MAX_SEARCH + 1));
        assert!(Request::parse(&long).is_err());
    }

    #[test]
    fn response_round_trip() {
        for r in [Response::ok(""), Response::ok("pong"), Response::err("boom"), {
            Response {
                large: true,
                ..Response::ok("big text\nline 2")
            }
        }] {
            let back = Response::parse(&r.encode()).unwrap();
            assert_eq!(back.ok, r.ok);
            assert_eq!(back.large, r.large);
            assert_eq!(back.message, r.message);
            assert_eq!(back.error, r.error);
        }
        assert!(Response::parse("{}").is_err());
    }

    #[test]
    fn an_open_launcher_trigger_shows_the_window_with_its_query() {
        let (h, sink, _) = handler(true);
        let r = h.handle(Request::Trigger {
            bundle: "b".into(),
            id: "browse".into(),
            arg: String::new(),
        });
        assert!(r.ok, "{r:?}");
        assert_eq!(sink.take(), vec![AppEvent::Search("open docs".into())]);
        // a trigger that only shows Large Type opens nothing
        let r = h.handle(Request::Trigger {
            bundle: "b".into(),
            id: "big".into(),
            arg: String::new(),
        });
        assert!(r.large);
        assert!(sink.take().is_empty());
    }

    #[test]
    fn handler_maps_requests_to_events_and_engine_calls() {
        let (h, sink, backend) = handler(true);
        assert_eq!(h.handle(Request::Ping), Response::ok("pong"));
        assert!(h.handle(Request::Show).ok);
        assert!(h.handle(Request::Search { text: "saf".into() }).ok);
        assert!(h.handle(Request::Reload).ok);
        assert!(h.handle(Request::Quit).ok);
        assert!(h.handle(Request::Prefs).ok);
        assert!(h.handle(Request::Theme { file: "/x".into() }).ok);
        assert_eq!(
            sink.take(),
            vec![
                AppEvent::Show,
                AppEvent::Search("saf".into()),
                AppEvent::Reload,
                AppEvent::Quit,
                AppEvent::Preferences,
                AppEvent::Theme("/x".into())
            ]
        );
        let r = h.handle(Request::Trigger {
            bundle: "b".into(),
            id: "big".into(),
            arg: "a".into(),
        });
        assert!(r.ok && r.large && r.message == "HELLO");
        let r = h.handle(Request::Trigger {
            bundle: "b".into(),
            id: "missing".into(),
            arg: "".into(),
        });
        assert!(!r.ok && r.error == "no such trigger");
        assert_eq!(backend.triggers.lock().unwrap().len(), 2);

        let (h, sink, _) = handler(false);
        let r = h.handle(Request::Theme { file: "/x".into() });
        assert!(!r.ok);
        assert!(sink.take().is_empty(), "an invalid theme must not reach the window");
    }

    #[test]
    fn socket_path_falls_back_when_too_long() {
        let short = Paths::under(Path::new("/tmp/x"));
        assert_eq!(socket_path(&short, 501), PathBuf::from("/tmp/x/data/run/ipc.sock"));
        let long = Paths::under(&PathBuf::from("/tmp").join("d".repeat(120)));
        assert_eq!(socket_path(&long, 501), PathBuf::from("/tmp/ashdash-501/ipc.sock"));
    }

    #[test]
    fn authorize_only_same_uid() {
        assert!(authorize(501, 501));
        assert!(!authorize(0, 501));
        assert!(!authorize(502, 501));
    }

    #[test]
    fn capped_line_reader() {
        assert_eq!(read_line_capped(&b"{\"a\":1}\nrest"[..]).unwrap(), "{\"a\":1}\n");
        let big = vec![b'a'; MAX_LINE as usize + 10];
        assert!(read_line_capped(&big[..]).is_err());
        assert!(read_line_capped(&[0xff, 0xfe, b'\n'][..]).is_err());
    }

    #[test]
    fn end_to_end_over_a_real_socket() {
        let dir = temp("e2e");
        let uid = current_uid();
        let path = dir.join("run").join("ipc.sock");
        let Acquired::Primary(primary) = acquire(&path, uid).unwrap() else {
            panic!("expected to become primary")
        };
        // Permissions: directory 0700, socket 0600.
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);

        // A second instance is refused while the first lives.
        assert!(matches!(acquire(&path, uid).unwrap(), Acquired::AlreadyRunning));

        let (h, sink, _) = handler(true);
        let server = serve(primary, Arc::new(h), uid);
        let t = Duration::from_secs(5);
        assert_eq!(send(&path, &Request::Ping, t).unwrap().message, "pong");
        assert!(send(&path, &Request::Search { text: "saf".into() }, t).unwrap().ok);
        let r = send(
            &path,
            &Request::Trigger {
                bundle: "b".into(),
                id: "big".into(),
                arg: "".into(),
            },
            t,
        )
        .unwrap();
        assert!(r.large && r.message == "HELLO");
        assert_eq!(sink.take(), vec![AppEvent::Search("saf".into())]);

        // Garbage gets an error answer, not a hang or a crash.
        let mut s = UnixStream::connect(&path).unwrap();
        s.write_all(b"this is not json\n").unwrap();
        let mut line = String::new();
        io::BufReader::new(&s).read_line(&mut line).unwrap();
        assert!(!Response::parse(&line).unwrap().ok);

        // Dropping the server frees the socket and the lock: a new instance may start.
        drop(server);
        assert!(matches!(send(&path, &Request::Ping, t), Err(ClientError::NotRunning)));
        assert!(matches!(acquire(&path, uid).unwrap(), Acquired::Primary(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Panics on `show`, answers everything else.
    struct PanickyHandler;
    impl Handler for PanickyHandler {
        fn handle(&self, req: Request) -> Response {
            if req == Request::Show {
                panic!("handler bug (expected in this test)");
            }
            Response::ok("pong")
        }
    }

    #[test]
    fn panicking_handlers_do_not_use_up_the_connection_slots() {
        let dir = temp("panic");
        let uid = current_uid();
        let path = dir.join("run").join("ipc.sock");
        let Acquired::Primary(primary) = acquire(&path, uid).unwrap() else {
            panic!("expected to become primary")
        };
        let server = serve(primary, Arc::new(PanickyHandler), uid);
        let t = Duration::from_secs(5);
        // Well over MAX_CONNECTIONS panics; each one used to leak a slot for good.
        for _ in 0..(MAX_CONNECTIONS * 2) {
            assert!(send(&path, &Request::Show, t).is_err());
        }
        // The handler threads release their slot while unwinding, which can trail the client's
        // error by a moment: give them a bounded time.
        let start = Instant::now();
        loop {
            let r = send(&path, &Request::Ping, t).unwrap();
            if r.ok {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the socket stayed locked out: {}",
                r.error
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_connection_slot_is_released_when_its_guard_drops_even_while_unwinding() {
        let active = Arc::new(AtomicUsize::new(0));
        let guard = admit(&active).expect("a free slot");
        assert_eq!(active.load(Ordering::SeqCst), 1);
        let a = Arc::clone(&active);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("boom (expected in this test)");
        }));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let held: Vec<_> = (0..MAX_CONNECTIONS).map(|_| admit(&a).expect("free")).collect();
        assert!(
            admit(&a).is_none(),
            "the {}th connection is refused",
            MAX_CONNECTIONS + 1
        );
        assert_eq!(a.load(Ordering::SeqCst), MAX_CONNECTIONS, "a refusal takes no slot");
        drop(held);
        assert_eq!(a.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stale_socket_is_replaced_and_foreign_files_are_not() {
        let dir = temp("stale");
        let uid = current_uid();
        let path = dir.join("run").join("ipc.sock");
        // A socket file left behind by a crashed process (bound, then abandoned without unlink).
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(acquire(&path, uid).unwrap(), Acquired::Primary(_)));

        // A regular file in the way is an error, never deleted.
        let dir2 = temp("foreign");
        let path2 = dir2.join("run").join("ipc.sock");
        fs::create_dir_all(path2.parent().unwrap()).unwrap();
        fs::write(&path2, "precious").unwrap();
        assert!(acquire(&path2, uid).is_err());
        assert_eq!(fs::read_to_string(&path2).unwrap(), "precious");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn directory_with_loose_permissions_is_tightened() {
        let dir = temp("perm");
        let run = dir.join("run");
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_dir(&run, current_uid()).unwrap();
        assert_eq!(fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o700);
        // Wrong owner is refused.
        assert!(prepare_dir(&run, current_uid() + 1).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn peer_credentials_of_a_local_connection_match_ourselves() {
        let dir = temp("peer");
        let path = dir.join("s");
        let l = UnixListener::bind(&path).unwrap();
        let c = UnixStream::connect(&path).unwrap();
        let (s, _) = l.accept().unwrap();
        assert_eq!(peer_uid(&s).unwrap(), current_uid());
        assert_eq!(peer_uid(&c).unwrap(), current_uid());
        let _ = fs::remove_dir_all(&dir);
    }
}
