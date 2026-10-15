//! In-memory bookmark cache, loaded off the query path.
//!
//! * Queries only call [`BookmarkCache::snapshot`] (an `Arc` clone) and
//!   [`BookmarkCache::poke`] (an atomic/mutex check that may spawn a thread).
//!   They never touch the disk, so a cold or stale cache costs microseconds:
//!   the first queries see an empty, `loaded: false` snapshot until the first
//!   background load finishes ("stale-or-empty meanwhile").
//! * A source is re-read only when its [`Fingerprint`] (mtime + size of its
//!   files) changed; unchanged sources reuse their previous result. A source
//!   whose last attempt failed is retried with exponential backoff (10 s,
//!   20 s ... capped at 5 min) so a permanently unreadable file does not
//!   cost an open attempt every few seconds. The fingerprint check itself
//!   (a few `stat`s) also runs on the background thread.
//! * Titles and hosts are turned into [`fuzzy::Candidate`]s once, when the
//!   snapshot is built, so a keystroke scores precomputed data only.
//! * The merged list drops non-openable schemes and de-duplicates by URL; the
//!   first source in precedence order wins.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::url::{clean_url, dedupe_key, display_host};
use super::{Bookmark, BookmarkSource, Fingerprint, SourceError, SourceErrorKind};
use crate::fuzzy::Candidate;

/// How often a query may trigger a change check.
pub const DEFAULT_RECHECK: Duration = Duration::from_secs(5);
/// First wait before retrying a failed source; doubles per consecutive failure.
pub const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(10);
/// Longest wait between retries of a failed source.
pub const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// A source that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub source_id: &'static str,
    pub source_name: &'static str,
    pub kind: SourceErrorKind,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub bookmarks: Vec<Bookmark>,
    pub issues: Vec<Issue>,
    /// False until the first background load has completed.
    pub loaded: bool,
    /// `titles[i]` / `hosts[i]` belong to `bookmarks[i]`.
    titles: Vec<Candidate>,
    hosts: Vec<Candidate>,
}

impl Snapshot {
    pub fn new(bookmarks: Vec<Bookmark>, issues: Vec<Issue>, loaded: bool) -> Snapshot {
        let titles = bookmarks.iter().map(|b| Candidate::new(&b.title)).collect();
        let hosts = bookmarks
            .iter()
            .map(|b| Candidate::new(&display_host(&b.url)))
            .collect();
        Snapshot {
            bookmarks,
            issues,
            loaded,
            titles,
            hosts,
        }
    }

    /// Every bookmark with its precomputed title and host candidates.
    pub fn scored(&self) -> impl Iterator<Item = (&Bookmark, &Candidate, &Candidate)> {
        self.bookmarks
            .iter()
            .zip(&self.titles)
            .zip(&self.hosts)
            .map(|((b, t), h)| (b, t, h))
    }
}

struct Entry {
    fingerprint: Fingerprint,
    result: Result<Vec<Bookmark>, SourceError>,
    /// Consecutive failed attempts with this fingerprint (0 after a success).
    failures: u32,
    /// A failed source is not retried before this moment (unless its files change).
    retry_at: Option<Instant>,
}

struct Inner {
    sources: Vec<Box<dyn BookmarkSource>>,
    entries: Mutex<Vec<Option<Entry>>>,
    snapshot: RwLock<Arc<Snapshot>>,
    loading: AtomicBool,
    last_check: Mutex<Option<Instant>>,
    recheck: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
}

#[derive(Clone)]
pub struct BookmarkCache {
    inner: Arc<Inner>,
}

impl BookmarkCache {
    pub fn new(sources: Vec<Box<dyn BookmarkSource>>) -> Self {
        Self::with_recheck(sources, DEFAULT_RECHECK)
    }

    pub fn with_recheck(sources: Vec<Box<dyn BookmarkSource>>, recheck: Duration) -> Self {
        Self::with_timing(sources, recheck, DEFAULT_BACKOFF_BASE, DEFAULT_BACKOFF_MAX)
    }

    /// `backoff_base`/`backoff_max` bound the wait before a failed source is retried.
    pub fn with_timing(
        sources: Vec<Box<dyn BookmarkSource>>,
        recheck: Duration,
        backoff_base: Duration,
        backoff_max: Duration,
    ) -> Self {
        let entries = sources.iter().map(|_| None).collect();
        BookmarkCache {
            inner: Arc::new(Inner {
                sources,
                entries: Mutex::new(entries),
                snapshot: RwLock::new(Arc::new(Snapshot::default())),
                loading: AtomicBool::new(false),
                last_check: Mutex::new(None),
                recheck,
                backoff_base,
                backoff_max,
            }),
        }
    }

