//! The confirmation shown before a workflow package is installed.
//!
//! A workflow is a bundle of scripts that will run with the user's permissions
//! (docs/SECURITY.md, SEC-03/13/14). The engine cannot make an untrusted one safe, so
//! before installing, the user sees who claims to have written it, its bundle id and
//! exactly what its object graph can do ([`TrustSummary`]), and must confirm.
//!
//! [`prompt_text`] builds the wording from a [`PackagePreview`]; it is pure and shared
//! by the launcher's in-window prompt, the Finder "open document" dialog
//! (`mac/opendocs.rs`) and `ashdash import`. All text that comes from the package is
//! stripped of control characters and length-capped here, so a hostile package cannot
//! reshape the prompt.

use std::path::PathBuf;
use std::time::Duration;

use ashdash_core::workflow::{PackagePreview, TrustSummary};
use eframe::egui::{self, Vec2};

use super::largetype::wrapped_lines;
use super::{LauncherApp, ROW_HEIGHT, c};

/// Return does not confirm until the prompt has been up this long: a Return
/// that was still being held when the prompt appeared must not install anything.
pub const CONFIRM_DELAY: Duration = Duration::from_millis(600);

const MAX_NAME: usize = 80;
const MAX_FIELD: usize = 160;
const MAX_COMMAND: usize = 120;
const MAX_LISTED: usize = 5;

/// Words for the prompt: a title and body lines (an empty line is a paragraph break).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptText {
    pub title: String,
    pub lines: Vec<String>,
}

impl PromptText {
    /// One string for places without layout (dialogs, terminals).
    pub fn plain(&self) -> String {
        let mut s = self.title.clone();
        s.push_str("\n\n");
        s.push_str(&self.lines.join("\n"));
        s
    }
}

