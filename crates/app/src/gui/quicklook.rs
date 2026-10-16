//! Quick Look for file results: `qlmanage -p <path>` spawned on Shift (tap)
//! or Cmd+Y and killed when the launcher moves on.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ashdash_core::actions::Process;
use ashdash_core::{Action, Item};
use eframe::egui;

use super::LauncherApp;

/// The file a Quick Look request on `item` would preview: the path of a
/// file-ish action that exists. `.app` bundles are skipped (the preview is
/// just their icon).
pub fn target(item: &Item) -> Option<PathBuf> {
    let p = match item.cmd_action.as_ref().unwrap_or(&item.action) {
        Action::Open(p) | Action::Reveal(p) => p,
        _ => match &item.action {
            Action::Open(p) | Action::Reveal(p) => p,
            _ => return None,
        },
    };
    previewable(p).then(|| p.clone())
}

fn previewable(p: &Path) -> bool {
    p.is_absolute() && p.extension().is_none_or(|e| !e.eq_ignore_ascii_case("app")) && p.exists()
}

/// Detects a lone tap of the Shift key: pressed with no other modifier and
/// released within `MAX_TAP` without any other input in between.
#[derive(Debug, Default)]
pub struct ShiftTap {
    down_at: Option<Duration>,
    dirty: bool,
}

impl ShiftTap {
    pub const MAX_TAP: Duration = Duration::from_millis(350);

    /// Feed one frame: whether shift is held, whether any other modifier is
    /// held, whether any key/text/pointer-click input happened this frame, and a
    /// monotonic timestamp. Returns true on the frame the tap completes.
    pub fn update(&mut self, shift: bool, other_mods: bool, other_input: bool, now: Duration) -> bool {
        if other_mods || other_input {
            self.dirty = true;
        }
        match (shift, self.down_at) {
            (true, None) => {
                self.down_at = Some(now);
                self.dirty = other_mods || other_input;
                false
            }
            (false, Some(t0)) => {
                self.down_at = None;
                let tap = !self.dirty && now.saturating_sub(t0) <= Self::MAX_TAP;
                self.dirty = false;
                tap
            }
            _ => false,
        }
    }
}

/// The running preview, if any. The process comes from the engine's `Os`
/// (`Engine::spawn_quick_look`), so dismissing it is `Process::kill`.
#[derive(Default)]
pub struct QuickLook {
    proc: Option<Box<dyn Process>>,
}

impl QuickLook {
    pub fn is_open(&mut self) -> bool {
        if self.proc.as_ref().is_some_and(|p| !p.is_running()) {
            self.proc = None;
        }
        self.proc.is_some()
    }

    /// Replace the current preview (if any) with whatever `spawn` starts.
    pub fn open_with(&mut self, spawn: impl FnOnce() -> Option<Box<dyn Process>>) {
        self.close();
        self.proc = spawn();
    }

    /// Kill the preview. Safe to call when nothing is open.
    pub fn close(&mut self) {
        if let Some(p) = self.proc.take() {
            p.kill();
        }
    }
}

impl Drop for QuickLook {
    fn drop(&mut self) {
        self.close();
    }
}

