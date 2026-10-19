//! Opt-in main-thread diagnostics for the frame trace (`ASHDASH_FRAME_TRACE=1`, see [`super::frametrace`]): *when does a key reach the
//! app, and what was the main thread doing before it did?*
//!
//! The frame trace alone cannot tell "the OS delivered the key late" from "the app was blocked and handled it late" (a long `gap` with a
//! short `dur` is both). Two cheap probes, installed only when the trace is on, separate them:
//!
//! * an `NSEvent` local monitor sees every key event inside `-[NSApplication sendEvent:]`, before winit and egui get it. It prints
//!   `[ft] t=… event key down code=35 age=1002.3` where `age` is the time from the event's own timestamp (made by the window server
//!   when the key was pressed) to the monitor: a large `age` means the key waited in a queue before the app looked at it.
//! * a `CFRunLoop` observer notes when the main run loop wakes and goes back to sleep. A stretch of more than [`BUSY_REPORT_MS`] between
//!   the two is a main thread that did not return to its run loop: `[ft] t=… loop busy=1003.1` (the paint of a frame, including
//!   `nextDrawable`, runs inside this stretch). A long sleep with a late key is the other case: `loop slept=…` in the key line.
//!
//! With `ASHDASH_STALL_SAMPLE=1` as well, a watchdog thread runs the system `sample` tool on the process when one stretch has lasted
//! [`SAMPLE_AFTER_MS`], and prints the main thread's part of its call graph: the stack of the blocked call, which settles what it waited for.
//!
//! The pure bookkeeping ([`LoopClock`]) is unit-tested; the AppKit / CoreFoundation glue is not (it needs a window server).

use std::time::{Duration, Instant};

/// A stretch of the main thread between waking up and going back to sleep that is at least this long is printed.
pub const BUSY_REPORT_MS: f64 = 50.0;

/// The watchdog samples the process when a stretch has lasted this long and is not over yet.
pub const SAMPLE_AFTER_MS: f64 = 250.0;

/// What the main run loop did lately, fed by the observer callbacks.
#[derive(Debug, Default, Clone)]
pub struct LoopClock {
    /// When the loop last woke up (None = it is asleep or has not run yet).
    awake_since: Option<Instant>,
    /// When the loop last went to sleep.
    asleep_since: Option<Instant>,
    /// How long the last sleep took, in ms.
    last_sleep_ms: f64,
    /// How long the last awake stretch took, in ms.
    last_busy_ms: f64,
}

fn ms(a: Instant, b: Instant) -> f64 {
    b.saturating_duration_since(a).as_secs_f64() * 1000.0
}

impl LoopClock {
    /// The loop woke up (`kCFRunLoopAfterWaiting`).
    pub fn woke(&mut self, now: Instant) {
        if let Some(s) = self.asleep_since.take() {
            self.last_sleep_ms = ms(s, now);
        }
        self.awake_since = Some(now);
    }

    /// The loop is about to sleep (`kCFRunLoopBeforeWaiting`). Returns the length of the stretch that ends now when it is worth a
    /// trace line.
    pub fn slept(&mut self, now: Instant) -> Option<f64> {
        let busy = self.awake_since.take().map(|a| ms(a, now));
        self.asleep_since = Some(now);
        if let Some(b) = busy {
            self.last_busy_ms = b;
        }
        busy.filter(|b| *b >= BUSY_REPORT_MS)
    }

    /// How long the current awake stretch has lasted (None while the loop sleeps).
    pub fn awake_for(&self, now: Instant) -> Option<f64> {
        self.awake_since.map(|a| ms(a, now))
    }

    /// The part of a key line that says what the main thread was doing: the length of the sleep before the loop woke for this key, and
    /// how long it has been awake.
    pub fn describe(&self, now: Instant) -> String {
        format!(
            "slept_before={:.1} awake_for={:.1} prev_busy={:.1}",
            self.last_sleep_ms,
            self.awake_for(now).unwrap_or(0.0),
            self.last_busy_ms
        )
    }
}

/// Whether the watchdog should run `sample` now: the stretch is long, and it has not been sampled yet (`last_sampled` = the start of the
/// stretch that was).
pub fn should_sample(awake_since: Option<Instant>, now: Instant, last_sampled: Option<Instant>) -> bool {
    match awake_since {
        Some(a) => ms(a, now) >= SAMPLE_AFTER_MS && last_sampled != Some(a),
        None => false,
    }
}

