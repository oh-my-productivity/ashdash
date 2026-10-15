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
        while let Ok(r) = self.res_rx.try_recv() {
            // answers for an older query, or while a panel shows its own list, are stale
            if r.generation != self.generation || self.panel_open() {
                continue;
            }
            let previous = self.items.get(self.selected).map(|i| i.id.clone());
            self.items = r.items;
            match r.kind {
                Kind::Pending => {
                    self.selected = 0;
                    self.scroll_to_selected = true;
                }
                Kind::Final => {
                    let keep = self.glue.keep_selection.take();
                    self.selected = reselect(keep.as_deref(), self.selected, &self.items);
                    self.scroll_to_selected = true;
                }
                // the same query again: stay on the same result, do not jump
                Kind::Rerun => self.selected = reselect(previous.as_deref(), self.selected, &self.items),
            }
        }
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
