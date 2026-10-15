//! What the running launcher tells the preferences window: whether the last settings reload
//! worked, which hotkey is really active, and what needs a restart. A small JSON file in the
//! data folder (`run/reload-status.json`), written by the launcher after every reload and
//! polled by the preferences window, which is a separate process.

use std::path::PathBuf;

use ashdash_core::Paths;
use ashdash_core::config::write_private;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherStatus {
    /// Process id of the launcher that wrote this.
    pub pid: u32,
    /// Seconds since the epoch.
    pub updated: u64,
    /// Things the user should know about; empty = everything applied.
    pub problems: Vec<String>,
    /// Settings that apply after a restart.
    pub restart_required: Vec<String>,
    /// The launcher hotkey actually registered (empty = none).
    pub hotkey_active: String,
}

impl LauncherStatus {
    fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "updated": self.updated,
            "problems": self.problems,
            "restart_required": self.restart_required,
            "hotkey_active": self.hotkey_active,
        })
    }

    fn from_json(v: &Value) -> Option<LauncherStatus> {
        let strings = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        Some(LauncherStatus {
            pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
            updated: v.get("updated").and_then(Value::as_u64).unwrap_or(0),
            problems: strings("problems"),
            restart_required: strings("restart_required"),
            hotkey_active: v.get("hotkey_active").and_then(Value::as_str).unwrap_or("").to_string(),
        })
    }
}

pub fn status_file(paths: &Paths) -> PathBuf {
    paths.data.join("run").join("reload-status.json")
}

pub fn write(paths: &Paths, status: &LauncherStatus) -> std::io::Result<()> {
    write_private(&status_file(paths), &serde_json::to_vec_pretty(&status.to_json())?)
}

pub fn read(paths: &Paths) -> Option<LauncherStatus> {
    LauncherStatus::from_json(&serde_json::from_slice(&std::fs::read(status_file(paths)).ok()?).ok()?)
}

/// Is a process with this id alive (same user)? `kill(pid, 0)` probes without signalling.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_a_missing_or_garbled_file_reads_as_none() {
        let root = std::env::temp_dir().join(format!("oa-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::under(&root);
        assert_eq!(read(&paths), None);
        let st = LauncherStatus {
            pid: 42,
            updated: 7,
            problems: vec!["x".into()],
            restart_required: vec!["app_dirs".into()],
            hotkey_active: "alt+shift+Space".into(),
        };
        write(&paths, &st).unwrap();
        assert_eq!(read(&paths), Some(st));
        std::fs::write(status_file(&paths), b"{ nope").unwrap();
        assert_eq!(read(&paths), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn liveness_probe() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
        assert!(!process_alive(u32::MAX - 1));
    }
}
