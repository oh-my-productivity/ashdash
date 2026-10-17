//! What macOS permissions Ashdash has, for the Permissions page and the General page's
//! "Full Disk Access not granted" card.
//!
//! **Probing never shows a macOS prompt.** Each probe is one of the calls Apple documents as
//! silent:
//!
//! * Accessibility: `AXIsProcessTrusted()` (not the `...WithOptions` variant that can prompt).
//! * Automation: `AEDeterminePermissionToAutomateTarget(..., askUserIfNeeded = false)`. A target
//!   that has never been asked reports "would require user consent" ([`Grant::AskedOnFirstUse`]);
//!   a target that is not running cannot be asked at all ([`Grant::Unknown`]).
//! * Full Disk Access: opening `~/Library/Safari/Bookmarks.plist` read-only (the file `bm` reads).
//!   FDA is the one TCC class that never prompts, a refused `open` is just `EPERM`.
//!
//! The mapping from raw answers to [`Grant`] is in pure functions ([`automation_from_status`],
//! [`full_disk_access_from_open`]); the system calls hide behind [`Probes`] so the page and its
//! tests use a fake. The answers are about the process that asks: the preferences window is the
//! same bundle as the launcher, so the grants are the same ones when it was started from
//! `Ashdash.app`; started from a terminal, macOS attributes them to the terminal.

use std::ffi::CStr;
use std::io;
use std::path::PathBuf;

/// What a probe could find out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Granted,
    NotGranted,
    /// Never asked: macOS shows its prompt the first time the feature is used.
    AskedOnFirstUse,
    /// Could not be determined (target not running, file missing, unexpected error).
    Unknown,
}

impl Grant {
    /// The word on the page.
    pub fn word(self) -> &'static str {
        match self {
            Grant::Granted => "Granted",
            Grant::NotGranted => "Not granted",
            Grant::AskedOnFirstUse => "Asked on first use",
            Grant::Unknown => "Unknown",
        }
    }
}

/// A pane of System Settings > Privacy & Security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Accessibility,
    Automation,
    FullDiskAccess,
}

impl Pane {
    /// The `x-apple.systempreferences:` deep link that opens this pane (works on macOS 13+ and
    /// before; `open <url>` is all it needs).
    pub fn url(self) -> &'static str {
        match self {
            Pane::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            Pane::Automation => "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
            Pane::FullDiskAccess => "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
        }
    }

    /// Open the pane. Returns at once; failures are only reported (nothing else to do).
    pub fn open(self) -> io::Result<()> {
        std::process::Command::new("/usr/bin/open")
            .arg(self.url())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

/// An app Ashdash drives with Apple events, and what for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomationTarget {
    pub name: &'static str,
    pub bundle_id: &'static str,
    pub used_for: &'static str,
}

/// The apps Ashdash's built-in actions talk to (`gui/glue`, `engine::system`, workflows'
/// "Run in Terminal"): one line each on the page.
pub const AUTOMATION_TARGETS: [AutomationTarget; 3] = [
    AutomationTarget {
        name: "System Events",
        bundle_id: "com.apple.systemevents",
        used_for: "log out, restart, notifications",
    },
    AutomationTarget {
        name: "Finder",
        bundle_id: "com.apple.finder",
        used_for: "empty trash, reveal files",
    },
    AutomationTarget {
        name: "Terminal",
        bundle_id: "com.apple.Terminal",
        used_for: "commands typed after >",
    },
];

/// The silent probes. [`SystemProbes`] asks macOS; tests use a fake.
pub trait Probes: Send + Sync {
    fn accessibility(&self) -> Grant;
    fn automation(&self, bundle_id: &str) -> Grant;
    fn full_disk_access(&self) -> Grant;
}

/// Everything the page shows, from one round of probing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub accessibility: Grant,
    /// One entry per [`AUTOMATION_TARGETS`] item, same order.
    pub automation: Vec<Grant>,
    pub full_disk_access: Grant,
}

pub fn check(probes: &dyn Probes) -> Snapshot {
    Snapshot {
        accessibility: probes.accessibility(),
        automation: AUTOMATION_TARGETS
            .iter()
            .map(|t| probes.automation(t.bundle_id))
            .collect(),
        full_disk_access: probes.full_disk_access(),
    }
}

