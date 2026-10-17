//! Crash reports: after a panic you can find out why.
//!
//! [`install`] chains a panic hook in the launcher and in the preferences window process. A panic
//! writes `<data dir>/crashes/crash-<UTC timestamp>.txt` (version, process kind, thread, outcome,
//! message, location, backtrace) and keeps the newest [`KEEP`] reports. Nothing is added from the
//! launcher's state: no query field, clipboard content, file contents or settings are read. The
//! panic message is the one free-form string and it is written by the code that panicked; Rust
//! itself quotes the offending string in some messages (slicing a `str` off a character boundary,
//! `unwrap` on an error), so it **may contain text the program was working on** (a search, a file
//! name). It is cut to its first line and [`MESSAGE_MAX`] characters, and a report should be read
//! before it is shared.
//!
//! A report says whether the panic was *fatal* or *recovered* (caught by a guard, or on a thread
//! whose death the program handles; see `ashdash_core::recovery`). The next launcher start shows
//! a one-time footer note, worded by that, and `ashdash paths` names the newest report. The same
//! panic (message and location) repeating in one process increments a `repeated:` line in its
//! report instead of writing a new file, so a loop cannot push the first report out.
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

/// The panic message is cut to its first line and at most this many characters.
pub const MESSAGE_MAX: usize = 200;

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
        // A panic inside a panic hook aborts the process (it cannot be caught), so nothing here may
        // panic: no indexing, no unwrap, I/O errors are dropped. Reporting is best effort.
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
            pid: std::process::id(),
            recovered: ashdash_core::recovery::is_recovered(),
            thread: thread.name().unwrap_or("<unnamed>").to_string(),
            message,
            location,
            backtrace: Backtrace::force_capture().to_string(),
        };
        let _ = write_report(&dir, &report, SystemTime::now());
        previous(info);
    }));
}

/// Everything a report says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub version: &'static str,
    pub kind: &'static str,
    /// The process id: "the same process" for [`write_report`]'s repeat counting.
    pub pid: u32,
    /// The program survived this panic (see `ashdash_core::recovery`).
    pub recovered: bool,
    pub thread: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
}

/// First line of `message`, at most [`MESSAGE_MAX`] characters (the last one `…` when cut).
pub fn clip_message(message: &str) -> String {
    let line = message.lines().next().unwrap_or("").trim_end();
    if line.chars().count() <= MESSAGE_MAX {
        return line.to_string();
    }
    let cut: String = line.chars().take(MESSAGE_MAX - 1).collect();
    format!("{cut}…")
}

const OUTCOME_FATAL: &str = "fatal";
const OUTCOME_RECOVERED: &str = "recovered (the program carried on)";

impl Report {
    fn render(&self, stamp: &str) -> String {
        format!(
            "Ashdash crash report\n\
             version:  {}\n\
             process:  {}\n\
             pid:      {}\n\
             outcome:  {}\n\
             time:     {stamp}\n\
             thread:   {}\n\
             location: {}\n\
             message:  {}\n\
             \n\
             The message above is the panic message, cut to its first line. It may quote text the\n\
             program was working on (a search, a file name): read this file before you share it.\n\
             The rest is a backtrace of code locations. It stays on this machine.\n\
             \n\
             backtrace:\n{}\n",
            self.version,
            self.kind,
            self.pid,
            if self.recovered {
                OUTCOME_RECOVERED
            } else {
                OUTCOME_FATAL
            },
            self.thread,
            self.location,
            clip_message(&self.message),
            self.backtrace
        )
    }

    /// Does `text` (a report file) describe this same panic in this same process?
    fn is_repeat_of(&self, text: &str) -> bool {
        let line = |key: &str| text.lines().find_map(|l| l.strip_prefix(key));
        line("pid:      ") == Some(self.pid.to_string().as_str())
            && line("location: ") == Some(self.location.as_str())
            && line("message:  ") == Some(clip_message(&self.message).as_str())
    }
}

