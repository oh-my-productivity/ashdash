//! Double-clicked / dropped documents: `.alfredworkflow`, `.alfredappearance`, `.alfredsnippets`.
//!
//! Finder (or `open -a Ashdash.app file`) delivers the files to the running app as the
//! `odoc` Apple Event. [`install`] registers a handler for it; the handler hands the paths
//! to [`open_documents`], the one function that does the work:
//!
//! * workflows - the package is opened once, the user is asked about that staged copy (a
//!   workflow is a bundle of scripts that will run on this Mac) and, on Import, exactly that
//!   copy is installed (`Engine::stage_workflow` / `install_staged`); the file is not read again;
//! * snippets - `import_snippet_package` into the collections directory;
//! * themes - validated with `Theme::load`, copied to `<data>/themes/` and selected in
//!   `config.json`. The running window keeps its current theme until the next start.
//!
//! Everything touching the outside world goes through the [`Importer`] trait, so the
//! decision logic is tested with a fake; the real implementation is [`SystemImporter`].
//!
//! URL events (`ashdash://`) are WP9's `urlscheme.rs`; this file only deals with documents.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ashdash_core::clipboard::snippets::{collections_dir, safe_file_stem};
use ashdash_core::config::write_private;
use ashdash_core::theme::Theme;
use ashdash_core::{Config, Engine, Paths, brand};
use anyhow::{Context, Result};

use crate::gui::importprompt::clean;

/// More files than this in one event are ignored (a runaway drop is never intended).
const MAX_FILES_PER_EVENT: usize = 20;
/// Themes are small property lists; refuse anything big before parsing it.
const MAX_THEME_BYTES: u64 = ashdash_core::theme::MAX_THEME_BYTES;
/// One line of a notification: file names and error texts are cut and cleaned to this length.
const MAX_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Workflow,
    Appearance,
    Snippets,
}

/// Decide what a file is from its extension (case-insensitive). The reference launcher workflow
/// extensions are all the same zip format.
pub fn classify(path: &Path) -> Option<DocKind> {
    if ashdash_core::workflow::is_workflow_package(path) {
        return Some(DocKind::Workflow);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "alfredappearance" => Some(DocKind::Appearance),
        "alfredsnippets" => Some(DocKind::Snippets),
        _ => None,
    }
}

/// The side effects of importing. Each method returns a one-line, user-facing result.
pub trait Importer {
    /// Open a workflow package, ask whether it may be installed and install what was shown.
    /// `Ok(None)` means the user declined (cancelled quietly).
    fn workflow(&self, file: &Path) -> Result<Option<String>>;
    fn snippets(&self, file: &Path) -> Result<String>;
    fn appearance(&self, file: &Path) -> Result<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Imported(String),
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub path: PathBuf,
    pub status: Status,
}

/// Import every recognised file. Unknown extensions, duplicates and missing files are
/// reported, never silently dropped; an error on one file does not stop the others.
pub fn open_documents(importer: &dyn Importer, files: &[PathBuf]) -> Vec<Imported> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, path) in files.iter().enumerate() {
        if i >= MAX_FILES_PER_EVENT {
            out.push(Imported {
                path: path.clone(),
                status: Status::Failed(format!("skipped: more than {MAX_FILES_PER_EVENT} files at once")),
            });
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let status = import_one(importer, path);
        out.push(Imported {
            path: path.clone(),
            status,
        });
    }
    out
}

fn import_one(importer: &dyn Importer, path: &Path) -> Status {
    let Some(kind) = classify(path) else {
        return Status::Failed("not a workflow, theme or snippets file".into());
    };
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Status::Failed("not a regular file".into()),
        Err(e) => return Status::Failed(format!("cannot read it: {e}")),
    }
    let result = match kind {
        DocKind::Workflow => importer.workflow(path),
        DocKind::Snippets => importer.snippets(path).map(Some),
        DocKind::Appearance => importer.appearance(path).map(Some),
    };
    match result {
        Ok(Some(msg)) => Status::Imported(msg),
        Ok(None) => Status::Cancelled,
        Err(e) => Status::Failed(format!("{e:#}")),
    }
}

