//! What the window says about the launcher and the settings file: the cells of the bottom
//! status strip, the RELOAD card and the attention cards. Pure functions of the data
//! the window already has (`cfg_error`, the launcher's `reload-status.json`), so they are tested.

use ashdash_core::Config;

use super::kit::{StatusItem, Tone};
use super::nav::Nav;
use super::status::LauncherStatus;
use crate::mac::permissions::{Grant, Snapshot};
use crate::prefs_model::needs_full_disk_access;

/// A warning or error with the full sentence (the old banners).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    pub tone: Tone,
    /// Small-caps heading; `None` for a one-sentence card.
    pub title: Option<&'static str>,
    pub lines: Vec<String>,
    /// A page that fixes it: the card gets a "Fix…" button that opens it.
    pub fix: Option<Nav>,
}

/// Safari bookmarks are on and macOS has refused Full Disk Access: `bm` returns nothing for
/// Safari. (Unknown / not yet probed is not a reason to alarm anybody.)
pub fn disk_access_missing(cfg: &Config, perms: Option<&Snapshot>) -> bool {
    needs_full_disk_access(cfg) && perms.is_some_and(|p| p.full_disk_access == Grant::NotGranted)
}

/// `HH:MM:SS` in local time for seconds since the epoch.
pub fn local_hms(secs: u64) -> String {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed tm we pass and reads the time_t by reference.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "--:--:--".into();
        }
        tm
    };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn problem_count(cfg_error: Option<&str>, launcher: Option<&LauncherStatus>, alive: bool) -> usize {
    usize::from(cfg_error.is_some()) + launcher.filter(|_| alive).map_or(0, |l| l.problems.len())
}

/// The line on the RELOAD card and its tone.
pub fn reload_line(cfg_error: Option<&str>, launcher: Option<&LauncherStatus>, alive: bool) -> (Tone, String) {
    let n = problem_count(cfg_error, launcher, alive);
    let problems = match n {
        1 => "1 problem".to_string(),
        n => format!("{n} problems"),
    };
    if cfg_error.is_some() {
        return (Tone::Error, format!("config.json cannot be read · {problems}"));
    }
    match launcher {
        None => (Tone::Warn, "the launcher has not reported yet".into()),
        Some(_) if !alive => (
            Tone::Warn,
            "the launcher is not running · applies when it starts".into(),
        ),
        Some(l) => (
            if n == 0 { Tone::Ok } else { Tone::Warn },
            format!("config.json applied {} · {problems}", local_hms(l.updated)),
        ),
    }
}

/// Cells of the bottom STATUS strip: config, problems, launcher (and restart / disk access,
/// when they need attention).
pub fn strip_items(
    cfg_error: Option<&str>,
    launcher: Option<&LauncherStatus>,
    alive: bool,
    disk_missing: bool,
) -> Vec<StatusItem> {
    let n = problem_count(cfg_error, launcher, alive);
    let config = match (cfg_error, launcher) {
        (Some(_), _) => ("unreadable".to_string(), Tone::Error),
        (None, None) => ("no report yet".to_string(), Tone::Warn),
        (None, Some(_)) if !alive => ("not applied".to_string(), Tone::Warn),
        (None, Some(l)) => (
            format!("applied {}", local_hms(l.updated)),
            if n == 0 { Tone::Ok } else { Tone::Warn },
        ),
    };
    let launcher_cell = match launcher {
        None => ("no report yet".to_string(), Tone::Off),
        Some(_) if alive => ("running".to_string(), Tone::Ok),
        Some(_) => ("not running".to_string(), Tone::Warn),
    };
    let mut items = vec![
        StatusItem {
            label: "Config",
            value: config.0,
            tone: config.1,
        },
        StatusItem {
            label: "Problems",
            value: n.to_string(),
            tone: if n == 0 { Tone::Ok } else { Tone::Error },
        },
        StatusItem {
            label: "Launcher",
            value: launcher_cell.0,
            tone: launcher_cell.1,
        },
    ];
    if let Some(l) = launcher.filter(|l| alive && !l.restart_required.is_empty()) {
        items.push(StatusItem {
            label: "Restart",
            value: l.restart_required.join(", "),
            tone: Tone::Warn,
        });
    }
    if disk_missing {
        items.push(StatusItem {
            label: "Disk access",
            value: "not granted".into(),
            tone: Tone::Warn,
        });
    }
    items
}