// ------------------------------------------------------------------ raw answer -> Grant

const NO_ERR: i32 = 0;
/// `errAEEventNotPermitted`: the user (or an MDM profile) said no.
const ERR_AE_NOT_PERMITTED: i32 = -1743;
/// `errAEEventWouldRequireUserConsent`: never asked; a real event would show the prompt.
const ERR_AE_WOULD_REQUIRE_CONSENT: i32 = -1744;

/// The `OSStatus` of `AEDeterminePermissionToAutomateTarget` as a [`Grant`]. `procNotFound`
/// (-600, the target is not running) and everything else is [`Grant::Unknown`].
pub fn automation_from_status(status: i32) -> Grant {
    match status {
        NO_ERR => Grant::Granted,
        ERR_AE_NOT_PERMITTED => Grant::NotGranted,
        ERR_AE_WOULD_REQUIRE_CONSENT => Grant::AskedOnFirstUse,
        _ => Grant::Unknown,
    }
}

/// The outcome of opening the protected file read-only. `EPERM`/`EACCES` is the refusal; a
/// missing file (no Safari profile on this Mac) says nothing about the grant.
pub fn full_disk_access_from_open(result: &io::Result<()>) -> Grant {
    match result {
        Ok(()) => Grant::Granted,
        Err(e) => match e.raw_os_error() {
            Some(libc::EPERM) | Some(libc::EACCES) => Grant::NotGranted,
            _ => match e.kind() {
                io::ErrorKind::PermissionDenied => Grant::NotGranted,
                _ => Grant::Unknown,
            },
        },
    }
}

/// The user's real home folder from the account database, not `$HOME`: the lab and the tests
/// run with a throw-away `HOME`, but the protection belongs to the real folder.
fn real_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into libc's static buffer (or null); we copy out of it at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(PathBuf::from(
            CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned(),
        ))
    }
}

/// The file `bm` reads for Safari; protected by Full Disk Access.
pub fn protected_probe_path() -> Option<PathBuf> {
    Some(real_home()?.join("Library/Safari/Bookmarks.plist"))
}

/// Its folder: protected the same way, and present on a Mac whose user never bookmarked anything
/// (the file is not).
fn protected_probe_dir() -> Option<PathBuf> {
    Some(real_home()?.join("Library/Safari"))
}

/// Open the file; if it does not exist, list its folder instead. Opens and lists only, nothing is read.
pub fn probe_protected_files() -> Grant {
    let (Some(file), Some(dir)) = (protected_probe_path(), protected_probe_dir()) else {
        return Grant::Unknown;
    };
    let first = full_disk_access_from_open(&std::fs::File::open(file).map(|_| ()));
    if first != Grant::Unknown {
        return first;
    }
    full_disk_access_from_open(&std::fs::read_dir(dir).map(|_| ()))
}

// ------------------------------------------------------------------ screenshots and the lab

/// For screenshots and the VM lab only: `ASHDASH_PREFS_FAKE_PERMISSIONS=fda=denied,automation=ask`
/// replaces the answer of a probe (keys `accessibility`, `automation`, `fda`; values `granted`,
/// `denied`, `ask`, `unknown`). A VM cannot be made to refuse Full Disk Access reliably, and a
/// screenshot of the "not granted" states needs them. It changes what the window shows, nothing else.
pub const OVERRIDE_ENV: &str = "ASHDASH_PREFS_FAKE_PERMISSIONS";

/// One probe's override from the text of [`OVERRIDE_ENV`].
pub fn override_for(spec: &str, key: &str) -> Option<Grant> {
    spec.split(',').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        match v.trim() {
            "granted" => Some(Grant::Granted),
            "denied" => Some(Grant::NotGranted),
            "ask" => Some(Grant::AskedOnFirstUse),
            "unknown" => Some(Grant::Unknown),
            _ => None,
        }
    })
}

fn env_override(key: &str) -> Option<Grant> {
    override_for(&std::env::var(OVERRIDE_ENV).ok()?, key)
}

