//! The platform-independent part of the pasteboard watcher.
//!
//! The app crate implements [`PasteboardSource`] on top of `NSPasteboard`
//! (`crates/app/src/clipboard_watch.rs`) and calls [`Watcher::step`] on a
//! timer. Everything that decides *what* gets recorded lives here so it can
//! be tested with a fake source: change detection, the concealed/transient/
//! per-app filter, and the text > files > image precedence.

use std::path::PathBuf;
use std::sync::Mutex;

use super::filter::{self, SkipReason};
use super::history::{ClipKind, ClipboardHistory};

/// Read access to the system pasteboard. Only [`change_count`](Self::change_count)
/// is called on every poll; the other methods run after a change was seen.
pub trait PasteboardSource {
    /// `NSPasteboard.changeCount`: increments whenever the contents change.
    fn change_count(&self) -> i64;
    /// Type identifiers (UTIs) currently on the pasteboard.
    fn types(&self) -> Vec<String>;
    /// Bundle id of the frontmost application, if known.
    fn frontmost_bundle_id(&self) -> Option<String>;
    /// File URLs on the pasteboard as paths (empty when none).
    fn read_files(&self) -> Vec<PathBuf>;
    fn read_text(&self) -> Option<String>;
    /// PNG data for an image on the pasteboard (converting if necessary).
    /// Implementations should refuse absurdly large images before copying them.
    fn read_png(&self) -> Option<Vec<u8>>;
}

/// Convert a `file://` URL string (as stored under `public.file-url`) to a
/// path. Returns `None` for other schemes, remote hosts, relative or empty
/// paths, and anything that does not decode to valid UTF-8 or that contains a
/// NUL or newline.
pub fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.trim().strip_prefix("file://")?;
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if rest[..i].is_empty() || rest[..i].eq_ignore_ascii_case("localhost") => &rest[i..],
        _ => return None,
    };
    let path = path.split(['?', '#']).next()?;
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    if s.len() < 2 || s.contains(['\0', '\n', '\r']) {
        return None;
    }
    Some(PathBuf::from(s))
}

/// What one poll did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing changed since the last poll (or this was the first poll,
    /// which only establishes the baseline).
    Unchanged,
    /// The change was deliberately not recorded.
    Skipped(SkipReason),
    /// A new entry of this kind was added to the history.
    Recorded(ClipKind),
    /// The change carried nothing recordable, or the history rejected it
    /// (blank, too large, duplicate of the latest entry).
    Rejected,
}

#[derive(Debug, Default)]
pub struct Watcher {
    last: Option<i64>,
}

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Poll once. Content that is on the pasteboard when the watcher starts is
    /// not recorded (the first call only remembers the change count).
    pub fn step(&mut self, src: &dyn PasteboardSource, history: &Mutex<ClipboardHistory>) -> Step {
        let count = src.change_count();
        let previous = self.last.replace(count);
        if previous.is_none_or(|p| p == count) {
            return Step::Unchanged;
        }

        let ignored = history.lock().map(|h| h.ignored_apps()).unwrap_or_default();
        let types = src.types();
        let frontmost = src.frontmost_bundle_id();
        if let Some(reason) = filter::skip_reason(&types, frontmost.as_deref(), &ignored) {
            return Step::Skipped(reason);
        }

        // Finder puts file names on the pasteboard as text too, so files win;
        // apps that copy rich content usually include text, which wins over
        // the image preview.
        let files = src.read_files();
        if !files.is_empty() {
            return record(history, ClipKind::Files, |h| h.push_files(&files));
        }
        if let Some(text) = src.read_text().filter(|t| !t.trim().is_empty()) {
            // A snippet pasted by Ashdash goes through the pasteboard (`pbcopy` cannot add the
            // marker types). It may hold a token or a password: not history material.
            if history.lock().is_ok_and(|mut h| h.take_own_paste(&text)) {
                return Step::Skipped(SkipReason::OwnPaste);
            }
            return record(history, ClipKind::Text, |h| h.push(&text));
        }
        if let Some(png) = src.read_png() {
            return record(history, ClipKind::Image, |h| h.push_image(&png));
        }
        Step::Rejected
    }
}

