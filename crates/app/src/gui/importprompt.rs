//! The confirmation shown before a workflow package is installed.
//!
//! A workflow is a bundle of scripts that will run with the user's permissions
//! (docs/SECURITY.md, SEC-03/13/14). The engine cannot make an untrusted one safe, so
//! before installing, the user sees who claims to have written it, its bundle id and
//! exactly what its object graph can do ([`TrustSummary`]), and must confirm.
//!
//! [`prompt_text`] builds the wording from a [`PackagePreview`]; it is pure and shared
//! by the launcher's in-window prompt, the preferences window, the Finder "open document"
//! dialog (`mac/opendocs.rs`) and `ashdash import`. All text that comes from the package is
//! stripped of control and Unicode format characters (bidi overrides, zero-width) and
//! length-capped here, so a hostile package cannot reshape or disguise the prompt.
//!
//! What is asked about is a [`StagedPackage`]: the package was opened once and the answer
//! installs that very copy, never the file on disk again.

use std::time::Duration;

use ashdash_core::Mods;
use ashdash_core::workflow::{PackagePreview, StagedPackage, TrustSummary, WritePlace};
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

/// Unicode "format" characters (general category Cf): invisible, yet able to reorder
/// (bidi embeddings, overrides and isolates) or hide (zero-width) text. Not whitespace and
/// not `char::is_control`, so they need their own list (the standard library has no
/// general-category lookup).
pub fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD
        | 0x0600..=0x0605
        | 0x061C
        | 0x06DD
        | 0x070F
        | 0x0890..=0x0891
        | 0x08E2
        | 0x180E
        | 0x200B..=0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x2064
        | 0x2066..=0x206F
        | 0xFEFF
        | 0xFFF9..=0xFFFB
        | 0x110BD
        | 0x110CD
        | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F)
}

/// Characters that draw nothing yet are neither whitespace nor format characters: Hangul and
/// Braille fillers, Khmer inherent vowels, the combining grapheme joiner. Shown as a space.
pub fn is_blank_looking(c: char) -> bool {
    matches!(
        c as u32,
        0x034F | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x2065 | 0x2800 | 0x3164 | 0xFFA0
    )
}

