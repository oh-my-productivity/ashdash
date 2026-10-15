//! macOS integration that is not part of the launcher window itself.
//!
//! * [`loginitem`] - "launch at login" through `SMAppService.mainApp`.
//! * [`opendocs`] - double-clicked `.alfredworkflow` / `.alfredappearance` / `.alfredsnippets` files.
//!
//! (The window-level AppKit glue lives in `gui/mac.rs`.)

pub mod loginitem;
pub mod opendocs;
