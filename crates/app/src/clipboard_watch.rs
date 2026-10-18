//! System pasteboard watcher: an `NSPasteboard` implementation of
//! [`PasteboardSource`] plus the polling thread.
//!
//! Only `changeCount` is read on every tick; contents are read after a change,
//! and only if the core filter lets the change through (concealed / transient /
//! auto-generated markers, per-app ignore list). All decisions live in
//! `ashdash_core::clipboard::watch` and are unit-tested there with a fake.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ashdash_core::Engine;
use ashdash_core::clipboard::history::DEFAULT_MAX_IMAGE_BYTES;
use ashdash_core::clipboard::watch::{PasteboardSource, Watcher, file_url_to_path};
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSWorkspace};
use objc2_foundation::{NSData, NSDictionary, NSString};

/// Poll interval. `changeCount` is a cheap integer read.
const POLL: Duration = Duration::from_millis(300);
/// Largest TIFF we are willing to convert to PNG (TIFF is uncompressed).
const MAX_TIFF_BYTES: usize = 64 << 20;

const UTI_TEXT: &str = "public.utf8-plain-text";
const UTI_PNG: &str = "public.png";
const UTI_TIFF: &str = "public.tiff";
const UTI_FILE_URL: &str = "public.file-url";

struct SystemPasteboard {
    board: Retained<NSPasteboard>,
}

impl SystemPasteboard {
    fn new() -> Self {
        SystemPasteboard {
            board: NSPasteboard::generalPasteboard(),
        }
    }

    fn data(&self, uti: &str) -> Option<Retained<NSData>> {
        self.board.dataForType(&NSString::from_str(uti))
    }
}

impl PasteboardSource for SystemPasteboard {
    fn change_count(&self) -> i64 {
        self.board.changeCount() as i64
    }

