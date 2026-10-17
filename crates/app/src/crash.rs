//! Crash reports: after a panic you can find out why.
//!
//! [`install`] chains a panic hook in the launcher and in the preferences window process. A panic
//! writes `<data dir>/crashes/crash-<UTC timestamp>.txt` (version, process kind, thread, message,
//! location, backtrace) and keeps the newest [`KEEP`] reports. Nothing else goes into a report: no
//! query text, no clipboard content, no file contents, no settings. The panic message is the one
//! free-form string and it is written by the code that panicked (a bug report should be read
//! before it is shared). The next launcher start shows a one-time footer note and `ashdash paths`
//! names the newest report.
//!
//! A report is never uploaded anywhere; this is a text file in your data folder.

use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ashdash_core::Paths;

/// Reports kept; older ones are deleted when a new one is written.
pub const KEEP: usize = 10;

const PREFIX: &str = "crash-";
const SUFFIX: &str = ".txt";
/// Name of the file (inside the crashes folder) that remembers the newest report already shown.
const SEEN_FILE: &str = "last-seen";

/// `<data dir>/crashes`.
pub fn crashes_dir(paths: &Paths) -> PathBuf {
    paths.data.join("crashes")
}

/// Chain a panic hook that writes a report, then runs the previous hook (the default one prints
/// the message and location on stderr). `kind` names the process: `launcher` or `prefs-window`.
pub fn install(paths: &Paths, kind: &'static str) {
    let dir = crashes_dir(paths);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // a failure while reporting must never turn into a second panic (which aborts)
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let thread = std::thread::current();
            let message = match info.payload().downcast_ref::<&str>() {
                Some(s) => (*s).to_string(),
                None => match info.payload().downcast_ref::<String>() {
                    Some(s) => s.clone(),
                    None => "(non-text panic payload)".to_string(),
                },
            };
            let location = info.location().map_or_else(
                || "unknown".to_string(),
                |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
            );
            let report = Report {
                version: env!("CARGO_PKG_VERSION"),
                kind,
                thread: thread.name().unwrap_or("<unnamed>").to_string(),
                message,
                location,
                backtrace: Backtrace::force_capture().to_string(),
            };
            let _ = write_report(&dir, &report, SystemTime::now());
        }));
        previous(info);
    }));
}

/// Everything a report says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub version: &'static str,
    pub kind: &'static str,
    pub thread: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
}

impl Report {
    fn render(&self, stamp: &str) -> String {
        format!(
            "Ashdash crash report\n\
             version:  {}\n\
             process:  {}\n\
             time:     {stamp}\n\
             thread:   {}\n\
             location: {}\n\
             message:  {}\n\
             \n\
             This file holds the panic message above and a backtrace; nothing you typed, copied or\n\
             opened is added to it. It stays on this machine.\n\
             \n\
             backtrace:\n{}\n",
            self.version, self.kind, self.thread, self.location, self.message, self.backtrace
        )
    }
}

/// Write `report` into `dir` (created if missing) and delete all but the newest [`KEEP`] reports.
/// Returns the file written.
pub fn write_report(dir: &Path, report: &Report, now: SystemTime) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut millis = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let path = loop {
        let stamp = utc_stamp(millis);
        let path = dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut f) => {
                f.write_all(report.render(&utc_iso(millis)).as_bytes())?;
                break path;
            }
            // two reports in the same millisecond: the later one gets the next name
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => millis += 1,
            Err(e) => return Err(e),
        }
    };
    rotate(dir, KEEP);
    Ok(path)
}

/// Delete all but the newest `keep` reports (file names sort by time).
fn rotate(dir: &Path, keep: usize) {
    let names = report_names(dir);
    for old in names.iter().take(names.len().saturating_sub(keep)) {
        let _ = fs::remove_file(dir.join(old));
    }
}

/// The report file names in `dir`, oldest first.
pub fn report_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        .collect();
    names.sort();
    names
}

