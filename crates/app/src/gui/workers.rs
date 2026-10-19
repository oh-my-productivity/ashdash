//! Background threads (query worker, clipboard watcher) and the per-frame
//! polling that drains their channels into the UI state.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ashdash_core::cancel::CancelToken;
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
fn drain(rx: &Receiver<Request>) -> Result<Option<Request>, ()> {
    let mut newest = None;
    loop {
        match rx.try_recv() {
            Ok(r) => newest = Some(r),
            Err(TryRecvError::Empty) => return Ok(newest),
            Err(TryRecvError::Disconnected) => return Err(()),
        }
    }
}

/// How long a query that would start a process (a Script Filter, Spotlight) waits for the next keystroke before it starts.
/// Typing "pup" against a keyword "p" would otherwise start three scripts 100 ms apart; with this only the last one starts, 40 ms
/// after the last key. Queries that answer from memory (applications, calculator, bookmarks ...) never wait.
pub(super) const COALESCE: Duration = Duration::from_millis(40);

/// How often a running query looks for a newer request. A wake-up costs microseconds, and only happens while a query is in flight.
const WATCH_POLL: Duration = Duration::from_millis(2);

type Request = (u64, String);

/// Block until there is something to run: the newest request (`carry` first: one that arrived while the previous query ran), or a
/// due rerun of the current one. `None` when the UI side is gone.
fn next_job(sched: &mut Scheduler, rx: &Receiver<Request>, carry: &mut Option<Request>) -> Option<(u64, String, bool)> {
    loop {
        let carried = carry.take();
        let mut newest = drain(rx).ok()?.or(carried);
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

/// Wait up to `window` for a newer request. `Ok(Some(newest))` when one came (and everything queued behind it is folded into it),
/// `Ok(None)` when none did, `Err` when the UI side is gone.
fn wait_for_newer(rx: &Receiver<Request>, window: Duration) -> Result<Option<Request>, ()> {
    match rx.recv_timeout(window) {
        Ok(first) => Ok(Some(drain(rx)?.unwrap_or(first))),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(()),
    }
}

/// How a query ended.
#[derive(Debug, PartialEq)]
enum Ran {
    /// The answer (empty when the query panicked).
    Done(Vec<Item>),
    /// A newer request arrived first; the query was cancelled and its answer is not wanted.
    Superseded(Request),
    /// The UI side is gone.
    UiGone,
}

/// Run `job` on its own thread and wait for it, but not for long: as soon as a newer request is queued on `rx` the job's
/// `CancelToken` is set (it kills the child process it waits for) and that request is returned, so a stale query never delays a
/// newer one. `on_slow` is called once if the job has run for `slow_after` (the launcher shows its placeholder rows); it returns
/// false when the UI side is gone.
fn run_superseding(
    rx: &Receiver<Request>,
    job: impl FnOnce(CancelToken) -> Vec<Item> + Send + 'static,
    slow_after: Duration,
    mut on_slow: impl FnMut() -> bool,
) -> Ran {
    let token = CancelToken::new();
    let (tx, answer) = channel();
    {
        let token = token.clone();
        std::thread::spawn(move || {
            // a panic ends this query only: the worker sees the channel close (below)
            ashdash_core::recovery::mark_thread();
            let _ = tx.send(job(token));
        });
    }
    let started = Instant::now();
    let mut slow_sent = false;
    loop {
        match answer.recv_timeout(WATCH_POLL) {
            Ok(items) => return Ran::Done(items),
            Err(RecvTimeoutError::Disconnected) => return Ran::Done(Vec::new()),
            Err(RecvTimeoutError::Timeout) => {}
        }
        match drain(rx) {
            Err(()) => {
                token.cancel();
                return Ran::UiGone;
            }
            Ok(Some(newer)) => {
                token.cancel();
                return Ran::Superseded(newer);
            }
            Ok(None) => {}
        }
        if !slow_sent && started.elapsed() >= slow_after {
            slow_sent = true;
            if !on_slow() {
                token.cancel();
                return Ran::UiGone;
            }
        }
    }
}

/// Query worker: always answers the newest request; an older one still running is cancelled the moment a newer one arrives.
/// A Script Filter's `rerun` repeats the query while it stays the current one;
/// slow queries show their `runningsubtext` rows first.
pub(super) fn spawn_query_worker(
    engine: Arc<Engine>,
    ctx: egui::Context,
    req_rx: Receiver<Request>,
    res_tx: Sender<QueryResult>,
) {
    std::thread::spawn(move || {
        let mut sched = Scheduler::default();
        let mut carry: Option<Request> = None;
        while let Some((generation, query, is_rerun)) = next_job(&mut sched, &req_rx, &mut carry) {
            // Something that starts a process waits a moment for the next key (see COALESCE); a fast query goes straight on.
            if !is_rerun && engine.query_is_slow(&query) {
                match wait_for_newer(&req_rx, COALESCE) {
                    Ok(None) => {}
                    Ok(newer) => {
                        carry = newer;
                        continue;
                    }
                    Err(()) => break,
                }
            }
            let ran = {
                let (engine, query) = (Arc::clone(&engine), query.clone());
                let on_slow = {
                    let (engine, query, res_tx, ctx) =
                        (Arc::clone(&engine), query.clone(), res_tx.clone(), ctx.clone());
                    move || {
                        if is_rerun {
                            return true;
                        }
                        let pending = engine.pending_items(&query);
                        if pending.is_empty() {
                            return true;
                        }
                        let r = QueryResult {
                            generation,
                            kind: Kind::Pending,
                            items: pending,
                        };
                        let sent = res_tx.send(r).is_ok();
                        ctx.request_repaint();
                        sent
                    }
                };
                run_superseding(
                    &req_rx,
                    move |token| engine.query_cancellable(&query, &token),
                    PENDING_AFTER,
                    on_slow,
                )
            };
            let items = match ran {
                Ran::Done(items) => items,
                Ran::Superseded(newer) => {
                    carry = Some(newer);
                    continue;
                }
                Ran::UiGone => break,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    fn item(id: &str) -> Item {
        Item::new(id, id)
    }

    /// A job that runs until its token is cancelled (like a Script Filter waiting for its child), then records that it saw it.
    fn until_cancelled(seen: Arc<AtomicBool>) -> impl FnOnce(CancelToken) -> Vec<Item> + Send + 'static {
        move |token| {
            let limit = Instant::now() + Duration::from_secs(10);
            while !token.is_cancelled() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(1));
            }
            seen.store(token.is_cancelled(), Ordering::SeqCst);
            Vec::new()
        }
    }

    #[test]
    fn the_newest_queued_request_wins_a_burst() {
        let (tx, rx) = channel::<Request>();
        for (g, q) in [(1, "p"), (2, "pu"), (3, "pup")] {
            tx.send((g, q.to_string())).unwrap();
        }
        assert_eq!(drain(&rx), Ok(Some((3, "pup".to_string()))));
        assert_eq!(drain(&rx), Ok(None));
        drop(tx);
        assert_eq!(drain(&rx), Err(()), "the UI side is gone");
    }

    #[test]
    fn a_request_that_arrived_during_a_query_is_the_next_job() {
        let (tx, rx) = channel::<Request>();
        let mut sched = Scheduler::default();
        let mut carry = Some((4, "pu".to_string()));
        assert_eq!(
            next_job(&mut sched, &rx, &mut carry),
            Some((4, "pu".to_string(), false))
        );
        assert!(carry.is_none(), "taken");
        // one queued behind it overtakes the carried one
        carry = Some((5, "pup".to_string()));
        tx.send((6, "pupp".to_string())).unwrap();
        assert_eq!(
            next_job(&mut sched, &rx, &mut carry),
            Some((6, "pupp".to_string(), false))
        );
        // a carried empty query (the launcher hid) is not a job: the worker goes back to waiting, here until the UI is gone
        carry = Some((7, String::new()));
        drop(tx);
        assert_eq!(next_job(&mut sched, &rx, &mut carry), None);
    }

    #[test]
    fn waiting_for_a_newer_request_folds_a_burst_into_its_last_key() {
        let (tx, rx) = channel::<Request>();
        assert_eq!(wait_for_newer(&rx, Duration::from_millis(5)), Ok(None));
        tx.send((1, "pu".to_string())).unwrap();
        tx.send((2, "pup".to_string())).unwrap();
        assert_eq!(
            wait_for_newer(&rx, Duration::from_secs(5)),
            Ok(Some((2, "pup".to_string())))
        );
        drop(tx);
        assert_eq!(wait_for_newer(&rx, Duration::from_secs(5)), Err(()));
    }

    #[test]
    fn a_finished_query_is_returned_as_done() {
        let (_tx, rx) = channel::<Request>();
        let r = run_superseding(&rx, |_| vec![item("a")], Duration::from_secs(5), || panic!("not slow"));
        assert_eq!(r, Ran::Done(vec![item("a")]));
    }

    #[test]
    fn a_newer_request_cancels_the_running_query_at_once() {
        let (tx, rx) = channel::<Request>();
        let seen = Arc::new(AtomicBool::new(false));
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            tx.send((9, "pup".to_string())).unwrap();
            tx // keep the channel open until the test is done
        });
        // the job runs for 10 s unless it is cancelled: the worker has to return the newer request without waiting for it
        let r = run_superseding(&rx, until_cancelled(Arc::clone(&seen)), Duration::from_secs(5), || true);
        let _tx = sender.join().unwrap();
        assert_eq!(r, Ran::Superseded((9, "pup".to_string())));
        // the job really saw the flag (it is what kills the child process)
        let limit = Instant::now() + Duration::from_secs(2);
        while !seen.load(Ordering::SeqCst) && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(seen.load(Ordering::SeqCst), "the job's token was never cancelled");
    }

    #[test]
    fn a_vanished_ui_cancels_the_query_and_ends_the_worker() {
        let (tx, rx) = channel::<Request>();
        let seen = Arc::new(AtomicBool::new(false));
        drop(tx);
        let r = run_superseding(&rx, until_cancelled(Arc::clone(&seen)), Duration::from_secs(5), || true);
        assert_eq!(r, Ran::UiGone);
    }

    #[test]
    fn the_placeholder_callback_runs_once_and_only_for_a_slow_query() {
        let (_tx, rx) = channel::<Request>();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let r = run_superseding(
            &rx,
            |_| {
                std::thread::sleep(Duration::from_millis(80));
                vec![item("late")]
            },
            Duration::from_millis(20),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            },
        );
        assert_eq!(r, Ran::Done(vec![item("late")]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // a quick one never asks
        let r = run_superseding(
            &rx,
            |_| vec![item("fast")],
            Duration::from_secs(5),
            || panic!("not slow"),
        );
        assert_eq!(r, Ran::Done(vec![item("fast")]));
    }

    #[test]
    fn a_placeholder_that_cannot_be_delivered_ends_the_worker() {
        let (_tx, rx) = channel::<Request>();
        let seen = Arc::new(AtomicBool::new(false));
        let r = run_superseding(&rx, until_cancelled(seen), Duration::from_millis(5), || false);
        assert_eq!(r, Ran::UiGone);
    }

    #[test]
    fn a_panicking_query_is_an_empty_answer_not_a_dead_worker() {
        let (_tx, rx) = channel::<Request>();
        let r = run_superseding(
            &rx,
            |_| -> Vec<Item> { panic!("test: a provider panics") },
            Duration::from_secs(5),
            || true,
        );
        assert_eq!(r, Ran::Done(Vec::new()));
    }
}
