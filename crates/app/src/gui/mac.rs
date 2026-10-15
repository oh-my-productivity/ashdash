//! Thin objc2 helpers.

/// Hide the whole application so macOS returns focus to the previous app.
pub fn hide_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
    }
}
