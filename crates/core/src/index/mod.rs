//! Persistent, watched index of installed applications.
//!
//! * The index is cached as versioned JSON in the cache directory ([`store`]). On start the
//!   cached list is published at once, so the engine is usable immediately; a background
//!   thread then re-walks the application folders (re-reading only bundles whose change
//!   stamp moved) and swaps in the difference.
//! * Without a usable cache (first run, other version, corrupt file, other set of folders) the
//!   first scan happens synchronously, exactly like the old behaviour.
//! * FSEvents (through `notify`) keeps the index current afterwards: events are debounced,
//!   mapped to the affected bundle or sub-folder ([`scan::classify`]) and applied
//!   incrementally; nothing runs while the file system is quiet.
//!
//! Readers only ever touch the shared `RwLock<Vec<AppEntry>>`; writers hold its write lock just
//! for the swap (or for a small in-place insert/remove), never while scanning.

pub mod icons;
mod scan;
mod store;
#[doc(hidden)]
pub mod synth;
mod watch;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use crate::apps::AppEntry;
use crate::config::Paths;

pub use scan::{MAX_DEPTH, Records};
pub use store::{INDEX_VERSION, Miss, Record};

use scan::{Root, Target};
use watch::{Batch, Msg};

/// Above this many changes the whole list is rebuilt instead of edited in place.
const REBUILD_THRESHOLD: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct IndexOptions {
    /// Watch the directories with FSEvents and keep the index current.
    pub watch: bool,
    /// After loading a cached index, re-walk the directories in the background.
    pub refresh_on_start: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            watch: true,
            refresh_on_start: true,
        }
    }
}

struct Shared {
    dirs: Vec<PathBuf>,
    roots: Vec<Root>,
    cache_file: PathBuf,
    live: Arc<RwLock<Vec<AppEntry>>>,
    records: Mutex<Records>,
    refreshed: AtomicBool,
    watching: AtomicBool,
}

/// Handle that owns the background refresher/watcher. Dropping it stops them.
pub struct AppIndex {
    shared: Arc<Shared>,
    stop: Sender<Msg>,
    from_cache: bool,
}

impl AppIndex {
    /// Open with the default options. `live` is the list the engine queries; it is filled
    /// before this returns (from the cache, or by a synchronous first scan).
    pub fn open(paths: &Paths, dirs: &[PathBuf], live: Arc<RwLock<Vec<AppEntry>>>) -> AppIndex {
        Self::open_with(paths, dirs, live, IndexOptions::default())
    }

    pub fn open_with(
        paths: &Paths,
        dirs: &[PathBuf],
        live: Arc<RwLock<Vec<AppEntry>>>,
        opts: IndexOptions,
    ) -> AppIndex {
        let cache_file = paths.cache.join(store::FILE_NAME);
        let shared = Arc::new(Shared {
            dirs: dirs.to_vec(),
            roots: dirs.iter().map(|d| Root::new(d)).collect(),
            cache_file,
            live,
            records: Mutex::new(Records::new()),
            refreshed: AtomicBool::new(false),
            watching: AtomicBool::new(false),
        });
        let (tx, rx) = channel();

        let cached = store::load(&shared.cache_file, dirs);
        let from_cache = cached.is_ok();
        let mut watcher = None;
        match cached {
            Ok(records) => {
                let mut recs = shared.lock();
                *recs = records.into_iter().map(|r| (r.path.clone(), r)).collect();
                shared.rebuild_live(&recs);
            }
            Err(_) => {
                // Arm the watcher first so nothing that happens during the scan is missed.
                if opts.watch {
                    watcher = watch::arm(dirs, tx.clone());
                    shared.watching.store(watcher.is_some(), Ordering::SeqCst);
                }
                {
                    let mut recs = shared.lock();
                    let found = scan::scan_all(&shared.dirs, &recs);
                    shared.replace_all(&mut recs, found);
                    shared.save(&recs, true);
                }
                shared.refreshed.store(true, Ordering::SeqCst);
            }
        }

        let background = from_cache && (opts.watch || opts.refresh_on_start);
        if background || watcher.is_some() {
            let sh = Arc::clone(&shared);
            let tx2 = tx.clone();
            let spawned = std::thread::Builder::new()
                .name("ashdash-app-index".into())
                .spawn(move || {
                    if from_cache && opts.watch {
                        watcher = watch::arm(&sh.dirs, tx2);
                        sh.watching.store(watcher.is_some(), Ordering::SeqCst);
                    }
                    if from_cache && opts.refresh_on_start {
                        sh.rescan();
                    }
                    sh.refreshed.store(true, Ordering::SeqCst);
                    if watcher.is_none() {
                        return;
                    }
                    while let Some(batch) = watch::next_batch(&rx, &sh.roots) {
                        sh.handle(batch);
                    }
                });
            if spawned.is_err() {
                shared.refreshed.store(true, Ordering::SeqCst);
            }
        } else {
            shared.refreshed.store(true, Ordering::SeqCst);
        }
        AppIndex {
            shared,
            stop: tx,
            from_cache,
        }
    }

    /// Re-walk the directories now (blocking) and publish any difference. Cheap when nothing
    /// changed: unchanged bundles are not re-read. Returns whether anything changed.
    pub fn rescan(&self) -> bool {
        self.shared.rescan()
    }

