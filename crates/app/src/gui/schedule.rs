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

/// What the launcher knows about the user's hand on the selection while answers arrive.
///
/// Two rules (`docs/PRODUCT.md` section 2, "Predictability"):
/// * when the final answer replaces the list, the result the user moved to stays highlighted
///   (by id); if the user has not moved the selection since the query was sent, the new top
///   result is highlighted;
/// * Return (and Cmd+number) act on what the user *saw*: if the list was replaced in the frame
///   whose input is being handled, the key means the row of the list that was on screen before.
///
/// Pure state: the frame loop calls [`begin_frame`](Self::begin_frame) first thing, the result
/// polling calls [`replaced`](Self::replaced) / [`replaced_with`](Self::replaced_with), the key
/// handling asks [`enter_target`](Self::enter_target) / [`number_target`](Self::number_target),
/// and [`end_frame`](Self::end_frame) closes the frame.
#[derive(Debug, Default)]
pub struct SelectionTrack {
    /// The query generation the fields below belong to.
    generation: u64,
    /// The selected row as of the last observation (user or code); a different row at the next
    /// frame start is a move by the user.
    baseline: usize,
    /// The user moved the selection since the query was sent.
    moved: bool,
    /// The result the user moved to last (by id), while `moved`.
    anchor: Option<String>,
    /// The list was replaced since the last handled input frame.
    replaced: Option<Replaced>,
}

/// The list on screen before the first replacement since the last handled input frame.
#[derive(Debug)]
struct Replaced {
    /// Ids of the rows the user saw, in order.
    seen_ids: Vec<String>,
    /// The row that was highlighted in that frame.
    seen_selected: usize,
    /// The row highlighted after the newest replacement; a different one at key handling means
    /// the keys of this frame moved it (meant for the old list).
    selected_after: usize,
    /// Every replacement since then was a repeat of the same query (`rerun`): the rows are the
    /// same results with fresh data, so a key never has to be refused (ids may be titles that
    /// change on every repeat).
    rerun_only: bool,
}

/// Where a key that names a row of the list the user saw lands in the list that is there now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Run this row.
    Row(usize),
    /// The list changed under the key press and the row cannot be told: do nothing, say so.
    Changed,
    /// There was no such row: ignore the key.
    Nothing,
}

impl SelectionTrack {
    /// Start of a frame, before results are applied: `selected` and `items` are what the
    /// previous frame drew. A different generation is a new query (unmoved, whatever the row);
    /// the same generation with a different row is the user moving.
    pub fn begin_frame(&mut self, generation: u64, selected: usize, items: &[ashdash_core::Item]) {
        if generation != self.generation {
            self.generation = generation;
            self.moved = false;
            self.anchor = None;
            self.baseline = selected;
        } else if selected != self.baseline {
            self.baseline = selected;
            self.moved = true;
            self.anchor = items.get(selected).map(|i| i.id.clone());
        }
    }

    /// End of the frame's input handling: the replacement (if any) has been accounted for.
    pub fn end_frame(&mut self) {
        self.replaced = None;
    }

    /// Forget everything (the launcher was shown or hidden).
    pub fn reset(&mut self) {
        *self = SelectionTrack::default();
    }

    fn note(&mut self, old: &[ashdash_core::Item], old_selected: usize, selected: usize, rerun: bool) -> usize {
        self.baseline = selected;
        match &mut self.replaced {
            Some(r) => {
                r.selected_after = selected;
                r.rerun_only &= rerun;
            }
            None => {
                self.replaced = Some(Replaced {
                    seen_ids: old.iter().map(|i| i.id.clone()).collect(),
                    seen_selected: old_selected,
                    selected_after: selected,
                    rerun_only: rerun,
                });
            }
        }
        selected
    }

    /// The list `old` (row `old_selected` highlighted) was replaced by `new` and the selection is
    /// set by the caller (placeholder rows: row 0).
    pub fn replaced_with(&mut self, old: &[ashdash_core::Item], old_selected: usize, selected: usize) -> usize {
        self.note(old, old_selected, selected, false)
    }

    /// The same query ran again (`rerun`) and the list was replaced; the caller keeps the selection
    /// on the same result (by id) or the same position. Unlike the other replacements this never
    /// makes a key press refuse: see [`enter_target`](Self::enter_target).
    pub fn replaced_with_rerun(&mut self, old: &[ashdash_core::Item], old_selected: usize, selected: usize) -> usize {
        self.note(old, old_selected, selected, true)
    }