    pub fn is_empty_config(&self) -> bool {
        self.inner.sources.is_empty()
    }

    /// The latest merged data. Never blocks on disk.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.inner.snapshot.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Called on the query path: starts a background check if one is due.
    pub fn poke(&self) {
        let due = {
            let mut last = self.inner.last_check.lock().unwrap_or_else(|e| e.into_inner());
            let due = last.is_none_or(|t| t.elapsed() >= self.inner.recheck);
            if due {
                *last = Some(Instant::now());
            }
            due
        };
        if due {
            self.refresh_in_background();
        }
    }

    /// Start a (re)load on a background thread unless one is already running.
    pub fn refresh_in_background(&self) {
        if self.inner.sources.is_empty() {
            self.inner.mark_loaded();
            return;
        }
        let cache = self.clone();
        let spawned = std::thread::Builder::new()
            .name("bookmarks-load".into())
            .spawn(move || {
                cache.refresh_blocking();
            });
        // If the OS refuses a thread we simply stay stale; nothing to report.
        drop(spawned);
    }

    /// Check all sources now, on the calling thread. Returns false if another
    /// refresh is already in progress (nothing was done).
    pub fn refresh_blocking(&self) -> bool {
        if self
            .inner
            .loading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.inner.loading);
        self.inner.refresh();
        true
    }
}

impl Inner {
    fn mark_loaded(&self) {
        let mut w = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
        if !w.loaded {
            *w = Arc::new(Snapshot::new(Vec::new(), Vec::new(), true));
        }
    }

    fn refresh(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for (i, src) in self.sources.iter().enumerate() {
            let fp = src.fingerprint();
            let now = Instant::now();
            let fresh = match &entries[i] {
                Some(e) if e.fingerprint == fp => match &e.result {
                    Ok(_) => true,
                    // failed: leave it alone until the backoff has passed
                    Err(_) => e.retry_at.is_some_and(|t| now < t),
                },
                _ => false,
            };
            if fresh {
                continue;
            }
            // A panicking source must not take the cache (or app) down.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.load()))
                .unwrap_or_else(|_| Err(SourceError::other("source panicked")));
            let prev_same = matches!(&entries[i], Some(e) if e.fingerprint == fp && e.result == result);
            if !prev_same {
                changed = true;
            }
            let (failures, retry_at) = if result.is_err() {
                let before = match &entries[i] {
                    Some(e) if e.fingerprint == fp => e.failures,
                    _ => 0,
                };
                let failures = before.saturating_add(1);
                let wait = self
                    .backoff_base
                    .saturating_mul(1u32 << (failures - 1).min(16))
                    .min(self.backoff_max);
                (failures, Some(Instant::now() + wait))
            } else {
                (0, None)
            };
            entries[i] = Some(Entry {
                fingerprint: fp,
                result,
                failures,
                retry_at,
            });
        }
        let first = !self.snapshot.read().unwrap_or_else(|e| e.into_inner()).loaded;
        if changed || first {
            let snap = build_snapshot(&self.sources, &entries);
            *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(snap);
        }
    }
}

fn build_snapshot(sources: &[Box<dyn BookmarkSource>], entries: &[Option<Entry>]) -> Snapshot {
    let mut seen = HashSet::new();
    let mut bookmarks = Vec::new();
    let mut issues = Vec::new();
    for (src, entry) in sources.iter().zip(entries) {
        match entry.as_ref().map(|e| &e.result) {
            Some(Ok(list)) => {
                for b in list {
                    let Some(url) = clean_url(&b.url) else { continue };
                    if !seen.insert(dedupe_key(&url)) {
                        continue;
                    }
                    let title = b.title.trim();
                    bookmarks.push(Bookmark {
                        title: if title.is_empty() {
                            url.clone()
                        } else {
                            title.to_string()
                        },
                        url,
                        folder: b.folder.clone(),
                        source: b.source.clone(),
                    });
                }
            }
            Some(Err(e)) => issues.push(Issue {
                source_id: src.id(),
                source_name: src.name(),
                kind: e.kind,
                message: e.message.clone(),
            }),
            None => {}
        }
    }
    Snapshot::new(bookmarks, issues, true)
}