/// A short title/body pair for a notification. `None` when everything was cancelled.
pub fn summarize(results: &[Imported]) -> Option<(String, String)> {
    let shown: Vec<&Imported> = results.iter().filter(|r| r.status != Status::Cancelled).collect();
    if shown.is_empty() {
        return None;
    }
    let failed = shown.iter().filter(|r| matches!(r.status, Status::Failed(_))).count();
    let title = match (failed, shown.len()) {
        (0, _) => brand::NAME.to_string(),
        (f, n) if f == n => format!("{} could not import", brand::NAME),
        _ => format!("{} imported with problems", brand::NAME),
    };
    let lines: Vec<String> = shown
        .iter()
        .map(|r| {
            // File names, workflow names and error texts all come from the files: one line each,
            // no control, format (bidi, zero-width) or blank-looking characters, bounded length.
            let line = match &r.status {
                Status::Imported(m) => m.clone(),
                Status::Failed(e) => format!("{}: {e}", display_name(&r.path)),
                Status::Cancelled => unreachable!("filtered above"),
            };
            clean(&line, MAX_LINE_CHARS)
        })
        .collect();
    Some((title, lines.join("\n")))
}

/// A file name made safe to show: the import prompt's `clean` (control and format characters,
/// bidi overrides and blank-looking characters removed, whitespace collapsed) cut to 80 characters,
/// so a hostile file name cannot reshape or spoof a dialog.
pub fn display_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    clean(&raw, 80)
}

/// `osascript` arguments for a warning dialog. Title and message travel as argv after
/// the script (`on run argv`), so they are never parsed as code. With `ask` the dialog has
/// Cancel (default) and Import buttons, otherwise a single OK.
pub fn alert_args(title: &str, message: &str, ask: bool) -> Vec<String> {
    let buttons = if ask {
        "buttons {\"Cancel\", \"Import\"} default button \"Cancel\" cancel button \"Cancel\" giving up after 300"
    } else {
        "buttons {\"OK\"} default button \"OK\" giving up after 300"
    };
    [
        "-e",
        "on run argv",
        "-e",
        &format!("display alert (item 1 of argv) message (item 2 of argv) as warning {buttons}"),
        "-e",
        "end run",
        "--",
        title,
        message,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Did the `osascript` dialog end with the user pressing the confirm button?
/// (`display alert` prints `button returned:Import, gave up:false`; Cancel exits non-zero.)
pub fn dialog_confirmed(exit_ok: bool, stdout: &str) -> bool {
    exit_ok && stdout.contains("button returned:Import") && stdout.contains("gave up:false")
}

// ------------------------------------------------------------ the real importer

/// Imports into the real data directory and tells the engine about it.
pub struct SystemImporter {
    pub engine: Arc<Engine>,
    pub paths: Paths,
    /// Shows the confirmation and says whether the user agreed ([`osascript_dialog`] in the app;
    /// tests pass a fake so no dialog appears).
    pub ask: Ask,
}

/// Shows the import confirmation; `true` = the user chose Import.
pub type Ask = Box<dyn Fn(&crate::gui::importprompt::PromptText) -> bool + Send + Sync>;

/// The confirmation as a native alert (`osascript`). No `osascript`: refuse rather than install
/// unconfirmed.
pub fn osascript_dialog(text: &crate::gui::importprompt::PromptText) -> bool {
    let out = ashdash_core::actions::system_command("osascript")
        .args(alert_args(&text.title, &text.lines.join("\n"), true))
        .output();
    match out {
        Ok(o) => dialog_confirmed(o.status.success(), &String::from_utf8_lossy(&o.stdout)),
        Err(_) => false,
    }
}

impl Importer for SystemImporter {
    fn workflow(&self, file: &Path) -> Result<Option<String>> {
        // Open the package once: what the package says it is and what it can do is read from
        // the staged copy, and that same copy is installed if the user agrees.
        let staged = match self.engine.stage_workflow(file) {
            Ok(s) => s,
            Err(e) => {
                let title = format!("Cannot read {}", display_name(file));
                let _ = ashdash_core::actions::system_command("osascript")
                    .args(alert_args(&title, &format!("{e:#}"), false))
                    .output();
                return Ok(None);
            }
        };
        let text = crate::gui::importprompt::prompt_text(staged.preview());
        if !(self.ask)(&text) {
            return Ok(None); // dropping `staged` deletes the copy
        }
        let dest = self.engine.install_staged(staged)?;
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Some(format!("Workflow installed ({name})")))
    }

    fn snippets(&self, file: &Path) -> Result<String> {
        let dir = collections_dir(&self.paths.snippets_file());
        let report = ashdash_core::clipboard::import_snippet_package(file, &dir)?;
        self.engine.reload();
        Ok(format!(
            "Snippets \"{}\": {} imported{}",
            report.collection,
            report.imported,
            if report.skipped > 0 {
                format!(", {} skipped", report.skipped)
            } else {
                String::new()
            }
        ))
    }

    fn appearance(&self, file: &Path) -> Result<String> {
        install_theme(&self.paths, file)
    }
}

/// Bumped every time a theme was installed and selected, so a running window can
/// notice and apply it without a restart (`gui` compares it once per frame).
static THEME_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn theme_epoch() -> u64 {
    THEME_EPOCH.load(Ordering::Acquire)
}

/// Validate, copy to `<data>/themes/` and select a theme in `config.json`.
pub fn install_theme(paths: &Paths, file: &Path) -> Result<String> {
    install_theme_with(paths, file, |f| ashdash_core::theme::read_limited(f, MAX_THEME_BYTES))
}

/// [`install_theme`] with the file read injected. The file is read **once**: the bytes that are
/// validated are the bytes that are written, so a file swapped in between cannot be installed
/// unchecked (and a theme is never read three times).
fn install_theme_with(paths: &Paths, file: &Path, read: impl FnOnce(&Path) -> Result<Vec<u8>>) -> Result<String> {
    let bytes = read(file).with_context(|| format!("reading {}", display_name(file)))?;
    let theme = Theme::from_bytes(&bytes).context("not a readable theme")?;
    let stem = safe_file_stem(if theme.name.is_empty() {
        file.file_stem().and_then(|s| s.to_str()).unwrap_or("theme")
    } else {
        &theme.name
    });
    let dest = paths.data.join("themes").join(format!("{stem}.alfredappearance"));
    write_private(&dest, &bytes).context("copying the theme")?;
    Config::edit_locked(paths, |config| {
        config.theme = Some(dest);
        Ok(())
    })?;
    THEME_EPOCH.fetch_add(1, Ordering::AcqRel);
    let shown = if theme.name.is_empty() {
        stem
    } else {
        theme.name.clone()
    };
    Ok(format!("Theme \"{}\" installed and applied.", clean(&shown, 80)))
}

/// Post a user notification through `osascript` (title/body as argv, never as script text).
pub fn notify(title: &str, body: &str) {
    let _ = ashdash_core::actions::system_command("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            "--",
            title,
            body,
        ])
        .status();
}

