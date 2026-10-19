//! Opt-in UI-thread frame tracing: set `ASHDASH_FRAME_TRACE=1` and the launcher prints one line to stderr for every frame that
//! was slow (longer than [`SLOW_MS`]) or that handled typed text, and one line for every query request and every batch of results.
//! Each line carries the wall clock (`t=`, Unix milliseconds) so an outside observer that posted the key at a known time can compute
//! "key to glyph drawn" and "key to results" (scripts/qa/perf-gui.py, `PERF_MODE=typing`).
//!
//! Off (the default) it costs one cached environment lookup per call and allocates nothing: every method returns at once.
//!
//! A line looks like
//! `[ft] t=1760870000123.4 frame dur=2.1 gap=16.7 text=1 q="pu" gen=2 rows=9 SLOW logic{poll=0.1 ...} ui{keys=0.2 results=1.1 ...}`
//! * `dur`: from the start of `logic()` to the end of `ui()` (what the app itself did; the GPU work after `ui()` is in the next `gap`),
//! * `gap`: from the start of the previous frame to the start of this one (a long gap with a short `dur` means the thread was blocked
//!   outside the app's own code: event delivery, presenting the frame, vsync),
//! * the phase lists name where the time went; a phase is printed only when it took at least 0.05 ms.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A frame longer than this is printed even without typed text (the budget of a 60 Hz display).
pub const SLOW_MS: f64 = 16.0;

/// Whether `ASHDASH_FRAME_TRACE` is set (and not `0`). Read once.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ASHDASH_FRAME_TRACE").is_some_and(|v| v != "0"))
}

/// Milliseconds since the Unix epoch, with a fraction.
pub fn unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn ms(a: Instant, b: Instant) -> f64 {
    b.saturating_duration_since(a).as_secs_f64() * 1000.0
}

/// Which part of the frame a phase belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Logic,
    Ui,
}

/// The state of the frame being traced. All methods are no-ops when tracing is off.
pub struct FrameTrace {
    on: bool,
    frame_start: Option<Instant>,
    prev_start: Option<Instant>,
    mark: Option<Instant>,
    part: Part,
    gap: f64,
    phases: Vec<(Part, &'static str, f64)>,
}

impl Default for FrameTrace {
    fn default() -> Self {
        FrameTrace::new(enabled())
    }
}

impl FrameTrace {
    pub fn new(on: bool) -> Self {
        FrameTrace {
            on,
            frame_start: None,
            prev_start: None,
            mark: None,
            part: Part::Logic,
            gap: 0.0,
            phases: Vec::new(),
        }
    }

    /// The start of a frame (the first line of `logic()`).
    pub fn begin(&mut self) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        self.gap = self.prev_start.map_or(0.0, |p| ms(p, now));
        self.prev_start = Some(now);
        self.frame_start = Some(now);
        self.mark = Some(now);
        self.part = Part::Logic;
        self.phases.clear();
    }

    /// `logic()` is over; what follows is `ui()`.
    pub fn enter_ui(&mut self) {
        if !self.on {
            return;
        }
        self.phase("logic-rest");
        self.part = Part::Ui;
    }

    /// Close the current phase under `name` (the time since the previous mark).
    pub fn phase(&mut self, name: &'static str) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        if let Some(m) = self.mark {
            self.phases.push((self.part, name, ms(m, now)));
        }
        self.mark = Some(now);
    }

    /// The end of the frame (`ui()` returned): print the line when the frame was slow or handled text.
    pub fn end(&mut self, text_events: usize, query: &str, generation: u64, rows: usize) {
        if !self.on {
            return;
        }
        let Some(start) = self.frame_start.take() else {
            return;
        };
        let dur = ms(start, Instant::now());
        let slow = dur > SLOW_MS;
        if !slow && text_events == 0 {
            return;
        }
        let list = |part: Part| {
            self.phases
                .iter()
                .filter(|(p, _, t)| *p == part && *t >= 0.05)
                .map(|(_, n, t)| format!("{n}={t:.2}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        eprintln!(
            "[ft] t={:.1} frame dur={dur:.2} gap={:.1} text={text_events} q={query:?} gen={generation} rows={rows}{} logic{{{}}} ui{{{}}}",
            unix_ms(),
            self.gap,
            if slow { " SLOW" } else { "" },
            list(Part::Logic),
            list(Part::Ui),
        );
    }
}

/// A query was sent to the worker.
pub fn request(generation: u64, query: &str) {
    if enabled() {
        eprintln!("[ft] t={:.1} request gen={generation} q={query:?}", unix_ms());
    }
}

/// A batch of results was applied to the list on screen.
pub fn results(generation: u64, kind: impl std::fmt::Debug, rows: usize) {
    if enabled() {
        eprintln!(
            "[ft] t={:.1} results gen={generation} kind={kind:?} rows={rows}",
            unix_ms()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_trace_records_nothing() {
        let mut t = FrameTrace::new(false);
        t.begin();
        t.phase("a");
        t.enter_ui();
        t.phase("b");
        t.end(3, "q", 1, 0);
        assert!(t.phases.is_empty());
        assert!(t.frame_start.is_none() && t.prev_start.is_none());
    }

    #[test]
    fn an_enabled_trace_collects_phases_per_part_and_the_gap() {
        let mut t = FrameTrace::new(true);
        t.begin();
        t.phase("poll");
        t.enter_ui();
        t.phase("keys");
        assert_eq!(
            t.phases
                .iter()
                .map(|(p, n, _)| (*p == Part::Logic, *n))
                .collect::<Vec<_>>(),
            [(true, "poll"), (true, "logic-rest"), (false, "keys")]
        );
        assert_eq!(t.gap, 0.0, "the first frame has no previous one");
        t.end(0, "", 0, 0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        t.begin();
        assert!(t.gap >= 4.0, "{}", t.gap);
        assert!(t.phases.is_empty(), "a new frame starts clean");
    }

    #[test]
    fn the_clock_is_unix_time() {
        // 2020-01-01 in ms; any machine running the tests is later than that
        assert!(unix_ms() > 1_577_836_800_000.0);
    }
}