/// Remove control characters, collapse whitespace runs and cap the length.
pub fn clean(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let mut out = String::new();
    let mut last_space = true;
    for ch in flat.chars() {
        if ch == ' ' {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    let out = out.trim_end().to_string();
    if out.chars().count() > max {
        let head: String = out.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        out
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `a, b, c (+2 more)`.
fn listed(items: &[String], max_each: usize) -> String {
    let shown: Vec<String> = items.iter().take(MAX_LISTED).map(|i| clean(i, max_each)).collect();
    let mut s = shown.join(", ");
    if items.len() > MAX_LISTED {
        s.push_str(&format!(" (+{} more)", items.len() - MAX_LISTED));
    }
    s
}

fn capability_lines(t: &TrustSummary) -> Vec<String> {
    let mut out = Vec::new();
    if t.runs_code() {
        let mut l = format!("Runs code: {}", plural(t.scripts, "script", "scripts"));
        if !t.interpreters.is_empty() {
            l.push_str(&format!(" ({})", listed(&t.interpreters, 40)));
        }
        out.push(l);
        if t.query_in_source > 0 {
            out.push(format!(
                "{} the typed text straight into the script source: unless the script quotes it, \
                 what you type becomes code.",
                if t.query_in_source == 1 {
                    "1 script pastes".to_string()
                } else {
                    format!("{} scripts paste", t.query_in_source)
                }
            ));
        }
    } else {
        out.push("Runs no scripts and no terminal commands.".into());
    }
    if !t.terminal_commands.is_empty() {
        out.push(format!(
            "Runs in Terminal: {}",
            listed(
                &t.terminal_commands.iter().map(|c| format!("“{c}”")).collect::<Vec<_>>(),
                MAX_COMMAND
            )
        ));
    }
    if !t.open_url_schemes.is_empty() {
        out.push(format!(
            "Opens URLs with the scheme: {} ({{dynamic}} = decided by data at run time)",
            listed(&t.open_url_schemes, 40)
        ));
    }
    if !t.external_files.is_empty() {
        out.push(format!(
            "Refers to files outside its folder: {}",
            listed(&t.external_files, MAX_COMMAND)
        ));
    }
    if t.external_triggers > 0 {
        out.push(format!(
            "Has {} that other workflows or ashdash:// links can start.",
            plural(t.external_triggers, "external trigger", "external triggers")
        ));
    }
    out
}

/// The wording of the confirmation for `p`.
pub fn prompt_text(p: &PackagePreview) -> PromptText {
    let name = clean(&p.name, MAX_NAME);
    let name = if name.is_empty() { "(unnamed)".to_string() } else { name };
    let author = clean(&p.author, MAX_FIELD);
    let bundle = clean(&p.bundle_id, MAX_FIELD);
    let mut lines = vec![
        format!("Name: {name}"),
        if author.is_empty() {
            "Author: not stated (anyone could have written this)".to_string()
        } else {
            format!("Author: {author} (as written in the package; not verified)")
        },
        format!("Bundle id: {}", if bundle.is_empty() { "none" } else { &bundle }),
    ];
    let version = clean(&p.version, 40);
    if !version.is_empty() {
        lines.push(format!("Version: {version}"));
    }
    let description = clean(&p.description, MAX_FIELD);
    if !description.is_empty() {
        lines.push(format!("Description: {description}"));
    }
    lines.push(String::new());
    lines.extend(capability_lines(&p.trust));
    lines.push(String::new());
    lines.push("Workflows run on your Mac with your permissions. Only install ones you trust.".into());
    PromptText {
        title: format!("Install workflow “{name}”?"),
        lines,
    }
}

/// Is `answer` a yes? Only "y" / "yes" (any case) count; anything else, including an
/// empty line, is a no.
pub fn answer_is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Ask on the terminal. Without a terminal there is nobody to ask: that is an error
/// telling the caller to pass `--yes`, never an implicit yes.
pub fn ask_yes_no(question: &str) -> anyhow::Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        anyhow::bail!("refusing to install without confirmation: no terminal to ask on (use --yes to install anyway)");
    }
    print!("{question}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(answer_is_yes(&line))
}

/// Whether Return may confirm yet.
pub fn confirm_ready(shown_for: Duration) -> bool {
    shown_for >= CONFIRM_DELAY
}

/// Height in points the prompt wants at `width`, capped at the room under the search field.
pub fn prompt_height(text: &PromptText, width: f32, font_size: f32) -> f32 {
    let cols = ((width - 24.0) / (font_size * 0.5)).floor().max(10.0) as usize;
    let rows: usize = 1 + text.lines.iter().map(|l| wrapped_lines(l, cols)).sum::<usize>() + 1;
    (rows as f32 * font_size * 1.2 + 16.0).min(super::VISIBLE_ROWS as f32 * ROW_HEIGHT)
}

/// A prompt that is up, waiting for Return or Esc.
pub struct ImportPrompt {
    pub path: PathBuf,
    pub text: PromptText,
    pub shown_at: std::time::Instant,
}

impl ImportPrompt {
    pub fn new(path: PathBuf, preview: &PackagePreview) -> Self {
        ImportPrompt {
            path,
            text: prompt_text(preview),
            shown_at: std::time::Instant::now(),
        }
    }
}

impl LauncherApp {
    pub(super) fn import_prompt_height(&self) -> Option<f32> {
        let p = self.glue.import.as_ref()?;
        Some(prompt_height(
            &p.text,
            self.theme.window_width - self.theme.window_padding * 2.0,
            self.theme.result_subtext_size + 1.0,
        ))
    }

    pub(super) fn draw_import_prompt(&self, ui: &mut egui::Ui, t: &ashdash_core::theme::Theme) {
        let Some(p) = &self.glue.import else {
            return;
        };
        let max_h = super::VISIBLE_ROWS as f32 * ROW_HEIGHT;
        let size = t.result_subtext_size + 1.0;
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&p.text.title)
                            .strong()
                            .size(size + 2.0)
                            .color(c(t.result_text)),
                    )
                    .wrap(),
                );
                for line in &p.text.lines {
                    if line.is_empty() {
                        ui.add_space(size * 0.6);
                        continue;
                    }
                    ui.add(egui::Label::new(egui::RichText::new(line).size(size).color(c(t.result_text))).wrap());
                }
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("↩ Install      Esc Cancel")
                            .strong()
                            .size(size)
                            .color(c(t.result_subtext_selected)),
                    )
                    .wrap(),
                );
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> PackagePreview {
        PackagePreview {
            name: "Weather".into(),
            bundle_id: "com.example.weather".into(),
            author: "Ann Author".into(),
            description: "Shows the weather".into(),
            version: "1.2".into(),
            trust: TrustSummary {
                name: "Weather".into(),
                bundle_id: "com.example.weather".into(),
                scripts: 2,
                interpreters: vec!["bash".into(), "python3".into()],
                query_in_source: 1,
                open_url_schemes: vec!["https".into()],
                terminal_commands: vec!["brew update".into()],
                external_files: vec!["~/Documents/x.txt".into()],
                external_triggers: 1,
            },
        }
    }

    #[test]
    fn the_prompt_names_the_package_and_everything_it_can_do() {
        let t = prompt_text(&preview());
        assert_eq!(t.title, "Install workflow “Weather”?");
        let all = t.plain();
        for needle in [
            "Name: Weather",
            "Author: Ann Author (as written in the package; not verified)",
            "Bundle id: com.example.weather",
            "Version: 1.2",
            "Description: Shows the weather",
            "Runs code: 2 scripts (bash, python3)",
            "1 script pastes the typed text",
            "Runs in Terminal: “brew update”",
            "scheme: https",
            "outside its folder: ~/Documents/x.txt",
            "1 external trigger",
            "Only install ones you trust",
        ] {
            assert!(all.contains(needle), "missing {needle:?} in:\n{all}");
        }
    }

    #[test]
    fn a_passive_workflow_says_so_and_a_nameless_one_is_flagged() {
        let p = PackagePreview {
            name: String::new(),
            bundle_id: String::new(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            trust: TrustSummary::default(),
        };
        let t = prompt_text(&p);
        let all = t.plain();
        assert!(t.title.contains("(unnamed)"));
        assert!(all.contains("Runs no scripts and no terminal commands."), "{all}");
        assert!(all.contains("Author: not stated"));
        assert!(all.contains("Bundle id: none"));
        assert!(!all.contains("Version:") && !all.contains("Description:"));
        assert!(!all.contains("Runs code"));
        assert!(!all.contains("Opens URLs"));
    }

    #[test]
    fn hostile_text_cannot_reshape_the_prompt() {
        let mut p = preview();
        p.name = "Evil\n\nAuthor: Apple Inc. (verified)\u{7}".into();
        p.author = format!("x\r\nBundle id: com.apple.safari{}", "y".repeat(500));
        p.trust.terminal_commands = vec!["rm -rf ~\n# looks fine".into(); 20];
        let t = prompt_text(&p);
        // no line of the output comes from a newline inside a package field
        assert!(t.title.chars().all(|c| !c.is_control()));
        assert_eq!(t.title.matches('\n').count(), 0);
        for line in &t.lines {
            assert!(
                !line.contains('\n') && !line.contains('\r') && !line.contains('\u{7}'),
                "{line:?}"
            );
            assert!(line.chars().count() < 800, "{}", line.chars().count());
        }
        // "Author:" / "Bundle id:" only ever start the lines we wrote
        assert_eq!(t.lines.iter().filter(|l| l.starts_with("Author:")).count(), 1);
        assert_eq!(t.lines.iter().filter(|l| l.starts_with("Bundle id:")).count(), 1);
        assert!(t.plain().contains("(+15 more)"), "long lists are cut");
    }

    #[test]
    fn clean_collapses_caps_and_strips() {
        assert_eq!(clean("  a \t b\n\nc  ", 80), "a b c");
        assert_eq!(clean("a\u{0}b\u{1b}[31m", 80), "ab[31m");
        assert_eq!(clean("abcdef", 4), "abc…");
        assert_eq!(clean("", 4), "");
        assert_eq!(clean("é世界😀", 3), "é世…");
    }

    #[test]
    fn only_an_explicit_yes_is_a_yes() {
        for yes in ["y", "Y", "yes", " YES \n", "Yes"] {
            assert!(answer_is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "ok", "y es", "yes please", "1"] {
            assert!(!answer_is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn return_waits_for_the_delay() {
        assert!(!confirm_ready(Duration::ZERO));
        assert!(!confirm_ready(Duration::from_millis(599)));
        assert!(confirm_ready(CONFIRM_DELAY));
    }

    #[test]
    fn height_grows_with_the_text_and_is_capped() {
        let small = prompt_text(&PackagePreview {
            name: "a".into(),
            bundle_id: String::new(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            trust: TrustSummary::default(),
        });
        let big = prompt_text(&preview());
        let (hs, hb) = (prompt_height(&small, 600.0, 13.0), prompt_height(&big, 600.0, 13.0));
        assert!(hs < hb, "{hs} {hb}");
        assert!(hb <= super::super::VISIBLE_ROWS as f32 * ROW_HEIGHT);
        // long lists are cut in the text itself ("+195 more"), so only a very narrow window hits the cap
        let mut huge = preview();
        huge.trust.terminal_commands = (0..200).map(|i| format!("command number {i}")).collect();
        assert!(prompt_text(&huge).plain().contains("(+195 more)"));
        assert_eq!(
            prompt_height(&prompt_text(&huge), 60.0, 13.0),
            super::super::VISIBLE_ROWS as f32 * ROW_HEIGHT
        );
    }
}