    /// The final answer `new` replaced `old`. Returns the row to highlight: the remembered result
    /// when going back from a panel (`keep`, falling back to `restore_index`), else the result the
    /// user moved to, else (never moved, or that result is gone) the top.
    pub fn replaced(
        &mut self,
        old: &[ashdash_core::Item],
        old_selected: usize,
        new: &[ashdash_core::Item],
        keep: Option<&str>,
        restore_index: usize,
    ) -> usize {
        // a move the user made since the query was sent (also while the re-run after "back" was in
        // flight) beats the remembered result; the remembered one is the fallback, then the top
        let moved_to = if self.moved {
            self.anchor
                .as_deref()
                .and_then(|id| new.iter().position(|i| i.id == id))
        } else {
            None
        };
        let selected = match moved_to {
            Some(i) => i,
            None if keep.is_some() => reselect(keep, restore_index, new),
            None => 0,
        };
        self.note(old, old_selected, selected, false)
    }

    /// Return was pressed with row `selected` highlighted in `items` (the current list).
    pub fn enter_target(&self, selected: usize, items: &[ashdash_core::Item]) -> Target {
        let Some(r) = &self.replaced else {
            return Target::Row(selected);
        };
        if r.rerun_only {
            // the same query again: the row the user saw if it is still there, else the highlighted
            // row (an arrow key in this frame moved it), clamped
            let seen = (selected == r.selected_after)
                .then(|| r.seen_ids.get(r.seen_selected))
                .flatten()
                .and_then(|id| items.iter().position(|i| i.id == *id));
            return match seen {
                Some(i) => Target::Row(i),
                None if items.is_empty() => Target::Nothing,
                None => Target::Row(selected.min(items.len() - 1)),
            };
        }
        if selected != r.selected_after {
            return Target::Changed; // an arrow key in the same frame was meant for the old list
        }
        match r.seen_ids.get(r.seen_selected) {
            Some(id) => find_row(id, items),
            None => Target::Changed,
        }
    }

    /// Cmd+`n` (0-based) was pressed: the n-th row of the list the user saw.
    pub fn number_target(&self, n: usize, items: &[ashdash_core::Item]) -> Target {
        if n >= super::VISIBLE_ROWS {
            return Target::Nothing;
        }
        match &self.replaced {
            None if n < items.len() => Target::Row(n),
            None => Target::Nothing,
            Some(r) => match r.seen_ids.get(n) {
                Some(id) if r.rerun_only => items
                    .iter()
                    .position(|i| i.id == *id)
                    .or((n < items.len()).then_some(n))
                    .map_or(Target::Nothing, Target::Row),
                Some(id) => find_row(id, items),
                None => Target::Nothing,
            },
        }
    }
}

