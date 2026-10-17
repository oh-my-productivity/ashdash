//! Which panics the program survives on purpose.
//!
//! The launcher's crash-report hook (in the app crate) runs for every panic in the process. A
//! panic that a guard catches, or that ends a thread whose death the rest of the program handles,
//! is not a crash: the report says so and the footer does not claim that Ashdash quit. The
//! guards mark themselves here, per thread, because the code that catches may live in this crate
//! while the hook lives in the app.

use std::any::Any;
use std::cell::Cell;

thread_local! {
    /// Depth of nested [`catching`] calls on this thread.
    static GUARDED: Cell<u32> = const { Cell::new(0) };
    /// The whole thread is one whose panic the program handles ([`mark_thread`]).
    static WHOLE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Run `f`; a panic inside is caught and returned. A panic hook that asks [`is_recovered`] during
/// that panic gets `true`.
pub fn catching<T>(f: impl FnOnce() -> T) -> Result<T, Box<dyn Any + Send>> {
    struct Leave;
    impl Drop for Leave {
        fn drop(&mut self) {
            let _ = GUARDED.try_with(|g| g.set(g.get().saturating_sub(1)));
        }
    }
    GUARDED.with(|g| g.set(g.get() + 1));
    let _leave = Leave;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
}

/// Declare that a panic anywhere on the calling thread is handled by whoever waits for it (a
/// per-query thread whose sender is dropped, a connection thread whose slot is released on
/// unwind). Call it first thing in the thread.
pub fn mark_thread() {
    WHOLE_THREAD.with(|w| w.set(true));
}

/// Is a panic on this thread, right now, one the program survives? Safe to call from a panic hook.
pub fn is_recovered() -> bool {
    GUARDED.try_with(|g| g.get() > 0).unwrap_or(false) || WHOLE_THREAD.try_with(Cell::get).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_code_inside_a_guard_or_a_marked_thread_is_recovered() {
        assert!(!is_recovered());
        let inside = catching(is_recovered).unwrap();
        assert!(inside, "inside the guard");
        let nested = catching(|| catching(is_recovered).unwrap() && is_recovered()).unwrap();
        assert!(nested);
        assert!(!is_recovered(), "the guard is left again");
        // a panic through the guard leaves it too
        let _ = catching(|| -> () { std::panic::resume_unwind(Box::new("x")) });
        assert!(!is_recovered());
        let marked = std::thread::spawn(|| {
            mark_thread();
            is_recovered()
        })
        .join()
        .unwrap();
        assert!(marked);
        assert!(!is_recovered(), "other threads are not marked");
    }
}
