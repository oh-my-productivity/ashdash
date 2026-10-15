//! One preferences window at a time: an exclusive `flock` on `run/prefs.lock`, held for the life
//! of the window process. A second `ashdash prefs` finds the lock taken, brings the first window
//! forward (its pid is written into the lock file) and exits.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use ashdash_core::Paths;

pub fn lock_file(paths: &Paths) -> PathBuf {
    paths.data.join("run").join("prefs.lock")
}

pub enum Acquired {
    /// We are the window; keep this alive.
    First(File),
    /// Another window process (pid, if known) owns it.
    Taken(Option<u32>),
}

pub fn acquire(path: &Path) -> std::io::Result<Acquired> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: plain flock on a descriptor we own.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r != 0 {
        let mut text = String::new();
        let _ = f.read_to_string(&mut text);
        return Ok(Acquired::Taken(text.trim().parse().ok()));
    }
    f.set_len(0)?;
    f.rewind()?;
    write!(f, "{}", std::process::id())?;
    f.flush()?;
    Ok(Acquired::First(f))
}

/// Bring the process `pid` to the front.
pub fn activate(pid: u32) {
    use objc2::rc::Retained;
    use objc2::{class, msg_send};
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    // SAFETY: a plain class method returning an optional object; the pid is just a number.
    let app: Option<Retained<NSRunningApplication>> =
        unsafe { msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid as libc::pid_t] };
    if let Some(app) = app {
        #[allow(deprecated)]
        let _ = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_taker_learns_who_holds_the_lock() {
        let root = std::env::temp_dir().join(format!("oa-single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("run/prefs.lock");
        let first = acquire(&path).unwrap();
        assert!(matches!(first, Acquired::First(_)));
        match acquire(&path).unwrap() {
            Acquired::Taken(pid) => assert_eq!(pid, Some(std::process::id())),
            Acquired::First(_) => panic!("lock must be exclusive"),
        }
        drop(first);
        assert!(
            matches!(acquire(&path).unwrap(), Acquired::First(_)),
            "released on drop"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