// ------------------------------------------------------------ Apple Event glue

mod ae {
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send, sel};
    use objc2_foundation::NSString;

    /// 'aevt' / 'odoc': the "open documents" event.
    const CORE_EVENT_CLASS: u32 = 0x6165_7674;
    const OPEN_DOCUMENTS: u32 = 0x6F64_6F63;
    /// keyDirectObject, '----'.
    const KEY_DIRECT_OBJECT: u32 = 0x2D2D_2D2D;

    pub struct Ivars {
        pub on_files: Box<dyn Fn(Vec<PathBuf>)>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "AshdashOpenDocumentsHandler"]
        #[ivars = Ivars]
        struct Handler;

        impl Handler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(&self, event: &AnyObject, _reply: &AnyObject) {
                let files = files_in(event);
                if !files.is_empty() {
                    (self.ivars().on_files)(files);
                }
            }

            /// AppKit installs its own `odoc` handler while launching, which would replace one
            /// registered earlier. Registering from this notification (sent before any
            /// queued document event is dispatched) makes ours the last writer, so a file that
            /// launched the app is not lost.
            #[unsafe(method(applicationWillFinishLaunching:))]
            fn application_will_finish_launching(&self, _notification: &AnyObject) {
                attach(self);
            }
        }

        unsafe impl NSObjectProtocol for Handler {}
    );

    /// The file paths in the direct object of an `odoc` event (a list of file URLs).
    fn files_in(event: &AnyObject) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let list: Option<Retained<AnyObject>> =
            unsafe { msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
        let Some(list) = list else {
            return out;
        };
        let count: isize = unsafe { msg_send![&*list, numberOfItems] };
        for i in 1..=count {
            // Apple Event lists are 1-based. A one-item direct object may not be a list.
            let item: Option<Retained<AnyObject>> = unsafe { msg_send![&*list, descriptorAtIndex: i] };
            if let Some(path) = item.as_deref().and_then(path_of) {
                out.push(path);
            }
        }
        if count == 0
            && let Some(path) = path_of(&list)
        {
            out.push(path);
        }
        out
    }

    fn path_of(descriptor: &AnyObject) -> Option<PathBuf> {
        let url: Option<Retained<AnyObject>> = unsafe { msg_send![descriptor, fileURLValue] };
        let path: Option<Retained<NSString>> = unsafe { msg_send![&*url?, path] };
        Some(PathBuf::from(path?.to_string()))
    }

    thread_local! {
        // The event manager does not retain its handler; keep it alive for the whole run.
        static HANDLER: std::cell::RefCell<Option<Retained<Handler>>> = const { std::cell::RefCell::new(None) };
    }

    fn attach(handler: &Handler) {
        unsafe {
            let manager: Retained<AnyObject> = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
            let _: () = msg_send![
                &*manager,
                setEventHandler: handler,
                andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                forEventClass: CORE_EVENT_CLASS,
                andEventID: OPEN_DOCUMENTS
            ];
        }
    }

    pub fn register(on_files: Box<dyn Fn(Vec<PathBuf>)>) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let handler: Retained<Handler> = {
            let this = Handler::alloc(mtm).set_ivars(Ivars { on_files });
            unsafe { msg_send![super(this), init] }
        };
        // Now (covers installs after launch) and again when AppKit is about to finish launching.
        attach(&handler);
        unsafe {
            let center: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
            let _: () = msg_send![
                &*center,
                addObserver: &*handler,
                selector: sel!(applicationWillFinishLaunching:),
                name: &*name,
                object: None::<&AnyObject>
            ];
        }
        HANDLER.with(|h| *h.borrow_mut() = Some(handler));
        true
    }
}