#[cfg(test)]
pub(crate) mod testsupport {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// A scripted source: contents and fingerprint can be changed between
    /// loads, and loads are counted.
    pub struct FakeSource {
        pub id: &'static str,
        pub result: Mutex<Result<Vec<Bookmark>, SourceError>>,
        pub version: Mutex<u64>,
        pub loads: AtomicUsize,
    }

    impl FakeSource {
        pub fn new(id: &'static str, result: Result<Vec<Bookmark>, SourceError>) -> Arc<Self> {
            Arc::new(FakeSource {
                id,
                result: Mutex::new(result),
                version: Mutex::new(1),
                loads: AtomicUsize::new(0),
            })
        }
    }

    impl BookmarkSource for Arc<FakeSource> {
        fn id(&self) -> &'static str {
            self.id
        }
        fn name(&self) -> &'static str {
            self.id
        }
        fn fingerprint(&self) -> Fingerprint {
            let v = *self.version.lock().unwrap();
            vec![(
                std::path::PathBuf::from(self.id),
                Some((std::time::UNIX_EPOCH + Duration::from_secs(v), v)),
            )]
        }
        fn load(&self) -> Result<Vec<Bookmark>, SourceError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            self.result.lock().unwrap().clone()
        }
    }

    pub fn bm(title: &str, url: &str, folder: &str) -> Bookmark {
        Bookmark {
            title: title.into(),
            url: url.into(),
            folder: folder.into(),
            source: "Test".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering::SeqCst;

    use super::testsupport::*;
    use super::*;

    fn boxed(f: &Arc<FakeSource>) -> Box<dyn BookmarkSource> {
        Box::new(Arc::clone(f))
    }

    #[test]
    fn starts_empty_and_unloaded_then_fills() {
        let s = FakeSource::new("a", Ok(vec![bm("A", "https://a.example/", "")]));
        let c = BookmarkCache::new(vec![boxed(&s)]);
        let before = c.snapshot();
        assert!(!before.loaded && before.bookmarks.is_empty());
        assert_eq!(s.loads.load(SeqCst), 0, "constructing must not read anything");
        assert!(c.refresh_blocking());
        let after = c.snapshot();
        assert!(after.loaded);
        assert_eq!(after.bookmarks.len(), 1);
    }

    #[test]
    fn unchanged_fingerprint_is_not_reloaded_but_changed_is() {
        let s = FakeSource::new("a", Ok(vec![bm("A", "https://a.example/", "")]));
        let c = BookmarkCache::new(vec![boxed(&s)]);
        c.refresh_blocking();
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 1);
        *s.result.lock().unwrap() = Ok(vec![bm("B", "https://b.example/", "")]);
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 1, "same mtime/size: still cached");
        assert_eq!(c.snapshot().bookmarks[0].title, "A");
        *s.version.lock().unwrap() = 2;
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 2);
        assert_eq!(c.snapshot().bookmarks[0].title, "B");
    }

    #[test]
    fn failed_sources_are_retried_and_recover() {
        let denied = SourceError {
            kind: SourceErrorKind::PermissionDenied,
            message: "nope".into(),
        };
        let s = FakeSource::new("safari", Err(denied));
        // no backoff here: this test is about recovery, not about waiting
        let c = BookmarkCache::with_timing(vec![boxed(&s)], DEFAULT_RECHECK, Duration::ZERO, Duration::ZERO);
        c.refresh_blocking();
        let snap = c.snapshot();
        assert!(snap.bookmarks.is_empty());
        assert_eq!(snap.issues.len(), 1);
        assert_eq!(snap.issues[0].kind, SourceErrorKind::PermissionDenied);
        // Permission granted later: same fingerprint, but errors are retried.
        *s.result.lock().unwrap() = Ok(vec![bm("S", "https://s.example/", "")]);
        c.refresh_blocking();
        let snap = c.snapshot();
        assert!(snap.issues.is_empty());
        assert_eq!(snap.bookmarks.len(), 1);
    }

    #[test]
    fn failures_back_off_instead_of_retrying_every_check() {
        let s = FakeSource::new("safari", Err(SourceError::other("denied")));
        let c = BookmarkCache::with_timing(
            vec![boxed(&s)],
            DEFAULT_RECHECK,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        );
        for _ in 0..5 {
            c.refresh_blocking();
        }
        assert_eq!(s.loads.load(SeqCst), 1, "failure cached: no retry within the backoff");
        assert_eq!(c.snapshot().issues.len(), 1, "the failure is still reported");
        // changed files are always worth another look, backoff or not
        *s.version.lock().unwrap() = 2;
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 2);
    }

    #[test]
    fn backoff_grows_exponentially_up_to_the_cap() {
        let s = FakeSource::new("a", Err(SourceError::other("x")));
        let c = BookmarkCache::with_timing(
            vec![boxed(&s)],
            DEFAULT_RECHECK,
            Duration::from_millis(40),
            Duration::from_millis(80),
        );
        c.refresh_blocking(); // failure 1: wait 40 ms
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 1);
        std::thread::sleep(Duration::from_millis(60));
        c.refresh_blocking(); // failure 2: wait 80 ms (capped)
        assert_eq!(s.loads.load(SeqCst), 2);
        std::thread::sleep(Duration::from_millis(30));
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 2, "second wait is longer than the first");
        std::thread::sleep(Duration::from_millis(80));
        c.refresh_blocking();
        assert_eq!(s.loads.load(SeqCst), 3);
    }

    #[test]
    fn snapshot_candidates_line_up_with_bookmarks() {
        let a = FakeSource::new(
            "a",
            Ok(vec![
                bm("Rust Book", "https://www.rust-lang.org/book", ""),
                bm("Hacker News", "https://news.ycombinator.com/", ""),
            ]),
        );
        let c = BookmarkCache::new(vec![boxed(&a)]);
        c.refresh_blocking();
        let snap = c.snapshot();
        let q = crate::fuzzy::Query::new("hn");
        let hits: Vec<_> = snap
            .scored()
            .filter(|(_, t, _)| q.score(t).is_some())
            .map(|(b, _, _)| b.title.as_str())
            .collect();
        assert_eq!(hits, ["Hacker News"]);
        let q = crate::fuzzy::Query::new("ycombinator");
        let by_host: Vec<_> = snap
            .scored()
            .filter(|(_, _, h)| q.score(h).is_some())
            .map(|(b, _, _)| b.title.as_str())
            .collect();
        assert_eq!(by_host, ["Hacker News"]);
    }

    #[test]
    fn dedupes_by_url_first_source_wins_and_filters_schemes() {
        let a = FakeSource::new(
            "a",
            Ok(vec![
                bm("From A", "https://Example.com/", "FA"),
                bm("Bookmarklet", "javascript:alert(1)", ""),
                bm("", "https://untitled.example/", ""),
                bm("Internal", "chrome://settings", ""),
            ]),
        );
        let b = FakeSource::new(
            "b",
            Ok(vec![
                bm("From B", "https://example.com", "FB"),
                bm("Other", "https://other.example/", ""),
                bm("Local", "file:///tmp/x.html", ""),
                bm("App", "file:///Applications/Calculator.app", ""),
            ]),
        );
        let c = BookmarkCache::new(vec![boxed(&a), boxed(&b)]);
        c.refresh_blocking();
        let snap = c.snapshot();
        let titles: Vec<_> = snap.bookmarks.iter().map(|b| b.title.as_str()).collect();
        // file: bookmarks would be launched, not displayed, by `open`: dropped
        assert_eq!(titles, vec!["From A", "https://untitled.example/", "Other"]);
    }

    #[test]
    fn concurrent_refresh_is_refused() {
        let s = FakeSource::new("a", Ok(vec![]));
        let c = BookmarkCache::new(vec![boxed(&s)]);
        c.inner.loading.store(true, Ordering::SeqCst);
        assert!(!c.refresh_blocking());
        c.inner.loading.store(false, Ordering::SeqCst);
        assert!(c.refresh_blocking());
    }

    #[test]
    fn poke_loads_in_background_and_is_rate_limited() {
        let s = FakeSource::new("a", Ok(vec![bm("A", "https://a.example/", "")]));
        let c = BookmarkCache::with_recheck(vec![boxed(&s)], Duration::from_secs(3600));
        c.poke();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !c.snapshot().loaded && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(c.snapshot().loaded);
        *s.version.lock().unwrap() = 9;
        c.poke(); // within the recheck window: no new check
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(s.loads.load(SeqCst), 1);
    }

    #[test]
    fn no_sources_becomes_loaded_immediately() {
        let c = BookmarkCache::new(vec![]);
        c.refresh_in_background();
        assert!(c.snapshot().loaded);
    }
}