/// The lines of the main thread in the text a `sample` run wrote (the first block under "Call graph:" whose header names the main
/// thread), at most `max` of them.
pub fn main_thread_stack(sample_output: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_graph = false;
    let mut in_main = false;
    for line in sample_output.lines() {
        if line.starts_with("Call graph:") {
            in_graph = true;
            continue;
        }
        if !in_graph {
            continue;
        }
        // a thread header: "    1234 Thread_567   DispatchQueue_1: com.apple.main-thread  (serial)"
        if line.contains("Thread_") {
            if in_main {
                break;
            }
            in_main = line.contains("main-thread");
        }
        if in_main {
            out.push(line.trim_end().to_string());
            if out.len() >= max {
                break;
            }
        } else if line.starts_with("Total number in stack") || line.starts_with("Sort by top of stack") {
            break;
        }
    }
    out
}

pub fn busy_line(busy_ms: f64) -> String {
    format!("[ft] t={:.1} loop busy={busy_ms:.1}", super::frametrace::unix_ms())
}

pub fn key_line(kind: &str, code: u16, age_ms: f64, clock: &LoopClock, now: Instant) -> String {
    format!(
        "[ft] t={:.1} event key {kind} code={code} age={age_ms:.1} {}",
        super::frametrace::unix_ms(),
        clock.describe(now)
    )
}

/// How often the watchdog looks at the clock.
pub const WATCHDOG_TICK: Duration = Duration::from_millis(50);

#[cfg(target_os = "macos")]
pub use mac_glue::install;

/// Install the probes when `ASHDASH_FRAME_TRACE` is on (nothing otherwise). Main thread only.
#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(target_os = "macos")]
mod mac_glue {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Mutex;
    use std::time::Instant;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
    use objc2_foundation::NSProcessInfo;

    use super::{LoopClock, WATCHDOG_TICK, busy_line, key_line, main_thread_stack, should_sample};

    static CLOCK: Mutex<LoopClock> = Mutex::new(LoopClock {
        awake_since: None,
        asleep_since: None,
        last_sleep_ms: 0.0,
        last_busy_ms: 0.0,
    });

