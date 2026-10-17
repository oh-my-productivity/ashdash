//! Pure scheduling for the query worker: which request to run, and when to
//! repeat it because a Script Filter asked for `rerun`.
//!
//! The worker owns one [`Scheduler`] and feeds it three events: a new request
//! arrived ([`Scheduler::request`]), a run finished ([`Scheduler::finished`]),
//! and time passes ([`Scheduler::wait`] / [`Scheduler::take_due`]). Nothing here
//! reads a clock or touches a channel, so the rules are tested with synthetic
//! instants.

use std::time::{Duration, Instant};

/// A newer request replaces the one being repeated; a hidden launcher (or an
/// open actions panel) sends an empty query to stop everything.
#[derive(Debug, Default)]
pub struct Scheduler {
    generation: u64,
    query: String,
    /// When the current query is to be run again, if a filter asked for it.
    due: Option<Instant>,
    /// When the first run of the current request finished: the reruns of one request stop
    /// [`RERUN_CEILING`] after it.
    first_run: Option<Instant>,
}

/// What the worker should do while idle.
#[derive(Debug, PartialEq, Eq)]
pub enum Wait {
    /// Block until a request arrives.
    Forever,
    /// Block at most this long, then call [`Scheduler::take_due`].
    For(Duration),
}

/// Rerun intervals are clamped by the core to 0.1..=5 s; this guards the
/// scheduler against anything else (a zero interval must never spin).
pub const MIN_RERUN: Duration = Duration::from_millis(100);
pub const MAX_RERUN: Duration = Duration::from_secs(5);

/// Reruns of one request stop after this long: a window left open on a query (behind a Large
/// Type, an import prompt, or just forgotten) would otherwise run the script every 0.1-5 s
/// for ever. Typing a new query starts a fresh period.
pub const RERUN_CEILING: Duration = Duration::from_secs(5 * 60);

impl Scheduler {
    /// Record the newest request. Returns true when it should be run (a
    /// non-empty query); an empty query only cancels any pending rerun.
    /// Any earlier rerun is cancelled either way: a changed query never
    /// inherits the schedule of the previous one.
    pub fn request(&mut self, generation: u64, query: &str) -> bool {
        self.generation = generation;
        self.due = None;
        self.first_run = None;
        self.query = query.to_string();
        !query.trim().is_empty()
    }

    /// A run for `generation` finished at `now`; `rerun` is the shortest
    /// interval the results asked for. Ignored when a newer request has been
    /// recorded in the meantime.
    pub fn finished(&mut self, generation: u64, now: Instant, rerun: Option<Duration>) {
        if generation != self.generation || self.query.trim().is_empty() {
            return;
        }
        let first = *self.first_run.get_or_insert(now);
        self.due = if now.saturating_duration_since(first) >= RERUN_CEILING {
            None
        } else {
            rerun.map(|d| now + d.clamp(MIN_RERUN, MAX_RERUN))
        };
    }

    /// How long the worker may sleep before it has to look at the clock again.
    pub fn wait(&self, now: Instant) -> Wait {
        match self.due {
            None => Wait::Forever,
            Some(t) => Wait::For(t.saturating_duration_since(now)),
        }
    }