/// The cards under the rows of the General page: a damaged settings file, problems the launcher
/// reported, settings that need a restart, and missing Full Disk Access. (That the launcher is
/// not running or has not reported is already said by the RELOAD card and the status strip.)
pub fn attention(
    cfg_error: Option<&str>,
    launcher: Option<&LauncherStatus>,
    alive: bool,
    disk_missing: bool,
) -> Vec<Attention> {
    let mut out = Vec::new();
    if let Some(e) = cfg_error {
        out.push(Attention {
            tone: Tone::Error,
            title: Some("Settings file"),
            lines: vec![
                format!("Warning: {e}"),
                "The window shows defaults and will not change the file.".into(),
            ],
            fix: None,
        });
    }
    if let Some(l) = launcher.filter(|_| alive) {
        for p in &l.problems {
            out.push(Attention {
                tone: Tone::Error,
                title: Some("Problem"),
                lines: vec![format!("Warning: {p}")],
                fix: None,
            });
        }
        if !l.restart_required.is_empty() {
            out.push(Attention {
                tone: Tone::Warn,
                title: Some("Restart required"),
                lines: vec![format!(
                    "Restart Ashdash for these to take effect: {}.",
                    l.restart_required.join(", ")
                )],
                fix: None,
            });
        }
    }
    if disk_missing {
        out.push(Attention {
            tone: Tone::Warn,
            title: None,
            lines: vec!["Full Disk Access not granted · Safari bookmarks (bm) return nothing".into()],
            fix: Some(Nav::Permissions),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(problems: &[&str], restart: &[&str]) -> LauncherStatus {
        LauncherStatus {
            pid: std::process::id(),
            updated: 1_700_000_000,
            problems: problems.iter().map(|s| s.to_string()).collect(),
            restart_required: restart.iter().map(|s| s.to_string()).collect(),
            hotkey_active: String::new(),
        }
    }

    fn cell<'a>(items: &'a [StatusItem], label: &str) -> &'a StatusItem {
        items.iter().find(|i| i.label == label).expect(label)
    }

    #[test]
    fn local_time_has_the_clock_shape() {
        let s = local_hms(1_700_000_000);
        let parts: Vec<_> = s.split(':').collect();
        assert_eq!(parts.len(), 3, "{s}");
        assert!(
            parts
                .iter()
                .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())),
            "{s}"
        );
    }

    #[test]
    fn a_healthy_launcher_shows_green_cells_and_no_cards() {
        let st = status(&[], &[]);
        let items = strip_items(None, Some(&st), true, false);
        assert_eq!(items.len(), 3);
        assert_eq!(cell(&items, "Config").tone, Tone::Ok);
        assert!(cell(&items, "Config").value.starts_with("applied "));
        assert_eq!(cell(&items, "Problems").value, "0");
        assert_eq!(cell(&items, "Launcher").value, "running");
        assert!(attention(None, Some(&st), true, false).is_empty());
        let (tone, line) = reload_line(None, Some(&st), true);
        assert_eq!(tone, Tone::Ok);
        assert!(line.ends_with("· 0 problems"), "{line}");
    }

    #[test]
    fn problems_and_restart_requests_are_counted_and_worded_like_the_old_banners() {
        let st = status(&["bad hotkey"], &["app_dirs"]);
        let items = strip_items(None, Some(&st), true, false);
        assert_eq!(cell(&items, "Problems").value, "1");
        assert_eq!(cell(&items, "Problems").tone, Tone::Error);
        assert_eq!(cell(&items, "Restart").value, "app_dirs");
        let cards = attention(None, Some(&st), true, false);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].lines[0], "Warning: bad hotkey");
        assert_eq!(cards[1].lines[0], "Restart Ashdash for these to take effect: app_dirs.");
        assert_eq!(
            reload_line(None, Some(&st), true).1.split(" · ").last(),
            Some("1 problem")
        );
    }

    #[test]
    fn an_unreadable_config_is_an_error_everywhere() {
        let items = strip_items(Some("config.json: bad"), None, false, false);
        assert_eq!(cell(&items, "Config").tone, Tone::Error);
        assert_eq!(cell(&items, "Problems").value, "1");
        let cards = attention(Some("config.json: bad"), None, false, false);
        assert_eq!(cards[0].tone, Tone::Error);
        assert_eq!(
            cards[0].lines[1],
            "The window shows defaults and will not change the file."
        );
        assert_eq!(cards.len(), 1, "the missing launcher report is said by the RELOAD card");
    }

    #[test]
    fn a_dead_launcher_does_not_report_its_old_problems() {
        let st = status(&["old"], &["x"]);
        let items = strip_items(None, Some(&st), false, false);
        assert_eq!(cell(&items, "Launcher").tone, Tone::Warn);
        assert_eq!(cell(&items, "Problems").value, "0");
        assert!(items.iter().all(|i| i.label != "Restart"));
        assert!(attention(None, Some(&st), false, false).is_empty());
        let (tone, line) = reload_line(None, Some(&st), false);
        assert_eq!(tone, Tone::Warn);
        assert!(line.contains("not running"), "{line}");
    }

    #[test]
    fn no_report_yet_is_neutral_not_green() {
        let items = strip_items(None, None, false, false);
        assert_eq!(cell(&items, "Launcher").tone, Tone::Off);
        assert_eq!(cell(&items, "Config").tone, Tone::Warn);
    }

    fn snapshot(fda: Grant) -> Snapshot {
        Snapshot {
            accessibility: Grant::Granted,
            automation: vec![Grant::Granted; 3],
            full_disk_access: fda,
        }
    }

    #[test]
    fn missing_disk_access_needs_safari_bookmarks_on_and_a_refusal() {
        let mut c = Config::default();
        assert!(disk_access_missing(&c, Some(&snapshot(Grant::NotGranted))));
        for other in [Grant::Granted, Grant::Unknown, Grant::AskedOnFirstUse] {
            assert!(!disk_access_missing(&c, Some(&snapshot(other))), "{other:?}");
        }
        assert!(!disk_access_missing(&c, None), "not probed yet");
        c.bookmarks.enabled_sources.retain(|s| s != "safari");
        assert!(!disk_access_missing(&c, Some(&snapshot(Grant::NotGranted))));
    }

    #[test]
    fn the_disk_access_card_and_light_say_what_breaks_and_point_at_permissions() {
        let st = status(&[], &[]);
        let items = strip_items(None, Some(&st), true, true);
        let disk = cell(&items, "Disk access");
        assert_eq!(disk.tone, Tone::Warn);
        assert_eq!(disk.value, "not granted");
        let cards = attention(None, Some(&st), true, true);
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].lines[0],
            "Full Disk Access not granted · Safari bookmarks (bm) return nothing"
        );
        assert_eq!(cards[0].fix, Some(Nav::Permissions));
        assert!(
            strip_items(None, Some(&st), true, false)
                .iter()
                .all(|i| i.label != "Disk access")
        );
    }
}
