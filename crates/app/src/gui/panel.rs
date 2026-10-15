//! The Universal Actions panel: Right Arrow on a file, URL or text result opens
//! a sub-list of what can be done with it; "Open With..." goes one level
//! deeper; Esc (or Left on an empty filter) goes back one level.
//!
//! Everything that decides something is a pure function or the small
//! [`PanelStack`] and is tested below; the `impl LauncherApp` part only moves
//! the results on and off the screen.
//!
//! Keys (see `keys.rs`): Right opens, Left/Esc go back, Option+Up adds the
//! selected file to the buffer, Option+Down removes it; with a non-empty buffer
//! Right opens the actions for the whole buffer. Cmd+Return keeps its existing
//! meaning (the result's alternative action, e.g. reveal), so it is not used here.

use std::path::{Path, PathBuf};

use ashdash_core::actions::{ItemKind, UniversalAction, item_kind};
use ashdash_core::{Action, Item, fuzzy};
use eframe::egui;

use super::LauncherApp;
use super::glue::AsyncMsg;
use super::schedule::reselect;

/// Prefix of ids of rows the launcher itself interprets.
pub const UI_INSTALL_PREFIX: &str = "ui:install:";
/// Most applications offered under "Open With...".
const OPEN_WITH_LIMIT: usize = 200;

/// File extensions of the reference launcher workflow packages (all the same zip format).
const WORKFLOW_EXTENSIONS: [&str; 4] = [
    "alfredworkflow",
    "alfred3workflow",
    "alfred4workflow",
    "alfred5workflow",
];

