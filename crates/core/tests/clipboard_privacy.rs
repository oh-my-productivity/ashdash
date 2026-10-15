//! SEC-19: secrets must not surface through clipboard or snippet rows, search results or the
//! files the core writes. Everything runs against a fake pasteboard in a temp environment;
//! no test here touches the real pasteboard or pastes anything.

mod common;

use std::cell::RefCell;
use std::path::PathBuf;

use ashdash_core::clipboard::filter::{CONCEALED_TYPE, TRANSIENT_TYPE};
use ashdash_core::clipboard::watch::{PasteboardSource, Step, Watcher};
use ashdash_core::item::Item;
use common::*;

const TEXT: &str = "public.utf8-plain-text";
const SECRET: &str = "hunter2-correct-horse";

#[derive(Default)]
struct FakePasteboard {
    count: RefCell<i64>,
    types: RefCell<Vec<String>>,
    app: RefCell<Option<String>>,
    text: RefCell<Option<String>>,
}

impl FakePasteboard {
    fn copy(&self, types: &[&str], app: Option<&str>, text: &str) {
        *self.count.borrow_mut() += 1;
        *self.types.borrow_mut() = types.iter().map(|s| s.to_string()).collect();
        *self.app.borrow_mut() = app.map(str::to_string);
        *self.text.borrow_mut() = Some(text.to_string());
    }
}

impl PasteboardSource for FakePasteboard {
    fn change_count(&self) -> i64 {
        *self.count.borrow()
    }
    fn types(&self) -> Vec<String> {
        self.types.borrow().clone()
    }
    fn frontmost_bundle_id(&self) -> Option<String> {
        self.app.borrow().clone()
    }
    fn read_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn read_text(&self) -> Option<String> {
        self.text.borrow().clone()
    }
    fn read_png(&self) -> Option<Vec<u8>> {
        None
    }
}

/// Every string an item shows without any user action (the row itself).
fn shown_on_screen(items: &[Item]) -> String {
    items
        .iter()
        .flat_map(|i| [Some(i.title.clone()), Some(i.subtitle.clone())])
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Everything an item can reveal, including the explicit Large Type / copy shortcuts.
fn exposed(items: &[Item]) -> String {
    items
        .iter()
        .flat_map(|i| {
            [
                Some(i.title.clone()),
                Some(i.subtitle.clone()),
                i.copy_text.clone(),
                i.large_type.clone(),
            ]
        })
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn concealed_transient_and_ignored_app_copies_never_reach_rows_or_disk() {
    let env = Env::new("priv-conceal", &["Safari"]);
    let e = env.engine();
    let (pb, mut w) = (FakePasteboard::default(), Watcher::new());
    let clip = e.clipboard();
    w.step(&pb, &clip); // baseline

    pb.copy(&[TEXT, CONCEALED_TYPE], Some("com.apple.Safari"), SECRET);
    assert!(matches!(w.step(&pb, &clip), Step::Skipped(_)));
    pb.copy(&[TEXT, TRANSIENT_TYPE], None, SECRET);
    assert!(matches!(w.step(&pb, &clip), Step::Skipped(_)));
    clip.lock().unwrap().set_ignored_apps(vec!["com.example.vault".into()]);
    pb.copy(&[TEXT], Some("com.example.vault"), SECRET);
    assert!(matches!(w.step(&pb, &clip), Step::Skipped(_)));

    for q in ["clip", "clip hunter", "clipboard", "clip correct"] {
        let rows = e.query(q);
        assert!(!exposed(&rows).contains("hunter2"), "{q}: {}", exposed(&rows));
    }
    let on_disk = std::fs::read_to_string(env.paths.clipboard_file()).unwrap_or_default();
    assert!(!on_disk.contains("hunter2"), "history file: {on_disk}");
}

#[test]
fn recorded_clipboard_text_only_appears_behind_the_clip_keyword() {
    let env = Env::new("priv-scope", &["Safari", "Calendar"]);
    let e = env.engine();
    let (pb, mut w) = (FakePasteboard::default(), Watcher::new());
    let clip = e.clipboard();
    w.step(&pb, &clip);
    pb.copy(&[TEXT], None, "plain note about hunter2 rotation");
    assert!(matches!(w.step(&pb, &clip), Step::Recorded(_)));

    for q in [
        "hunter2",
        "note about",
        "a",
        "saf",
        "snip hunter2",
        "open hunter2",
        "bm hunter2",
    ] {
        let rows = e.query(q);
        assert!(
            rows.iter()
                .all(|i| !i.id.starts_with("clip:") && !exposed(std::slice::from_ref(i)).contains("rotation")),
            "{q}: clipboard text leaked into {:?}",
            rows.iter().map(|i| i.title.clone()).collect::<Vec<_>>()
        );
    }
    // ...and it is there when the user asks for it
    let rows = e.query("clip hunter2");
    assert!(rows.iter().any(|i| i.title.contains("hunter2")), "{rows:?}");
}

#[test]
fn snippets_that_expand_the_clipboard_do_not_show_it_on_the_row() {
    let env = Env::new("priv-snip", &["Safari"]);
    std::fs::create_dir_all(&env.paths.data).unwrap();
    std::fs::write(
        env.paths.snippets_file(),
        serde_json::json!([
            {"name": "Direct", "keyword": "dd", "text": "pw={clipboard}"},
            {"name": "Indirect", "keyword": "ii", "text": "again {snippet:dd}"},
            {"name": "Older", "keyword": "oo", "text": "{clipboard:1}!"},
            {"name": "Static", "keyword": "ss", "text": "just text"}
        ])
        .to_string(),
    )
    .unwrap();
    let e = env.engine();
    assert!(e.clipboard().lock().unwrap().push(SECRET));
    assert!(e.clipboard().lock().unwrap().push("another-secret-value"));

    let rows = e.query("snip");
    assert_eq!(rows.len(), 4, "{:?}", rows.iter().map(|i| &i.title).collect::<Vec<_>>());
    for r in &rows {
        let shown = shown_on_screen(std::slice::from_ref(r));
        if r.title == "Static" {
            assert!(r.subtitle.contains("just text") && r.large_type.is_some());
        } else {
            assert!(
                !shown.contains("hunter2") && !shown.contains("another-secret"),
                "{}: {shown}",
                r.title
            );
            assert!(r.subtitle.contains("clipboard"), "{}", r.subtitle);
            // the explicit shortcuts (Large Type, copy) still expand it
            assert!(r.large_type.is_some() && r.copy_text.is_some(), "{}", r.title);
        }
    }
}