/// `text` with its `repeated: N times` line incremented (added after `message:` the first time).
fn count_repeat(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 24);
    let mut done = false;
    let has_count = text.lines().any(|l| l.starts_with("repeated: "));
    for line in text.split_inclusive('\n') {
        if let Some(n) = line.trim_end().strip_prefix("repeated: ") {
            let n: u64 = n.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            out.push_str(&format!("repeated: {} times\n", n.saturating_add(1)));
            done = true;
        } else {
            out.push_str(line);
            if !has_count && !done && line.starts_with("message:  ") {
                out.push_str("repeated: 1 times\n");
                done = true;
            }
        }
    }
    out
}

/// Write `report` into `dir` (created if missing) and delete all but the newest [`KEEP`] reports.
/// Returns the file written.
///
/// A panic identical (pid, location, message) to the newest report is counted in that report
/// (`repeated: N times`) and no file is written.
pub fn write_report(dir: &Path, report: &Report, now: SystemTime) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    if let Some(newest) = report_names(dir).pop() {
        let path = dir.join(newest);
        if let Ok(text) = fs::read_to_string(&path)
            && report.is_repeat_of(&text)
        {
            fs::write(&path, count_repeat(&text))?;
            return Ok(path);
        }
    }
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
    unseen_reports(paths).map(|u| u.newest)
}

/// What the launcher has not announced yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unseen {
    /// The newest report (what to mark as seen).
    pub newest: String,
    /// At least one unseen report is a fatal one (or cannot be read: assume the worst).
    pub fatal: bool,
}

/// The reports newer than the last one announced, if any.
pub fn unseen_reports(paths: &Paths) -> Option<Unseen> {
    let dir = crashes_dir(paths);
    let seen = fs::read_to_string(dir.join(SEEN_FILE)).ok();
    let names = report_names(&dir);
    let newest = newer_than_seen(&names, seen.as_deref())?;
    let seen = seen.as_deref().map(str::trim);
    let fatal = names
        .iter()
        .filter(|n| seen.is_none_or(|s| n.as_str() > s))
        .any(|n| !is_recovered_report(&dir.join(n)));
    Some(Unseen { newest, fatal })
}

fn is_recovered_report(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| t.lines().any(|l| l.starts_with("outcome:  recovered")))
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

