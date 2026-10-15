//! Opt-in diagnostics for UI smoke tests: set `ASHDASH_TRACE=1` to print
//! timestamped window/focus events to stderr. Costs one cached env lookup
//! when disabled.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ASHDASH_TRACE").is_some_and(|v| v != "0"))
}

pub fn trace(msg: impl FnOnce() -> String) {
    if enabled() {
        let t = START.get_or_init(Instant::now).elapsed().as_millis();
        eprintln!("[trace {t:>7}ms] {}", msg());
    }
}
