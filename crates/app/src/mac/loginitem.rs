//! Launch at login via `SMAppService.mainApp` (macOS 13+).
//!
//! The logic is a small state machine over [`Status`] ([`run`]) that talks to the
//! system only through the [`Backend`] trait, so every branch is unit-tested with a
//! fake. [`SystemBackend`] is the thin objc2 FFI.
//!
//! `SMAppService.mainApp` registers *the running bundle*. It therefore only works when
//! the process was started from inside an `.app` (see [`is_app_bundle_exe`]); from a
//! plain `cargo run` binary every command except `status` fails with a clear message
//! instead of registering something unrelated.

use std::fmt;
use std::path::Path;

/// `SMAppServiceStatus`, as returned by `-[SMAppService status]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not registered as a login item.
    NotRegistered,
    /// Registered and allowed to run at login.
    Enabled,
    /// Registered, but the user has to approve it in System Settings > General > Login Items.
    RequiresApproval,
    /// The service could not find the app (for example it was moved or deleted).
    NotFound,
}

impl Status {
    /// Map the raw `NSInteger` (0 notRegistered, 1 enabled, 2 requiresApproval, 3 notFound).
    pub fn from_raw(raw: isize) -> Option<Status> {
        match raw {
            0 => Some(Status::NotRegistered),
            1 => Some(Status::Enabled),
            2 => Some(Status::RequiresApproval),
            3 => Some(Status::NotFound),
            _ => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Status::NotRegistered => "off",
            Status::Enabled => "enabled",
            Status::RequiresApproval => {
                "waiting for approval: open System Settings > General > Login Items and allow Ashdash"
            }
            Status::NotFound => {
                "not found by macOS (it usually has to live in /Applications; was Ashdash.app moved or deleted?)"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Enable,
    Disable,
    Status,
}

impl Command {
    pub fn parse(arg: Option<&str>) -> Option<Command> {
        match arg? {
            "enable" | "on" => Some(Command::Enable),
            "disable" | "off" => Some(Command::Disable),
            "status" => Some(Command::Status),
            _ => None,
        }
    }
}

/// Why launch-at-login cannot be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The executable is not inside an `.app` bundle.
    NotInBundle,
    /// macOS older than 13 has no `SMAppService`.
    OsTooOld,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unavailable::NotInBundle => f.write_str(
                "launch at login needs the app bundle: this binary is not running from inside an .app \
                 (build one with packaging/build-app.sh and run that)",
            ),
            Unavailable::OsTooOld => f.write_str("launch at login needs macOS 13 or newer"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unavailable(Unavailable),
    /// The system call itself failed (message from `NSError`).
    System(String),
    /// The call succeeded but the status afterwards is not what was asked for.
    NotApplied {
        wanted: &'static str,
        got: Status,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable(u) => u.fmt(f),
            Error::System(m) => write!(f, "macOS refused: {m}"),
            Error::NotApplied { wanted, got } => {
                write!(
                    f,
                    "asked macOS to {wanted} the login item, but it is now {}",
                    got.describe()
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// What the system exposes. Implemented by [`SystemBackend`] and by test fakes.
pub trait Backend {
    fn availability(&self) -> Result<(), Unavailable>;
    fn status(&self) -> Result<Status, Error>;
    fn register(&self) -> Result<(), Error>;
    fn unregister(&self) -> Result<(), Error>;
    /// Did this app ever register itself (a marker in the data folder written by `enable`, removed
    /// by `disable`)? `SMAppService` reports `notFound` both for "never registered" and for
    /// "registered, but the bundle is gone", and this is the only way to tell them apart.
    fn ever_registered(&self) -> bool;
    fn remember_registration(&self, registered: bool);
}

/// The single system call (if any) a command needs given the current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nothing,
    Register,
    Unregister,
}

/// Pure transition table.
pub fn plan(cmd: Command, current: Status) -> Step {
    match (cmd, current) {
        (Command::Status, _) => Step::Nothing,
        (Command::Enable, Status::NotRegistered | Status::NotFound) => Step::Register,
        // Already on, or on and only waiting for the user to approve it: registering again changes nothing.
        (Command::Enable, Status::Enabled | Status::RequiresApproval) => Step::Nothing,
        (Command::Disable, Status::Enabled | Status::RequiresApproval) => Step::Unregister,
        (Command::Disable, Status::NotRegistered | Status::NotFound) => Step::Nothing,
    }
}

/// Is the resulting status acceptable for the command? `RequiresApproval` counts as success
/// for `enable`: the registration is in place, only the user's consent is outstanding.
fn satisfied(cmd: Command, after: Status) -> bool {
    match cmd {
        Command::Status => true,
        Command::Enable => matches!(after, Status::Enabled | Status::RequiresApproval),
        Command::Disable => matches!(after, Status::NotRegistered | Status::NotFound),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `status` on a system where the feature cannot work.
    Unavailable(Unavailable),
    /// The state after the command ran; `changed` tells whether a system call was made.
    State { status: Status, changed: bool },
}

impl Outcome {
    pub fn message(&self) -> String {
        match self {
            Outcome::Unavailable(u) => format!("unavailable: {u}"),
            Outcome::State { status, changed } => {
                let note = if *changed { "" } else { " (no change)" };
                format!("launch at login: {}{note}", status.describe())
            }
        }
    }
}

/// What the user should be told: `notFound` from a service that this app never registered is just
/// "off" (a fresh install); only a registration that has lost its bundle is a warning.
pub fn effective_status(raw: Status, ever_registered: bool) -> Status {
    match raw {
        Status::NotFound if !ever_registered => Status::NotRegistered,
        other => other,
    }
}

/// Run one command. `status` reports unavailability as data; `enable`/`disable` fail with it.
pub fn run(cmd: Command, backend: &dyn Backend) -> Result<Outcome, Error> {
    if let Err(why) = backend.availability() {
        return match cmd {
            Command::Status => Ok(Outcome::Unavailable(why)),
            _ => Err(Error::Unavailable(why)),
        };
    }
    let current = effective_status(backend.status()?, backend.ever_registered());
    let step = plan(cmd, current);
    match step {
        Step::Nothing => {}
        Step::Register => {
            backend.register()?;
            backend.remember_registration(true);
        }
        Step::Unregister => {
            backend.unregister()?;
            backend.remember_registration(false);
        }
    }
    let after = if step == Step::Nothing {
        current
    } else {
        effective_status(backend.status()?, backend.ever_registered())
    };
    if !satisfied(cmd, after) {
        let wanted = if cmd == Command::Enable { "enable" } else { "disable" };
        return Err(Error::NotApplied { wanted, got: after });
    }
    Ok(Outcome::State {
        status: after,
        changed: step != Step::Nothing,
    })
}

/// True when `exe` looks like `<something>.app/Contents/MacOS/<binary>`.
pub fn is_app_bundle_exe(exe: &Path) -> bool {
    let mut parts = exe.components().rev();
    let (Some(_bin), Some(macos), Some(contents), Some(app)) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    macos.as_os_str() == "MacOS"
        && contents.as_os_str() == "Contents"
        && Path::new(app.as_os_str()).extension().is_some_and(|e| e == "app")
}

/// CLI entry point: `ashdash login-item enable|disable|status`.
pub fn cli(arg: Option<&str>) -> anyhow::Result<()> {
    let Some(cmd) = Command::parse(arg) else {
        anyhow::bail!("usage: ashdash login-item enable|disable|status");
    };
    let outcome = run(cmd, &SystemBackend)?;
    println!("{}", outcome.message());
    Ok(())
}

// ---------------------------------------------------------------- FFI

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// The real `SMAppService.mainApp`.
pub struct SystemBackend;

mod ffi {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSError;

    use super::{Error, Status};

    /// `+[SMAppService mainAppService]`, or `None` before macOS 13.
    pub fn main_app_service() -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"SMAppService")?;
        unsafe { msg_send![class, mainAppService] }
    }

    pub fn status(service: &AnyObject) -> Result<Status, Error> {
        let raw: isize = unsafe { msg_send![service, status] };
        Status::from_raw(raw).ok_or_else(|| Error::System(format!("unknown SMAppService status {raw}")))
    }

    fn describe(e: &NSError) -> String {
        e.localizedDescription().to_string()
    }

    pub fn register(service: &AnyObject) -> Result<(), Error> {
        let r: Result<(), Retained<NSError>> = unsafe { msg_send![service, registerAndReturnError: _] };
        r.map_err(|e| Error::System(describe(&e)))
    }

    pub fn unregister(service: &AnyObject) -> Result<(), Error> {
        let r: Result<(), Retained<NSError>> = unsafe { msg_send![service, unregisterAndReturnError: _] };
        r.map_err(|e| Error::System(describe(&e)))
    }
}

impl SystemBackend {
    fn service(&self) -> Result<objc2::rc::Retained<objc2::runtime::AnyObject>, Error> {
        ffi::main_app_service().ok_or(Error::Unavailable(Unavailable::OsTooOld))
    }
}

impl Backend for SystemBackend {
    fn availability(&self) -> Result<(), Unavailable> {
        let in_bundle = std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| is_app_bundle_exe(&p));
        if !in_bundle {
            return Err(Unavailable::NotInBundle);
        }
        if ffi::main_app_service().is_none() {
            return Err(Unavailable::OsTooOld);
        }
        Ok(())
    }

    fn status(&self) -> Result<Status, Error> {
        ffi::status(&*self.service()?)
    }

    fn register(&self) -> Result<(), Error> {
        ffi::register(&*self.service()?)
    }

    fn unregister(&self) -> Result<(), Error> {
        ffi::unregister(&*self.service()?)
    }

    fn ever_registered(&self) -> bool {
        marker_path().is_some_and(|p| p.exists())
    }

    fn remember_registration(&self, registered: bool) {
        let Some(path) = marker_path() else { return };
        if registered {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, b"registered by `ashdash login-item enable`\n");
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// `<data folder>/login-item-registered`.
fn marker_path() -> Option<std::path::PathBuf> {
    Some(ashdash_core::Paths::system().data.join("login-item-registered"))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use super::*;

    /// Models SMAppService: calls change state; `stubborn` makes register a silent no-op;
    /// `fail` makes calls return an error.
    struct Fake {
        available: Result<(), Unavailable>,
        status: Cell<Status>,
        calls: RefCell<Vec<&'static str>>,
        stubborn: bool,
        fail: Option<&'static str>,
        needs_approval: bool,
        /// The "this app registered itself once" marker.
        marker: Cell<bool>,
    }

    impl Fake {
        fn new(status: Status) -> Fake {
            Fake {
                available: Ok(()),
                status: Cell::new(status),
                calls: RefCell::new(Vec::new()),
                stubborn: false,
                fail: None,
                needs_approval: false,
                marker: Cell::new(false),
            }
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Backend for Fake {
        fn ever_registered(&self) -> bool {
            self.marker.get()
        }
        fn remember_registration(&self, registered: bool) {
            self.marker.set(registered);
        }
        fn availability(&self) -> Result<(), Unavailable> {
            self.available.clone()
        }
        fn status(&self) -> Result<Status, Error> {
            Ok(self.status.get())
        }
        fn register(&self) -> Result<(), Error> {
            self.calls.borrow_mut().push("register");
            if let Some(m) = self.fail {
                return Err(Error::System(m.into()));
            }
            if !self.stubborn {
                self.status.set(if self.needs_approval {
                    Status::RequiresApproval
                } else {
                    Status::Enabled
                });
            }
            Ok(())
        }
        fn unregister(&self) -> Result<(), Error> {
            self.calls.borrow_mut().push("unregister");
            if let Some(m) = self.fail {
                return Err(Error::System(m.into()));
            }
            if !self.stubborn {
                self.status.set(Status::NotRegistered);
            }
            Ok(())
        }
    }

    #[test]
    fn a_fresh_install_reports_off_not_a_warning() {
        // SMAppService says notFound before the first registration (PKG-17).
        let f = Fake::new(Status::NotFound);
        let out = run(Command::Status, &f).unwrap();
        assert_eq!(
            out,
            Outcome::State {
                status: Status::NotRegistered,
                changed: false
            }
        );
        let msg = out.message();
        assert!(msg.contains("off"), "{msg}");
        assert!(!msg.contains("not found") && !msg.contains("moved or deleted"), "{msg}");
        assert!(f.calls().is_empty());
    }

    #[test]
    fn a_registration_whose_bundle_is_gone_keeps_the_warning() {
        let f = Fake::new(Status::NotFound);
        f.marker.set(true);
        let out = run(Command::Status, &f).unwrap();
        assert_eq!(
            out,
            Outcome::State {
                status: Status::NotFound,
                changed: false
            }
        );
        assert!(out.message().contains("moved or deleted"));
    }

    #[test]
    fn enable_then_disable_walks_off_on_off_and_clears_the_marker() {
        let f = Fake::new(Status::NotFound); // fresh install
        let on = run(Command::Enable, &f).unwrap();
        assert_eq!(
            on,
            Outcome::State {
                status: Status::Enabled,
                changed: true
            }
        );
        assert!(f.marker.get());
        // the bundle disappears later: now it is a real problem
        f.status.set(Status::NotFound);
        assert!(run(Command::Status, &f).unwrap().message().contains("moved or deleted"));
        f.status.set(Status::Enabled);
        let off = run(Command::Disable, &f).unwrap();
        assert!(off.message().contains("off"));
        assert!(!f.marker.get());
        // after a disable the OS may say notFound again; still just "off"
        f.status.set(Status::NotFound);
        assert!(run(Command::Status, &f).unwrap().message().contains("launch at login: off"));
    }

    #[test]
    fn effective_status_only_rewrites_notfound_without_a_marker() {
        for (raw, ever, want) in [
            (Status::NotFound, false, Status::NotRegistered),
            (Status::NotFound, true, Status::NotFound),
            (Status::Enabled, false, Status::Enabled),
            (Status::Enabled, true, Status::Enabled),
            (Status::NotRegistered, false, Status::NotRegistered),
            (Status::RequiresApproval, false, Status::RequiresApproval),
        ] {
            assert_eq!(effective_status(raw, ever), want, "{raw:?} ever={ever}");
        }
    }

    #[test]
    fn raw_status_mapping() {
        assert_eq!(Status::from_raw(0), Some(Status::NotRegistered));
        assert_eq!(Status::from_raw(1), Some(Status::Enabled));
        assert_eq!(Status::from_raw(2), Some(Status::RequiresApproval));
        assert_eq!(Status::from_raw(3), Some(Status::NotFound));
        assert_eq!(Status::from_raw(4), None);
        assert_eq!(Status::from_raw(-1), None);
    }

    #[test]
    fn command_parsing() {
        assert_eq!(Command::parse(Some("enable")), Some(Command::Enable));
        assert_eq!(Command::parse(Some("disable")), Some(Command::Disable));
        assert_eq!(Command::parse(Some("status")), Some(Command::Status));
        assert_eq!(Command::parse(Some("toggle")), None);
        assert_eq!(Command::parse(None), None);
    }

    #[test]
    fn transition_table() {
        use Command as C;
        use Status as S;
        use Step as P;
        let table = [
            (C::Enable, S::NotRegistered, P::Register),
            (C::Enable, S::NotFound, P::Register),
            (C::Enable, S::Enabled, P::Nothing),
            (C::Enable, S::RequiresApproval, P::Nothing),
            (C::Disable, S::Enabled, P::Unregister),
            (C::Disable, S::RequiresApproval, P::Unregister),
            (C::Disable, S::NotRegistered, P::Nothing),
            (C::Disable, S::NotFound, P::Nothing),
            (C::Status, S::Enabled, P::Nothing),
            (C::Status, S::NotRegistered, P::Nothing),
        ];
        for (cmd, st, want) in table {
            assert_eq!(plan(cmd, st), want, "{cmd:?} from {st:?}");
        }
    }

    #[test]
    fn enable_registers_once_and_is_idempotent() {
        let f = Fake::new(Status::NotRegistered);
        let out = run(Command::Enable, &f).unwrap();
        assert_eq!(
            out,
            Outcome::State {
                status: Status::Enabled,
                changed: true
            }
        );
        let out = run(Command::Enable, &f).unwrap();
        assert_eq!(
            out,
            Outcome::State {
                status: Status::Enabled,
                changed: false
            }
        );
        assert_eq!(f.calls(), vec!["register"]);
    }

    #[test]
    fn disable_unregisters_and_is_idempotent() {
        let f = Fake::new(Status::Enabled);
        assert_eq!(
            run(Command::Disable, &f).unwrap(),
            Outcome::State {
                status: Status::NotRegistered,
                changed: true
            }
        );
        assert_eq!(
            run(Command::Disable, &f).unwrap(),
            Outcome::State {
                status: Status::NotRegistered,
                changed: false
            }
        );
        assert_eq!(f.calls(), vec!["unregister"]);
    }

    #[test]
    fn requires_approval_counts_as_enabled_and_is_explained() {
        let mut f = Fake::new(Status::NotRegistered);
        f.needs_approval = true;
        let out = run(Command::Enable, &f).unwrap();
        assert_eq!(
            out,
            Outcome::State {
                status: Status::RequiresApproval,
                changed: true
            }
        );
        assert!(out.message().contains("Login Items"));
    }

    #[test]
    fn status_never_changes_anything() {
        for st in [
            Status::NotRegistered,
            Status::Enabled,
            Status::RequiresApproval,
            Status::NotFound,
        ] {
            let f = Fake::new(st);
            f.marker.set(true); // a NotFound with a marker stays NotFound
            assert_eq!(
                run(Command::Status, &f).unwrap(),
                Outcome::State {
                    status: st,
                    changed: false
                }
            );
            assert!(f.calls().is_empty());
        }
    }

    #[test]
    fn unavailable_is_data_for_status_and_an_error_otherwise() {
        let mut f = Fake::new(Status::NotRegistered);
        f.available = Err(Unavailable::NotInBundle);
        let out = run(Command::Status, &f).unwrap();
        assert_eq!(out, Outcome::Unavailable(Unavailable::NotInBundle));
        assert!(out.message().contains("packaging/build-app.sh"));
        assert_eq!(
            run(Command::Enable, &f),
            Err(Error::Unavailable(Unavailable::NotInBundle))
        );
        assert_eq!(
            run(Command::Disable, &f),
            Err(Error::Unavailable(Unavailable::NotInBundle))
        );
        assert!(f.calls().is_empty(), "nothing may be registered when unavailable");

        f.available = Err(Unavailable::OsTooOld);
        assert!(run(Command::Enable, &f).unwrap_err().to_string().contains("macOS 13"));
    }

    #[test]
    fn system_errors_and_silent_failures_are_reported() {
        let mut f = Fake::new(Status::NotRegistered);
        f.fail = Some("Operation not permitted");
        let err = run(Command::Enable, &f).unwrap_err();
        assert_eq!(err, Error::System("Operation not permitted".into()));
        assert!(err.to_string().contains("Operation not permitted"));

        let mut f = Fake::new(Status::NotRegistered);
        f.stubborn = true;
        assert_eq!(
            run(Command::Enable, &f),
            Err(Error::NotApplied {
                wanted: "enable",
                got: Status::NotRegistered
            })
        );
        let mut f = Fake::new(Status::Enabled);
        f.stubborn = true;
        assert_eq!(
            run(Command::Disable, &f),
            Err(Error::NotApplied {
                wanted: "disable",
                got: Status::Enabled
            })
        );
    }

    #[test]
    fn bundle_path_detection() {
        let yes = |p: &str| is_app_bundle_exe(&PathBuf::from(p));
        assert!(yes("/Applications/Ashdash.app/Contents/MacOS/ashdash"));
        assert!(yes("/tmp/x y/Ashdash.app/Contents/MacOS/ashdash"));
        assert!(!yes("/Users/me/project/target/release/ashdash"));
        assert!(!yes("/Applications/Ashdash.app/Contents/Resources/ashdash"));
        assert!(!yes("/Applications/Ashdash/Contents/MacOS/ashdash"));
        assert!(!yes("ashdash"));
        assert!(!yes("/Contents/MacOS/ashdash"));
    }

    #[test]
    fn this_test_binary_is_not_in_a_bundle() {
        // cargo test runs a bare binary, so the real backend must refuse politely.
        assert_eq!(SystemBackend.availability(), Err(Unavailable::NotInBundle));
        assert!(matches!(
            run(Command::Enable, &SystemBackend),
            Err(Error::Unavailable(Unavailable::NotInBundle))
        ));
    }
}
