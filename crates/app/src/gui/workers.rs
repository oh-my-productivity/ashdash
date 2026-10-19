//! Background threads (query worker, clipboard watcher) and the per-frame
//! polling that drains their channels into the UI state.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::Instant;

use ashdash_core::{Engine, Item, workflow};
use eframe::egui;

use super::schedule::{Kind, PENDING_AFTER, Scheduler, Wait, reselect};
use super::{IconState, LauncherApp};

/// One batch of results from the query worker.
pub(super) struct QueryResult {
    pub generation: u64,
    pub kind: Kind,
    pub items: Vec<Item>,
}

/// The newest request already queued, if any. `Err` when the UI side is gone.
fn drain(rx: &Receiver<(u64, String)>) -> Result<Option<(u64, String)>, ()> {
    let mut newest = None;
    loop {
        match rx.try_recv() {
            Ok(r) => newest = Some(r),
            Err(TryRecvError::Empty) => return Ok(newest),
            Err(TryRecvError::Disconnected) => return Err(()),
        }
    }
}

/// Block until there is something to run: the newest request, or a due rerun
/// of the current one. `None` when the UI side is gone.
fn next_job(sched: &mut Scheduler, rx: &Receiver<(u64, String)>) -> Option<(u64, String, bool)> {
    loop {
        let mut newest = drain(rx).ok()?;
        if newest.is_none() {
            let now = Instant::now();
            if let Some((g, q)) = sched.take_due(now) {
                return Some((g, q, true));
            }
            newest = match sched.wait(now) {
                Wait::Forever => Some(rx.recv().ok()?),
                Wait::For(d) => match rx.recv_timeout(d) {
                    Ok(r) => Some(r),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return None,
                },
            };
            // a burst of keystrokes: let the rest of it overtake this one
            if let Some(later) = drain(rx).ok()? {
                newest = Some(later);
            }
        }
        if let Some((g, q)) = newest
            && sched.request(g, &q)
        {
            return Some((g, q, false));
        }
    }
}

/// Query worker: always answers the newest request, dropping stale ones.
/// A Script Filter's `rerun` repeats the query while it stays the current one;
/// slow queries show their `runningsubtext` rows first.
pub(super) fn spawn_query_worker(
    engine: Arc<Engine>,
    ctx: egui::Context,
    req_rx: Receiver<(u64, String)>,
    res_tx: Sender<QueryResult>,
) {
    std::thread::spawn(move || {
        let mut sched = Scheduler::default();
        while let Some((generation, query, is_rerun)) = next_job(&mut sched, &req_rx) {
            let (tx, rx) = channel();
            {
                let (engine, query) = (Arc::clone(&engine), query.clone());
                std::thread::spawn(move || {
                    // a panic ends this query only: the worker sees the channel close (below)
                    ashdash_core::recovery::mark_thread();
                    let _ = tx.send(engine.query(&query));
                });
            }
            let items = match rx.recv_timeout(PENDING_AFTER) {
                Ok(items) => items,
                Err(RecvTimeoutError::Timeout) => {
                    if !is_rerun {
                        let pending = engine.pending_items(&query);
                        if !pending.is_empty() {
                            let r = QueryResult {
                                generation,
                                kind: Kind::Pending,
                                items: pending,
                            };
                            if res_tx.send(r).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                    }
                    rx.recv().unwrap_or_default()
                }
                Err(RecvTimeoutError::Disconnected) => Vec::new(),
            };
            sched.finished(generation, Instant::now(), workflow::rerun_interval(&items));
            let kind = if is_rerun { Kind::Rerun } else { Kind::Final };
            if res_tx
                .send(QueryResult {
                    generation,
                    kind,
                    items,
                })
                .is_err()
            {
                break;
            }
            ctx.request_repaint();
        }
    });
}

/// Watch the system pasteboard and record changes in the history.
pub(super) fn spawn_clipboard_watcher(engine: Arc<Engine>) {
    crate::clipboard_watch::spawn(engine);
}

impl LauncherApp {
    pub(super) fn poll_background(&mut self, ctx: &egui::Context) {
        while self.toggle_rx.try_recv().is_ok() {
            if self.visible {
                self.hide(ctx);
            } else {
                self.show(ctx);
            }
        }
        // what the previous frame showed: a selection that differs from the one remembered is the
        // user's own move (see `SelectionTrack`)
        self.glue.track.begin_frame(self.generation, self.selected, &self.items);
        while let Ok(r) = self.res_rx.try_recv() {
            // answers for an older query, or while a panel shows its own list, are stale
            if r.generation != self.generation || self.panel_open() {
                continue;
            }
            let previous = self.items.get(self.selected).map(|i| i.id.clone());
            let old_selected = self.selected;
            let old = std::mem::replace(&mut self.items, r.items);
            super::frametrace::results(r.generation, r.kind, self.items.len());
            let track = &mut self.glue.track;
            match r.kind {
                Kind::Pending => {
                    self.selected = track.replaced_with(&old, old_selected, 0);
                    self.scroll_to_selected = true;
                }
                Kind::Final => {
                    // the result the user moved to stays highlighted; unmoved: the new top
                    let keep = self.glue.keep_selection.take();
                    self.selected = track.replaced(&old, old_selected, &self.items, keep.as_deref(), old_selected);
                    self.scroll_to_selected = true;
                }
                // the same query again: stay on the same result, do not jump
                Kind::Rerun => {
                    let same = reselect(previous.as_deref(), old_selected, &self.items);
                    self.selected = track.replaced_with_rerun(&old, old_selected, same);
                }
            }
        }
        self.announce_crash_report();
        while let Ok(msg) = self.glue.rx.try_recv() {
            self.apply_async(ctx, msg);
        }
        while let Ok((app, png)) = self.icon_rx.try_recv() {
            self.icons.insert(app, IconState::Ready(png));
        }
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            self.present_outcome(ctx, outcome);
        }
    }
}

impl LauncherApp {
    /// After a crash: the first time the launcher is shown, say so in the footer, once. The note
    /// is a status message, so typing clears it; the report is marked seen when it is shown.
    fn announce_crash_report(&mut self) {
        if !self.visible || self.glue.crash_checked {
            return;
        }
        self.glue.crash_checked = true;
        let paths = &self.engine.paths;
        if let Some(unseen) = crate::crash::unseen_reports(paths) {
            crate::crash::mark_seen(paths, &unseen.newest);
            if self.status.is_none() {
                self.status = Some(crate::crash::footer_note(unseen.fatal).into());
            }
        }
    }
}
