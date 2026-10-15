//! Every user-visible name lives here so a rename touches one file.
//! "Ashdash" is a working name; see docs/PLAN.md (WP12) before publishing.

pub const NAME: &str = "Ashdash";
/// Binary / data-directory name.
pub const SLUG: &str = "ashdash";
/// Our own URL scheme. Deliberately not `reference://`, so a real reference launcher install keeps working.
pub const URL_SCHEME: &str = "ashdash";
pub const BUNDLE_ID: &str = "dev.ashdash.launcher";
/// Default global hotkey. Cmd+Space is Spotlight; Option+Space is used by the reference launcher's
/// common setups too, so a second default avoids a clash when both run.
pub const DEFAULT_HOTKEY: &str = "alt+shift+space";
