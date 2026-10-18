//! The name constants live here; the packaging scripts read them from this file.
//! The name was chosen on 2026-10-18 (owner decision); the project was developed under the working name "Ashdash".
//! A rename is more than these constants (Cargo package names, the scripting dictionary file, docs): see "Where the name
//! lives" in docs/CLEAN_ROOM.md.

pub const NAME: &str = "Ashdash";
/// Binary / data-directory name.
pub const SLUG: &str = "ashdash";
/// Our own URL scheme. Deliberately not `reference://`, so a real reference launcher install keeps working.
pub const URL_SCHEME: &str = "ashdash";
pub const BUNDLE_ID: &str = "io.github.oh-my-productivity.ashdash";
/// Default global hotkey. Cmd+Space is Spotlight; Option+Space is used by the reference launcher's
/// common setups too, so a second default avoids a clash when both run.
pub const DEFAULT_HOTKEY: &str = "alt+shift+space";