// ------------------------------------------------------------------ the real thing

/// Asks macOS. All calls are silent (see the module docs).
pub struct SystemProbes;

impl Probes for SystemProbes {
    fn accessibility(&self) -> Grant {
        if let Some(g) = env_override("accessibility") {
            return g;
        }
        // SAFETY: plain query, no arguments.
        if unsafe { ffi::AXIsProcessTrusted() } != 0 {
            Grant::Granted
        } else {
            Grant::NotGranted
        }
    }

    fn automation(&self, bundle_id: &str) -> Grant {
        if let Some(g) = env_override("automation") {
            return g;
        }
        ffi::automation_status(bundle_id).map_or(Grant::Unknown, automation_from_status)
    }

    fn full_disk_access(&self) -> Grant {
        if let Some(g) = env_override("fda") {
            return g;
        }
        probe_protected_files()
    }
}

mod ffi {
    use std::ffi::c_void;

    /// `AEDesc`: a descriptor type and an opaque handle.
    #[repr(C)]
    pub struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }

    /// `typeApplicationBundleID` ('bund').
    const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
    /// `typeWildCard` ('****'): "any event class / id", which asks about the app as a whole.
    const WILDCARD: u32 = u32::from_be_bytes(*b"****");

    // CoreServices has the Apple-event calls, ApplicationServices the accessibility one
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub fn AXIsProcessTrusted() -> u8;
        fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, class: u32, id: u32, ask: u8) -> i32;
    }

    /// The `OSStatus` for `bundle_id`, or `None` when the descriptor could not be built.
    pub fn automation_status(bundle_id: &str) -> Option<i32> {
        let mut desc = AEDesc {
            descriptor_type: 0,
            data_handle: std::ptr::null_mut(),
        };
        // SAFETY: `desc` is a valid out-pointer and the bytes outlive the call (AECreateDesc copies them);
        // the descriptor is disposed before returning. askUserIfNeeded is 0: this never prompts.
        unsafe {
            let made = AECreateDesc(
                TYPE_APPLICATION_BUNDLE_ID,
                bundle_id.as_ptr().cast(),
                bundle_id.len() as isize,
                &mut desc,
            );
            if made != 0 {
                return None;
            }
            let status = AEDeterminePermissionToAutomateTarget(&desc, WILDCARD, WILDCARD, 0);
            AEDisposeDesc(&mut desc);
            Some(status)
        }
    }
}

#[cfg(test)]
pub mod fake {
    use super::{Grant, Probes};

    /// A probe set with fixed answers that records what was asked.
    pub struct Fake {
        pub accessibility: Grant,
        pub automation: Grant,
        pub fda: Grant,
        pub asked: std::sync::Mutex<Vec<String>>,
        /// How many rounds of probing ran (counted by `accessibility`, asked once per round).
        pub rounds: std::sync::atomic::AtomicUsize,
    }

    impl Fake {
        pub fn all(g: Grant) -> Fake {
            Fake {
                accessibility: g,
                automation: g,
                fda: g,
                asked: Default::default(),
                rounds: Default::default(),
            }
        }
    }