/// Remove control and format characters, collapse whitespace runs (and blank-looking
/// characters, shown as spaces) and cap the length.
pub fn clean(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() || is_blank_looking(c) {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !c.is_control() && !is_format_char(*c))
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

/// What the workflow can do, one line each. The first line is the verdict about code: it only
/// says "Runs no scripts and no terminal commands." for a workflow that can do nothing else
/// the summary knows about (see [`TrustSummary::is_passive`]); a workflow that writes files,
/// opens URLs or files, replaces the clipboard or has triggers gets a qualified lead-in and
/// the specifics below it. Shared by every prompt and the workflow list.
pub(crate) fn capability_lines(t: &TrustSummary) -> Vec<String> {
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
    } else if t.is_passive() {
        out.push("Runs no scripts and no terminal commands.".into());
    } else {
        out.push("Runs no scripts or Terminal commands, but it can do the following:".into());
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
    let outside: Vec<_> = t.file_writes.iter().filter(|w| w.is_outside()).collect();
    let own: Vec<_> = t
        .file_writes
        .iter()
        .filter(|w| w.place == WritePlace::WorkflowFolder)
        .collect();
    let inside: Vec<_> = t
        .file_writes
        .iter()
        .filter(|w| matches!(w.place, WritePlace::WorkflowData | WritePlace::WorkflowCache))
        .collect();
    if !outside.is_empty() {
        out.push(format!(
            "Writes files anywhere your account can (or at a place decided at run time): {}",
            listed(&outside.iter().map(|w| w.describe()).collect::<Vec<_>>(), MAX_COMMAND)
        ));
    }
    if !own.is_empty() {
        // The installed folder holds info.plist and the scripts: whatever is written there can
        // add objects or change code after this prompt has been answered.
        out.push(format!(
            "Can modify its own files (scripts, settings) after you install it: {}",
            listed(&own.iter().map(|w| w.describe()).collect::<Vec<_>>(), MAX_COMMAND)
        ));
    }
    if !inside.is_empty() {
        out.push(format!(
            "Writes files in its data or cache folder: {}",
            listed(&inside.iter().map(|w| w.describe()).collect::<Vec<_>>(), MAX_COMMAND)
        ));
    }
    if t.copies_to_clipboard > 0 {
        out.push("Replaces what is on your clipboard.".into());
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
    if !t.opens_at_run_time.is_empty() {
        out.push(format!(
            "Opens files chosen at run time: {} (this can be an app or script shipped inside the package)",
            listed(&t.opens_at_run_time, MAX_COMMAND)
        ));
    }
    if !t.launched_apps.is_empty() {
        let apps: Vec<String> = t
            .launched_apps
            .iter()
            .map(|a| {
                if a.shipped {
                    format!("{} (shipped with the workflow)", a.app)
                } else {
                    a.app.clone()
                }
            })
            .collect();
        out.push(format!("Launches an application: {}", listed(&apps, MAX_COMMAND)));
    }
    if !t.opens_shipped_files.is_empty() {
        out.push(format!(
            "Opens files shipped with the workflow: {} (this can be an app or script)",
            listed(&t.opens_shipped_files, MAX_COMMAND)
        ));
    }
    if t.results_steer_later_steps {
        out.push(
            "Its results can change what later steps do (a Script Filter item can set the \
             argument and variables that the scripts, URLs, files or Terminal commands after it use)."
                .into(),
        );
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
    if let Some(old) = &p.replaces {
        let old_name = clean(&old.name, MAX_NAME);
        let old_version = clean(&old.version, 40);
        lines.push(format!(
            "This replaces “{}”{} (installed).",
            if old_name.is_empty() { "(unnamed)" } else { &old_name },
            if old_version.is_empty() {
                String::new()
            } else {
                format!(" {old_version}")
            }
        ));
    }
    if p.saved_settings {
        lines.push(
            "Settings saved for this workflow identifier are kept and the new package can read them \
             (they may include tokens or passwords)."
                .into(),
        );
    }
    lines.push(String::new());
    lines.extend(capability_lines(&p.trust));
    lines.push(String::new());
    lines.push("Workflows run on your Mac with your permissions. Only install ones you trust.".into());
    PromptText {
        title: format!(
            "{} workflow “{name}”?",
            if p.replaces.is_some() { "Replace" } else { "Install" }
        ),
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

/// What a key press does to the open prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    Nothing,
    Cancel,
    Install,
}

/// Esc cancels. Return installs only when it is a fresh press (not a key-repeat event of a
/// Return that is still held), has no modifier, and the prompt has been up long enough
/// ([`confirm_ready`]). A held Return therefore never confirms by repeating.
pub fn prompt_key(esc: bool, enter: Option<Mods>, enter_fresh: bool, shown_for: Duration) -> PromptKey {
    if esc {
        PromptKey::Cancel
    } else if enter.is_some_and(|m| m.is_none()) && enter_fresh && confirm_ready(shown_for) {
        PromptKey::Install
    } else {
        PromptKey::Nothing
    }
}

/// Height in points the prompt wants at `width`, capped at the room under the search field.
pub fn prompt_height(text: &PromptText, width: f32, font_size: f32) -> f32 {
    let cols = ((width - 24.0) / (font_size * 0.5)).floor().max(10.0) as usize;
    let rows: usize = 1 + text.lines.iter().map(|l| wrapped_lines(l, cols)).sum::<usize>() + 1;
    (rows as f32 * font_size * 1.2 + 16.0).min(super::VISIBLE_ROWS as f32 * ROW_HEIGHT)
}

/// A prompt that is up, waiting for Return or Esc. It owns the staged copy of the package:
/// confirming installs it, any other way out drops it (which deletes it).
pub struct ImportPrompt {
    pub staged: StagedPackage,
    pub text: PromptText,
    pub shown_at: std::time::Instant,
}

impl ImportPrompt {
    pub fn new(staged: StagedPackage) -> Self {
        ImportPrompt {
            text: prompt_text(staged.preview()),
            staged,
            shown_at: std::time::Instant::now(),
        }
    }
}

/// Install what the open prompt showed (its staged copy), on a worker thread. Returns the
/// folder name of the installed workflow. The package file is not read again.
pub fn install_confirmed(engine: &ashdash_core::Engine, prompt: ImportPrompt) -> Result<String, String> {
    engine
        .install_staged(prompt.staged)
        .map(|dest| {
            dest.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        })
        .map_err(|e| format!("{e:#}"))
}

/// `ashdash import`: show what the staged package can do, ask (unless `yes`) and install that
/// very copy. Declining or failing to ask drops the staged copy; nothing is installed.
pub fn cli_import(
    staged: StagedPackage,
    yes: bool,
    ask: impl FnOnce(&str) -> anyhow::Result<bool>,
    install: impl FnOnce(StagedPackage) -> anyhow::Result<std::path::PathBuf>,
) -> anyhow::Result<std::path::PathBuf> {
    println!("{}\n", prompt_text(staged.preview()).plain());
    if !yes && !ask("Install this workflow? [y/N] ")? {
        anyhow::bail!("not installed");
    }
    install(staged)
}

/// A minimal "stored" (uncompressed) zip writer for tests, so no zip dependency is needed here.
#[cfg(test)]
pub(crate) mod test_zip {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }

    /// A zip archive holding `files` (name, bytes), uncompressed.
    pub fn stored(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let (crc, n) = (crc32(data), name.as_bytes());
            let offset = z.len() as u32;
            z.extend_from_slice(&[0x50, 0x4b, 3, 4, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            z.extend_from_slice(&crc.to_le_bytes());
            z.extend_from_slice(&(data.len() as u32).to_le_bytes());
            z.extend_from_slice(&(data.len() as u32).to_le_bytes());
            z.extend_from_slice(&(n.len() as u16).to_le_bytes());
            z.extend_from_slice(&[0, 0]);
            z.extend_from_slice(n);
            z.extend_from_slice(data);
            central.extend_from_slice(&[0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(n.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(n);
        }
        let cd_off = z.len() as u32;
        z.extend_from_slice(&central);
        z.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
        z.extend_from_slice(&(files.len() as u16).to_le_bytes());
        z.extend_from_slice(&(files.len() as u16).to_le_bytes());
        z.extend_from_slice(&(central.len() as u32).to_le_bytes());
        z.extend_from_slice(&cd_off.to_le_bytes());
        z.extend_from_slice(&[0, 0]);
        z
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
    use ashdash_core::workflow::{
        FileWriteInfo, INCOMING_ARGUMENT, InstalledWorkflow, LaunchedApp, WriteKind, WritePlace,
    };

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
                ..TrustSummary::default()
            },
            replaces: None,
            saved_settings: false,
        }
    }

    fn write_file(path: &str, kind: WriteKind, place: WritePlace) -> FileWriteInfo {
        FileWriteInfo {
            path: path.into(),
            place,
            kind,
            dynamic: path.contains('{'),
            creates_folders: false,
        }
    }

    fn only_write_file() -> PackagePreview {
        PackagePreview {
            name: "Dotfiles".into(),
            bundle_id: "x.dot".into(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            trust: TrustSummary {
                file_writes: vec![write_file("~/.zshrc", WriteKind::Append, WritePlace::Anywhere)],
                ..TrustSummary::default()
            },
            replaces: None,
            saved_settings: false,
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
            replaces: None,
            saved_settings: false,
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
    fn a_workflow_that_only_writes_a_file_is_not_called_harmless() {
        let all = prompt_text(&only_write_file()).plain();
        assert!(!all.contains("Runs no scripts and no terminal commands"), "{all}");
        assert!(
            all.contains("Runs no scripts or Terminal commands, but it can"),
            "{all}"
        );
        assert!(all.contains("Writes files anywhere"), "{all}");
        assert!(all.contains("appends to ~/.zshrc"), "{all}");
    }

    #[test]
    fn the_harmless_sentence_is_only_for_workflows_that_do_nothing_else() {
        let sentence = "Runs no scripts and no terminal commands";
        let lead = |f: fn(&mut TrustSummary)| {
            let mut t = TrustSummary::default();
            f(&mut t);
            capability_lines(&t).join("\n")
        };
        assert!(lead(|_| {}).contains(sentence));
        for (what, text) in [
            ("url", lead(|t| t.open_url_schemes = vec!["https".into()])),
            ("file", lead(|t| t.external_files = vec!["~/x".into()])),
            (
                "runtime file",
                lead(|t| t.opens_at_run_time = vec![INCOMING_ARGUMENT.into()]),
            ),
            ("trigger", lead(|t| t.external_triggers = 1)),
            ("clipboard", lead(|t| t.copies_to_clipboard = 1)),
            ("steered", lead(|t| t.results_steer_later_steps = true)),
            (
                "launched app",
                lead(|t| {
                    t.launched_apps = vec![LaunchedApp {
                        app: "payload.app".into(),
                        shipped: true,
                    }]
                }),
            ),
            (
                "shipped file",
                lead(|t| t.opens_shipped_files = vec!["payload.app".into()]),
            ),
            (
                "write",
                lead(|t| t.file_writes = vec![write_file("n.txt", WriteKind::Overwrite, WritePlace::WorkflowData)]),
            ),
        ] {
            assert!(!text.contains(sentence), "{what}: {text}");
        }
        // a workflow with scripts leads with the code verdict instead
        assert!(lead(|t| t.scripts = 1).starts_with("Runs code"));
    }

    #[test]
    fn a_launched_application_is_a_capability_line_and_a_shipped_one_is_called_out() {
        // Review item 1: Keyword -> Open File (".", openwith inside the package).
        let t = TrustSummary {
            launched_apps: vec![
                LaunchedApp {
                    app: "payload.app".into(),
                    shipped: true,
                },
                LaunchedApp {
                    app: "Safari".into(),
                    shipped: false,
                },
            ],
            opens_shipped_files: vec!["docs/readme.md".into()],
            ..TrustSummary::default()
        };
        let all = capability_lines(&t).join("\n");
        assert!(!all.contains("Runs no scripts and no terminal commands"), "{all}");
        assert!(
            all.contains("Runs no scripts or Terminal commands, but it can"),
            "{all}"
        );
        assert!(
            all.contains("Launches an application: payload.app (shipped with the workflow)"),
            "{all}"
        );
        assert!(all.contains("Safari"), "{all}");
        assert!(!all.contains("Safari (shipped"), "{all}");
        assert!(
            all.contains("Opens files shipped with the workflow: docs/readme.md"),
            "{all}"
        );
    }

    #[test]
    fn writes_are_split_into_outside_and_in_its_own_folders() {
        let mut p = only_write_file();
        p.trust.file_writes = vec![
            write_file(
                "~/Library/LaunchAgents/x.plist",
                WriteKind::Overwrite,
                WritePlace::Anywhere,
            ),
            write_file("{query}", WriteKind::Prepend, WritePlace::Anywhere),
            write_file("notes.txt", WriteKind::Append, WritePlace::WorkflowData),
        ];
        let all = prompt_text(&p).plain();
        assert!(all.contains("overwrites ~/Library/LaunchAgents/x.plist"), "{all}");
        assert!(all.contains("prepends to {query} (decided at run time)"), "{all}");
        assert!(
            all.contains("Writes files in its data or cache folder: appends to notes.txt (its data folder)"),
            "{all}"
        );
        // lists are cut like the other lists
        p.trust.file_writes = (0..12)
            .map(|i| write_file(&format!("/tmp/f{i}"), WriteKind::Overwrite, WritePlace::Anywhere))
            .collect();
        assert!(prompt_text(&p).plain().contains("(+7 more)"));
    }

    #[test]
    fn writing_into_its_own_installed_folder_is_a_separate_warning() {
        // Write File with "relative to the workflow folder" can overwrite info.plist or a
        // script after the prompt: not "its own folders" in the harmless sense.
        let mut p = only_write_file();
        p.trust.file_writes = vec![
            write_file("notes.txt", WriteKind::Append, WritePlace::WorkflowData),
            write_file("run.sh", WriteKind::Overwrite, WritePlace::WorkflowFolder),
        ];
        let all = prompt_text(&p).plain();
        assert!(
            all.contains(
                "Can modify its own files (scripts, settings) after you install it: overwrites run.sh (its own folder)"
            ),
            "{all}"
        );
        assert!(
            all.contains("Writes files in its data or cache folder: appends to notes.txt"),
            "{all}"
        );
        assert!(!all.contains("Writes files in its own folders"), "{all}");
        assert!(!all.contains("Runs no scripts and no terminal commands"), "{all}");
        // only the own-folder write: still a line of its own, no data/cache line
        p.trust.file_writes = vec![write_file("run.sh", WriteKind::Overwrite, WritePlace::WorkflowFolder)];
        let all = prompt_text(&p).plain();
        assert!(all.contains("Can modify its own files"), "{all}");
        assert!(!all.contains("data or cache folder"), "{all}");
    }

    #[test]
    fn script_filter_steering_is_stated() {
        let t = TrustSummary {
            results_steer_later_steps: true,
            ..TrustSummary::default()
        };
        let text = capability_lines(&t).join("\n");
        assert!(text.contains("Its results can change what later steps do"), "{text}");
    }

    #[test]
    fn files_opened_at_run_time_are_listed_and_not_called_harmless() {
        let sentence = "Runs no scripts and no terminal commands";
        let t = TrustSummary {
            opens_at_run_time: vec![INCOMING_ARGUMENT.into(), "{var:app}".into()],
            ..TrustSummary::default()
        };
        let text = capability_lines(&t).join("\n");
        assert!(!text.contains(sentence), "{text}");
        assert!(
            text.contains("Opens files chosen at run time: (the incoming argument), {var:app}"),
            "{text}"
        );
        assert!(text.contains("an app or script shipped inside the package"), "{text}");
    }

    #[test]
    fn a_replacement_is_announced_with_the_installed_name_and_version() {
        let mut p = preview();
        p.replaces = Some(InstalledWorkflow {
            name: "Weather".into(),
            version: "1.0".into(),
        });
        let t = prompt_text(&p);
        assert_eq!(t.title, "Replace workflow “Weather”?");
        assert!(
            t.plain().contains("This replaces “Weather” 1.0 (installed)."),
            "{}",
            t.plain()
        );
        assert!(!t.plain().contains("Settings saved for this workflow"));
        p.saved_settings = true;
        let all = prompt_text(&p).plain();
        assert!(
            all.contains("Settings saved for this workflow identifier are kept") && all.contains("tokens or passwords"),
            "{all}"
        );
        // a fresh install says neither
        let all = prompt_text(&preview()).plain();
        assert!(!all.contains("This replaces") && !all.contains("Settings saved"));
        // installed-side text is cleaned too
        p.replaces = Some(InstalledWorkflow {
            name: "Old\nAuthor: Apple\u{202e}".into(),
            version: "1\u{200b}.0".into(),
        });
        let lines = prompt_text(&p).lines;
        let l = lines.iter().find(|l| l.starts_with("This replaces")).unwrap();
        assert_eq!(l, "This replaces “Old Author: Apple” 1.0 (installed).");
    }

    #[test]
    fn format_characters_are_stripped_and_long_names_are_capped() {
        // right-to-left override, isolates, zero-width space/joiner, BOM, soft hyphen, tag chars
        let evil = "pa\u{202e}yp\u{2066}al\u{2069}\u{200b}\u{200d}\u{feff}\u{ad}\u{e0041}";
        assert_eq!(clean(evil, 80), "paypal");
        for c in [
            '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
            '\u{200e}', '\u{200f}', '\u{061c}', '\u{2060}',
        ] {
            assert!(is_format_char(c), "{c:?}");
            assert_eq!(clean(&format!("a{c}b"), 80), "ab");
        }
        for c in ['a', ' ', 'é', '世', '😀', '-', '\u{301}'] {
            assert!(!is_format_char(c), "{c:?}");
        }
        // none of it reaches the prompt, whichever field it hides in
        let mut p = preview();
        p.name = "Safe\u{202e}gnp.exe".into();
        p.author = "A\u{200b}pple".into();
        p.trust.terminal_commands = vec!["echo \u{202e}hi".into()];
        let all = prompt_text(&p).plain();
        assert!(all.chars().all(|c| !is_format_char(c)), "{all:?}");
        assert!(all.contains("Safegnp.exe") && all.contains("Author: Apple"));
        // a 10 000-character name is cut
        p.name = "n".repeat(10_000);
        let t = prompt_text(&p);
        assert!(t.title.chars().count() < 120, "{}", t.title.chars().count());
        assert!(t.lines[0].chars().count() <= "Name: ".len() + MAX_NAME);
        assert!(t.plain().chars().count() < 3000);
    }

    #[test]
    fn blank_looking_characters_are_collapsed_like_spaces() {
        // Re-review N7: not format characters (so not hidden by the list above) but they draw
        // nothing, so a field made of them looks empty and one containing them looks padded.
        for c in [
            '\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}', '\u{2800}', '\u{17b4}', '\u{17b5}', '\u{034f}', '\u{2065}',
        ] {
            assert!(is_blank_looking(c), "{c:?}");
            assert_eq!(clean(&format!("a{c}{c}b"), 80), "a b", "{c:?}");
            assert_eq!(clean(&format!("{c}{c}"), 80), "", "{c:?}");
        }
        // plain Unicode whitespace of every width already collapses ...
        for c in [
            '\u{2000}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{205f}', '\u{3000}', '\u{a0}',
        ] {
            assert_eq!(clean(&format!("a{c}{c} b"), 80), "a b", "{c:?}");
        }
        // ... real characters and combining marks stay
        for c in ['a', 'é', '世', '😀', '\u{301}', '\u{fe0f}'] {
            assert!(!is_blank_looking(c), "{c:?}");
        }
        // an author field that only looks empty is reported as not stated
        let mut p = preview();
        p.author = "\u{3164}\u{2800}".into();
        p.name = "\u{3164}".into();
        let t = prompt_text(&p);
        assert!(
            t.lines.iter().any(|l| l.starts_with("Author: not stated")),
            "{:?}",
            t.lines
        );
        assert!(t.lines[0] == "Name: (unnamed)", "{:?}", t.lines[0]);
    }

    #[test]
    fn a_held_return_never_installs_by_repeating() {
        let late = CONFIRM_DELAY + Duration::from_millis(1);
        let enter = Some(Mods::NONE);
        // a fresh press after the delay installs
        assert_eq!(prompt_key(false, enter, true, late), PromptKey::Install);
        // key-repeat events of a Return that is still down do not, however long the prompt is up
        assert_eq!(prompt_key(false, enter, false, late), PromptKey::Nothing);
        assert_eq!(
            prompt_key(false, enter, false, Duration::from_secs(60)),
            PromptKey::Nothing
        );
        // too early, or with a modifier, even a fresh press does not
        assert_eq!(prompt_key(false, enter, true, Duration::ZERO), PromptKey::Nothing);
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        assert_eq!(prompt_key(false, Some(cmd), true, late), PromptKey::Nothing);
        // no Return at all
        assert_eq!(prompt_key(false, None, true, late), PromptKey::Nothing);
        // Esc always cancels, repeat or not
        assert_eq!(prompt_key(true, enter, false, Duration::ZERO), PromptKey::Cancel);
    }

    fn doc_plist(name: &str, bundle: &str, marker: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>name</key><string>{name}</string><key>bundleid</key><string>{bundle}</string>
<key>objects</key><array>
<dict><key>uid</key><string>A</string><key>type</key><string>alfred.workflow.action.script</string>
<key>config</key><dict><key>type</key><integer>0</integer><key>script</key><string>echo {marker}</string></dict></dict>
</array></dict></plist>"#
        )
    }

    /// The launcher's prompt asks about the staged copy and installs that copy, whatever
    /// happens to the file after the prompt appeared.
    #[test]
    fn the_launcher_prompt_installs_the_copy_it_showed() {
        let root = std::env::temp_dir().join(format!("oa-importprompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = ashdash_core::Paths::under(&root);
        let engine = ashdash_core::Engine::new(
            paths.clone(),
            ashdash_core::Config {
                app_dirs: vec![],
                ..Default::default()
            },
        );
        let pkg = root.join("w.alfredworkflow");
        let write = |name: &str, bundle: &str, marker: &str| {
            let plist = doc_plist(name, bundle, marker);
            std::fs::write(&pkg, test_zip::stored(&[("info.plist", plist.as_bytes())])).unwrap();
        };
        write("Shown", "t.prompt.shown", "SHOWN");

        let prompt = ImportPrompt::new(engine.stage_workflow(&pkg).unwrap());
        assert_eq!(prompt.text.title, "Install workflow “Shown”?");
        write("Evil", "t.prompt.evil", "SWAPPED"); // the user is still reading
        let name = install_confirmed(&engine, prompt).unwrap();
        assert_eq!(name, "user.workflow.t.prompt.shown");
        let dir = paths.workflows_dir();
        let text = std::fs::read_to_string(dir.join(&name).join("info.plist")).unwrap();
        assert!(text.contains("SHOWN") && !text.contains("SWAPPED"), "{text}");
        assert!(!dir.join("user.workflow.t.prompt.evil").exists());

        // any other way out of the prompt (Esc, hiding, a newer prompt) deletes the staged copy
        let prompt = ImportPrompt::new(engine.stage_workflow(&pkg).unwrap());
        assert!(prompt.text.title.starts_with("Install"), "{}", prompt.text.title);
        drop(prompt);
        assert_eq!(std::fs::read_dir(paths.data.join("import-staging")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_cli_installs_the_staged_copy_or_nothing() {
        let root = std::env::temp_dir().join(format!("oa-cliimport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = ashdash_core::Paths::under(&root);
        let engine = ashdash_core::Engine::new(
            paths.clone(),
            ashdash_core::Config {
                app_dirs: vec![],
                ..Default::default()
            },
        );
        let pkg = root.join("w.alfredworkflow");
        let write = |name: &str, bundle: &str, marker: &str| {
            let plist = doc_plist(name, bundle, marker);
            std::fs::write(&pkg, test_zip::stored(&[("info.plist", plist.as_bytes())])).unwrap();
        };
        write("Shown", "t.cli.shown", "SHOWN");
        let staging = paths.data.join("import-staging");
        let left = || std::fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0);

        // "y" typed after the file was swapped: the shown copy is installed
        let dest = cli_import(
            engine.stage_workflow(&pkg).unwrap(),
            false,
            |q| {
                assert!(q.contains("[y/N]"));
                write("Evil", "t.cli.evil", "SWAPPED");
                Ok(answer_is_yes("y"))
            },
            |s| engine.install_staged(s),
        )
        .unwrap();
        assert!(dest.ends_with("user.workflow.t.cli.shown"));
        let text = std::fs::read_to_string(dest.join("info.plist")).unwrap();
        assert!(text.contains("SHOWN") && !text.contains("SWAPPED"), "{text}");
        assert!(!paths.workflows_dir().join("user.workflow.t.cli.evil").exists());
        assert_eq!(left(), 0);

        // "n", an unanswerable question (no terminal), and --yes
        write("Other", "t.cli.other", "OTHER");
        let no = cli_import(
            engine.stage_workflow(&pkg).unwrap(),
            false,
            |_| Ok(false),
            |_| panic!("no install"),
        );
        assert_eq!(no.unwrap_err().to_string(), "not installed");
        let err = cli_import(
            engine.stage_workflow(&pkg).unwrap(),
            false,
            |_| anyhow::bail!("no terminal"),
            |_| panic!("no install"),
        );
        assert!(err.unwrap_err().to_string().contains("no terminal"));
        assert!(!paths.workflows_dir().join("user.workflow.t.cli.other").exists());
        assert_eq!(left(), 0, "declined or unanswered: the copy is deleted");
        let dest = cli_import(
            engine.stage_workflow(&pkg).unwrap(),
            true,
            |_| panic!("--yes does not ask"),
            |s| engine.install_staged(s),
        )
        .unwrap();
        assert!(dest.ends_with("user.workflow.t.cli.other"));
        let _ = std::fs::remove_dir_all(root);
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
            replaces: None,
            saved_settings: false,
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