fn find_row(id: &str, items: &[ashdash_core::Item]) -> Target {
    items
        .iter()
        .position(|i| i.id == id)
        .map_or(Target::Changed, Target::Row)
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

    // ---- selection when a list is replaced (PRODUCT.md section 2)

    fn items(ids: &[&str]) -> Vec<ashdash_core::Item> {
        ids.iter().map(|i| ashdash_core::Item::new(*i, *i)).collect()
    }

    /// A query was just sent (`generation` 1), the old list `old` is on screen with row `at` highlighted.
    fn sent(old: &[&str], at: usize) -> (SelectionTrack, Vec<ashdash_core::Item>) {
        let old = items(old);
        let mut t = SelectionTrack::default();
        t.begin_frame(1, 0, &old); // the frame that shows the new query text
        if at != 0 {
            t.begin_frame(1, at, &old); // the user moved down in the next one
        }
        (t, old)
    }

    #[test]
    fn final_answer_keeps_the_moved_selection_by_id() {
        let (mut t, old) = sent(&["a", "b", "c", "d"], 2);
        let new = items(&["x", "c", "y", "z"]);
        let sel = t.replaced(&old, 2, &new, None, 0);
        assert_eq!(sel, 1, "the user was on 'c': it is still under the cursor");
    }

    #[test]
    fn final_answer_selects_the_new_top_when_the_selection_was_not_moved() {
        let (mut t, old) = sent(&["a", "b", "c"], 0);
        let new = items(&["x", "a", "y"]);
        assert_eq!(t.replaced(&old, 0, &new, None, 0), 0, "row 0, not 'a'");
    }

    #[test]
    fn final_answer_goes_to_the_top_when_the_highlighted_item_is_gone() {
        let (mut t, old) = sent(&["a", "b", "c"], 2);
        let new = items(&["x", "y", "z", "w"]);
        assert_eq!(t.replaced(&old, 2, &new, None, 0), 0);
    }

    #[test]
    fn moving_back_to_the_top_still_counts_as_moved() {
        let (mut t, old) = sent(&["a", "b", "c"], 1);
        t.begin_frame(1, 0, &old); // back up on 'a'
        let new = items(&["x", "y", "a"]);
        assert_eq!(
            t.replaced(&old, 0, &new, None, 0),
            2,
            "the user chose 'a'; it stays chosen"
        );
    }

    #[test]
    fn a_new_query_starts_unmoved_again() {
        let (mut t, old) = sent(&["a", "b", "c"], 2);
        t.begin_frame(2, 0, &old); // typed another character: selection reset to row 0, new generation
        let new = items(&["x", "c"]);
        assert_eq!(t.replaced(&old, 0, &new, None, 0), 0);
    }

    #[test]
    fn the_pending_rows_do_not_erase_what_the_user_chose() {
        let (mut t, old) = sent(&["a", "b", "c"], 2);
        let pending = items(&["pending"]);
        assert_eq!(t.replaced(&old, 2, &pending, None, 0), 0, "placeholder rows: top");
        // next frame: nothing moved, the placeholder is highlighted
        t.begin_frame(1, 0, &pending);
        t.end_frame();
        let fin = items(&["q", "c", "r"]);
        assert_eq!(
            t.replaced(&pending, 0, &fin, None, 0),
            1,
            "still the item the user picked"
        );
    }

    #[test]
    fn going_back_from_a_panel_restores_the_remembered_result() {
        let (mut t, old) = sent(&["a"], 0);
        let new = items(&["x", "b", "y"]);
        assert_eq!(t.replaced(&old, 0, &new, Some("b"), 2), 1, "by id");
        let new = items(&["x", "y", "z"]);
        assert_eq!(
            t.replaced(&old, 0, &new, Some("b"), 2),
            2,
            "gone: the old row, as before"
        );
        assert_eq!(t.replaced(&old, 0, &new, Some("b"), 9), 2, "clamped");
    }

    #[test]
    fn return_runs_the_row_the_user_saw_not_the_one_that_arrived_under_the_cursor() {
        let (mut t, old) = sent(&["a", "b", "c"], 1);
        let new = items(&["x", "y", "b"]);
        let sel = t.replaced(&old, 1, &new, None, 0);
        assert_eq!(sel, 2);
        assert_eq!(t.enter_target(sel, &new), Target::Row(2), "'b' was highlighted");
        // a frame without a replacement: plain selected row
        t.end_frame();
        assert_eq!(t.enter_target(1, &new), Target::Row(1));
    }

    #[test]
    fn return_does_nothing_when_the_row_the_user_saw_is_gone() {
        let (mut t, old) = sent(&["a", "b", "c"], 1);
        let new = items(&["x", "y"]);
        let sel = t.replaced(&old, 1, &new, None, 0);
        assert_eq!(t.enter_target(sel, &new), Target::Changed);
    }

    #[test]
    fn return_does_nothing_when_nothing_was_highlighted_in_the_frame_the_user_saw() {
        let mut t = SelectionTrack::default();
        t.begin_frame(1, 0, &[]);
        let new = items(&["x", "y"]);
        let sel = t.replaced(&[], 0, &new, None, 0);
        assert_eq!(t.enter_target(sel, &new), Target::Changed);
    }

    #[test]
    fn a_move_in_the_same_frame_as_the_replacement_is_not_guessed() {
        let (mut t, old) = sent(&["a", "b", "c"], 0);
        let new = items(&["x", "a", "y"]);
        let sel = t.replaced(&old, 0, &new, None, 0);
        // Down was pressed in the same frame: it was meant for the old list
        assert_eq!(t.enter_target(sel + 1, &new), Target::Changed);
    }

    #[test]
    fn cmd_number_means_the_row_the_user_saw() {
        let (mut t, old) = sent(&["a", "b", "c"], 0);
        let new = items(&["x", "c", "a"]);
        t.replaced(&old, 0, &new, None, 0);
        assert_eq!(t.number_target(2, &new), Target::Row(1), "row 3 was 'c'");
        assert_eq!(t.number_target(1, &new), Target::Changed, "'b' is gone");
        assert_eq!(t.number_target(5, &new), Target::Nothing, "there was no row 6");
        t.end_frame();
        assert_eq!(t.number_target(1, &new), Target::Row(1));
        assert_eq!(t.number_target(2, &new), Target::Row(2));
        assert_eq!(t.number_target(3, &new), Target::Nothing);
    }

    #[test]
    fn several_replacements_in_one_frame_still_refer_to_the_frame_the_user_saw() {
        let (mut t, old) = sent(&["a", "b", "c"], 1);
        let pending = items(&["p"]);
        t.replaced(&old, 1, &pending, None, 0);
        let fin = items(&["x", "b"]);
        let sel = t.replaced(&pending, 0, &fin, None, 0);
        assert_eq!(sel, 1);
        assert_eq!(t.enter_target(sel, &fin), Target::Row(1), "the user saw 'b' selected");
    }

    // ---- Wave 6 review fixes (docs/reviews/wave5.md 1 and 2)

    /// A clock-style list: the id of a row is its title, so every rerun gives new ids.
    fn clock(second: u32) -> Vec<ashdash_core::Item> {
        (0..3)
            .map(|i| {
                let t = format!("12:00:{:02} zone {i}", second);
                ashdash_core::Item::new(t.clone(), t)
            })
            .collect()
    }

    #[test]
    fn a_rerun_with_changing_ids_never_makes_return_refuse() {
        let old = clock(1);
        let mut t = SelectionTrack::default();
        t.begin_frame(1, 1, &old);
        let new = clock(2);
        let sel = t.replaced_with_rerun(&old, 1, reselect(Some(&old[1].id), 1, &new));
        assert_eq!(sel, 1, "the id is gone: the position stays");
        assert_eq!(
            t.enter_target(sel, &new),
            Target::Row(1),
            "Return runs the selected row"
        );
        assert_eq!(t.number_target(2, &new), Target::Row(2), "Cmd+3 runs the third row");
        assert_eq!(t.number_target(5, &new), Target::Nothing);
    }

    #[test]
    fn a_rerun_that_shrinks_the_list_runs_the_clamped_selected_row() {
        let old = clock(1);
        let mut t = SelectionTrack::default();
        t.begin_frame(1, 2, &old);
        let new: Vec<_> = clock(2).into_iter().take(2).collect();
        let sel = t.replaced_with_rerun(&old, 2, reselect(Some(&old[2].id), 2, &new));
        assert_eq!(sel, 1);
        assert_eq!(t.enter_target(sel, &new), Target::Row(1));
        assert_eq!(
            t.number_target(2, &new),
            Target::Nothing,
            "there is no third row any more"
        );
        assert_eq!(t.enter_target(0, &[]), Target::Nothing, "an empty list has no row");
    }

    #[test]
    fn a_rerun_with_stable_ids_follows_the_row_the_user_saw() {
        let old = items(&["a", "b", "c"]);
        let mut t = SelectionTrack::default();
        t.begin_frame(1, 1, &old);
        let new = items(&["c", "b", "a"]);
        let sel = t.replaced_with_rerun(&old, 1, reselect(Some("b"), 1, &new));
        assert_eq!(t.enter_target(sel, &new), Target::Row(1), "'b' is still 'b'");
        assert_eq!(t.number_target(0, &new), Target::Row(2), "row 1 was 'a'");
    }

    #[test]
    fn a_rerun_after_a_real_replacement_in_the_same_frame_stays_strict() {
        let (mut t, old) = sent(&["a", "b", "c"], 1);
        let fin = items(&["x", "y"]);
        let sel = t.replaced(&old, 1, &fin, None, 0);
        let again = items(&["x2", "y2"]);
        let sel = t.replaced_with_rerun(&fin, sel, 0);
        assert_eq!(t.enter_target(sel, &again), Target::Changed, "'b' is gone");
    }

    #[test]
    fn back_from_a_panel_a_move_made_during_the_rerun_wins() {
        // back from a panel: new generation, the remembered result is 'b'
        let old = items(&["a", "b", "c"]);
        let mut t = SelectionTrack::default();
        t.begin_frame(2, 1, &old); // the restored list, 'b' highlighted
        t.begin_frame(2, 2, &old); // the user moves down to 'c' while the query runs
        let new = items(&["a", "b", "c"]);
        assert_eq!(
            t.replaced(&old, 2, &new, Some("b"), 1),
            2,
            "the user's 'c', not the remembered 'b'"
        );
    }

    #[test]
    fn back_from_a_panel_the_remembered_result_wins_when_the_user_did_not_move() {
        let old = items(&["a", "b", "c"]);
        let mut t = SelectionTrack::default();
        t.begin_frame(2, 1, &old);
        let new = items(&["x", "b", "c"]);
        assert_eq!(t.replaced(&old, 1, &new, Some("b"), 1), 1);
    }

    #[test]
    fn back_from_a_panel_the_remembered_result_is_the_fallback_when_the_moved_to_row_is_gone() {
        let old = items(&["a", "b", "c"]);
        let mut t = SelectionTrack::default();
        t.begin_frame(2, 1, &old);
        t.begin_frame(2, 2, &old);
        let new = items(&["a", "b"]);
        assert_eq!(t.replaced(&old, 2, &new, Some("b"), 1), 1);
    }
}
