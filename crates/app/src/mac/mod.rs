//! macOS integration that is not part of the launcher window itself.
//!
//! * [`loginitem`] - "launch at login" through `SMAppService.mainApp`.
//! * [`permissions`] - silent probes of Accessibility, Automation and Full Disk Access.
//! * [`opendocs`] - double-clicked workflow packages, appearance files and snippet collections.
//!
//! (The window-level AppKit glue lives in `gui/mac.rs`.)

pub mod loginitem;
pub mod opendocs;
pub mod permissions;