    /// Whether the list was initially served from the cache file.
    pub fn loaded_from_cache(&self) -> bool {
        self.from_cache
    }

    /// Whether the initial background refresh has finished (true at once without a cache).
    pub fn refreshed(&self) -> bool {
        self.shared.refreshed.load(Ordering::SeqCst)
    }

    /// Whether the FSEvents watcher is armed.
    pub fn watching(&self) -> bool {
        self.shared.watching.load(Ordering::SeqCst)
    }
}

impl Drop for AppIndex {
    fn drop(&mut self) {
        let _ = self.stop.send(Msg::Stop);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Records> {
        self.records.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, recs: &Records, create_parents: bool) {
        // Best effort: a cache that cannot be written only costs the next start a scan.
        let _ = store::save(&self.cache_file, &self.dirs, recs.values(), create_parents);
    }

    fn rescan(&self) -> bool {
        let mut recs = self.lock();
        let found = scan::scan_all(&self.dirs, &recs);
        let changed = self.replace_all(&mut recs, found);
        if changed {
            self.save(&recs, false);
        }
        changed
    }

    /// Make `recs` equal to `found`, publishing only the difference.
    fn replace_all(&self, recs: &mut Records, found: Records) -> bool {
        let upserts: BTreeMap<PathBuf, Record> = found
            .iter()
            .filter(|(k, r)| recs.get(*k) != Some(*r))
            .map(|(k, r)| (k.clone(), r.clone()))
            .collect();
        let removals: Vec<PathBuf> = recs.keys().filter(|k| !found.contains_key(*k)).cloned().collect();
        self.apply(recs, upserts, removals)
    }

    fn handle(&self, batch: Batch) {
        if batch.rescan_all {
            self.rescan();
            return;
        }
        let mut recs = self.lock();
        let mut upserts: BTreeMap<PathBuf, Record> = BTreeMap::new();
        let mut removals: BTreeSet<PathBuf> = BTreeSet::new();
        for t in batch.targets {
            match t {
                Target::Bundle(p) => {
                    let rec = if p.is_dir() { scan::probe(&p, recs.get(&p)) } else { None };
                    match rec {
                        Some(r) => {
                            upserts.insert(p, r);
                        }
                        None => {
                            removals.insert(p);
                        }
                    }
                }
                Target::Subtree { path, depth } => {
                    let mut found = Records::new();
                    if depth > 0 && path.is_dir() {
                        scan::walk(&path, depth, &recs, &mut found);
                    }
                    for (k, _) in recs.range(path.clone()..).take_while(|(k, _)| k.starts_with(&path)) {
                        if !found.contains_key(k) {
                            removals.insert(k.clone());
                        }
                    }
                    upserts.extend(found);
                }
            }
        }
        // a path found again by a later target wins over a removal from an earlier one
        removals.retain(|p| !upserts.contains_key(p));
        upserts.retain(|k, r| recs.get(k) != Some(r));
        removals.retain(|p| recs.contains_key(p));
        if self.apply(&mut recs, upserts, removals.into_iter().collect()) {
            self.save(&recs, false);
        }
    }

    /// Apply a diff to the records and the live list. Returns whether anything changed.
    fn apply(&self, recs: &mut Records, upserts: BTreeMap<PathBuf, Record>, removals: Vec<PathBuf>) -> bool {
        if upserts.is_empty() && removals.is_empty() {
            return false;
        }
        let n = upserts.len() + removals.len();
        for p in &removals {
            recs.remove(p);
        }
        if n > REBUILD_THRESHOLD {
            recs.extend(upserts);
            self.rebuild_live(recs);
        } else {
            let mut live = self.live.write().unwrap_or_else(|e| e.into_inner());
            for p in removals.iter().chain(upserts.keys()) {
                if let Some(i) = live.iter().position(|a| a.path == *p) {
                    live.remove(i);
                }
            }
            for (k, r) in upserts {
                if !r.background {
                    let entry = AppEntry::new(r.name.clone(), r.path.clone());
                    let pos = live
                        .binary_search_by(|a| order(a, &entry))
                        .unwrap_or_else(|insert_at| insert_at);
                    live.insert(pos, entry);
                }
                recs.insert(k, r);
            }
        }
        true
    }

    /// Build the visible list from `recs` and swap it in.
    fn rebuild_live(&self, recs: &Records) {
        let mut list: Vec<AppEntry> = recs
            .values()
            .filter(|r| !r.background)
            .map(|r| AppEntry::new(r.name.clone(), r.path.clone()))
            .collect();
        list.sort_by_cached_key(|a| (a.name.to_lowercase(), a.path.clone()));
        *self.live.write().unwrap_or_else(|e| e.into_inner()) = list;
    }
}

/// Display order: case-insensitive name, then path.
fn order(a: &AppEntry, b: &AppEntry) -> std::cmp::Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.path.cmp(&b.path))
}

/// One-shot scan without cache or watcher (what [`crate::apps::scan`] returns).
pub fn scan_entries(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let found = scan::scan_all(dirs, &Records::new());
    let mut list: Vec<AppEntry> = found
        .into_values()
        .filter(|r| !r.background)
        .map(|r| AppEntry::new(r.name, r.path))
        .collect();
    list.sort_by_cached_key(|a| (a.name.to_lowercase(), a.path.clone()));
    list
}