impl LauncherApp {
    /// Per-frame housekeeping: follow the selection, notice when the preview
    /// process ended and take the focus back.
    pub(super) fn tick_quicklook(&mut self, ctx: &egui::Context) {
        let open = self.quicklook.is_open();
        if open {
            let current = self.items.get(self.selected).map(|i| i.id.as_str());
            if current != self.quicklook_for.as_deref() {
                self.quicklook.close();
            } else {
                ctx.request_repaint_after(Duration::from_millis(150));
            }
        }
        let open = self.quicklook.is_open();
        if self.quicklook_was_open && !open {
            // Preview closed (Esc in its window, or killed): pick the launcher back up.
            self.quicklook_for = None;
            self.shown_at = std::time::Instant::now();
            self.want_focus = true;
            self.raise_launcher(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.quicklook_was_open = open;
    }

    /// Cmd+Y / Shift tap: toggle a preview of the selected file result.
    pub(super) fn toggle_quicklook(&mut self) {
        let Some(item) = self.items.get(self.selected) else {
            return;
        };
        let Some(path) = target(item) else {
            return;
        };
        if self.quicklook.is_open() {
            self.quicklook.close();
        } else {
            let id = item.id.clone();
            self.open_quicklook(&[path], Some(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn clean_shift_tap_triggers() {
        let mut t = ShiftTap::default();
        assert!(!t.update(false, false, false, ms(0)));
        assert!(!t.update(true, false, false, ms(100)));
        assert!(t.update(false, false, false, ms(200)));
        // and can trigger again
        assert!(!t.update(true, false, false, ms(1000)));
        assert!(t.update(false, false, false, ms(1100)));
    }

    #[test]
    fn long_hold_is_not_a_tap() {
        let mut t = ShiftTap::default();
        t.update(true, false, false, ms(0));
        assert!(!t.update(false, false, false, ms(900)));
    }

    #[test]
    fn shift_with_another_key_is_not_a_tap() {
        let mut t = ShiftTap::default();
        t.update(true, false, false, ms(0));
        t.update(true, false, true, ms(50)); // shift+letter
        assert!(!t.update(false, false, false, ms(100)));
        // state recovered
        t.update(true, false, false, ms(500));
        assert!(t.update(false, false, false, ms(550)));
    }

    #[test]
    fn shift_with_another_modifier_is_not_a_tap() {
        let mut t = ShiftTap::default();
        t.update(true, true, false, ms(0));
        assert!(!t.update(false, false, false, ms(80)));
        let mut t = ShiftTap::default();
        t.update(true, false, false, ms(0));
        t.update(true, true, false, ms(30));
        assert!(!t.update(false, false, false, ms(80)));
    }

    #[test]
    fn target_uses_existing_non_app_files_only() {
        let dir = std::env::temp_dir().join(format!("ashdash-ql-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "x").unwrap();
        let app = dir.join("Thing.app");
        std::fs::create_dir_all(&app).unwrap();

        let open = Item::new("f", "note.txt").action(Action::Open(file.clone()));
        assert_eq!(target(&open), Some(file.clone()));
        let mut with_cmd = Item::new("f", "note.txt").action(Action::Open(file.clone()));
        with_cmd.cmd_action = Some(Action::Reveal(file.clone()));
        assert_eq!(target(&with_cmd), Some(file.clone()));
        assert_eq!(target(&Item::new("a", "Thing").action(Action::Open(app))), None);
        assert_eq!(
            target(&Item::new("m", "gone").action(Action::Open(dir.join("missing.txt")))),
            None
        );
        assert_eq!(
            target(&Item::new("u", "u").action(Action::OpenUrl("https://x.y".into()))),
            None
        );
        assert_eq!(
            target(&Item::new("r", "rel").action(Action::Open(PathBuf::from("rel.txt")))),
            None
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_nothing_is_fine() {
        let mut q = QuickLook::default();
        assert!(!q.is_open());
        q.close();
    }

    #[derive(Clone, Default)]
    struct Fake {
        running: Arc<AtomicBool>,
        kills: Arc<AtomicUsize>,
    }

    impl Process for Fake {
        fn kill(&self) {
            self.kills.fetch_add(1, Ordering::SeqCst);
            self.running.store(false, Ordering::SeqCst);
        }
        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }
    }

    fn started(f: &Fake) -> Option<Box<dyn Process>> {
        f.running.store(true, Ordering::SeqCst);
        Some(Box::new(f.clone()))
    }

    #[test]
    fn dismissing_kills_the_process_handle() {
        let f = Fake::default();
        let mut q = QuickLook::default();
        q.open_with(|| started(&f));
        assert!(q.is_open());
        q.close();
        assert_eq!(f.kills.load(Ordering::SeqCst), 1);
        assert!(!q.is_open());
        q.close();
        assert_eq!(f.kills.load(Ordering::SeqCst), 1, "a second close kills nothing");
    }

    #[test]
    fn a_preview_that_ended_by_itself_is_noticed_and_not_killed_again() {
        let f = Fake::default();
        let mut q = QuickLook::default();
        q.open_with(|| started(&f));
        f.running.store(false, Ordering::SeqCst); // the user pressed Esc in the preview
        assert!(!q.is_open());
        drop(q);
        assert_eq!(f.kills.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn opening_another_preview_ends_the_first_and_drop_ends_the_last() {
        let (a, b) = (Fake::default(), Fake::default());
        let mut q = QuickLook::default();
        q.open_with(|| started(&a));
        q.open_with(|| started(&b));
        assert_eq!(a.kills.load(Ordering::SeqCst), 1);
        assert!(q.is_open());
        drop(q);
        assert_eq!(b.kills.load(Ordering::SeqCst), 1, "no preview outlives the launcher");
    }

    #[test]
    fn a_failed_spawn_leaves_nothing_open() {
        let mut q = QuickLook::default();
        q.open_with(|| None);
        assert!(!q.is_open());
    }
}