/// The newest report if it is newer than `seen` (or nothing was seen yet).
pub fn newer_than_seen(names: &[String], seen: Option<&str>) -> Option<String> {
    let newest = names.iter().max()?;
    match seen {
        Some(s) if newest.as_str() <= s.trim() => None,
        _ => Some(newest.clone()),
    }
}

/// The newest report the launcher has not announced yet.
pub fn unseen(paths: &Paths) -> Option<String> {
    let dir = crashes_dir(paths);
    let seen = fs::read_to_string(dir.join(SEEN_FILE)).ok();
    newer_than_seen(&report_names(&dir), seen.as_deref())
}

/// The newest report, if any (for `ashdash paths`).
pub fn latest(paths: &Paths) -> Option<String> {
    report_names(&crashes_dir(paths)).pop()
}

/// Remember that `name` has been announced.
pub fn mark_seen(paths: &Paths, name: &str) {
    let dir = crashes_dir(paths);
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(SEEN_FILE), name);
    }
}

/// The footer text for a report not announced yet.
pub fn footer_note() -> &'static str {
    "Ashdash quit unexpectedly · report in crashes/"
}

/// `20261017T120102.345Z`: sorts as time does.
fn utc_stamp(millis: u64) -> String {
    let (y, mo, d, h, mi, s, ms) = civil(millis);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}.{ms:03}Z")
}