pub fn is_workflow_package(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| WORKFLOW_EXTENSIONS.iter().any(|w| e.eq_ignore_ascii_case(w)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Actions,
    OpenWith,
    Buffer,
}

/// What the launcher showed before a level was opened, so going back restores it.
#[derive(Debug, Clone, Default)]
pub struct Restore {
    pub text: String,
    pub items: Vec<Item>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct Level {
    pub kind: Kind,
    /// Hint shown in the empty search field ("Actions for report.pdf").
    pub title: String,
    /// The whole list; the field's text filters what is shown.
    pub entries: Vec<Item>,
    pub restore: Restore,
    /// Ties asynchronously loaded entries to this level.
    pub token: u64,
}

/// The open levels, outermost first.
#[derive(Debug, Default)]
pub struct PanelStack {
    levels: Vec<Level>,
}

impl PanelStack {
    pub fn push(&mut self, level: Level) {
        self.levels.push(level);
    }

    pub fn pop(&mut self) -> Option<Level> {
        self.levels.pop()
    }

    pub fn top(&self) -> Option<&Level> {
        self.levels.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut Level> {
        self.levels.last_mut()
    }

    pub fn is_open(&self) -> bool {
        !self.levels.is_empty()
    }

    pub fn clear(&mut self) {
        self.levels.clear();
    }
}

/// Things the launcher does itself instead of handing the row to the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// List applications to open the file with.
    OpenWith(PathBuf),
    /// Quick Look through the engine's process handle (the launcher stays open).
    QuickLook(Vec<PathBuf>),
    /// Preview a workflow package and ask before installing it.
    InstallWorkflow(PathBuf),
}

pub fn ui_action(item: &Item) -> Option<UiAction> {
    match &item.action {
        Action::Universal(UniversalAction::OpenWithMenu(p)) => Some(UiAction::OpenWith(p.clone())),
        Action::Universal(UniversalAction::QuickLook(ps)) => Some(UiAction::QuickLook(ps.clone())),
        Action::None => item
            .id
            .strip_prefix(UI_INSTALL_PREFIX)
            .map(|p| UiAction::InstallWorkflow(PathBuf::from(p))),
        _ => None,
    }
}

/// The path a result stands for, if it is a file or folder (what the buffer holds).
pub fn source_path(item: &Item) -> Option<PathBuf> {
    match item_kind(item) {
        ItemKind::File(p) | ItemKind::Folder(p) => Some(p),
        _ => None,
    }
}

/// The rows of the actions panel for `item`: the engine's list, with the two
/// fix-ups only the UI can make ("Open With..." is a dead end for the core, a
/// workflow package gets an "Install Workflow..." row first).
pub fn build_actions(item: &Item, core_actions: Vec<Item>) -> Vec<Item> {
    let mut rows: Vec<Item> = core_actions
        .into_iter()
        .map(|mut a| {
            if matches!(a.action, Action::Universal(UniversalAction::OpenWithMenu(_))) {
                a.valid = true;
                a.subtitle = "Choose an application".into();
            }
            a
        })
        .collect();
    if let ItemKind::File(p) = item_kind(item)
        && is_workflow_package(&p)
    {
        let install = Item::new(format!("{UI_INSTALL_PREFIX}{}", p.display()), "Install Workflow…")
            .subtitle("Shows what it can do, then asks")
            .icon(ashdash_core::Icon::Glyph("⬇"))
            .score(rows.len() as f64 + 1.0);
        rows.insert(0, install);
    }
    rows
}

/// "Actions for report.pdf" style hint for the empty field.
pub fn title_for(kind: Kind, item: &Item, buffer_len: usize) -> String {
    let name = item.title.chars().take(40).collect::<String>();
    match kind {
        Kind::Actions => format!("Actions for {name}"),
        Kind::OpenWith => format!("Open {name} with…"),
        Kind::Buffer => format!(
            "Actions for {buffer_len} buffered item{}",
            if buffer_len == 1 { "" } else { "s" }
        ),
    }
}

/// Rows whose title matches `text`, best match first; the engine's order is
/// kept for equal scores and for an empty filter.
pub fn filter_entries(entries: &[Item], text: &str) -> Vec<Item> {
    let text = text.trim();
    if text.is_empty() {
        return entries.to_vec();
    }
    let mut scored: Vec<(f64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fuzzy::score(text, &e.title).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored.into_iter().map(|(_, i)| entries[i].clone()).collect()
}

/// Right Arrow opens the panel only when it would not move the text cursor
/// (cursor at the end, no modifier) and there is something to show.
pub fn right_opens_panel(cursor_at_end: bool, modifiers_held: bool, has_rows: bool) -> bool {
    cursor_at_end && !modifiers_held && has_rows
}

/// Left Arrow goes back only when it would not move the text cursor.
pub fn left_goes_back(cursor_at_start: bool, modifiers_held: bool, panel_open: bool) -> bool {
    cursor_at_start && !modifiers_held && panel_open
}

/// The line under the results about the buffer.
pub fn buffer_line(len: usize) -> Option<String> {
    (len > 0).then(|| {
        format!(
            "Buffer: {len} item{} · Right: actions · Alt+Down: remove",
            if len == 1 { "" } else { "s" }
        )
    })
}

/// The footer line under the results. Priority: a message, a running load,
/// a background reload, the buffer.
pub fn footer(status: Option<&str>, loading: bool, reloading: bool, buffer_len: usize) -> Option<String> {
    if let Some(s) = status {
        return Some(s.to_string());
    }
    if loading {
        return Some("Loading applications…".into());
    }
    if reloading {
        return Some("Reloading…".into());
    }
    buffer_line(buffer_len)
}

// ---------------------------------------------------------------- the window side

impl LauncherApp {
    pub(super) fn panel_open(&self) -> bool {
        self.glue.panels.is_open()
    }

    pub(super) fn search_hint(&self) -> String {
        self.glue
            .panels
            .top()
            .map_or_else(|| "Ashdash".to_string(), |l| l.title.clone())
    }

    pub(super) fn footer_text(&self) -> Option<String> {
        footer(
            self.status.as_deref(),
            self.glue.loading,
            self.visible && self.engine.reload_pending(),
            self.glue.buffer.len(),
        )
    }

    /// Stop the query worker (and ignore anything it still sends): the list
    /// on screen is not the engine's any more, or the launcher is hidden.
    pub(super) fn cancel_query(&mut self) {
        self.generation += 1;
        let _ = self.req_tx.send((self.generation, String::new()));
    }

    fn push_level(&mut self, kind: Kind, title: String, entries: Vec<Item>) {
        let restore = Restore {
            text: self.query.clone(),
            items: self.items.clone(),
            selected: self.selected,
        };
        if !self.panel_open() {
            self.cancel_query();
        }
        self.glue.token += 1;
        let token = self.glue.token;
        self.glue.panels.push(Level {
            kind,
            title,
            entries: entries.clone(),
            restore,
            token,
        });
        self.query.clear();
        self.items = entries;
        self.selected = 0;
        self.scroll_to_selected = true;
        self.confirming = None;
        self.status = None;
        self.want_focus = true;
        self.glue.cursor_to_end = true;
    }

    /// Right Arrow: open the actions for the buffer, or for the selected result.
    /// Returns true when it opened something (the key is then consumed).
    pub(super) fn open_actions(&mut self, ctx: &egui::Context) -> bool {
        if self.glue.import.is_some() {
            return false;
        }
        if self.panel_open() {
            // Right on "Open With..." goes one level deeper
            if let Some(UiAction::OpenWith(p)) = self.items.get(self.selected).and_then(ui_action) {
                self.open_with(ctx, p);
                return true;
            }
            return false;
        }
        if !self.glue.buffer.is_empty() {
            let rows = self.glue.buffer.actions();
            let title = title_for(Kind::Buffer, &Item::new("", ""), self.glue.buffer.len());
            self.push_level(Kind::Buffer, title, rows);
            return true;
        }
        let Some(item) = self.items.get(self.selected).cloned() else {
            return false;
        };
        let rows = build_actions(&item, self.engine.actions_for(&item));
        if rows.is_empty() {
            return false;
        }
        let title = title_for(Kind::Actions, &item, 0);
        self.push_level(Kind::Actions, title, rows);
        true
    }

    /// "Open With...": list the applications (Spotlight, so off the UI thread).
    pub(super) fn open_with(&mut self, ctx: &egui::Context, path: PathBuf) {
        let name = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let title = title_for(Kind::OpenWith, &Item::new("", name), 0);
        self.push_level(Kind::OpenWith, title, Vec::new());
        self.glue.loading = true;
        let (engine, tx, ctx, token) = (
            std::sync::Arc::clone(&self.engine),
            self.glue.tx.clone(),
            ctx.clone(),
            self.glue.token,
        );
        std::thread::spawn(move || {
            let entries = engine.open_with_items(&path, "", OPEN_WITH_LIMIT);
            let _ = tx.send(AsyncMsg::List { token, entries });
            ctx.request_repaint();
        });
    }

    /// The text field changed while a panel is open: filter locally.
    pub(super) fn refilter_panel(&mut self) {
        let Some(level) = self.glue.panels.top() else {
            return;
        };
        self.items = filter_entries(&level.entries, &self.query);
        self.selected = 0;
        self.scroll_to_selected = true;
    }

    /// Esc / Left: one level back. Returns to the results from the outermost level.
    pub(super) fn panel_back(&mut self) {
        let Some(level) = self.glue.panels.pop() else {
            return;
        };
        self.glue.loading = false;
        self.confirming = None;
        self.status = None;
        self.want_focus = true;
        self.glue.cursor_to_end = true;
        self.query = level.restore.text.clone();
        if self.glue.panels.is_open() {
            // back to the previous panel level: its list as it was
            self.items = level.restore.items;
            self.selected = level.restore.selected;
            self.scroll_to_selected = true;
        } else {
            // back to the results: refresh them (re-arms a Script Filter's rerun)
            self.items = level.restore.items.clone();
            self.selected = level.restore.selected;
            self.glue.keep_selection = level.restore.items.get(level.restore.selected).map(|i| i.id.clone());
            self.scroll_to_selected = true;
            self.request_query();
        }
    }

    pub(super) fn panel_apply(&mut self, msg: &AsyncMsg) {
        if let AsyncMsg::List { token, entries } = msg
            && let Some(level) = self.glue.panels.top_mut()
            && level.token == *token
        {
            level.entries = entries.clone();
            self.glue.loading = false;
            let filtered = filter_entries(&level.entries, &self.query);
            let keep = self.items.get(self.selected).map(|i| i.id.clone());
            self.selected = reselect(keep.as_deref(), self.selected, &filtered);
            self.items = filtered;
        }
    }

    // ------------------------------------------------------------ buffer

    /// Option+Up: put the selected file in the buffer.
    pub(super) fn buffer_add(&mut self) {
        let Some(path) = self.items.get(self.selected).and_then(source_path) else {
            self.status = Some("Only files and folders can be buffered".into());
            return;
        };
        match self.glue.buffer.add(path) {
            Ok(_) => self.status = None,
            Err(e) => self.status = Some(format!("{e:#}")),
        }
    }

    /// Option+Down: take the selected file out of the buffer (or the last one added).
    pub(super) fn buffer_remove(&mut self) {
        let selected = self.items.get(self.selected).and_then(source_path);
        let target = selected
            .filter(|p| self.glue.buffer.contains(p))
            .or_else(|| self.glue.buffer.paths().last().cloned());
        if let Some(p) = target {
            self.glue.buffer.remove(&p);
        }
        self.status = None;
        if self.glue.buffer.is_empty() && matches!(self.glue.panels.top().map(|l| l.kind), Some(Kind::Buffer)) {
            self.panel_back();
        }
    }
}

#[cfg(test)]
mod tests {
    use ashdash_core::actions::{UniversalAction, actions_for};

    use super::*;

    fn file_item(p: &Path) -> Item {
        Item::new("f", p.file_name().unwrap().to_string_lossy()).action(Action::Open(p.to_path_buf()))
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ashdash-panel-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn level(kind: Kind, title: &str, restore_text: &str) -> Level {
        Level {
            kind,
            title: title.into(),
            entries: vec![],
            restore: Restore {
                text: restore_text.into(),
                ..Default::default()
            },
            token: 0,
        }
    }

    #[test]
    fn the_stack_goes_back_one_level_at_a_time() {
        let mut s = PanelStack::default();
        assert!(!s.is_open());
        assert!(s.pop().is_none());
        s.push(level(Kind::Actions, "Actions for a", "saf"));
        s.push(level(Kind::OpenWith, "Open a with…", ""));
        assert_eq!(s.top().unwrap().restore.text, "");
        assert_eq!(s.pop().unwrap().kind, Kind::OpenWith);
        assert!(s.is_open());
        assert_eq!(s.top().unwrap().kind, Kind::Actions);
        assert_eq!(s.pop().unwrap().kind, Kind::Actions);
        assert!(!s.is_open());
        s.push(level(Kind::Buffer, "b", ""));
        s.clear();
        assert!(!s.is_open());
    }

    #[test]
    fn right_arrow_opens_only_when_the_cursor_cannot_move_right() {
        assert!(right_opens_panel(true, false, true));
        assert!(!right_opens_panel(false, false, true), "cursor inside the text");
        assert!(!right_opens_panel(true, true, true), "a modifier is a word jump");
        assert!(!right_opens_panel(true, false, false), "nothing to show");
        assert!(left_goes_back(true, false, true));
        assert!(!left_goes_back(false, false, true));
        assert!(!left_goes_back(true, false, false), "no panel: Left is the cursor's");
        assert!(!left_goes_back(true, true, true));
    }

    #[test]
    fn file_results_get_the_engine_actions_with_open_with_enabled() {
        let dir = temp("rows");
        let f = dir.join("report.pdf");
        std::fs::write(&f, "x").unwrap();
        let item = file_item(&f);
        let core = actions_for(&item);
        let core_open_with = core
            .iter()
            .find(|a| matches!(a.action, Action::Universal(UniversalAction::OpenWithMenu(_))))
            .unwrap();
        assert!(!core_open_with.valid, "the core marks it as a dead end");

        let rows = build_actions(&item, core.clone());
        assert_eq!(rows.len(), core.len(), "a pdf gets no extra rows");
        let ow = rows
            .iter()
            .find(|a| matches!(a.action, Action::Universal(UniversalAction::OpenWithMenu(_))))
            .unwrap();
        assert!(ow.valid);
        assert_eq!(ow.subtitle, "Choose an application");
        assert_eq!(ui_action(ow), Some(UiAction::OpenWith(f.clone())));
        // the other rows are untouched
        assert_eq!(rows[0], core[0]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn workflow_packages_offer_an_install_row_first() {
        let dir = temp("install");
        let pkg = dir.join("Thing.alfredworkflow");
        std::fs::write(&pkg, "x").unwrap();
        let item = file_item(&pkg);
        let rows = build_actions(&item, actions_for(&item));
        let first = &rows[0];
        assert_eq!(first.title, "Install Workflow…");
        assert!(first.valid);
        assert_eq!(ui_action(first), Some(UiAction::InstallWorkflow(pkg.clone())));
        assert!(rows.len() > 1);
        // a plain file does not
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        let rows = build_actions(&file_item(&txt), actions_for(&file_item(&txt)));
        assert!(
            rows.iter()
                .all(|r| ui_action(r) != Some(UiAction::InstallWorkflow(txt.clone())))
        );
        assert!(rows.iter().all(|r| r.title != "Install Workflow…"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extensions_are_matched_case_insensitively() {
        for ok in [
            "a.alfredworkflow",
            "/x/A.ALFRED5WORKFLOW",
            "b.alfred3workflow",
            "c.alfred4workflow",
        ] {
            assert!(is_workflow_package(Path::new(ok)), "{ok}");
        }
        for no in [
            "a.zip",
            "alfredworkflow",
            "a.alfredworkflow.txt",
            "a.alfredsnippets",
            "",
        ] {
            assert!(!is_workflow_package(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn quick_look_and_unrelated_rows_are_classified() {
        let ql = Item::new("q", "Quick Look").action(Action::Universal(UniversalAction::QuickLook(vec!["/a".into()])));
        assert_eq!(ui_action(&ql), Some(UiAction::QuickLook(vec!["/a".into()])));
        let copy = Item::new("c", "Copy").action(Action::Universal(UniversalAction::CopyText("x".into())));
        assert_eq!(ui_action(&copy), None);
        // a plain row with an id that merely looks similar is not an install request
        assert_eq!(ui_action(&Item::new("other:ui:install:/x", "t")), None);
        // an install id on a row that already has a real action is not trusted either
        let sneaky = Item::new(format!("{UI_INSTALL_PREFIX}/x"), "t").action(Action::Open("/x".into()));
        assert_eq!(ui_action(&sneaky), None);
    }

    #[test]
    fn text_and_url_results_have_actions_but_other_results_do_not() {
        let url = Item::new("u", "x").action(Action::OpenUrl("https://example.org".into()));
        assert!(!build_actions(&url, actions_for(&url)).is_empty());
        let none = Item::new("n", "Safari").action(Action::None);
        assert!(build_actions(&none, actions_for(&none)).is_empty());
    }

    #[test]
    fn filtering_ranks_by_title_and_keeps_the_engine_order_otherwise() {
        let dir = temp("filter");
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let rows = actions_for(&file_item(&f));
        assert_eq!(filter_entries(&rows, ""), rows);
        assert_eq!(filter_entries(&rows, "   "), rows);
        let hits = filter_entries(&rows, "copy");
        assert!(
            hits.len() >= 3 && hits.iter().all(|r| r.title.to_lowercase().contains("copy")),
            "{hits:?}"
        );
        let hits = filter_entries(&rows, "trash");
        assert_eq!(hits[0].title, "Move to Trash");
        assert!(filter_entries(&rows, "zzzzqq").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn buffer_collects_files_and_offers_actions_on_all_of_them() {
        let dir = temp("buffer");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "x").unwrap();
        assert_eq!(source_path(&file_item(&a)), Some(a.clone()));
        assert_eq!(
            source_path(&Item::new("u", "u").action(Action::OpenUrl("https://x.y".into()))),
            None
        );
        assert_eq!(source_path(&Item::new("s", "s").action(Action::None)), None);

        let mut buf = ashdash_core::actions::Buffer::new();
        assert!(buf.actions().is_empty());
        assert_eq!(buffer_line(buf.len()), None);
        buf.add(a.clone()).unwrap();
        buf.add(b.clone()).unwrap();
        assert!(!buf.add(a.clone()).unwrap(), "no duplicates");
        assert_eq!(
            buffer_line(buf.len()).unwrap(),
            "Buffer: 2 items · Right: actions · Alt+Down: remove"
        );
        assert!(buffer_line(1).unwrap().starts_with("Buffer: 1 item "));
        let rows = buf.actions();
        assert!(rows.iter().any(|r| r.title == "Copy 2 items"), "{rows:?}");
        assert_eq!(
            title_for(Kind::Buffer, &Item::new("", ""), 2),
            "Actions for 2 buffered items"
        );
        assert_eq!(
            title_for(Kind::Buffer, &Item::new("", ""), 1),
            "Actions for 1 buffered item"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn titles_are_short() {
        let long = "x".repeat(200);
        let t = title_for(Kind::Actions, &Item::new("", long), 0);
        assert!(t.chars().count() < 60);
        assert_eq!(
            title_for(Kind::OpenWith, &Item::new("", "a.pdf"), 0),
            "Open a.pdf with…"
        );
    }

    #[test]
    fn footer_priorities() {
        assert_eq!(footer(None, false, false, 0), None);
        assert_eq!(footer(Some("Trash it?"), true, true, 3).as_deref(), Some("Trash it?"));
        assert_eq!(footer(None, true, true, 3).as_deref(), Some("Loading applications…"));
        assert_eq!(footer(None, false, true, 3).as_deref(), Some("Reloading…"));
        assert!(footer(None, false, false, 3).unwrap().starts_with("Buffer: 3 items"));
    }
}