fn record(history: &Mutex<ClipboardHistory>, kind: ClipKind, f: impl FnOnce(&mut ClipboardHistory) -> bool) -> Step {
    match history.lock() {
        Ok(mut h) => {
            if f(&mut h) {
                Step::Recorded(kind)
            } else {
                Step::Rejected
            }
        }
        Err(_) => Step::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::super::filter::{CONCEALED_TYPE, TRANSIENT_TYPE};
    use super::super::history::tests::{fake_png, tmp};
    use super::*;

    #[derive(Default)]
    struct Fake {
        count: Cell<i64>,
        types: RefCell<Vec<String>>,
        app: RefCell<Option<String>>,
        files: RefCell<Vec<PathBuf>>,
        text: RefCell<Option<String>>,
        png: RefCell<Option<Vec<u8>>>,
        reads: Cell<u32>,
    }

    impl Fake {
        /// Simulate a copy: bump the change count and set the contents.
        fn copy(&self, types: &[&str], app: Option<&str>, text: Option<&str>) {
            self.count.set(self.count.get() + 1);
            *self.types.borrow_mut() = types.iter().map(|s| s.to_string()).collect();
            *self.app.borrow_mut() = app.map(str::to_string);
            *self.text.borrow_mut() = text.map(str::to_string);
            self.files.borrow_mut().clear();
            *self.png.borrow_mut() = None;
        }
    }

    impl PasteboardSource for Fake {
        fn change_count(&self) -> i64 {
            self.count.get()
        }
        fn types(&self) -> Vec<String> {
            self.reads.set(self.reads.get() + 1);
            self.types.borrow().clone()
        }
        fn frontmost_bundle_id(&self) -> Option<String> {
            self.app.borrow().clone()
        }
        fn read_files(&self) -> Vec<PathBuf> {
            self.files.borrow().clone()
        }
        fn read_text(&self) -> Option<String> {
            self.text.borrow().clone()
        }
        fn read_png(&self) -> Option<Vec<u8>> {
            self.png.borrow().clone()
        }
    }

    const TEXT: &str = "public.utf8-plain-text";

    #[test]
    fn file_urls_decode_to_paths() {
        let p = |u: &str| file_url_to_path(u).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(p("file:///Users/me/a.txt").as_deref(), Some("/Users/me/a.txt"));
        assert_eq!(p("file://localhost/tmp/x").as_deref(), Some("/tmp/x"));
        assert_eq!(p("file:///tmp/a%20b%C3%A9.txt").as_deref(), Some("/tmp/a bé.txt"));
        assert_eq!(p("  file:///tmp/dir/  ").as_deref(), Some("/tmp/dir/"));
        assert_eq!(p("file:///tmp/a.txt?x=1#frag").as_deref(), Some("/tmp/a.txt"));
        for bad in [
            "http://example.com/a",
            "file://otherhost/a",
            "file://",
            "file:///",
            "file:///tmp/%zz",
            "file:///tmp/%2",
            "file:///tmp/%FF",
            "file:///tmp/a%0Ab",
            "file:///tmp/a%00b",
            "tmp/a",
            "",
        ] {
            assert_eq!(p(bad), None, "{bad}");
        }
    }

    fn hist() -> Mutex<ClipboardHistory> {
        Mutex::new(ClipboardHistory::in_memory(10))
    }

    fn texts(h: &Mutex<ClipboardHistory>) -> Vec<String> {
        h.lock().unwrap().entries().iter().map(|e| e.text.clone()).collect()
    }

    #[test]
    fn first_poll_is_a_baseline_and_unchanged_counts_read_nothing() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        fake.copy(&[TEXT], None, Some("already there"));
        assert_eq!(w.step(&fake, &h), Step::Unchanged);
        assert!(texts(&h).is_empty(), "pre-existing content is not recorded");
        for _ in 0..5 {
            assert_eq!(w.step(&fake, &h), Step::Unchanged);
        }
        assert_eq!(fake.reads.get(), 0, "no pasteboard reads while the count is unchanged");
        fake.copy(&[TEXT], None, Some("new"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        assert_eq!(texts(&h), vec!["new"]);
        assert_eq!(fake.reads.get(), 1);
    }

    #[test]
    fn concealed_and_transient_entries_are_never_recorded() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        fake.copy(&[TEXT, CONCEALED_TYPE], Some("com.apple.Safari"), Some("hunter2"));
        assert_eq!(
            w.step(&fake, &h),
            Step::Skipped(SkipReason::MarkerType(CONCEALED_TYPE.into()))
        );
        fake.copy(&[TEXT, TRANSIENT_TYPE], None, Some("123456"));
        assert!(matches!(w.step(&fake, &h), Step::Skipped(SkipReason::MarkerType(_))));
        assert!(texts(&h).is_empty());
        // The skipped change is consumed: the next poll does not retry it.
        assert_eq!(w.step(&fake, &h), Step::Unchanged);
        // Normal copies afterwards still work.
        fake.copy(&[TEXT], None, Some("fine"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
    }

    #[test]
    fn a_snippet_pasted_by_ashdash_is_not_recorded_but_the_users_own_copies_are() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        // the command layer notes the text, then the pasteboard changes (frontmost app: the user's)
        h.lock().unwrap().note_own_paste("api-token-1234");
        fake.copy(&[TEXT], Some("com.apple.TextEdit"), Some("api-token-1234"));
        assert_eq!(w.step(&fake, &h), Step::Skipped(SkipReason::OwnPaste));
        assert!(texts(&h).is_empty());
        assert_eq!(w.step(&fake, &h), Step::Unchanged, "the change is consumed");
        // one note answers once: the user copying the same text later is recorded
        fake.copy(&[TEXT], Some("com.apple.TextEdit"), Some("api-token-1234"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        // other text is never skipped because a paste is pending
        h.lock().unwrap().note_own_paste("snippet");
        fake.copy(&[TEXT], None, Some("something else"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        assert_eq!(texts(&h), vec!["something else", "api-token-1234"]);
    }

    #[test]
    fn an_old_note_does_not_hide_a_later_copy() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        let long_ago = std::time::Instant::now() - super::super::history::OWN_PASTE_TTL * 2;
        h.lock().unwrap().note_own_paste_at("expired", long_ago);
        fake.copy(&[TEXT], None, Some("expired"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        // a failed write takes its note back
        h.lock().unwrap().note_own_paste("never written");
        h.lock().unwrap().forget_own_paste("never written");
        fake.copy(&[TEXT], None, Some("never written"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
    }

    #[test]
    fn markers_still_win_over_the_own_paste_note() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        h.lock().unwrap().note_own_paste("x");
        fake.copy(&[TEXT, CONCEALED_TYPE], None, Some("x"));
        assert!(matches!(w.step(&fake, &h), Step::Skipped(SkipReason::MarkerType(_))));
    }

    #[test]
    fn ignored_apps_are_skipped_using_the_history_setting() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        // Built-in defaults cover common password managers until configured.
        fake.copy(&[TEXT], Some("com.1password.1password"), Some("secret"));
        assert_eq!(
            w.step(&fake, &h),
            Step::Skipped(SkipReason::IgnoredApp("com.1password.1password".into()))
        );
        h.lock().unwrap().set_ignored_apps(vec!["com.example.vault".into()]);
        fake.copy(&[TEXT], Some("com.1password.1password"), Some("now allowed"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        fake.copy(&[TEXT], Some("COM.EXAMPLE.VAULT"), Some("secret2"));
        assert!(matches!(w.step(&fake, &h), Step::Skipped(SkipReason::IgnoredApp(_))));
        fake.copy(&[TEXT], Some("com.apple.Safari"), Some("ok"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        assert_eq!(texts(&h), vec!["ok", "now allowed"]);
    }

    #[test]
    fn files_beat_text_and_text_beats_image() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);

        fake.copy(&[TEXT, "public.file-url"], None, Some("a.txt"));
        *fake.files.borrow_mut() = vec![PathBuf::from("/tmp/a.txt")];
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Files));

        fake.copy(&[TEXT, "public.png"], None, Some("caption"));
        *fake.png.borrow_mut() = Some(vec![1, 2, 3]);
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        assert_eq!(h.lock().unwrap().entries()[0].text, "caption");
    }

    #[test]
    fn images_are_recorded_when_there_is_no_text() {
        let root = tmp("watch-img");
        let h = Mutex::new(ClipboardHistory::load(root.join("clipboard.json"), 10));
        let (fake, mut w) = (Fake::default(), Watcher::new());
        w.step(&fake, &h);
        fake.copy(&["public.png"], None, None);
        *fake.png.borrow_mut() = Some(fake_png(32, 16, 3));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Image));
        assert_eq!(h.lock().unwrap().entries()[0].label(), "Image 32×16");
        // Garbage "png" data is rejected, not stored.
        fake.copy(&["public.png"], None, None);
        *fake.png.borrow_mut() = Some(vec![1, 2, 3]);
        assert_eq!(w.step(&fake, &h), Step::Rejected);
        assert_eq!(h.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blank_empty_and_duplicate_changes_are_rejected() {
        let (fake, h, mut w) = (Fake::default(), hist(), Watcher::new());
        w.step(&fake, &h);
        fake.copy(&[TEXT], None, Some("   \n"));
        assert_eq!(w.step(&fake, &h), Step::Rejected);
        fake.copy(&[], None, None);
        assert_eq!(w.step(&fake, &h), Step::Rejected);
        fake.copy(&[TEXT], None, Some("same"));
        assert_eq!(w.step(&fake, &h), Step::Recorded(ClipKind::Text));
        fake.copy(&[TEXT], None, Some("same")); // copied again: count changes, text is the latest entry
        assert_eq!(w.step(&fake, &h), Step::Rejected);
        assert_eq!(texts(&h), vec!["same"]);
    }
}
