//! Helpers for tests that guard performance without depending on how fast the machine is.
//!
//! A test must never compare a wall-clock time with a constant: a debug build is 4 to 60 times slower than release, and a
//! shared CI runner is several times slower and noisier than a developer machine. Three kinds of guard are used instead:
//!
//! * **Work counting** ([`allocations_in`]): how many heap allocations a call makes is exact and identical on every machine.
//! * **Scaling** ([`best_of_pair`]): run the same code on a small and a large input, in the same test and interleaved, and compare
//!   the two times with each other. A linear algorithm gives about the size ratio, a quadratic one the square of it, whatever
//!   the machine does. The times are CPU times of the thread, so a busy machine does not distort them, and the best of several
//!   rounds is compared, because noise only ever adds time.
//! * **Absolute ceilings in release builds only**: `perf_*` tests, `#[ignore]`d in the normal run and executed by the `perf`
//!   CI job with `cargo test --release -p ashdash-core -- --ignored perf_` (see docs/CI.md). They start with
//!   [`require_release`].

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    // `const` initialiser and no destructor: touching it from inside the allocator cannot allocate or recurse.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn count() {
    let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
}

/// Counts allocations per thread (so tests running in parallel do not see each other), then defers to the system allocator.
struct Counting;

// SAFETY: every method forwards to `System` with the arguments it was given; the only addition is a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The result of `f` and the number of heap allocations (and reallocations) it made on the calling thread.
pub(crate) fn allocations_in<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = ALLOCATIONS.with(Cell::get);
    let out = f();
    (out, ALLOCATIONS.with(Cell::get) - before)
}

/// CPU time consumed so far by the calling thread, or `None` where the OS cannot say.
fn thread_cpu_time() -> Option<Duration> {
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    }
    // CLOCK_THREAD_CPUTIME_ID
    const CLOCK: i32 = if cfg!(target_os = "macos") { 16 } else { 3 };
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable `timespec` (two 64-bit fields on every 64-bit target) for the duration of the call.
    let rc = unsafe { clock_gettime(CLOCK, &mut ts) };
    (rc == 0).then(|| Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32))
}

/// Measures the CPU time of the calling thread (the wall clock where that is unavailable). CPU time does not grow when the
/// scheduler parks the thread behind other processes, which is what makes a ratio of two timings stable on a loaded machine: a
/// wall-clock comparison of a 10 ms run with a 100 ms run is not, because the long run is interrupted far more often.
struct Stopwatch {
    cpu: Option<Duration>,
    wall: Instant,
}

impl Stopwatch {
    fn start() -> Stopwatch {
        Stopwatch {
            cpu: thread_cpu_time(),
            wall: Instant::now(),
        }
    }

    fn elapsed(&self) -> Duration {
        match (self.cpu, thread_cpu_time()) {
            (Some(start), Some(now)) => now.saturating_sub(start),
            _ => self.wall.elapsed(),
        }
    }
}

/// Run `a` and `b` alternately `rounds` times and return the least CPU time each took in one round. Interleaving gives both
/// the same share of whatever else the machine is doing; taking the minimum drops the rounds a neighbour inflated (cache,
/// frequency, a hypervisor taking the core). Both closures must run on the calling thread.
pub(crate) fn best_of_pair(rounds: usize, mut a: impl FnMut(), mut b: impl FnMut()) -> (Duration, Duration) {
    let (mut best_a, mut best_b) = (Duration::MAX, Duration::MAX);
    for _ in 0..rounds {
        let t = Stopwatch::start();
        a();
        best_a = best_a.min(t.elapsed());
        let t = Stopwatch::start();
        b();
        best_b = best_b.min(t.elapsed());
    }
    (best_a, best_b)
}

/// For `perf_*` tests: an absolute time is only meaningful for an optimised build.
pub(crate) fn require_release() {
    if cfg!(debug_assertions) {
        panic!(
            "perf_ tests have absolute time ceilings that only hold for an optimised build: \
             cargo test --release -p ashdash-core -- --ignored perf_"
        );
    }
}