    fn types(&self) -> Vec<String> {
        // `types` describes the first item; marker types could in principle sit
        // on any item, so take the union.
        let mut out: Vec<String> = self
            .board
            .types()
            .map(|a| a.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        if let Some(items) = self.board.pasteboardItems() {
            for item in items.iter() {
                out.extend(item.types().iter().map(|s| s.to_string()));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn frontmost_bundle_id(&self) -> Option<String> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .map(|s| s.to_string())
    }

    fn read_files(&self) -> Vec<PathBuf> {
        let Some(items) = self.board.pasteboardItems() else {
            return Vec::new();
        };
        let key = NSString::from_str(UTI_FILE_URL);
        items
            .iter()
            .filter_map(|item| item.stringForType(&key))
            .filter_map(|url| file_url_to_path(&url.to_string()))
            .collect()
    }

    fn read_text(&self) -> Option<String> {
        self.board
            .stringForType(&NSString::from_str(UTI_TEXT))
            .map(|s| s.to_string())
    }

    fn read_png(&self) -> Option<Vec<u8>> {
        if let Some(png) = self.data(UTI_PNG) {
            // Refuse before copying anything large into our address space.
            return (png.len() <= DEFAULT_MAX_IMAGE_BYTES).then(|| png.to_vec());
        }
        let tiff = self.data(UTI_TIFF)?;
        if tiff.len() > MAX_TIFF_BYTES {
            return None;
        }
        let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
        // SAFETY: `properties` is an empty dictionary, valid for any file type.
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        (png.len() <= DEFAULT_MAX_IMAGE_BYTES).then(|| png.to_vec())
    }
}

/// Start the watcher thread. Content on the pasteboard at startup is not
/// recorded; later changes go through the core filter into the engine's history.
pub fn spawn(engine: Arc<Engine>) {
    let _ = std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || {
            let history = engine.clipboard();
            let board = SystemPasteboard::new();
            let mut watcher = Watcher::new();
            loop {
                // Each poll gets its own autorelease pool: this thread has no run loop.
                autoreleasepool(|_| {
                    watcher.step(&board, &history);
                });
                std::thread::sleep(POLL);
            }
        });
}

/// Live checks against the real pasteboard. They overwrite the clipboard with
/// synthetic values, so they are `#[ignore = "ci: ..."]`d and run on CI only
/// (`cargo test -p ashdash clipboard_watch -- --ignored --test-threads=1`, docs/TESTING.md).
/// The current clipboard is restored afterwards (all flavours of the first
/// item are saved in memory and written back).
#[cfg(test)]
mod live {
    use std::io::Write;
    use std::process::Stdio;
    use std::sync::Mutex;

    use ashdash_core::clipboard::filter::CONCEALED_TYPE;
    use ashdash_core::clipboard::watch::Step;
    use ashdash_core::clipboard::{ClipKind, ClipboardHistory, SkipReason, restore};

    use super::*;

    fn pbcopy(bytes: &[u8]) {
        let mut c = ashdash_core::actions::system_command("pbcopy")
            .env("LC_ALL", "en_US.UTF-8")
            .stdin(Stdio::piped())
            .spawn()
            .expect("pbcopy");
        c.stdin.take().unwrap().write_all(bytes).unwrap();
        c.wait().unwrap();
    }

    /// Copies every flavour of the current clipboard into memory (never to disk
    /// or the log) and puts it back when dropped.
    struct Guard(Vec<(String, Vec<u8>)>);

    impl Drop for Guard {
        fn drop(&mut self) {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            for (uti, bytes) in &self.0 {
                board.setData_forType(Some(&NSData::with_bytes(bytes)), &NSString::from_str(uti));
            }
        }
    }

    fn guard(src: &SystemPasteboard) -> Guard {
        let flavours = src
            .types()
            .into_iter()
            .filter_map(|t| src.data(&t).map(|d| (t, d.to_vec())))
            .collect();
        Guard(flavours)
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn set_types(board: &NSPasteboard, items: &[(&str, &[u8])]) {
        board.clearContents();
        for (uti, bytes) in items {
            let t = NSString::from_str(uti);
            if uti.starts_with("public.utf8") || *uti == CONCEALED_TYPE || *uti == "public.file-url" {
                assert!(board.setString_forType(&NSString::from_str(&String::from_utf8_lossy(bytes)), &t));
            } else {
                assert!(board.setData_forType(Some(&NSData::with_bytes(bytes)), &t));
            }
        }
    }

    fn tmp_history(tag: &str) -> (Mutex<ClipboardHistory>, PathBuf) {
        let root = std::env::temp_dir().join(format!("oa-live-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (
            Mutex::new(ClipboardHistory::load(root.join("clipboard.json"), 20)),
            root,
        )
    }

    #[test]
    #[ignore = "ci: overwrites the real clipboard with synthetic values"]
    fn text_concealed_image_tiff_files_and_restore() {
        let src = SystemPasteboard::new();
        let _restore_clipboard = guard(&src);
        let (history, root) = tmp_history("all");
        let mut w = Watcher::new();
        let text = format!("ashdash-live-test-{}", std::process::id());
        assert_eq!(w.step(&src, &history), Step::Unchanged, "baseline");

        // plain text via pbcopy
        pbcopy(text.as_bytes());
        assert_eq!(w.step(&src, &history), Step::Recorded(ClipKind::Text));
        assert_eq!(history.lock().unwrap().latest(), Some(text.as_str()));
        assert_eq!(w.step(&src, &history), Step::Unchanged, "no change, no read");

        // concealed marker next to the same kind of text
        set_types(
            &src.board,
            &[("public.utf8-plain-text", b"hunter2-live"), (CONCEALED_TYPE, b"")],
        );
        assert!(src.types().iter().any(|t| t == CONCEALED_TYPE), "{:?}", src.types());
        assert_eq!(
            w.step(&src, &history),
            Step::Skipped(SkipReason::MarkerType(CONCEALED_TYPE.into()))
        );
        assert!(
            history
                .lock()
                .unwrap()
                .entries()
                .iter()
                .all(|e| !e.text.contains("hunter2"))
        );

        // PNG
        let png4 = png(4, 3);
        set_types(&src.board, &[("public.png", &png4)]);
        assert_eq!(w.step(&src, &history), Step::Recorded(ClipKind::Image));
        assert_eq!(history.lock().unwrap().entries()[0].label(), "Image 4×3");

        // TIFF only: converted to PNG
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&png4)).expect("rep");
        let tiff = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::TIFF, &NSDictionary::new()) }
            .expect("tiff")
            .to_vec();
        set_types(&src.board, &[("public.tiff", &tiff)]);
        assert_eq!(w.step(&src, &history), Step::Recorded(ClipKind::Image));
        assert_eq!(history.lock().unwrap().entries()[0].label(), "Image 4×3");

        // file URL (with an escape)
        let file = std::env::temp_dir().join(format!("oa live file {}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        let url = format!("file://{}", file.to_string_lossy().replace(' ', "%20"));
        set_types(&src.board, &[("public.file-url", url.as_bytes())]);
        assert_eq!(w.step(&src, &history), Step::Recorded(ClipKind::Files));
        assert_eq!(
            history.lock().unwrap().entries()[0].files(),
            vec![file.to_string_lossy()]
        );

        // restore: file list
        let entry = history.lock().unwrap().entries()[0].clone();
        pbcopy(b"overwritten");
        restore::restore(&history.lock().unwrap(), &entry).expect("restore files");
        assert!(src.types().iter().any(|t| t == "public.file-url"), "{:?}", src.types());
        assert_eq!(src.read_files(), vec![file.clone()]);

        // restore: image
        let img = history
            .lock()
            .unwrap()
            .entries()
            .iter()
            .find(|e| e.kind == ClipKind::Image)
            .cloned()
            .unwrap();
        restore::restore(&history.lock().unwrap(), &img).expect("restore image");
        let back = src.read_png().expect("png on the pasteboard after restore");
        assert_eq!(ashdash_core::clipboard::history::png_dimensions(&back), Some((4, 3)));

        // restore: text with non-ASCII
        history.lock().unwrap().push("héllo → wörld ✓");
        let e = history.lock().unwrap().entries()[0].clone();
        restore::restore(&history.lock().unwrap(), &e).unwrap();
        assert_eq!(src.read_text().as_deref(), Some("héllo → wörld ✓"));

        // the frontmost app lookup does not crash (value depends on the machine)
        let _ = src.frontmost_bundle_id();
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "ci: overwrites the real clipboard with synthetic values"]
    fn change_count_increments_on_copy() {
        let src = SystemPasteboard::new();
        let _g = guard(&src);
        let before = src.change_count();
        pbcopy(b"ashdash-live-count");
        assert!(src.change_count() > before);
    }

    #[test]
    #[ignore = "ci: overwrites the real clipboard with synthetic values"]
    fn spawned_watcher_records_into_the_engine_history() {
        use ashdash_core::{Config, Paths};
        let src = SystemPasteboard::new();
        let _g = guard(&src);
        let root = std::env::temp_dir().join(format!("oa-live-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let engine = Arc::new(Engine::new(
            Paths::under(&root),
            Config {
                app_dirs: vec![],
                ..Config::default()
            },
        ));
        spawn(Arc::clone(&engine));
        std::thread::sleep(Duration::from_millis(800)); // baseline poll
        let text = format!("ashdash-live-engine-{}", std::process::id());
        pbcopy(text.as_bytes());
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(engine.clipboard().lock().unwrap().latest(), Some(text.as_str()));
        let rows = engine.query("clip ashdash-live-engine");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Pastes into a scratch TextEdit document (created and closed by the test,
    /// never saved). Needs Accessibility for the process running the tests and
    /// Automation permission for TextEdit; without Accessibility it checks the
    /// error message instead.
    #[test]
    #[ignore = "ci: drives TextEdit and posts real key events"]
    fn paste_into_scratch_textedit_document() {
        use ashdash_core::clipboard::paste;
        let osa = |script: &str| {
            let o = ashdash_core::actions::system_command("osascript")
                .args(["-e", script])
                .output()
                .expect("osascript");
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let src = SystemPasteboard::new();
        let _g = guard(&src);
        let trusted = paste::is_accessibility_trusted();
        println!("accessibility trusted: {trusted}");
        osa("tell application \"TextEdit\"\nactivate\nmake new document\nend tell");
        // A cold TextEdit launch can take seconds; never post keys until it is
        // frontmost with a document, or they land in another app.
        let ready = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            osa("tell application \"System Events\" to return name of first process whose frontmost is true")
                == "TextEdit"
                && osa("tell application \"TextEdit\" to return count of documents") != "0"
        });
        assert!(ready, "TextEdit did not become frontmost with a document");
        std::thread::sleep(Duration::from_millis(900));
        let result = paste::paste_text("ashdash-paste-test Dear , bye", Some(5), Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(500));
        // A second paste lands where the caret ended up, proving `{cursor}` placement.
        let second = if trusted {
            paste::paste_text("X", None, Duration::from_millis(250))
                .map(|_| std::thread::sleep(Duration::from_millis(400)))
        } else {
            Ok(())
        };
        let body = osa("tell application \"TextEdit\" to get text of document 1");
        osa("tell application \"TextEdit\"\nclose document 1 saving no\nend tell");
        println!("paste result: {result:?}; document text: {body:?}");
        if trusted {
            result.expect("paste");
            second.expect("second paste");
            assert_eq!(body, "ashdash-paste-test Dear X, bye");
        } else {
            assert_eq!(result, Err(paste::PasteError::NotTrusted));
            assert!(body.is_empty());
        }
    }
}
