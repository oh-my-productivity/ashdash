//! When does the launcher hide because it lost the keyboard focus?
//!
//! The decision is made from egui's `viewport().focused`, which on macOS is `NSWindow.isKeyWindow`, refreshed
//! on every frame and followed by a repaint whenever winit reports a focus change (egui-winit 0.36
//! `WindowEvent::Focused` -> `repaint: true`). It is made in a frame, so a frame has to come after the loss.
//!
//! Two things make a "not focused" reading unreliable and are handled here:
//!
//! * Right after `show()` the window still reports the `false` it had while hidden, until AppKit makes it key
//!   (6-50 ms in the lab traces). That is not a loss.
//! * A real loss can arrive soon after `show()` too (another app is activated 300 ms after the hotkey). Ignoring every
//!   `false` for a fixed time after `show()` and never looking again (the first version of this rule: `false` and
//!   400 ms since show) leaves the window on screen: nothing else asks for a frame, and a visible, unfocused window
//!   that nobody touches is not repainted. That was the "launcher stays after activating Finder" bug (W6-FOCUS,
//!   `docs/spikes/ui-focus.md`).
//!
//! So: a `false` after the window was seen focused since `show()` is a loss at once; a `false` while it has not been
//! focused yet is given [`GRACE`] to turn `true`, and the caller is told to draw a frame when that time is up.

use std::time::Duration;

/// How long a freshly shown window may take to become the key window before "not focused" counts.
pub(super) const GRACE: Duration = Duration::from_millis(400);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FocusStep {
    /// Nothing to do.
    Stay,
    /// The window lost the focus (or never got it): hide.
    Hide,
    /// Not focused yet and still inside the grace period: draw another frame after this long and decide then.
    Recheck(Duration),
}

#[derive(Debug, Default)]
pub(super) struct FocusWatch {
    /// The window reported `focused == Some(true)` at some point since the last `show()`.
    gained: bool,
}

impl FocusWatch {
    /// Call from `show()`.
    pub(super) fn reset(&mut self) {
        self.gained = false;
    }

    /// `focused`: egui's reading in this frame; `since_show`: time since `show()`; `quicklook_open`: a Quick Look
    /// preview is up and legitimately has the focus.
    pub(super) fn step(&mut self, focused: Option<bool>, since_show: Duration, quicklook_open: bool) -> FocusStep {
        if quicklook_open {
            // The focus is elsewhere on purpose. When the preview ends the launcher has to take it back
            // (`tick_quicklook` restarts the clock), and until then it does not hold it.
            self.gained = false;
            return FocusStep::Stay;
        }
        match focused {
            Some(true) => {
                self.gained = true;
                FocusStep::Stay
            }
            Some(false) if self.gained || since_show >= GRACE => FocusStep::Hide,
            Some(false) => FocusStep::Recheck(GRACE - since_show),
            None => FocusStep::Stay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The lab trace of the bug: focus true at +8 ms, another app activated, the next frame reads false at +345 ms.
    #[test]
    fn a_loss_soon_after_show_hides_at_once() {
        let mut w = FocusWatch::default();
        assert_eq!(w.step(Some(false), ms(0), false), FocusStep::Recheck(GRACE));
        assert_eq!(w.step(Some(true), ms(8), false), FocusStep::Stay);
        assert_eq!(w.step(Some(false), ms(345), false), FocusStep::Hide);
    }

    #[test]
    fn a_loss_at_any_time_after_gaining_focus_hides() {
        for t in [1, 100, 399, 400, 401, 5_000] {
            let mut w = FocusWatch::default();
            w.step(Some(true), ms(1), false);
            assert_eq!(w.step(Some(false), ms(t), false), FocusStep::Hide, "t={t}");
        }
    }

    #[test]
    fn the_false_a_hidden_window_still_reports_is_not_a_loss() {
        let mut w = FocusWatch::default();
        // frames right after show(): stale false, then true. No hide, and the caller is asked for a frame at the end of the grace period.
        assert_eq!(w.step(Some(false), ms(0), false), FocusStep::Recheck(GRACE));
        assert_eq!(w.step(Some(false), ms(100), false), FocusStep::Recheck(ms(300)));
        assert_eq!(w.step(Some(true), ms(120), false), FocusStep::Stay);
        assert_eq!(w.step(Some(true), ms(2_000), false), FocusStep::Stay);
    }

    #[test]
    fn a_window_that_never_gets_focus_hides_when_the_grace_is_over() {
        let mut w = FocusWatch::default();
        assert_eq!(w.step(Some(false), ms(399), false), FocusStep::Recheck(ms(1)));
        assert_eq!(w.step(Some(false), ms(400), false), FocusStep::Hide);
    }

    #[test]
    fn unknown_focus_never_hides() {
        let mut w = FocusWatch::default();
        assert_eq!(w.step(None, ms(5_000), false), FocusStep::Stay);
        w.step(Some(true), ms(1), false);
        assert_eq!(w.step(None, ms(5_000), false), FocusStep::Stay);
    }

    #[test]
    fn a_quick_look_preview_keeps_the_launcher_up_before_and_after_the_grace_period() {
        let mut w = FocusWatch::default();
        w.step(Some(true), ms(10), false);
        assert_eq!(w.step(Some(false), ms(200), true), FocusStep::Stay);
        assert_eq!(w.step(Some(false), ms(9_000), true), FocusStep::Stay);
        // The preview ended in this frame; the launcher asked for its focus back and the clock restarted (`shown_at`):
        // the first frames still read false, which is the hand-over (this is what tick_quicklook arranges) ...
        assert_eq!(w.step(Some(false), ms(0), false), FocusStep::Recheck(GRACE));
        assert_eq!(w.step(Some(true), ms(30), false), FocusStep::Stay);
        // ... and a real loss after that hides at once
        assert_eq!(w.step(Some(false), ms(500), false), FocusStep::Hide);
        // if the focus never comes back after the preview, the launcher goes when the grace period is over
        let mut w = FocusWatch::default();
        w.step(Some(true), ms(10), false);
        w.step(Some(false), ms(9_000), true);
        assert_eq!(w.step(Some(false), ms(100), false), FocusStep::Recheck(ms(300)));
        assert_eq!(w.step(Some(false), ms(400), false), FocusStep::Hide);
    }

    #[test]
    fn show_starts_a_new_round() {
        let mut w = FocusWatch::default();
        w.step(Some(true), ms(1), false);
        w.reset();
        // the window is hidden and shown again: the stale false of the hidden window must not count as a loss
        assert_eq!(w.step(Some(false), ms(2), false), FocusStep::Recheck(ms(398)));
    }
}