/// The footer text for a report not announced yet: `fatal` is "quit", otherwise the panic was
/// survived.
pub fn footer_note(fatal: bool) -> &'static str {
    if fatal {
        "Ashdash quit unexpectedly · report in crashes/"
    } else {
        "Ashdash hit an internal error and carried on · report in crashes/"
    }
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
            pid: 4711,
            recovered: false,
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

    /// Run `body` with the crash hook installed on `paths` (the previous hook only records that
    /// it ran). The panic hook is process-global: one test at a time touches it.
    fn with_hook(paths: &Paths, kind: &'static str, body: impl FnOnce()) -> bool {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        static HOOK: Mutex<()> = Mutex::new(());
        let _guard = HOOK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::panic::take_hook();
        let chained = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&chained);
        std::panic::set_hook(Box::new(move |_| flag.store(true, Ordering::SeqCst)));
        install(paths, kind);
        body();
        drop(std::panic::take_hook());
        std::panic::set_hook(original);
        chained.load(Ordering::SeqCst)
    }

    /// The reports in `paths`' folder. The panic hook is process-wide, so a test elsewhere in this
    /// binary that panics on purpose while a hook is installed can add a report: tests that count
    /// reports look at their own panics only (`mine`).
    fn texts(paths: &Paths) -> Vec<String> {
        let dir = crashes_dir(paths);
        report_names(&dir)
            .iter()
            .map(|n| fs::read_to_string(dir.join(n)).unwrap())
            .collect()
    }

    #[test]
    fn the_hook_writes_a_report_and_chains_the_previous_hook() {
        let root = tmp("hook");
        let paths = Paths::under(&root);
        let chained = with_hook(&paths, "prefs-window", || {
            let result = std::thread::Builder::new()
                .name("crash-test-thread".into())
                .spawn(|| -> () { panic!("boom-4711") })
                .unwrap()
                .join();
            assert!(result.is_err());
        });
        assert!(chained, "the previous hook still ran");
        let text = texts(&paths)
            .into_iter()
            .find(|t| t.contains("boom-4711"))
            .expect("a report with the panic message");
        assert!(text.contains("process:  prefs-window"), "{text}");
        assert!(text.contains("thread:   crash-test-thread"), "{text}");
        assert!(text.contains("crash.rs:"), "the location is the panic site: {text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("backtrace:"));
        assert!(text.contains(&format!("pid:      {}", std::process::id())));
        assert!(text.contains("outcome:  fatal"), "{text}");
        let _ = fs::remove_dir_all(root);
    }

    // ---- Wave 6 review fixes (docs/reviews/wave5.md 3 to 6)

    #[test]
    fn the_message_is_cut_to_its_first_line_and_200_characters() {
        assert_eq!(clip_message("short"), "short");
        assert_eq!(clip_message("first\nsecond SECRET"), "first");
        assert_eq!(clip_message("first\r\nsecond"), "first");
        assert_eq!(clip_message(""), "");
        let long = "é".repeat(500);
        let clipped = clip_message(&long);
        assert_eq!(clipped.chars().count(), MESSAGE_MAX, "cut on a character boundary");
        assert!(clipped.ends_with('…'));
        assert_eq!(
            clip_message(&"x".repeat(MESSAGE_MAX)),
            "x".repeat(MESSAGE_MAX),
            "exactly 200 is kept"
        );
    }

    #[test]
    fn a_str_slice_panic_on_a_query_does_not_carry_the_tail_of_the_query() {
        let root = tmp("slice");
        let paths = Paths::under(&root);
        // std quotes the whole string in the message of this panic
        let query = format!("café {} TAIL-SECRET\nsecond-line-secret", "a".repeat(220));
        with_hook(&paths, "launcher", || {
            let q = query.clone();
            let result = std::thread::spawn(move || {
                let n = q.len().min(4); // inside the 'é'
                let _ = &q[..n];
            })
            .join();
            assert!(result.is_err());
        });
        let text = texts(&paths)
            .into_iter()
            .find(|t| t.contains("char boundary"))
            .expect("a report");
        let message = text.lines().find(|l| l.starts_with("message:")).unwrap();
        assert!(message.contains("not a char boundary"), "{message}");
        assert!(message.chars().count() <= "message:  ".len() + MESSAGE_MAX, "{message}");
        assert!(
            !text.contains("TAIL-SECRET"),
            "the tail of the query is in the report:\n{text}"
        );
        assert!(!text.contains("second-line-secret"), "{text}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_report_says_honestly_that_the_message_may_quote_text() {
        let text = report("x").render("2026-10-17T00:00:00.000Z");
        assert!(!text.contains("nothing you typed"), "{text}");
        assert!(text.contains("may quote text the"), "{text}");
        assert!(text.contains("read this file before you share it"), "{text}");
    }

    #[test]
    fn recovered_panics_are_marked_and_get_their_own_footer() {
        let root = tmp("recovered");
        let paths = Paths::under(&root);
        with_hook(&paths, "launcher", || {
            // inside a guard
            let r = ashdash_core::recovery::catching(|| -> () { panic!("guarded-1") });
            assert!(r.is_err());
            // a marked thread (a per-query or connection thread)
            let r = std::thread::spawn(|| {
                ashdash_core::recovery::mark_thread();
                panic!("marked-2")
            })
            .join();
            assert!(r.is_err());
        });
        let all = texts(&paths);
        for marker in ["message:  guarded-1", "message:  marked-2"] {
            let t = all.iter().find(|t| t.contains(marker)).expect(marker);
            assert!(t.contains("outcome:  recovered"), "{t}");
        }
        assert_eq!(
            footer_note(false),
            "Ashdash hit an internal error and carried on · report in crashes/"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_panic_with_no_recovery_is_fatal_and_says_quit() {
        let root = tmp("fatal");
        let paths = Paths::under(&root);
        with_hook(&paths, "launcher", || {
            assert!(std::thread::spawn(|| panic!("unguarded")).join().is_err());
        });
        let all = texts(&paths);
        let t = all
            .iter()
            .find(|t| t.contains("message:  unguarded"))
            .expect("a report");
        assert!(t.contains("outcome:  fatal"), "{t}");
        assert_eq!(footer_note(true), "Ashdash quit unexpectedly · report in crashes/");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn one_fatal_report_among_unseen_recovered_ones_makes_the_note_fatal() {
        let root = tmp("mixed");
        let paths = Paths::under(&root);
        let dir = crashes_dir(&paths);
        write_report(&dir, &report("old"), at(1_700_000_000)).unwrap();
        let first = unseen_reports(&paths).unwrap();
        assert!(first.fatal);
        mark_seen(&paths, &first.newest);
        // a fatal one, then a recovered one on top of it
        let mut fatal = report("fatal one");
        fatal.location = "src/a.rs:1:1".into();
        write_report(&dir, &fatal, at(1_700_000_100)).unwrap();
        let mut ok = report("recovered one");
        ok.recovered = true;
        write_report(&dir, &ok, at(1_700_000_200)).unwrap();
        let u = unseen_reports(&paths).unwrap();
        assert!(u.fatal, "the fatal report has not been announced yet");
        assert_eq!(u.newest, report_names(&dir).pop().unwrap());
        // only recovered ones are unseen after the fatal one was announced
        mark_seen(&paths, &report_names(&dir)[1]);
        let u = unseen_reports(&paths).unwrap();
        assert!(!u.fatal);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_same_panic_again_is_counted_in_its_report_not_written_again() {
        let dir = tmp("repeat");
        let a = write_report(&dir, &report("boom"), at(1_700_000_000)).unwrap();
        assert!(!fs::read_to_string(&a).unwrap().contains("repeated:"));
        let b = write_report(&dir, &report("boom"), at(1_700_000_001)).unwrap();
        assert_eq!(a, b);
        let c = write_report(&dir, &report("boom"), at(1_700_000_002)).unwrap();
        assert_eq!(a, c);
        assert_eq!(report_names(&dir).len(), 1);
        let text = fs::read_to_string(&a).unwrap();
        assert!(text.contains("\nrepeated: 2 times\n"), "{text}");
        assert_eq!(text.matches("repeated:").count(), 1);
        // the backtrace and the rest are untouched
        assert!(text.contains("0: frame"));
        // a different message, location, or process is a new report
        write_report(&dir, &report("other"), at(1_700_000_003)).unwrap();
        let mut elsewhere = report("other");
        elsewhere.location = "src/y.rs:9:9".into();
        write_report(&dir, &elsewhere, at(1_700_000_004)).unwrap();
        let mut other_process = report("other");
        other_process.location = "src/y.rs:9:9".into();
        other_process.pid = 4712;
        write_report(&dir, &other_process, at(1_700_000_005)).unwrap();
        assert_eq!(report_names(&dir).len(), 4);
        // only the NEWEST report is compared: A, B, A is three reports
        write_report(&dir, &report("boom"), at(1_700_000_006)).unwrap();
        assert_eq!(report_names(&dir).len(), 5);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_panic_loop_leaves_the_first_report_and_a_count() {
        let root = tmp("loop");
        let paths = Paths::under(&root);
        with_hook(&paths, "launcher", || {
            write_report(
                &crashes_dir(&paths),
                &Report {
                    location: "src/root.rs:1:1".into(),
                    ..report("the root cause")
                },
                SystemTime::now(),
            )
            .unwrap();
            for _ in 0..25 {
                let _ = std::thread::spawn(|| -> () { panic!("every keystroke") }).join();
            }
        });
        let mine: Vec<String> = texts(&paths)
            .into_iter()
            .filter(|t| t.contains("the root cause") || t.contains("every keystroke"))
            .collect();
        assert_eq!(mine.len(), 2, "the root cause and one report for the loop");
        assert!(mine[0].contains("the root cause"));
        assert!(mine[1].contains("repeated: 24 times"), "{}", mine[1]);
        let _ = fs::remove_dir_all(root);
    }
}
