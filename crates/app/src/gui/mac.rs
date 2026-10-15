//! AppKit glue (objc2). Everything here must run on the main thread; each
//! function returns quietly when called from another thread.

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

/// Hide the whole application so macOS returns focus to the previous app.
pub fn hide_app() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
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
            layer.setMasksToBounds(true);
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
