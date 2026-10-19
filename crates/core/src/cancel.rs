//! Cooperative cancellation of a running query.
//!
//! The launcher asks the engine a question for every keystroke. The answer to an older keystroke is worthless once a newer one was
//! typed, so the query worker flags it ([`CancelToken::cancel`]) and starts the newest at once instead of waiting for it. Code that
//! can run long (a Script Filter, a Spotlight search) looks at the flag while it waits for its child process and kills the child
//! when it is set; short code never looks.
//!
//! The token travels as a thread-local ([`scope`] / [`is_cancelled`]) instead of as a parameter, because the long-running code is
//! many calls below `Engine::query` (provider, workflow, script runner, `Os::output_bounded`) and every one of those signatures
//! would change. A thread that is not inside a [`scope`] is never cancelled, so everything else behaves as before.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A flag shared between the thread that decides to give up and the thread that runs the work.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Ask the work to stop. Idempotent; visible to every clone.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

thread_local! {
    static CURRENT: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
}

/// Run `f` with `token` as this thread's current token (restored afterwards, also when `f` unwinds).
pub fn scope<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<CancelToken>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CURRENT.with(|c| *c.borrow_mut() = previous);
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.borrow_mut().replace(token.clone())));
    f()
}

/// The token of the [`scope`] this thread is in, to hand to a helper thread that works for the same query (which runs
/// `scope(&token, ..)` itself).
pub fn current() -> Option<CancelToken> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Whether the work this thread is doing was cancelled. False outside a [`scope`].
pub fn is_cancelled() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(CancelToken::is_cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_cancelled_outside_a_scope() {
        assert!(!is_cancelled());
        let t = CancelToken::new();
        t.cancel();
        assert!(!is_cancelled(), "a token nobody scoped does not touch this thread");
    }

    #[test]
    fn a_helper_thread_can_join_the_scope_of_the_thread_that_started_it() {
        assert!(current().is_none());
        let t = CancelToken::new();
        scope(&t, || {
            let mine = current().expect("in a scope");
            let seen = std::thread::scope(|s| {
                s.spawn(|| scope(&mine, || (is_cancelled(), current().is_some())))
                    .join()
                    .unwrap()
            });
            assert_eq!(seen, (false, true));
            t.cancel();
            let cancelled = std::thread::scope(|s| s.spawn(|| scope(&mine, is_cancelled)).join().unwrap());
            assert!(cancelled, "the helper sees the cancel of the query");
        });
    }

    #[test]
    fn a_scope_sees_its_token_and_clones_share_the_flag() {
        let t = CancelToken::new();
        let other = t.clone();
        scope(&t, || {
            assert!(!is_cancelled());
            other.cancel();
            assert!(is_cancelled());
        });
        assert!(!is_cancelled(), "left the scope");
        assert!(t.is_cancelled());
    }

    #[test]
    fn scopes_nest_and_restore_the_outer_token() {
        let (outer, inner) = (CancelToken::new(), CancelToken::new());
        scope(&outer, || {
            inner.cancel();
            scope(&inner, || assert!(is_cancelled()));
            assert!(!is_cancelled(), "back in the outer scope, whose token is not set");
        });
    }

    #[test]
    fn the_token_is_restored_when_the_work_panics() {
        let t = CancelToken::new();
        t.cancel();
        let r = std::panic::catch_unwind(|| scope(&t, || panic!("boom")));
        assert!(r.is_err());
        assert!(!is_cancelled());
    }

    #[test]
    fn another_thread_has_its_own_token() {
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            assert!(is_cancelled());
            assert!(!std::thread::spawn(is_cancelled).join().unwrap());
        });
    }
}