/// Register for the open-documents Apple Event. Call once, on the main thread, before the
/// event loop starts. Imports run on a worker thread so the UI never blocks on a dialog.
/// Returns `false` when called off the main thread.
pub fn install(engine: Arc<Engine>) -> bool {
    let paths = engine.paths.clone();
    let importer = Arc::new(SystemImporter {
        engine,
        paths,
        ask: Box::new(osascript_dialog),
    });
    ae::register(Box::new(move |files| {
        let importer = Arc::clone(&importer);
        std::thread::spawn(move || {
            let results = open_documents(&*importer, &files);
            if let Some((title, body)) = summarize(&results) {
                notify(&title, &body);
            }
        });
    }))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::process::Command;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-opendocs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    }

    #[derive(Default)]
    struct Fake {
        allow: bool,
        fail_snippets: bool,
        log: RefCell<Vec<String>>,
    }

    impl Importer for Fake {
        fn workflow(&self, file: &Path) -> Result<Option<String>> {
            self.log.borrow_mut().push(format!("confirm {}", display_name(file)));
            if !self.allow {
                return Ok(None);
            }
            self.log.borrow_mut().push(format!("workflow {}", display_name(file)));
            Ok(Some("Workflow installed".into()))
        }
        fn snippets(&self, file: &Path) -> Result<String> {
            self.log.borrow_mut().push(format!("snippets {}", display_name(file)));
            if self.fail_snippets {
                anyhow::bail!("bad archive");
            }
            Ok("Snippets imported".into())
        }
        fn appearance(&self, file: &Path) -> Result<String> {
            self.log.borrow_mut().push(format!("appearance {}", display_name(file)));
            Ok("Theme installed".into())
        }
    }

    #[test]
    fn classification_by_extension() {
        let k = |s: &str| classify(Path::new(s));
        assert_eq!(k("/x/a.alfredworkflow"), Some(DocKind::Workflow));
        assert_eq!(k("/x/a.ALFRED5WORKFLOW"), Some(DocKind::Workflow));
        assert_eq!(k("a.alfred3workflow"), Some(DocKind::Workflow));
        assert_eq!(k("a.alfred4workflow"), Some(DocKind::Workflow));
        assert_eq!(k("a.alfredappearance"), Some(DocKind::Appearance));
        assert_eq!(k("a.alfredsnippets"), Some(DocKind::Snippets));
        assert_eq!(k("a.zip"), None);
        assert_eq!(k("alfredworkflow"), None);
        assert_eq!(k("a.alfredworkflow.txt"), None);
        assert_eq!(k("a.prefs"), None);
    }

    #[test]
    fn dispatches_each_kind_to_its_importer() {
        let dir = temp_dir("dispatch");
        let files = [
            touch(&dir, "w.alfredworkflow"),
            touch(&dir, "t.alfredappearance"),
            touch(&dir, "s.alfredsnippets"),
        ];
        let f = Fake {
            allow: true,
            ..Default::default()
        };
        let r = open_documents(&f, &files);
        assert!(r.iter().all(|i| matches!(i.status, Status::Imported(_))), "{r:?}");
        assert_eq!(
            *f.log.borrow(),
            vec![
                "confirm w.alfredworkflow",
                "workflow w.alfredworkflow",
                "appearance t.alfredappearance",
                "snippets s.alfredsnippets"
            ]
        );
    }

    #[test]
    fn declined_workflow_is_never_installed() {
        let dir = temp_dir("declined");
        let w = touch(&dir, "w.alfredworkflow");
        let f = Fake {
            allow: false,
            ..Default::default()
        };
        let r = open_documents(&f, &[w]);
        assert_eq!(r[0].status, Status::Cancelled);
        assert_eq!(*f.log.borrow(), vec!["confirm w.alfredworkflow"]);
        assert_eq!(summarize(&r), None, "cancelling is silent");
    }

    #[test]
    fn only_workflows_ask_for_confirmation() {
        let dir = temp_dir("noconfirm");
        let f = Fake::default(); // allow = false would block workflows only
        let r = open_documents(
            &f,
            &[touch(&dir, "a.alfredsnippets"), touch(&dir, "b.alfredappearance")],
        );
        assert!(r.iter().all(|i| matches!(i.status, Status::Imported(_))));
    }

    #[test]
    fn bad_inputs_are_reported_and_do_not_stop_the_rest() {
        let dir = temp_dir("bad");
        let ok = touch(&dir, "ok.alfredsnippets");
        let dir_named_like_doc = dir.join("folder.alfredsnippets");
        std::fs::create_dir_all(&dir_named_like_doc).unwrap();
        let files = vec![
            dir.join("missing.alfredworkflow"),
            touch(&dir, "notes.txt"),
            dir_named_like_doc,
            ok.clone(),
        ];
        let f = Fake {
            allow: true,
            ..Default::default()
        };
        let r = open_documents(&f, &files);
        assert!(matches!(&r[0].status, Status::Failed(m) if m.contains("cannot read")));
        assert!(matches!(&r[1].status, Status::Failed(m) if m.contains("not a workflow")));
        assert!(matches!(&r[2].status, Status::Failed(m) if m.contains("regular file")));
        assert_eq!(r[3].status, Status::Imported("Snippets imported".into()));
        assert_eq!(*f.log.borrow(), vec!["snippets ok.alfredsnippets"]);
    }

    #[test]
    fn importer_errors_become_failed_statuses() {
        let dir = temp_dir("fail");
        let f = Fake {
            fail_snippets: true,
            ..Default::default()
        };
        let r = open_documents(&f, &[touch(&dir, "s.alfredsnippets")]);
        assert_eq!(r[0].status, Status::Failed("bad archive".into()));
        let (title, body) = summarize(&r).unwrap();
        assert!(title.contains("could not import"));
        assert!(body.contains("s.alfredsnippets: bad archive"));
    }

    #[test]
    fn duplicates_are_collapsed_and_floods_are_capped() {
        let dir = temp_dir("dups");
        let a = touch(&dir, "a.alfredsnippets");
        let f = Fake::default();
        let r = open_documents(&f, &[a.clone(), a]);
        assert_eq!(r.len(), 1);
        assert_eq!(f.log.borrow().len(), 1);

        let many: Vec<PathBuf> = (0..MAX_FILES_PER_EVENT + 5)
            .map(|i| touch(&dir, &format!("n{i}.alfredsnippets")))
            .collect();
        let f = Fake::default();
        let r = open_documents(&f, &many);
        assert_eq!(f.log.borrow().len(), MAX_FILES_PER_EVENT);
        assert_eq!(r.len(), many.len());
        assert!(matches!(&r[MAX_FILES_PER_EVENT].status, Status::Failed(m) if m.contains("skipped")));
    }

    #[test]
    fn summary_titles() {
        let ok = Imported {
            path: "a".into(),
            status: Status::Imported("Done A".into()),
        };
        let bad = Imported {
            path: "/x/b.alfredsnippets".into(),
            status: Status::Failed("nope".into()),
        };
        assert_eq!(summarize(std::slice::from_ref(&ok)).unwrap().0, brand::NAME);
        assert!(
            summarize(&[ok.clone(), bad.clone()])
                .unwrap()
                .0
                .contains("with problems")
        );
        let (_, body) = summarize(&[ok, bad]).unwrap();
        assert_eq!(body, "Done A\nb.alfredsnippets: nope");
    }

    #[test]
    fn display_names_cannot_inject_lines_or_overflow() {
        assert_eq!(
            display_name(Path::new("/x/a\nb\u{7}.alfredworkflow")),
            "a b.alfredworkflow"
        );
        let long = format!("/x/{}.alfredworkflow", "n".repeat(300));
        let shown = display_name(Path::new(&long));
        assert_eq!(shown.chars().count(), 80);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn finder_messages_clean_file_names_like_the_import_prompt() {
        // bidi override, zero-width space, word joiner, Hangul filler, newline and tab in a name
        let nasty = "/x/inv\u{202e}gnp.\u{200b}ok\u{2060}\u{3164}\n\tnow.alfredsnippets";
        let shown = display_name(Path::new(nasty));
        assert!(
            !shown
                .chars()
                .any(|c| c.is_control() || crate::gui::importprompt::is_format_char(c) || c == '\u{3164}'),
            "{shown:?}"
        );
        // the notification body, for a failure and for an import message carrying the same text
        let bad = Imported {
            path: nasty.into(),
            status: Status::Failed("could not read \u{202e}evil\u{200b}\nSecond line".into()),
        };
        let good = Imported {
            path: "/x/ok".into(),
            status: Status::Imported("Workflow installed (a\u{202e}b\nc)".into()),
        };
        let (title, body) = summarize(&[good, bad]).unwrap();
        for text in [&title, &body] {
            assert!(
                text.chars()
                    .all(|c| c == '\n' || (!c.is_control() && !crate::gui::importprompt::is_format_char(c))),
                "{text:?}"
            );
        }
        // each result is still exactly one line, whatever the names contained
        assert_eq!(body.lines().count(), 2, "{body:?}");
        // a long error cannot flood the notification either
        let flood = Imported {
            path: "/x/y".into(),
            status: Status::Failed("e".repeat(5000)),
        };
        assert!(summarize(&[flood]).unwrap().1.chars().count() <= MAX_LINE_CHARS);
    }

    #[test]
    fn dialog_text_travels_as_arguments_never_as_script() {
        let nasty = "x\" & (do shell script \"touch /tmp/pwned\") & \"";
        let args = alert_args(nasty, "line1\nline2", true);
        let dashdash = args.iter().position(|a| a == "--").expect("separator");
        assert_eq!(&args[dashdash + 1..], [nasty, "line1\nline2"]);
        // nothing before the separator contains the untrusted text
        assert!(
            args[..dashdash]
                .iter()
                .all(|a| !a.contains("pwned") && !a.contains("line1"))
        );
        assert!(args.join(" ").contains("buttons {\"Cancel\", \"Import\"}"));
        assert!(alert_args("t", "m", false).join(" ").contains("buttons {\"OK\"}"));
    }

    #[test]
    fn dialog_answer_parsing() {
        assert!(dialog_confirmed(true, "button returned:Import, gave up:false\n"));
        assert!(!dialog_confirmed(false, "button returned:Import, gave up:false\n"));
        assert!(!dialog_confirmed(true, "button returned:Cancel, gave up:false\n"));
        assert!(!dialog_confirmed(true, "button returned:Import, gave up:true\n"));
        assert!(!dialog_confirmed(true, ""));
    }

    const THEME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>alfredtheme</key><dict>
<key>name</key><string>Night/Owl</string>
<key>window</key><dict><key>color</key><string>#112233FF</string><key>width</key><real>640</real></dict>
</dict></dict></plist>"#;

    #[test]
    fn theme_install_copies_validates_and_selects() {
        let root = temp_dir("theme");
        let paths = Paths::under(&root);
        let src = root.join("download.alfredappearance");
        std::fs::write(&src, THEME_XML).unwrap();
        let msg = install_theme(&paths, &src).unwrap();
        assert!(msg.contains("Night/Owl"), "{msg}");

        let config = Config::load(&paths);
        let installed = config.theme.expect("theme selected");
        assert!(installed.starts_with(paths.data.join("themes")));
        assert!(
            !installed.to_string_lossy().contains("Night/Owl"),
            "name must be sanitised"
        );
        assert_eq!(Theme::load(&installed).unwrap().name, "Night/Owl");

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn theme_install_reads_the_file_once_and_installs_what_it_validated() {
        let root = temp_dir("theme-once");
        let paths = Paths::under(&root);
        let src = root.join("download.alfredappearance");
        std::fs::write(&src, "swapped in after the check: not even a plist").unwrap();
        let reads = std::cell::Cell::new(0);
        let msg = install_theme_with(&paths, &src, |_| {
            reads.set(reads.get() + 1);
            Ok(THEME_XML.as_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(reads.get(), 1, "one read of the source");
        assert!(msg.contains("Night/Owl"), "{msg}");
        let installed = Config::load(&paths).theme.unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), THEME_XML.as_bytes());
        // a read error is reported and leaves the config alone
        let root = temp_dir("theme-once-err");
        let paths = Paths::under(&root);
        assert!(install_theme_with(&paths, &src, |_| anyhow::bail!("gone")).is_err());
        assert!(Config::load(&paths).theme.is_none());
    }

    #[test]
    fn a_deeply_nested_theme_is_refused_not_fatal() {
        let root = temp_dir("theme-deep");
        let paths = Paths::under(&root);
        let src = root.join("deep.alfredappearance");
        let depth = 120_000;
        std::fs::write(
            &src,
            format!(
                r#"<?xml version="1.0"?><plist version="1.0"><dict><key>alfredtheme</key><dict><key>x</key>{}{}</dict></dict></plist>"#,
                "<array>".repeat(depth),
                "</array>".repeat(depth)
            ),
        )
        .unwrap();
        assert!(install_theme(&paths, &src).is_err());
        assert!(Config::load(&paths).theme.is_none());
    }

    #[test]
    fn theme_install_waits_for_the_config_lock() {
        let root = temp_dir("theme-lock");
        let paths = Paths::under(&root);
        let src = root.join("download.alfredappearance");
        std::fs::write(&src, THEME_XML).unwrap();
        let guard = ashdash_core::config::ConfigLock::acquire(&paths).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (p2, s2) = (paths.clone(), src.clone());
        std::thread::spawn(move || {
            let _ = tx.send(install_theme(&p2, &s2).is_ok());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(),
            "the theme was written while another writer held the config lock"
        );
        drop(guard);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
        assert!(Config::load(&paths).theme.is_some());
    }

    #[test]
    fn theme_install_rejects_garbage_without_touching_config() {
        let root = temp_dir("theme-bad");
        let paths = Paths::under(&root);
        let src = root.join("x.alfredappearance");
        std::fs::write(&src, "not a plist").unwrap();
        assert!(install_theme(&paths, &src).is_err());
        assert!(!paths.config_file().exists());
        assert!(Config::load(&paths).theme.is_none());

        let big = root.join("big.alfredappearance");
        std::fs::write(&big, vec![b'x'; (MAX_THEME_BYTES + 1) as usize]).unwrap();
        assert!(format!("{:#}", install_theme(&paths, &big).unwrap_err()).contains("large"));
    }

    /// End to end through the real importer with a sample package built by /usr/bin/zip.
    #[test]
    fn real_importer_installs_a_workflow_and_snippets() {
        if Command::new("zip").arg("-v").output().is_err() {
            eprintln!("skipping: no zip tool");
            return;
        }
        let root = temp_dir("real");
        let paths = Paths::under(&root);
        let engine = Arc::new(Engine::new(paths.clone(), Config::default()));
        let importer = SystemImporter {
            engine: Arc::clone(&engine),
            paths: paths.clone(),
            ask: Box::new(|_| true),
        };

        // Workflow package: a zip with info.plist at the top level.
        let wf_dir = root.join("wf-src");
        std::fs::create_dir_all(&wf_dir).unwrap();
        std::fs::write(
            wf_dir.join("info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>name</key><string>Opendocs Sample</string>
<key>bundleid</key><string>test.opendocs.sample</string><key>objects</key><array/></dict></plist>"#,
        )
        .unwrap();
        let wf_pkg = root.join("sample.alfredworkflow");
        let status = Command::new("zip")
            .current_dir(&wf_dir)
            .arg("-q")
            .arg(&wf_pkg)
            .arg("info.plist")
            .status()
            .unwrap();
        assert!(status.success());
        let msg = importer.workflow(&wf_pkg).unwrap().expect("confirmed");
        assert!(msg.contains("test.opendocs.sample"), "{msg}");
        assert!(
            engine
                .workflow_summaries()
                .iter()
                .any(|(_, b, _)| b == "test.opendocs.sample")
        );

        // Snippets package: one snippet JSON per file, in the documented reference launcher layout.
        let sn_dir = root.join("sn-src");
        std::fs::create_dir_all(&sn_dir).unwrap();
        std::fs::write(
            sn_dir.join("one [AAAA-1].json"),
            r#"{"alfredsnippet":{"snippet":"hello there","uid":"AAAA-1","name":"Greeting","keyword":"hi"}}"#,
        )
        .unwrap();
        let sn_pkg = root.join("Samples.alfredsnippets");
        let status = Command::new("zip")
            .current_dir(&sn_dir)
            .arg("-q")
            .arg(&sn_pkg)
            .arg("one [AAAA-1].json")
            .status()
            .unwrap();
        assert!(status.success());
        let msg = importer.snippets(&sn_pkg).unwrap();
        assert!(msg.contains("Samples") && msg.contains("1 imported"), "{msg}");
    }

    /// A workflow package for the Finder path, built with /usr/bin/zip (`false` when there is none).
    fn write_workflow_zip(path: &Path, name: &str, bundle: &str, marker: &str) -> bool {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>name</key><string>{name}</string>
<key>bundleid</key><string>{bundle}</string><key>objects</key><array>
<dict><key>uid</key><string>A</string><key>type</key><string>alfred.workflow.action.script</string>
<key>config</key><dict><key>type</key><integer>0</integer><key>script</key><string>echo {marker}</string></dict></dict>
</array></dict></plist>"#
        );
        let src = path.with_extension("src");
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("info.plist"), plist).unwrap();
        let _ = std::fs::remove_file(path);
        Command::new("zip")
            .current_dir(&src)
            .arg("-q")
            .arg(path)
            .arg("info.plist")
            .status()
            .is_ok_and(|s| s.success())
    }

    /// The Finder dialog stays up for minutes; whatever happens to the file meanwhile, the
    /// installed workflow is the one the dialog described.
    #[test]
    fn finder_import_installs_the_copy_the_dialog_described() {
        let root = temp_dir("swap");
        let paths = Paths::under(&root);
        let engine = Arc::new(Engine::new(paths.clone(), Config::default()));
        let pkg = root.join("w.alfredworkflow");
        if !write_workflow_zip(&pkg, "Harmless", "test.swap.harmless", "SHOWN") {
            eprintln!("skipping: no zip tool");
            return;
        }

        let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (swap_pkg, log) = (pkg.clone(), Arc::clone(&asked));
        let importer = SystemImporter {
            engine: Arc::clone(&engine),
            paths: paths.clone(),
            ask: Box::new(move |text| {
                log.lock().unwrap().push(text.plain());
                // the user is still reading; the file on disk is replaced by something else
                assert!(write_workflow_zip(&swap_pkg, "Evil", "test.swap.evil", "SWAPPED"));
                true
            }),
        };
        let msg = importer.workflow(&pkg).unwrap().expect("confirmed");
        assert!(msg.contains("test.swap.harmless"), "{msg}");
        let shown = asked.lock().unwrap().concat();
        assert!(shown.contains("Harmless") && !shown.contains("Evil"), "{shown}");
        let wf = paths.workflows_dir();
        assert!(wf.join("user.workflow.test.swap.harmless").is_dir());
        assert!(!wf.join("user.workflow.test.swap.evil").exists());
        let text = std::fs::read_to_string(wf.join("user.workflow.test.swap.harmless/info.plist")).unwrap();
        assert!(text.contains("SHOWN") && !text.contains("SWAPPED"), "{text}");
    }

    #[test]
    fn declining_the_dialog_installs_nothing_and_leaves_no_staged_copy() {
        let root = temp_dir("decline");
        let paths = Paths::under(&root);
        let engine = Arc::new(Engine::new(paths.clone(), Config::default()));
        let pkg = root.join("w.alfredworkflow");
        if !write_workflow_zip(&pkg, "No", "test.decline", "x") {
            eprintln!("skipping: no zip tool");
            return;
        }
        let importer = SystemImporter {
            engine,
            paths: paths.clone(),
            ask: Box::new(|_| false),
        };
        assert_eq!(importer.workflow(&pkg).unwrap(), None);
        assert!(!paths.workflows_dir().join("user.workflow.test.decline").exists());
        let staging = paths.data.join("import-staging");
        assert_eq!(std::fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0), 0);
    }
}
