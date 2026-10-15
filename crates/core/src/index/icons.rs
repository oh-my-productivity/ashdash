//! Bounding the icon cache and the `sips` conversions that fill it.
//!
//! * [`Limiter`] is a counting semaphore: at most N conversions run at once, so scrolling a long
//!   result list cannot fork dozens of `sips` processes. The process-wide instance is
//!   [`sips_limiter`]; callers (GUI workers) only need [`crate::apps::app_icon_png`], which
//!   goes through it.
//! * [`prune_dir`] keeps the cache directory at a fixed number of files, dropping the least
//!   recently used (by modification time; hits are "touched" by [`touch`]).

use std::path::Path;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::SystemTime;

/// Most icons kept in the cache directory.
pub const MAX_CACHED_ICONS: usize = 500;
/// Default number of simultaneous `sips` processes.
pub const DEFAULT_MAX_SIPS: usize = 3;

/// A counting semaphore with an adjustable limit.
pub struct Limiter {
    state: Mutex<State>,
    freed: Condvar,
}

struct State {
    max: usize,
    running: usize,
}

/// Releases its slot when dropped.
pub struct Permit<'a>(&'a Limiter);

impl Limiter {
    pub const fn new(max: usize) -> Limiter {
        Limiter {
            state: Mutex::new(State {
                max: if max == 0 { 1 } else { max },
                running: 0,
            }),
            freed: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Block until a slot is free.
    pub fn acquire(&self) -> Permit<'_> {
        let mut st = self.lock();
        while st.running >= st.max {
            st = self.freed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.running += 1;
        Permit(self)
    }

    /// Take a slot only if one is free right now.
    pub fn try_acquire(&self) -> Option<Permit<'_>> {
        let mut st = self.lock();
        if st.running >= st.max {
            return None;
        }
        st.running += 1;
        Some(Permit(self))
    }

    /// Change the limit (minimum 1). Waiters are woken so a raised limit takes effect at once.
    pub fn set_max(&self, max: usize) {
        self.lock().max = max.max(1);
        self.freed.notify_all();
    }

    pub fn max(&self) -> usize {
        self.lock().max
    }

    /// Slots currently taken.
    pub fn running(&self) -> usize {
        self.lock().running
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.running = st.running.saturating_sub(1);
        drop(st);
        self.0.freed.notify_one();
    }
}

/// The limiter every `sips` conversion in this process goes through.
pub fn sips_limiter() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(DEFAULT_MAX_SIPS))
}

/// Mark a cached file as just used (for LRU ordering). Best effort.
pub fn touch(file: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(file) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Delete the oldest `.png` files in `dir` until at most `keep` remain. Returns how many were
/// removed. Other files (in-flight `.part` conversions) are never counted or touched.
pub fn prune_dir(dir: &Path, keep: usize) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(SystemTime, std::path::PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        })
        .collect();
    if files.len() <= keep {
        return 0;
    }
    // oldest first; the path breaks ties so the outcome is deterministic
    files.sort();
    let excess = files.len() - keep;
    files
        .into_iter()
        .take(excess)
        .filter(|(_, p)| std::fs::remove_file(p).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("oa-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn prune_removes_least_recently_used_png_files_only() {
        let d = temp("prune");
        let base = SystemTime::now() - Duration::from_secs(1000);
        for i in 0..10u64 {
            let p = d.join(format!("{i:02}.png"));
            std::fs::write(&p, b"x").unwrap();
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            f.set_modified(base + Duration::from_secs(i)).unwrap();
        }
        std::fs::write(d.join("inflight.part"), b"x").unwrap();
        // a cache hit on the oldest entry makes it the youngest
        touch(&d.join("00.png"));
        assert_eq!(prune_dir(&d, 6), 4);
        let left: Vec<String> = {
            let mut v: Vec<_> = std::fs::read_dir(&d)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            left,
            ["00.png", "05.png", "06.png", "07.png", "08.png", "09.png", "inflight.part"]
        );
        assert_eq!(prune_dir(&d, 6), 0);
        assert_eq!(prune_dir(&d.join("missing"), 6), 0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn limiter_caps_concurrency() {
        let l = Arc::new(Limiter::new(2));
        let now = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (l, now, peak) = (l.clone(), now.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _p = l.acquire();
                    let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(l.running(), 0);
    }

    #[test]
    fn try_acquire_and_set_max() {
        let l = Limiter::new(1);
        let p = l.try_acquire().unwrap();
        assert!(l.try_acquire().is_none());
        l.set_max(2);
        let q = l.try_acquire().unwrap();
        assert_eq!(l.running(), 2);
        drop(p);
        drop(q);
        assert_eq!(l.running(), 0);
        l.set_max(0);
        assert_eq!(l.max(), 1);
    }
}
