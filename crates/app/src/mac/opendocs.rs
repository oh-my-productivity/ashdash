//! Double-clicked / dropped documents: `.alfredworkflow`, `.alfredappearance`, `.alfredsnippets`.
//!
//! Finder (or `open -a Ashdash.app file`) delivers the files to the running app as the
//! `odoc` Apple Event. [`install`] registers a handler for it; the handler hands the paths
//! to [`open_documents`], the one function that does the work:
//!
//! * workflows - confirmed with the user first (a workflow is a bundle of scripts that will
//!   run on this Mac), then `Engine::import_workflow`;
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
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ashdash_core::clipboard::snippets::{collections_dir, safe_file_stem};
use ashdash_core::config::write_private;
use ashdash_core::theme::Theme;
use ashdash_core::{Config, Engine, Paths, brand};
use anyhow::{Context, Result, bail};

/// More files than this in one event are ignored (a runaway drop is never intended).
const MAX_FILES_PER_EVENT: usize = 20;
/// Themes are small property lists; refuse anything big before parsing it.
const MAX_THEME_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Workflow,
    Appearance,
    Snippets,
}

/// Decide what a file is from its extension (case-insensitive). The reference launcher workflow
/// extensions are all the same zip format.
pub fn classify(path: &Path) -> Option<DocKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "alfredworkflow" | "alfred3workflow" | "alfred4workflow" | "alfred5workflow" => Some(DocKind::Workflow),
        "alfredappearance" => Some(DocKind::Appearance),
        "alfredsnippets" => Some(DocKind::Snippets),
        _ => None,
    }
}

/// The side effects of importing. Each method returns a one-line, user-facing result.
pub trait Importer {
    /// Ask whether a workflow may be installed. `false` cancels quietly.
    fn confirm_workflow(&self, file: &Path) -> bool;
    fn workflow(&self, file: &Path) -> Result<String>;
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
    if kind == DocKind::Workflow && !importer.confirm_workflow(path) {
        return Status::Cancelled;
    }
    let result = match kind {
        DocKind::Workflow => importer.workflow(path),
        DocKind::Snippets => importer.snippets(path),
        DocKind::Appearance => importer.appearance(path),
    };
    match result {
        Ok(msg) => Status::Imported(msg),
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
            let name = r
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match &r.status {
                Status::Imported(m) => m.clone(),
                Status::Failed(e) => format!("{name}: {e}"),
                Status::Cancelled => unreachable!("filtered above"),
            }
        })
        .collect();
    Some((title, lines.join("\n")))
}

/// Strip control characters and cap the length so a hostile file name cannot reshape a dialog.
pub fn display_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let clean: String = raw.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > 80 {
        let head: String = clean.chars().take(79).collect();
        format!("{head}…")
    } else {
        clean
    }
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
}

impl Importer for SystemImporter {
    fn confirm_workflow(&self, file: &Path) -> bool {
        // What the package says it is and what it can do, read without installing it.
        let preview = match ashdash_core::workflow::preview_package(file) {
            Ok(p) => p,
            Err(e) => {
                let title = format!("Cannot read {}", display_name(file));
                let _ = Command::new("osascript")
                    .args(alert_args(&title, &format!("{e:#}"), false))
                    .output();
                return false;
            }
        };
        let text = crate::gui::importprompt::prompt_text(&preview);
        let out = Command::new("osascript")
            .args(alert_args(&text.title, &text.lines.join("\n"), true))
            .output();
        match out {
            Ok(o) => dialog_confirmed(o.status.success(), &String::from_utf8_lossy(&o.stdout)),
            Err(_) => false, // no osascript: refuse rather than install unconfirmed
        }
    }

    fn workflow(&self, file: &Path) -> Result<String> {
        let dest = self.engine.import_workflow(file)?;
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(format!("Workflow installed ({name})"))
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
    let size = std::fs::metadata(file)?.len();
    if size > MAX_THEME_BYTES {
        bail!("the theme file is unusually large ({size} bytes)");
    }
    let theme = Theme::load(file).context("not a readable theme")?;
    let bytes = std::fs::read(file)?;
    let stem = safe_file_stem(if theme.name.is_empty() {
        file.file_stem().and_then(|s| s.to_str()).unwrap_or("theme")
    } else {
        &theme.name
    });
    let dest = paths.data.join("themes").join(format!("{stem}.alfredappearance"));
    write_private(&dest, &bytes).context("copying the theme")?;
    let mut config = Config::load(paths);
    config.theme = Some(dest);
    config.save(paths).context("saving config.json")?;
    THEME_EPOCH.fetch_add(1, Ordering::AcqRel);
    let shown = if theme.name.is_empty() {
        stem
    } else {
        theme.name.clone()
    };
    Ok(format!("Theme \"{shown}\" installed and applied."))
}

/// Post a user notification through `osascript` (title/body as argv, never as script text).
pub fn notify(title: &str, body: &str) {
    let _ = Command::new("osascript")
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
    let importer = Arc::new(SystemImporter { engine, paths });
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
        fn confirm_workflow(&self, file: &Path) -> bool {
            self.log.borrow_mut().push(format!("confirm {}", display_name(file)));
            self.allow
        }
        fn workflow(&self, file: &Path) -> Result<String> {
            self.log.borrow_mut().push(format!("workflow {}", display_name(file)));
            Ok("Workflow installed".into())
        }
        fn snippets(&self, file: &Path) -> Result<String> {
            self.log.borrow_mut().push(format!("snippets {}", display_name(file)));
            if self.fail_snippets {
                bail!("bad archive");
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
            "ab.alfredworkflow"
        );
        let long = format!("/x/{}.alfredworkflow", "n".repeat(300));
        let shown = display_name(Path::new(&long));
        assert_eq!(shown.chars().count(), 80);
        assert!(shown.ends_with('…'));
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
        assert!(install_theme(&paths, &big).unwrap_err().to_string().contains("large"));
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
        let msg = importer.workflow(&wf_pkg).unwrap();
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
}