/// `2026-10-17T12:01:02.345Z`.
fn utc_iso(millis: u64) -> String {
    let (y, mo, d, h, mi, s, ms) = civil(millis);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// Milliseconds since the epoch as (year, month, day, hour, minute, second, millisecond), UTC.
/// Days to civil date after Howard Hinnant's algorithm.
fn civil(millis: u64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
        (millis % 1000) as u32,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-crash-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn report(message: &str) -> Report {
        Report {
            version: "9.9.9",
            kind: "launcher",
            thread: "main".into(),
            message: message.into(),
            location: "src/x.rs:1:2".into(),
            backtrace: "0: frame".into(),
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn timestamps_are_utc_and_sort_as_time() {
        assert_eq!(utc_stamp(0), "19700101T000000.000Z");
        // 2026-10-17 12:01:02.345 UTC
        let ms = (20_743 * 86_400 + 12 * 3600 + 60 + 2) * 1000 + 345;
        assert_eq!(utc_stamp(ms), "20261017T120102.345Z");
        assert_eq!(utc_iso(ms), "2026-10-17T12:01:02.345Z");
        // leap day and a year end
        assert_eq!(utc_stamp(951_782_400_000), "20000229T000000.000Z");
        assert_eq!(utc_stamp(1_767_225_599_000), "20251231T235959.000Z");
        assert!(utc_stamp(ms) < utc_stamp(ms + 1));
        assert!(utc_stamp(999_999_999_999) < utc_stamp(1_000_000_000_000));
    }

    #[test]
    fn a_report_names_version_process_thread_message_and_location() {
        let dir = tmp("write");
        let path = write_report(&dir, &report("index out of bounds"), at(1_792_238_462)).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("crash-2026") && name.ends_with("Z.txt"), "{name}");
        let text = fs::read_to_string(&path).unwrap();
        for needle in [
            "version:  9.9.9",
            "process:  launcher",
            "thread:   main",
            "location: src/x.rs:1:2",
            "message:  index out of bounds",
            "0: frame",
        ] {
            assert!(text.contains(needle), "{needle} missing in\n{text}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_last_ten_reports_are_kept() {
        let dir = tmp("rotate");
        for i in 0..13u64 {
            write_report(&dir, &report(&format!("panic {i}")), at(1_700_000_000 + i * 60)).unwrap();
        }
        let names = report_names(&dir);
        assert_eq!(names.len(), KEEP);
        let oldest = fs::read_to_string(dir.join(&names[0])).unwrap();
        let newest = fs::read_to_string(dir.join(names.last().unwrap())).unwrap();
        assert!(oldest.contains("panic 3"), "the three oldest are gone: {oldest}");
        assert!(newest.contains("panic 12"));
        // files that are not reports are left alone
        fs::write(dir.join("last-seen"), "x").unwrap();
        write_report(&dir, &report("one more"), at(1_800_000_000)).unwrap();
        assert!(dir.join("last-seen").exists());
        assert_eq!(report_names(&dir).len(), KEEP);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn two_reports_in_the_same_millisecond_both_survive_in_order() {
        let dir = tmp("same-ms");
        let a = write_report(&dir, &report("first"), at(1_700_000_000)).unwrap();
        let b = write_report(&dir, &report("second"), at(1_700_000_000)).unwrap();
        assert_ne!(a, b);
        let names = report_names(&dir);
        assert_eq!(names.len(), 2);
        assert!(fs::read_to_string(dir.join(&names[1])).unwrap().contains("second"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn newer_than_last_seen() {
        let names: Vec<String> = ["crash-20261017T100000.000Z.txt", "crash-20261017T110000.000Z.txt"]
            .map(String::from)
            .to_vec();
        // never seen anything: announce the newest
        assert_eq!(newer_than_seen(&names, None).as_deref(), Some(names[1].as_str()));
        // seen the older one: the newer is announced
        assert_eq!(
            newer_than_seen(&names, Some(&names[0])).as_deref(),
            Some(names[1].as_str())
        );
        // seen the newest (also with a trailing newline in the marker): nothing
        assert_eq!(newer_than_seen(&names, Some(&names[1])), None);
        assert_eq!(newer_than_seen(&names, Some(&format!("{}\n", names[1]))), None);
        // the marker names a report that was rotated away but is newer than all kept: nothing
        assert_eq!(newer_than_seen(&names, Some("crash-20261018T000000.000Z.txt")), None);
        // no reports at all
        assert_eq!(newer_than_seen(&[], None), None);
        assert_eq!(newer_than_seen(&[], Some("x")), None);
    }

    #[test]
    fn announcing_a_report_once_then_a_newer_one() {
        let root = tmp("seen");
        let paths = Paths::under(&root);
        assert_eq!(unseen(&paths), None);
        assert_eq!(latest(&paths), None);
        let dir = crashes_dir(&paths);
        write_report(&dir, &report("one"), at(1_700_000_000)).unwrap();
        let first = unseen(&paths).expect("a new report");
        assert_eq!(latest(&paths).as_deref(), Some(first.as_str()));
        mark_seen(&paths, &first);
        assert_eq!(unseen(&paths), None, "announced once only");
        assert_eq!(latest(&paths).as_deref(), Some(first.as_str()), "paths still names it");
        write_report(&dir, &report("two"), at(1_700_000_100)).unwrap();
        let second = unseen(&paths).expect("the newer report");
        assert!(second > first);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_hook_writes_a_report_and_chains_the_previous_hook() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        // the panic hook is process-global: one test at a time touches it
        static HOOK: Mutex<()> = Mutex::new(());
        let _guard = HOOK.lock().unwrap_or_else(|e| e.into_inner());

        let root = tmp("hook");
        let paths = Paths::under(&root);
        let original = std::panic::take_hook();
        let chained = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&chained);
        std::panic::set_hook(Box::new(move |_| flag.store(true, Ordering::SeqCst)));
        install(&paths, "prefs-window");

        let result = std::thread::Builder::new()
            .name("crash-test-thread".into())
            .spawn(|| -> () { panic!("boom-4711") })
            .unwrap()
            .join();
        assert!(result.is_err());
        let ours = std::panic::take_hook();
        drop(ours);
        std::panic::set_hook(original);

        assert!(chained.load(Ordering::SeqCst), "the previous hook still ran");
        let dir = crashes_dir(&paths);
        let text = report_names(&dir)
            .iter()
            .map(|n| fs::read_to_string(dir.join(n)).unwrap())
            .find(|t| t.contains("boom-4711"))
            .expect("a report with the panic message");
        assert!(text.contains("process:  prefs-window"), "{text}");
        assert!(text.contains("thread:   crash-test-thread"), "{text}");
        assert!(text.contains("crash.rs:"), "the location is the panic site: {text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("backtrace:"));
        let _ = fs::remove_dir_all(root);
    }
}