    const AFTER_WAITING: usize = 1 << 6;
    const BEFORE_WAITING: usize = 1 << 5;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopAddObserver(rl: *mut c_void, observer: *mut c_void, mode: *const c_void);
        fn CFRunLoopObserverCreate(
            allocator: *const c_void,
            activities: usize,
            repeats: u8,
            order: isize,
            callout: extern "C" fn(*mut c_void, usize, *mut c_void),
            context: *mut c_void,
        ) -> *mut c_void;
    }

    extern "C" fn observer(_observer: *mut c_void, activity: usize, _info: *mut c_void) {
        let now = Instant::now();
        let Ok(mut clock) = CLOCK.lock() else { return };
        if activity == AFTER_WAITING {
            clock.woke(now);
        } else if activity == BEFORE_WAITING
            && let Some(busy) = clock.slept(now)
        {
            eprintln!("{}", busy_line(busy));
        }
    }

    pub fn install() {
        if !super::super::frametrace::enabled() {
            return;
        }
        let Some(_mtm) = MainThreadMarker::new() else { return };
        // The observers run last (before winit's own `order: CFIndex::MAX` ones were added, equal order runs in creation order), so a
        // stretch includes the redraw winit runs from its own before-waiting observer, which is where the frame is painted and presented.
        // SAFETY: plain CoreFoundation calls on the main thread with a callout that lives for the whole process and a null context.
        unsafe {
            let obs = CFRunLoopObserverCreate(
                std::ptr::null(),
                AFTER_WAITING | BEFORE_WAITING,
                1,
                isize::MAX,
                observer,
                std::ptr::null_mut(),
            );
            if !obs.is_null() {
                CFRunLoopAddObserver(CFRunLoopGetMain(), obs, kCFRunLoopCommonModes);
            }
        }
        let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged;
        let handler = RcBlock::new(|ev: NonNull<NSEvent>| -> *mut NSEvent {
            let now = Instant::now();
            // SAFETY: AppKit passes a live event to the monitor; it is only read here.
            let (kind, code, stamp) = unsafe {
                let e = ev.as_ref();
                let kind = match e.r#type() {
                    NSEventType::KeyDown => "down",
                    NSEventType::KeyUp => "up",
                    _ => "flags",
                };
                (kind, e.keyCode(), e.timestamp())
            };
            let age = (NSProcessInfo::processInfo().systemUptime() - stamp) * 1000.0;
            if let Ok(clock) = CLOCK.lock() {
                eprintln!("{}", key_line(kind, code, age, &clock, now));
            }
            ev.as_ptr()
        });
        // SAFETY: the block returns the event it was given (a valid pointer); the monitor lives as long as the process (never removed).
        let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
        std::mem::forget(monitor);
        if std::env::var_os("ASHDASH_STALL_SAMPLE").is_some_and(|v| v != "0") {
            std::thread::spawn(watchdog);
        }
    }

    /// Run `sample` on ourselves when the main thread has been awake for too long, and print where it was.
    fn watchdog() {
        let pid = std::process::id();
        let mut last: Option<Instant> = None;
        let mut n = 0u32;
        loop {
            std::thread::sleep(WATCHDOG_TICK);
            let (awake, now) = match CLOCK.lock() {
                Ok(c) => (c.awake_since, Instant::now()),
                Err(_) => return,
            };
            if !should_sample(awake, now, last) {
                continue;
            }
            last = awake;
            n += 1;
            let file = std::env::temp_dir().join(format!("ashdash-stall-{pid}-{n}.txt"));
            // 1 s at 10 ms: the stack of a 1 s stall is caught many times
            let ok = std::process::Command::new("/usr/bin/sample")
                .args([&pid.to_string(), "1", "10", "-file"])
                .arg(&file)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let text = if ok {
                std::fs::read_to_string(&file).unwrap_or_default()
            } else {
                String::new()
            };
            eprintln!(
                "[ft] t={:.1} stall-sample n={n} file={} ok={ok}",
                super::super::frametrace::unix_ms(),
                file.display()
            );
            for l in main_thread_stack(&text, 60) {
                eprintln!("[ft-stack] {l}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_short_stretch_is_not_reported_and_a_long_one_is() {
        let t = Instant::now();
        let mut c = LoopClock::default();
        c.woke(at(t, 0));
        assert_eq!(c.slept(at(t, 10)), None);
        c.woke(at(t, 500));
        let busy = c.slept(at(t, 1503)).expect("a 1 s stretch is reported");
        assert!((busy - 1003.0).abs() < 1.0, "{busy}");
    }

    #[test]
    fn the_sleep_before_a_wake_is_remembered() {
        let t = Instant::now();
        let mut c = LoopClock::default();
        c.woke(at(t, 0));
        c.slept(at(t, 5));
        c.woke(at(t, 905));
        assert!((c.last_sleep_ms - 900.0).abs() < 1.0);
        assert!(c.describe(at(t, 910)).contains("slept_before=900.0"));
        assert!((c.awake_for(at(t, 910)).unwrap() - 5.0).abs() < 1.0);
    }

    #[test]
    fn while_asleep_there_is_no_awake_time() {
        let t = Instant::now();
        let mut c = LoopClock::default();
        c.woke(at(t, 0));
        c.slept(at(t, 1));
        assert_eq!(c.awake_for(at(t, 50)), None);
    }

    #[test]
    fn the_watchdog_samples_a_long_stretch_once() {
        let t = Instant::now();
        assert!(!should_sample(None, at(t, 5000), None), "asleep: nothing to sample");
        assert!(!should_sample(Some(t), at(t, 100), None), "too short");
        assert!(should_sample(Some(t), at(t, 300), None));
        assert!(
            !should_sample(Some(t), at(t, 600), Some(t)),
            "this stretch was sampled already"
        );
        assert!(
            should_sample(Some(at(t, 700)), at(t, 1000), Some(t)),
            "a new stretch is sampled again"
        );
    }

    #[test]
    fn the_main_thread_block_is_cut_out_of_a_sample() {
        let text = "Analysis of sampling ashdash (pid 1)\n\nCall graph:\n    10 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 10 start  (in dyld)\n    +   10 nextDrawable  (in QuartzCore)\n    10 Thread_2: wgpu\n    + 10 read\n\nTotal number in stack (recursive counted multiple, when >=5):\n";
        let got = main_thread_stack(text, 50);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got[2].contains("nextDrawable"));
        assert!(main_thread_stack("no graph here", 5).is_empty());
        assert_eq!(main_thread_stack(text, 2).len(), 2);
    }

    #[test]
    fn the_lines_have_the_shape_the_report_reads() {
        let c = LoopClock::default();
        let l = key_line("down", 35, 1002.34, &c, Instant::now());
        assert!(l.starts_with("[ft] t="), "{l}");
        assert!(l.contains(" event key down code=35 age=1002.3 slept_before="), "{l}");
        assert!(busy_line(1003.1).ends_with(" loop busy=1003.1"));
    }
}
