//! What the window says about the launcher and the settings file: the cells of the bottom
//! status strip, the RELOAD card and the attention cards. Pure functions of the data
//! the window already has (`cfg_error`, the launcher's `reload-status.json`), so they are tested.

use super::kit::{StatusItem, Tone};
use super::status::LauncherStatus;

/// A warning or error with the full sentence (the old banners).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    pub tone: Tone,
    pub title: &'static str,
    pub lines: Vec<String>,
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

/// Cells of the bottom STATUS strip: config, problems, launcher (and restart, when needed).
pub fn strip_items(cfg_error: Option<&str>, launcher: Option<&LauncherStatus>, alive: bool) -> Vec<StatusItem> {
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
    items
}

/// The sentences the old banners showed, one card each.
pub fn attention(cfg_error: Option<&str>, launcher: Option<&LauncherStatus>, alive: bool) -> Vec<Attention> {
    let mut out = Vec::new();
    if let Some(e) = cfg_error {
        out.push(Attention {
            tone: Tone::Error,
            title: "Settings file",
            lines: vec![
                format!("Warning: {e}"),
                "The window shows defaults and will not change the file.".into(),
            ],
        });
    }
    match launcher {
        None => out.push(Attention {
            tone: Tone::Warn,
            title: "Launcher",
            lines: vec!["The launcher has not reported yet: changes apply when it is running.".into()],
        }),
        Some(_) if !alive => out.push(Attention {
            tone: Tone::Warn,
            title: "Launcher",
            lines: vec!["Ashdash's launcher is not running: changes apply the next time it starts.".into()],
        }),
        Some(l) => {
            for p in &l.problems {
                out.push(Attention {
                    tone: Tone::Error,
                    title: "Problem",
                    lines: vec![format!("Warning: {p}")],
                });
            }
            if !l.restart_required.is_empty() {
                out.push(Attention {
                    tone: Tone::Warn,
                    title: "Restart required",
                    lines: vec![format!(
                        "Restart Ashdash for these to take effect: {}.",
                        l.restart_required.join(", ")
                    )],
                });
            }
        }
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
        let items = strip_items(None, Some(&st), true);
        assert_eq!(items.len(), 3);
        assert_eq!(cell(&items, "Config").tone, Tone::Ok);
        assert!(cell(&items, "Config").value.starts_with("applied "));
        assert_eq!(cell(&items, "Problems").value, "0");
        assert_eq!(cell(&items, "Launcher").value, "running");
        assert!(attention(None, Some(&st), true).is_empty());
        let (tone, line) = reload_line(None, Some(&st), true);
        assert_eq!(tone, Tone::Ok);
        assert!(line.ends_with("· 0 problems"), "{line}");
    }

    #[test]
    fn problems_and_restart_requests_are_counted_and_worded_like_the_old_banners() {
        let st = status(&["bad hotkey"], &["app_dirs"]);
        let items = strip_items(None, Some(&st), true);
        assert_eq!(cell(&items, "Problems").value, "1");
        assert_eq!(cell(&items, "Problems").tone, Tone::Error);
        assert_eq!(cell(&items, "Restart").value, "app_dirs");
        let cards = attention(None, Some(&st), true);
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
        let items = strip_items(Some("config.json: bad"), None, false);
        assert_eq!(cell(&items, "Config").tone, Tone::Error);
        assert_eq!(cell(&items, "Problems").value, "1");
        let cards = attention(Some("config.json: bad"), None, false);
        assert_eq!(cards[0].tone, Tone::Error);
        assert_eq!(
            cards[0].lines[1],
            "The window shows defaults and will not change the file."
        );
        assert_eq!(cards.len(), 2, "the missing launcher report is still said");
    }

    #[test]
    fn a_dead_launcher_does_not_report_its_old_problems() {
        let st = status(&["old"], &["x"]);
        let items = strip_items(None, Some(&st), false);
        assert_eq!(cell(&items, "Launcher").tone, Tone::Warn);
        assert_eq!(cell(&items, "Problems").value, "0");
        assert!(items.iter().all(|i| i.label != "Restart"));
        let cards = attention(None, Some(&st), false);
        assert_eq!(cards.len(), 1);
        assert!(cards[0].lines[0].contains("not running"));
    }

    #[test]
    fn no_report_yet_is_neutral_not_green() {
        let items = strip_items(None, None, false);
        assert_eq!(cell(&items, "Launcher").tone, Tone::Off);
        assert_eq!(cell(&items, "Config").tone, Tone::Warn);
    }
}
