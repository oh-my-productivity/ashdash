//! `clip` / `clipboard` history search.

use super::{Provider, ProviderOutput, QueryCtx, strip_keyword, truncate};
use crate::clipboard::{ClipCommand, ClipEntry, ClipKind, ClipboardHistory};
use crate::item::{Action, Icon, Item};

/// Image entries up to this size are shown with their own thumbnail; larger
/// ones fall back to a glyph so the result list never decodes huge files.
const MAX_THUMBNAIL_BYTES: u64 = 512 * 1024;

pub struct ClipboardProvider;

/// The one place that decides how a clipboard command is exposed as an [`Action`].
pub(super) fn clip_action(cmd: ClipCommand) -> Action {
    Action::Clipboard(cmd)
}

/// "just now", "5 min ago", "3 h ago", "2 d ago".
fn age(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn kb(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

fn subtitle(e: &ClipEntry, now: u64) -> String {
    let what = match e.kind {
        ClipKind::Text => format!(
            "{} chars, {} line(s)",
            e.text.chars().count(),
            e.text.lines().count().max(1)
        ),
        ClipKind::Image => match &e.image {
            Some(i) => format!("image {}×{}, {}", i.width, i.height, kb(i.bytes)),
            None => "image".into(),
        },
        ClipKind::Files => format!("{} file(s)", e.files().len()),
    };
    let pin = if e.pinned { "pinned · " } else { "" };
    format!("{pin}{what} · {} — ↩ to copy", age(now.saturating_sub(e.ts)))
}

/// Build the result row for one history entry.
fn item_for(history: &ClipboardHistory, e: &ClipEntry, rank: usize, now: u64) -> Item {
    let id = e.id();
    let mut it = Item::new(format!("clip:{id}"), truncate(&e.label(), 120))
        .subtitle(subtitle(e, now))
        .icon(Icon::Glyph("⎘"))
        .score(10_000.0 - rank as f64);
    match e.kind {
        ClipKind::Text => {
            // Return pastes into the frontmost app (copies only, with a hint, when Accessibility is missing);
            // Cmd+Return just puts it back on the clipboard.
            it = it.action(clip_action(ClipCommand::Paste { id: id.clone() }));
            it.cmd_action = Some(clip_action(ClipCommand::Restore { id }));
            it.copy_text = Some(e.text.clone());
            it.large_type = Some(e.text.clone());
        }
        ClipKind::Image => {
            let path = history.image_path(e);
            if let (Some(p), Some(img)) = (&path, &e.image)
                && img.bytes <= MAX_THUMBNAIL_BYTES
            {
                it = it.icon(Icon::Image(p.clone()));
            }
            it = it.action(clip_action(ClipCommand::Restore { id }));
        }
        ClipKind::Files => {
            it = it.action(clip_action(ClipCommand::Restore { id }));
            it.large_type = Some(e.text.clone());
        }
    }
    it
}

/// `clip clear` / `clip clear all`: the history-wiping rows (confirmed by the engine before running).
fn clear_row(rest: &str) -> Option<Item> {
    let (cmd, title, sub) = match rest.trim().to_ascii_lowercase().as_str() {
        "clear" => (
            ClipCommand::Clear,
            "Clear clipboard history",
            "Removes everything except pinned entries",
        ),
        "clear all" => (
            ClipCommand::ClearAll,
            "Clear clipboard history including pinned",
            "Removes every entry",
        ),
        _ => return None,
    };
    Some(
        Item::new(
            format!(
                "clip-{}",
                if cmd == ClipCommand::Clear {
                    "clear"
                } else {
                    "clear-all"
                }
            ),
            title,
        )
        .subtitle(sub)
        .icon(Icon::Glyph("⎘"))
        .action(clip_action(cmd))
        .score(20_000.0),
    )
}

impl Provider for ClipboardProvider {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query;
        let Some(rest) = strip_keyword(q, "clipboard").or_else(|| strip_keyword(q, "clip")) else {
            return ProviderOutput::none();
        };
        let st = ctx.engine_state;
        let history = st.clipboard.lock().unwrap();
        let hits = history.search(rest, st.config.max_results);
        let now = crate::clipboard::now();
        let mut items: Vec<Item> = clear_row(rest).into_iter().collect();
        items.extend(hits.iter().enumerate().map(|(i, e)| item_for(&history, e, i, now)));
        if items.is_empty() {
            items.push(Item::new("noclip", "Clipboard history is empty").valid(false));
        }
        ProviderOutput::exclusive(items)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::testutil::engine;
    use super::*;
    use crate::clipboard::history::tests::fake_png;

    #[test]
    fn clip_keyword_is_exclusive_even_when_empty() {
        let (e, root) = engine("clip");
        let r = e.query("clip");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "noclip");
        // "Clipper.app" would match a general search for "clip"; it must not here.
        assert!(r.iter().all(|i| !i.id.starts_with("app:")));
        e.clipboard().lock().unwrap().push("hello world");
        let r = e.query("clipboard hello");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "hello world");
        let id = e.clipboard().lock().unwrap().entries()[0].id();
        assert_eq!(r[0].action, Action::Clipboard(ClipCommand::Paste { id: id.clone() }));
        assert_eq!(r[0].cmd_action, Some(Action::Clipboard(ClipCommand::Restore { id })));
        assert_eq!(r[0].copy_text.as_deref(), Some("hello world"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rows_for_every_kind() {
        let (e, root) = engine("clipkinds");
        {
            let clip = e.clipboard();
            let mut h = clip.lock().unwrap();
            assert!(h.push("plain text\nsecond line"));
            assert!(h.push_files(&[PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]));
            assert!(h.push_image(&fake_png(640, 480, 5)));
            let id = h.entries().iter().find(|e| e.text.starts_with("plain")).unwrap().id();
            h.set_pinned(&id, true);
        }
        let r = e.query("clip");
        assert_eq!(r.len(), 3);

        // Pinned text sorts first on an empty query.
        assert_eq!(r[0].title, "plain text");
        assert!(
            r[0].subtitle.starts_with("pinned · 22 chars, 2 line(s)"),
            "{}",
            r[0].subtitle
        );
        assert_eq!(r[0].large_type.as_deref(), Some("plain text\nsecond line"));

        let img = r.iter().find(|i| i.title == "Image 640×480").expect("image row");
        assert!(img.subtitle.contains("image 640×480"), "{}", img.subtitle);
        assert!(matches!(img.icon, Icon::Image(_)), "small image uses its own thumbnail");
        assert!(
            matches!(img.action, Action::Clipboard(ClipCommand::Restore { .. })),
            "{:?}",
            img.action
        );

        let files = r.iter().find(|i| i.title == "a.txt and 1 more").expect("files row");
        assert!(files.subtitle.starts_with("2 file(s)"), "{}", files.subtitle);
        assert!(matches!(files.action, Action::Clipboard(ClipCommand::Restore { .. })));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn age_and_size_formatting() {
        assert_eq!(age(0), "just now");
        assert_eq!(age(59), "just now");
        assert_eq!(age(60), "1 min ago");
        assert_eq!(age(7_200), "2 h ago");
        assert_eq!(age(200_000), "2 d ago");
        assert_eq!(kb(1), "1 KB");
        assert_eq!(kb(2048), "2 KB");
        assert_eq!(kb(3 << 20), "3.0 MB");
    }

    #[test]
    fn large_images_do_not_get_thumbnails() {
        let (e, root) = engine("clipbig");
        {
            let clip = e.clipboard();
            let mut h = clip.lock().unwrap();
            let mut big = fake_png(4000, 3000, 1);
            big.extend(vec![0u8; MAX_THUMBNAIL_BYTES as usize + 1]);
            assert!(h.push_image(&big));
        }
        let r = e.query("clip image");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].icon, Icon::Glyph("⎘"));
        let _ = std::fs::remove_dir_all(root);
    }
}