    /// If the rerun is due at `now`, consume it and return the request to repeat.
    pub fn take_due(&mut self, now: Instant) -> Option<(u64, String)> {
        match self.due {
            Some(t) if t <= now => {
                self.due = None;
                Some((self.generation, self.query.clone()))
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn is_scheduled(&self) -> bool {
        self.due.is_some()
    }
}

/// How the UI should treat a batch of results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Placeholder rows ("Searching...") shown while the real query runs.
    Pending,
    /// The answer to a request.
    Final,
    /// The answer to a repeat of the same query: the selection stays on the
    /// same result.
    Rerun,
}

/// How long a query may run before its placeholder rows are shown.
pub const PENDING_AFTER: Duration = Duration::from_millis(150);

/// Where the selection goes when the list is replaced: stay on the same result
/// (by id) if it is still there, otherwise keep the position, clamped.
pub fn reselect(previous_id: Option<&str>, previous_index: usize, items: &[ashdash_core::Item]) -> usize {
    if let Some(id) = previous_id
        && let Some(i) = items.iter().position(|it| it.id == id)
    {
        return i;
    }
    previous_index.min(items.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn idle_without_a_rerun_request() {
        let mut s = Scheduler::default();
        assert!(s.request(1, "demo x"));
        let now = t0();
        s.finished(1, now, None);
        assert_eq!(s.wait(now), Wait::Forever);
        assert_eq!(s.take_due(now + ms(10_000)), None);
        assert!(!s.is_scheduled());
    }

    #[test]
    fn reruns_the_same_query_after_the_interval() {
        let mut s = Scheduler::default();
        s.request(7, "clock");
        let now = t0();
        s.finished(7, now, Some(ms(500)));
        assert_eq!(s.wait(now), Wait::For(ms(500)));
        assert_eq!(s.wait(now + ms(200)), Wait::For(ms(300)));
        assert_eq!(s.take_due(now + ms(499)), None, "not yet");
        assert_eq!(s.take_due(now + ms(500)), Some((7, "clock".into())));
        // consumed: the next run has to ask again
        assert_eq!(s.wait(now + ms(500)), Wait::Forever);
        assert_eq!(s.take_due(now + ms(2000)), None);
        s.finished(7, now + ms(600), Some(ms(500)));
        assert_eq!(s.take_due(now + ms(1100)), Some((7, "clock".into())));
    }

    #[test]
    fn a_new_request_cancels_the_pending_rerun() {
        let mut s = Scheduler::default();
        s.request(1, "clock");
        let now = t0();
        s.finished(1, now, Some(ms(500)));
        assert!(s.is_scheduled());
        s.request(2, "clocks");
        assert!(!s.is_scheduled());
        assert_eq!(s.take_due(now + ms(10_000)), None);
        // the finished run of the OLD request must not re-arm the schedule
        s.finished(1, now + ms(10), Some(ms(500)));
        assert!(!s.is_scheduled());
        // the new one can
        s.finished(2, now + ms(20), Some(ms(100)));
        assert_eq!(s.take_due(now + ms(120)), Some((2, "clocks".into())));
    }

    #[test]
    fn an_empty_request_stops_everything() {
        let mut s = Scheduler::default();
        s.request(1, "clock");
        let now = t0();
        s.finished(1, now, Some(ms(500)));
        assert!(!s.request(2, ""), "nothing to run");
        assert_eq!(s.wait(now), Wait::Forever);
        s.finished(2, now, Some(ms(500)));
        assert!(!s.is_scheduled(), "an empty query is never repeated");
        assert!(!s.request(3, "   "));
    }

    #[test]
    fn intervals_are_clamped() {
        let mut s = Scheduler::default();
        s.request(1, "x");
        let now = t0();
        s.finished(1, now, Some(Duration::ZERO));
        assert_eq!(s.wait(now), Wait::For(MIN_RERUN), "a zero interval cannot spin");
        s.finished(1, now, Some(Duration::from_secs(3600)));
        assert_eq!(s.wait(now), Wait::For(MAX_RERUN));
    }

    #[test]
    fn reruns_stop_after_the_ceiling_and_typing_starts_a_new_period() {
        let mut s = Scheduler::default();
        s.request(1, "clock");
        let start = t0();
        s.finished(1, start, Some(ms(500)));
        // a rerun just inside the ceiling is still scheduled ...
        let late = start + RERUN_CEILING - ms(1);
        s.finished(1, late, Some(ms(500)));
        assert!(s.is_scheduled());
        // ... one at the ceiling is the last
        s.finished(1, start + RERUN_CEILING, Some(ms(500)));
        assert!(!s.is_scheduled(), "still repeating after {RERUN_CEILING:?}");
        assert_eq!(s.wait(start + RERUN_CEILING), Wait::Forever);
        // the same query typed again (a new request) gets a fresh period
        s.request(2, "clock");
        let again = start + RERUN_CEILING + ms(10);
        s.finished(2, again, Some(ms(500)));
        assert!(s.is_scheduled());
        s.finished(2, again + RERUN_CEILING / 2, Some(ms(500)));
        assert!(s.is_scheduled());
    }

    #[test]
    fn selection_follows_the_result_not_the_row() {
        use ashdash_core::Item;
        let items: Vec<Item> = ["a", "b", "c"].iter().map(|i| Item::new(*i, *i)).collect();
        assert_eq!(reselect(Some("c"), 0, &items), 2, "same result, new position");
        assert_eq!(reselect(Some("gone"), 2, &items), 2, "falls back to the old row");
        assert_eq!(reselect(Some("gone"), 9, &items), 2, "clamped");
        assert_eq!(reselect(None, 1, &items), 1);
        assert_eq!(reselect(Some("x"), 3, &[]), 0);
    }

    #[test]
    fn a_late_wakeup_runs_once() {
        let mut s = Scheduler::default();
        s.request(1, "x");
        let now = t0();
        s.finished(1, now, Some(ms(100)));
        assert!(s.take_due(now + ms(900)).is_some());
        assert!(s.take_due(now + ms(900)).is_none(), "missed beats are not replayed");
    }
}