    impl Probes for Fake {
        fn accessibility(&self) -> Grant {
            self.rounds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.accessibility
        }
        fn automation(&self, bundle_id: &str) -> Grant {
            self.asked.lock().unwrap().push(bundle_id.to_string());
            self.automation
        }
        fn full_disk_access(&self) -> Grant {
            self.fda
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    #[test]
    fn the_four_words() {
        assert_eq!(Grant::Granted.word(), "Granted");
        assert_eq!(Grant::NotGranted.word(), "Not granted");
        assert_eq!(Grant::AskedOnFirstUse.word(), "Asked on first use");
        assert_eq!(Grant::Unknown.word(), "Unknown");
    }

    #[test]
    fn automation_statuses_map_to_grants() {
        assert_eq!(automation_from_status(0), Grant::Granted);
        assert_eq!(automation_from_status(-1743), Grant::NotGranted);
        assert_eq!(automation_from_status(-1744), Grant::AskedOnFirstUse);
        assert_eq!(
            automation_from_status(-600),
            Grant::Unknown,
            "procNotFound: app not running"
        );
        assert_eq!(automation_from_status(-1), Grant::Unknown);
    }

    #[test]
    fn a_refused_open_is_not_granted_and_a_missing_file_is_unknown() {
        assert_eq!(full_disk_access_from_open(&Ok(())), Grant::Granted);
        for code in [libc::EPERM, libc::EACCES] {
            let e = Err(io::Error::from_raw_os_error(code));
            assert_eq!(full_disk_access_from_open(&e), Grant::NotGranted, "{code}");
        }
        let missing = Err(io::Error::from_raw_os_error(libc::ENOENT));
        assert_eq!(full_disk_access_from_open(&missing), Grant::Unknown);
        let other = Err(io::Error::other("x"));
        assert_eq!(full_disk_access_from_open(&other), Grant::Unknown);
    }

    #[test]
    fn the_lab_override_parses_known_keys_and_values_only() {
        let spec = "fda=denied, automation=ask,accessibility=unknown";
        assert_eq!(override_for(spec, "fda"), Some(Grant::NotGranted));
        assert_eq!(override_for(spec, "automation"), Some(Grant::AskedOnFirstUse));
        assert_eq!(override_for(spec, "accessibility"), Some(Grant::Unknown));
        assert_eq!(override_for("fda=granted", "fda"), Some(Grant::Granted));
        assert_eq!(override_for("fda=maybe", "fda"), None);
        assert_eq!(override_for("fda=denied", "automation"), None);
        assert_eq!(override_for("", "fda"), None);
    }

    /// `cargo test -p ashdash real_probes -- --ignored --nocapture` prints what this process is
    /// really granted and how long the silent probes took (no prompt may appear, nothing may block).
    #[test]
    #[ignore = "prints the real answers of this machine"]
    fn real_probes_print_their_answers() {
        let t = std::time::Instant::now();
        let snap = check(&SystemProbes);
        eprintln!("PROBES {snap:?} in {:?}", t.elapsed());
    }

    #[test]
    fn every_pane_has_a_security_deep_link() {
        for pane in [Pane::Accessibility, Pane::Automation, Pane::FullDiskAccess] {
            assert!(
                pane.url()
                    .starts_with("x-apple.systempreferences:com.apple.preference.security?Privacy_"),
                "{pane:?}"
            );
        }
        assert!(Pane::FullDiskAccess.url().ends_with("Privacy_AllFiles"));
        assert!(Pane::Accessibility.url().ends_with("Privacy_Accessibility"));
        assert!(Pane::Automation.url().ends_with("Privacy_Automation"));
    }

    #[test]
    fn a_check_asks_each_target_once_and_keeps_the_order() {
        let fake = Fake {
            accessibility: Grant::NotGranted,
            automation: Grant::AskedOnFirstUse,
            fda: Grant::Unknown,
            asked: Default::default(),
            rounds: Default::default(),
        };
        let snap = check(&fake);
        assert_eq!(snap.accessibility, Grant::NotGranted);
        assert_eq!(snap.automation, vec![Grant::AskedOnFirstUse; 3]);
        assert_eq!(snap.full_disk_access, Grant::Unknown);
        let asked = fake.asked.lock().unwrap().clone();
        let want: Vec<String> = AUTOMATION_TARGETS.iter().map(|t| t.bundle_id.to_string()).collect();
        assert_eq!(asked, want);
    }

    #[test]
    fn the_probe_path_is_in_the_real_home_not_in_a_throwaway_home() {
        let p = protected_probe_path().expect("a passwd entry");
        assert!(p.ends_with("Library/Safari/Bookmarks.plist"));
        let throwaway = std::env::temp_dir();
        assert!(!p.starts_with(throwaway));
    }

    /// The real probes run in CI and on a dev machine without ever prompting; only check that
    /// they answer (the value depends on the machine).
    #[test]
    fn the_system_probes_answer_without_blocking() {
        let snap = check(&SystemProbes);
        assert_eq!(snap.automation.len(), AUTOMATION_TARGETS.len());
    }
}
