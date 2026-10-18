//! The name constants live here; the packaging scripts read them from this file.
//! A rename is more than these constants: the Cargo package names, the scripting dictionary file and the docs carry the
//! name too.

pub const NAME: &str = "Ashdash";
/// Binary / data-directory name.
pub const SLUG: &str = "ashdash";
/// Our own URL scheme.
pub const URL_SCHEME: &str = "ashdash";
pub const BUNDLE_ID: &str = "io.github.oh-my-productivity.ashdash";
/// Default global hotkey. Cmd+Space is Spotlight and Option+Space is a common choice for
/// other launchers, so the default avoids both.
pub const DEFAULT_HOTKEY: &str = "alt+shift+space";
