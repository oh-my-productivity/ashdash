//! AppKit glue (objc2). Everything here must run on the main thread; each
//! function returns quietly when called from another thread.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplication, NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode,
};
use objc2_foundation::NSString;

use super::geometry::{Rect, flip_point_y, flip_y};
use super::style::WindowStyle;

/// `NSStatusWindowLevel`: above normal and floating windows, below screen savers.
const LEVEL_STATUS: isize = 25;

/// `NSNormalWindowLevel`.
const LEVEL_NORMAL: isize = 0;

/// Raise the launcher to the status level (`true`) or drop it to the normal
/// window level (`false`), so a Quick Look preview that has the focus is drawn
/// above it instead of below.
pub fn set_launcher_raised(title: &str, raised: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if let Some(window) = find_window(mtm, title) {
        window.setLevel(if raised { LEVEL_STATUS } else { LEVEL_NORMAL });
    }
}

/// Hide the whole application so macOS returns focus to the previous app.
pub fn hide_app() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
    }
}

/// True while the launcher window has been woken natively (alpha 0) and has not been revealed yet.
static CLOAKED: AtomicBool = AtomicBool::new(false);

/// Make a hidden launcher window count as "visible" to winit/eframe *right now*, without showing anything,
/// then run `then` (the repaint request) on the main thread.
///
/// Why: eframe throttles every repaint request of an invisible window to one per 100 ms
/// (`INVISIBLE_WINDOW_REPAINT_INTERVAL`, eframe 0.36.2 `native/run.rs`), so a hotkey or `ashdash show`
/// that only calls `Context::request_repaint` is seen by the app up to 100 ms late. A window that AppKit
/// reports as visible is repainted at once. So the hotkey / IPC handler orders the window front fully
/// transparent (alpha 0); the normal `show()` then runs on the next frame, and [`reveal`] makes it opaque
/// after the first frame with the fresh content has been drawn (no flash of the previous query).
/// Safe to call from any thread and when the window is already visible or does not exist (then only `then` runs).
pub fn wake_hidden_launcher(title: &'static str, then: impl FnOnce() + Send + 'static) {
    if MainThreadMarker::new().is_some() {
        wake_on_main(title);
        then();
    } else {
        run_on_main_queue(Box::new(move || {
            wake_on_main(title);
            then();
        }));
    }
}

type MainJob = Box<dyn FnOnce() + Send + 'static>;

unsafe extern "C" {
    // libdispatch is part of libSystem (always linked on macOS); no crate needed for these two symbols.
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_async_f(
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

extern "C" fn run_main_job(context: *mut std::ffi::c_void) {
    // SAFETY: `context` is the `Box<MainJob>` leaked by `run_on_main_queue`; libdispatch calls this exactly once.
    let job = unsafe { Box::from_raw(context.cast::<MainJob>()) };
    (*job)();
}

/// Run `job` asynchronously on the main dispatch queue (the AppKit main thread).
fn run_on_main_queue(job: MainJob) {
    let context = Box::into_raw(Box::new(job)).cast::<std::ffi::c_void>();
    // SAFETY: `_dispatch_main_q` is the main queue object (what `dispatch_get_main_queue()` returns); the
    // context is released in `run_main_job`.
    unsafe { dispatch_async_f(std::ptr::addr_of!(_dispatch_main_q), context, run_main_job) };
}

fn wake_on_main(title: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = find_window(mtm, title) else {
        return;
    };
    if window.isVisible() {
        return;
    }
    CLOAKED.store(true, Ordering::SeqCst);
    window.setAlphaValue(0.0);
    let app = NSApplication::sharedApplication(mtm);
    if app.isHidden() {
        app.unhideWithoutActivation();
    }
    window.orderFrontRegardless();
}

/// True between [`wake_hidden_launcher`] and [`reveal`].
pub fn is_cloaked() -> bool {
    CLOAKED.load(Ordering::SeqCst)
}

/// Make the launcher window opaque again (no-op unless it was woken by [`wake_hidden_launcher`]).
pub fn reveal(title: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !CLOAKED.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Some(window) = find_window(mtm, title) {
        window.setAlphaValue(1.0);
    }
}

fn find_window(mtm: MainThreadMarker, title: &str) -> Option<Retained<NSWindow>> {
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|w| w.title().to_string() == title)
}

thread_local! {
    static EFFECT_VIEW: std::cell::RefCell<Option<Retained<NSVisualEffectView>>> =
        const { std::cell::RefCell::new(None) };
}

/// Make the window usable over full-screen apps and on every Space, put it
/// above normal windows, and apply blur / rounded corners / shadow.
/// Idempotent; call it before every show so theme changes are picked up.
pub fn style_launcher_window(title: &str, style: &WindowStyle) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = find_window(mtm, title) else {
        return;
    };
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setLevel(LEVEL_STATUS);
    window.setHidesOnDeactivate(false);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(true);

    let Some(content) = window.contentView() else {
        return;
    };
    EFFECT_VIEW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let view = slot.get_or_insert_with(|| {
            let v = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.bounds());
            v.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            v.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            v.setState(NSVisualEffectState::Active);
            v.setWantsLayer(true);
            content.addSubview_positioned_relativeTo(&v, NSWindowOrderingMode::Below, None);
            v
        });
        match style.vibrancy {
            Some(vib) => {
                view.setHidden(false);
                view.setMaterial(if vib.dark {
                    NSVisualEffectMaterial::HUDWindow
                } else {
                    NSVisualEffectMaterial::Popover
                });
                let name = if vib.dark {
                    "NSAppearanceNameVibrantDark"
                } else {
                    "NSAppearanceNameVibrantLight"
                };
                if let Some(app) = objc2_app_kit::NSAppearance::appearanceNamed(&NSString::from_str(name)) {
                    view.setAppearance(Some(&app));
                }
            }
            None => view.setHidden(true),
        }
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(style.corner_radius);
            // square corners: nothing to clip
            layer.setMasksToBounds(style.clip_to_corners);
        }
    });
    window.invalidateShadow();
}

#[allow(dead_code)]
fn _assert_view(_: &NSView) {}

/// Monitor rectangles (top-left origin, points) and the mouse position, for
/// [`super::geometry::launcher_position`]. The first screen is the primary one.
pub fn screens_and_mouse() -> Option<(Vec<Rect>, (f64, f64))> {
    let mtm = MainThreadMarker::new()?;
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let primary_h = screens.iter().next()?.frame().size.height;
    let rects = screens
        .iter()
        .map(|s| {
            let f = s.frame();
            flip_y(
                primary_h,
                Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
            )
        })
        .collect();
    let m = objc2_app_kit::NSEvent::mouseLocation();
    Some((rects, (m.x, flip_point_y(primary_h, m.y))))
}
