//! FSEvents (via `notify`) watching of the application directories, with debouncing.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::scan::{Root, Target, classify};

/// Quiet period after the last event before the index is updated.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// A steady stream of events cannot postpone the update forever.
const MAX_DELAY: Duration = Duration::from_secs(2);
/// Beyond this many paths in one batch a full re-scan is cheaper than classifying them one by one.
const MAX_PATHS: usize = 2_000;

pub enum Msg {
    Paths(Vec<PathBuf>),
    /// The watcher lost track (overflow, error): re-examine everything.
    Rescan,
    Stop,
}

/// Start watching every existing directory. Missing directories are skipped (they are picked up
/// by the next restart). Returns `None` if nothing could be watched.
pub fn arm(dirs: &[PathBuf], tx: Sender<Msg>) -> Option<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = match res {
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => return,
            Ok(ev) => tx.send(Msg::Paths(ev.paths)),
            Err(_) => tx.send(Msg::Rescan),
        };
    })
    .ok()?;
    let mut any = false;
    for d in dirs.iter().filter(|d| d.is_dir()) {
        any |= watcher.watch(d, RecursiveMode::Recursive).is_ok();
    }
    any.then_some(watcher)
}

/// What a debounced batch of events asks for.
#[derive(Default)]
pub struct Batch {
    pub targets: BTreeSet<Target>,
    pub rescan_all: bool,
}

/// Block until something happens, then gather events until things go quiet.
/// Returns `None` when asked to stop (or when the owner went away).
pub fn next_batch(rx: &Receiver<Msg>, roots: &[Root]) -> Option<Batch> {
    let mut batch = Batch::default();
    let mut seen = 0usize;
    let mut absorb = |m: Msg, batch: &mut Batch| -> bool {
        match m {
            Msg::Stop => return false,
            Msg::Rescan => batch.rescan_all = true,
            Msg::Paths(ps) => {
                for p in ps {
                    seen += 1;
                    if seen > MAX_PATHS {
                        batch.rescan_all = true;
                    } else if let Some(t) = classify(roots, &p) {
                        batch.targets.insert(t);
                    }
                }
            }
        }
        true
    };
    if !absorb(rx.recv().ok()?, &mut batch) {
        return None;
    }
    let started = Instant::now();
    loop {
        let left = MAX_DELAY.saturating_sub(started.elapsed()).min(DEBOUNCE);
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(m) => {
                if !absorb(m, &mut batch) {
                    return None;
                }
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
    Some(batch)
}
