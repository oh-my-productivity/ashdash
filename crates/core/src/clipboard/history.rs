//! The clipboard history store: text, image and file-list entries, dedupe,
//! retention (count and age), pinning and persistence.
//!
//! Storage: a JSON array (`clipboard.json`, mode 0600) next to a private
//! `clipboard-images/` directory (mode 0700) that holds PNG files (mode 0600)
//! referenced by name from the JSON. Older files that only contain
//! `{text, ts}` objects load unchanged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::filter::DEFAULT_IGNORED_APPS;
use super::now;
use crate::config::write_private;
use crate::fuzzy;

/// Longest text accepted for one entry (1 MiB). Larger copies are not recorded.
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// Default cap on the PNG size of one image entry (8 MiB).
pub const DEFAULT_MAX_IMAGE_BYTES: usize = 8 << 20;
/// Most paths kept for one file-list entry.
pub const MAX_FILES_PER_ENTRY: usize = 500;

const IMAGES_DIR: &str = "clipboard-images";
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    #[default]
    Text,
    Image,
    /// A list of file paths (copied in Finder). `ClipEntry::text` holds one
    /// path per line.
    Files,
}

impl ClipKind {
    fn is_text(&self) -> bool {
        *self == ClipKind::Text
    }
}

/// Reference to a PNG stored in the images directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageRef {
    /// File name inside the images directory (`<16 hex digits>.png`).
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipEntry {
    /// Text content; for [`ClipKind::Files`] the paths, one per line; empty
    /// for images.
    pub text: String,
    pub ts: u64,
    #[serde(default, skip_serializing_if = "ClipKind::is_text")]
    pub kind: ClipKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRef>,
    /// Pinned entries are exempt from retention and from `clear`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// FNV-1a 64: a stable, dependency-free content hash (dedupe and file names,
/// not security).
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Only accept file names we generate ourselves, so a tampered JSON file can
/// never make us read or delete anything outside the images directory.
fn is_safe_image_name(name: &str) -> bool {
    name.len() == 20
        && name.ends_with(".png")
        && name[..16]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl ClipEntry {
    fn text_entry(text: &str, ts: u64) -> Self {
        ClipEntry {
            text: text.to_string(),
            ts,
            kind: ClipKind::Text,
            image: None,
            pinned: false,
        }
    }

    /// Identity used for dedupe: same kind and same content.
    pub fn key(&self) -> String {
        match (&self.kind, &self.image) {
            (ClipKind::Image, Some(i)) => format!("i:{}", i.file),
            (ClipKind::Image, None) => "i:".into(),
            (ClipKind::Files, _) => format!("f:{}", self.text),
            (ClipKind::Text, _) => format!("t:{}", self.text),
        }
    }

    /// Stable id for commands (`restore`, `pin`, ...). Unique per entry because
    /// the history never holds two entries with the same [`key`](Self::key).
    pub fn id(&self) -> String {
        format!("{:016x}", fnv1a(self.key().as_bytes()))
    }

    /// File paths of a [`ClipKind::Files`] entry.
    pub fn files(&self) -> Vec<&str> {
        if self.kind == ClipKind::Files {
            self.text.lines().filter(|l| !l.is_empty()).collect()
        } else {
            Vec::new()
        }
    }

    /// Short human title for result rows.
    pub fn label(&self) -> String {
        match self.kind {
            ClipKind::Text => self
                .text
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string(),
            ClipKind::Image => match &self.image {
                Some(i) => format!("Image {}×{}", i.width, i.height),
                None => "Image".into(),
            },
            ClipKind::Files => {
                let files = self.files();
                let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
                match files.as_slice() {
                    [] => "Files".into(),
                    [one] => name(one),
                    [first, rest @ ..] => format!("{} and {} more", name(first), rest.len()),
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct ClipboardHistory {
    entries: Vec<ClipEntry>, // newest first
    max: usize,
    max_age_secs: Option<u64>,
    max_image_bytes: Option<usize>,
    ignored_apps: Option<Vec<String>>,
    path: Option<PathBuf>,
}

impl ClipboardHistory {
    pub fn load(path: PathBuf, max: usize) -> Self {
        let mut entries: Vec<ClipEntry> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let dir = images_dir_for(&path);
        // Drop image entries whose file is gone or whose name is not ours.
        entries.retain(|e| match (&e.kind, &e.image) {
            (ClipKind::Image, Some(i)) => is_safe_image_name(&i.file) && dir.join(&i.file).is_file(),
            (ClipKind::Image, None) => false,
            _ => true,
        });
        ClipboardHistory {
            entries,
            max,
            path: Some(path),
            ..Default::default()
        }
    }

    pub fn in_memory(max: usize) -> Self {
        ClipboardHistory {
            max,
            ..Default::default()
        }
    }

    // ------------------------------------------------------------ settings

    /// Set the retention policy and apply it immediately. `max_entries` bounds
    /// the number of *unpinned* entries; `max_age_secs` drops unpinned entries
    /// older than that. Pinned entries are never removed by retention.
    pub fn set_retention(&mut self, max_entries: usize, max_age_secs: Option<u64>) {
        self.max = max_entries;
        self.max_age_secs = max_age_secs.filter(|s| *s > 0);
        if self.prune_at(now()) > 0 {
            self.save();
        }
    }

    /// Convenience for a "keep N days" setting; `None` or `0` keeps forever.
    pub fn set_max_age_days(&mut self, days: Option<u32>) {
        self.set_retention(self.max, days.map(|d| d as u64 * 86_400));
    }

    /// Bundle ids of apps whose copies are never recorded (see
    /// [`filter::should_record`](super::filter::should_record)). Until this is
    /// called, [`DEFAULT_IGNORED_APPS`] applies; pass an empty list to ignore none.
    pub fn set_ignored_apps(&mut self, apps: Vec<String>) {
        self.ignored_apps = Some(apps);
    }

    /// The effective per-app ignore list.
    pub fn ignored_apps(&self) -> Vec<String> {
        match &self.ignored_apps {
            Some(list) => list.clone(),
            None => DEFAULT_IGNORED_APPS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn set_max_image_bytes(&mut self, bytes: usize) {
        self.max_image_bytes = Some(bytes);
    }

    pub fn max_image_bytes(&self) -> usize {
        self.max_image_bytes.unwrap_or(DEFAULT_MAX_IMAGE_BYTES)
    }

    /// Directory holding the PNG files; `None` for an in-memory history.
    pub fn images_dir(&self) -> Option<PathBuf> {
        self.path.as_deref().map(images_dir_for)
    }

    // ------------------------------------------------------------ recording

    /// Record text. Returns false for blank, oversized or duplicate-of-latest.
    pub fn push(&mut self, text: &str) -> bool {
        self.push_text_at(text, now())
    }

    pub(crate) fn push_text_at(&mut self, text: &str, ts: u64) -> bool {
        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
            return false;
        }
        self.insert(ClipEntry::text_entry(text, ts), ts)
    }

    /// Record a list of files. Paths containing a newline are refused (they
    /// could not be restored faithfully).
    pub fn push_files(&mut self, paths: &[PathBuf]) -> bool {
        self.push_files_at(paths, now())
    }

    pub(crate) fn push_files_at(&mut self, paths: &[PathBuf], ts: u64) -> bool {
        let list: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty() && !p.contains('\n') && !p.contains('\r'))
            .take(MAX_FILES_PER_ENTRY)
            .collect();
        if list.is_empty() {
            return false;
        }
        self.insert(
            ClipEntry {
                text: list.join("\n"),
                ts,
                kind: ClipKind::Files,
                image: None,
                pinned: false,
            },
            ts,
        )
    }

    /// Record a PNG image (stored under the images directory). Returns false
    /// when the data is not a PNG, exceeds the size cap, the history is
    /// in-memory, or the image is already the latest entry.
    pub fn push_image(&mut self, png: &[u8]) -> bool {
        self.push_image_at(png, now())
    }

    pub(crate) fn push_image_at(&mut self, png: &[u8], ts: u64) -> bool {
        let Some(dir) = self.images_dir() else {
            return false;
        };
        let Some((width, height)) = png_dimensions(png) else {
            return false;
        };
        if png.len() > self.max_image_bytes() || width == 0 || height == 0 {
            return false;
        }
        let file = format!("{:016x}.png", fnv1a(png));
        let target = dir.join(&file);
        let same_on_disk = std::fs::metadata(&target).is_ok_and(|m| m.len() == png.len() as u64);
        if !same_on_disk && write_image(&dir, &target, png).is_err() {
            return false;
        }
        self.insert(
            ClipEntry {
                text: String::new(),
                ts,
                kind: ClipKind::Image,
                image: Some(ImageRef {
                    file,
                    width,
                    height,
                    bytes: png.len() as u64,
                }),
                pinned: false,
            },
            ts,
        )
    }

    /// Put `entry` first (replacing an older entry with the same content and
    /// inheriting its pin), then apply retention and persist.
    fn insert(&mut self, mut entry: ClipEntry, ts: u64) -> bool {
        let key = entry.key();
        if self.entries.first().is_some_and(|e| e.key() == key) {
            return false;
        }
        if let Some(pos) = self.entries.iter().position(|e| e.key() == key) {
            entry.pinned = self.entries.remove(pos).pinned;
        }
        self.entries.insert(0, entry);
        self.prune_at(ts);
        self.save();
        true
    }

    // ------------------------------------------------------------ retention

    /// Apply the retention policy using the current time; returns how many
    /// entries were removed. Does not persist (callers that need it use
    /// [`set_retention`](Self::set_retention) or record something).
    pub fn prune(&mut self) -> usize {
        self.prune_at(now())
    }

    pub(crate) fn prune_at(&mut self, now: u64) -> usize {
        let max = self.max.max(1);
        let max_age = self.max_age_secs;
        let mut unpinned = 0usize;
        let mut doomed = Vec::new();
        let mut keep = Vec::with_capacity(self.entries.len());
        for e in std::mem::take(&mut self.entries) {
            let drop = !e.pinned && {
                unpinned += 1;
                unpinned > max || max_age.is_some_and(|a| now.saturating_sub(e.ts) > a)
            };
            if drop {
                doomed.push(e);
            } else {
                keep.push(e);
            }
        }
        self.entries = keep;
        let n = doomed.len();
        for e in &doomed {
            self.delete_files(e);
        }
        n
    }

    /// Remove everything that is not pinned (the `clip clear` command).
    /// Returns the number of removed entries.
    pub fn clear(&mut self) -> usize {
        let (pinned, doomed): (Vec<_>, Vec<_>) = std::mem::take(&mut self.entries).into_iter().partition(|e| e.pinned);
        self.entries = pinned;
        for e in &doomed {
            self.delete_files(e);
        }
        self.save();
        doomed.len()
    }

    /// Remove everything including pinned entries.
    pub fn clear_all(&mut self) -> usize {
        let doomed = std::mem::take(&mut self.entries);
        for e in &doomed {
            self.delete_files(e);
        }
        self.save();
        doomed.len()
    }

    /// Pin or unpin an entry by [`ClipEntry::id`]. Returns false when unknown.
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id() == id) else {
            return false;
        };
        e.pinned = pinned;
        self.save();
        true
    }

    /// Delete one entry by id (pinned or not).
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(pos) = self.entries.iter().position(|e| e.id() == id) else {
            return false;
        };
        let e = self.entries.remove(pos);
        self.delete_files(&e);
        self.save();
        true
    }

    fn delete_files(&self, e: &ClipEntry) {
        if let (Some(dir), Some(img)) = (self.images_dir(), &e.image)
            && is_safe_image_name(&img.file)
        {
            let _ = std::fs::remove_file(dir.join(&img.file));
        }
    }

    // ------------------------------------------------------------ queries

    pub fn entries(&self) -> &[ClipEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, id: &str) -> Option<&ClipEntry> {
        self.entries.iter().find(|e| e.id() == id)
    }

    /// The newest text entry.
    pub fn latest(&self) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.kind == ClipKind::Text)
            .map(|e| e.text.as_str())
    }

    /// Text of the newest `n` text entries (index 0 = latest); this is what
    /// the `{clipboard:N}` snippet placeholder indexes.
    pub fn recent_texts(&self, n: usize) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.kind == ClipKind::Text)
            .take(n)
            .map(|e| e.text.clone())
            .collect()
    }

    /// Absolute path of an image entry's PNG, if it has one.
    pub fn image_path(&self, e: &ClipEntry) -> Option<PathBuf> {
        let img = e.image.as_ref()?;
        is_safe_image_name(&img.file).then(|| self.images_dir().map(|d| d.join(&img.file)))?
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<ClipEntry> {
        let needle = query.trim().to_lowercase();
        let mut v: Vec<(f64, usize, &ClipEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                fuzzy::score(query, &e.label())
                    .or_else(|| {
                        // match anywhere in the text, but rank below title-style matches
                        e.text.to_lowercase().contains(&needle).then_some(1.0)
                    })
                    .map(|s| (s, i, e))
            })
            .collect();
        // Higher score first; ties: pinned, then recency.
        v.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.2.pinned.cmp(&a.2.pinned))
                .then(a.1.cmp(&b.1))
        });
        v.into_iter().take(limit).map(|(_, _, e)| e.clone()).collect()
    }

    // ------------------------------------------------------------ persistence

    fn save(&self) {
        if let Some(p) = &self.path
            && let Ok(b) = serde_json::to_vec(&self.entries)
        {
            let _ = write_private(p, &b);
        }
    }
}

fn images_dir_for(history_file: &Path) -> PathBuf {
    history_file.parent().unwrap_or(Path::new(".")).join(IMAGES_DIR)
}

/// Width and height from a PNG's IHDR chunk, `None` when it is not a PNG.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || png[..8] != PNG_MAGIC || &png[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h))
}

fn write_image(dir: &Path, target: &Path, png: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    write_private(target, png)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("oa-clip-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A syntactically valid PNG header (enough for dimension parsing); the
    /// store never decodes pixels.
    pub fn fake_png(w: u32, h: u32, salt: u8) -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v.extend_from_slice(&[0, 0, 0, 0]); // crc (unchecked)
        v.push(salt);
        v
    }

    fn texts(h: &ClipboardHistory) -> Vec<&str> {
        h.entries().iter().map(|e| e.text.as_str()).collect()
    }

    #[test]
    fn dedupes_and_orders_newest_first() {
        let mut h = ClipboardHistory::in_memory(3);
        assert!(h.push("a"));
        assert!(h.push("b"));
        assert!(!h.push("b"));
        assert!(h.push("a"));
        let all = h.search("", 10);
        assert_eq!(all.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        h.push("c");
        h.push("d");
        assert_eq!(h.search("", 10).len(), 3);
    }

    #[test]
    fn ignores_blank_and_oversized() {
        let mut h = ClipboardHistory::in_memory(3);
        assert!(!h.push("   "));
        assert!(!h.push(&"x".repeat(MAX_TEXT_BYTES + 1)));
        assert!(h.push(&"x".repeat(MAX_TEXT_BYTES)));
    }

    #[test]
    fn duplicate_moves_to_front_keeping_pin() {
        let mut h = ClipboardHistory::in_memory(10);
        h.push_text_at("a", 1);
        h.push_text_at("b", 2);
        let id = h.entries()[1].id();
        assert!(h.set_pinned(&id, true));
        assert!(h.push_text_at("a", 3));
        assert_eq!(texts(&h), vec!["a", "b"]);
        assert!(h.entries()[0].pinned, "re-copying a pinned entry keeps the pin");
        assert_eq!(h.entries()[0].ts, 3);
    }

    #[test]
    fn retention_by_count_keeps_newest() {
        let mut h = ClipboardHistory::in_memory(3);
        for (i, t) in ["1", "2", "3", "4", "5"].iter().enumerate() {
            h.push_text_at(t, 100 + i as u64);
        }
        assert_eq!(texts(&h), vec!["5", "4", "3"]);
        h.set_retention(2, None);
        assert_eq!(texts(&h), vec!["5", "4"]);
    }

    #[test]
    fn retention_by_age() {
        let mut h = ClipboardHistory::in_memory(10);
        h.max_age_secs = Some(100);
        h.push_text_at("old", 1_000);
        h.push_text_at("mid", 1_050);
        h.push_text_at("new", 1_120); // old is now 120 s old > 100
        assert_eq!(texts(&h), vec!["new", "mid"]);
        assert_eq!(h.prune_at(1_160), 1); // mid is 110 s old
        assert_eq!(texts(&h), vec!["new"]);
        assert_eq!(h.prune_at(1_220), 0); // exactly 100 s is still kept
        assert_eq!(h.prune_at(1_221), 1);
        assert!(h.is_empty());
    }

    #[test]
    fn age_zero_means_forever() {
        let mut h = ClipboardHistory::in_memory(10);
        h.set_retention(10, Some(0));
        h.push_text_at("a", 1);
        assert_eq!(h.prune_at(u64::MAX / 2), 0);
    }

    #[test]
    fn pinned_entries_survive_count_and_age_retention() {
        let mut h = ClipboardHistory::in_memory(2);
        h.max_age_secs = Some(10);
        h.push_text_at("keep", 1);
        let id = h.entries()[0].id();
        h.set_pinned(&id, true);
        for i in 0..6 {
            h.push_text_at(&format!("n{i}"), 1_000 + i);
        }
        // keep is ancient and beyond the count, but pinned; pinned does not use up the quota.
        assert_eq!(texts(&h), vec!["n5", "n4", "keep"]);
        assert_eq!(h.prune_at(1_000_000), 2);
        assert_eq!(texts(&h), vec!["keep"]);
    }

    #[test]
    fn clear_keeps_pinned_and_clear_all_does_not() {
        let mut h = ClipboardHistory::in_memory(10);
        h.push("a");
        h.push("b");
        let id = h.find_id_by_text("a");
        h.set_pinned(&id, true);
        assert_eq!(h.clear(), 1);
        assert_eq!(texts(&h), vec!["a"]);
        assert_eq!(h.clear_all(), 1);
        assert!(h.is_empty());
    }

    impl ClipboardHistory {
        fn find_id_by_text(&self, t: &str) -> String {
            self.entries().iter().find(|e| e.text == t).unwrap().id()
        }
    }

    #[test]
    fn pin_unpin_and_remove_unknown_ids() {
        let mut h = ClipboardHistory::in_memory(10);
        h.push("a");
        assert!(!h.set_pinned("nope", true));
        assert!(!h.remove("nope"));
        let id = h.entries()[0].id();
        assert!(h.set_pinned(&id, true) && h.entries()[0].pinned);
        assert!(h.set_pinned(&id, false) && !h.entries()[0].pinned);
        assert!(h.remove(&id));
        assert!(h.is_empty());
    }

    #[test]
    fn search_ranks_title_matches_and_puts_pinned_first_on_ties() {
        let mut h = ClipboardHistory::in_memory(10);
        h.push("zzz needle at the end\nline two");
        h.push("needle");
        h.push("hello\nthe needle on line two");
        let r = h.search("needle", 10);
        assert_eq!(r[0].text, "needle", "exact title match first");
        assert_eq!(r.len(), 3, "body matches are included");
        let id = h.find_id_by_text("hello\nthe needle on line two");
        h.set_pinned(&id, true);
        let all = h.search("", 10);
        assert_eq!(
            all[0].text, "hello\nthe needle on line two",
            "pinned first on empty query"
        );
        assert!(h.search("nomatch", 10).is_empty());
    }

    #[test]
    fn legacy_json_loads_and_roundtrips_without_new_fields_for_text() {
        let root = tmp("legacy");
        let f = root.join("clipboard.json");
        std::fs::write(&f, r#"[{"text":"old","ts":5}]"#).unwrap();
        let mut h = ClipboardHistory::load(f.clone(), 10);
        assert_eq!(h.latest(), Some("old"));
        assert!(h.push("new"));
        let raw = std::fs::read_to_string(&f).unwrap();
        assert!(!raw.contains("pinned") && !raw.contains("kind"), "{raw}");
        let again = ClipboardHistory::load(f, 10);
        assert_eq!(again.entries().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn history_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("perm");
        let f = root.join("clipboard.json");
        let mut h = ClipboardHistory::load(f.clone(), 10);
        h.push("secret-ish");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn images_are_stored_privately_and_deduped() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("img");
        let f = root.join("clipboard.json");
        let mut h = ClipboardHistory::load(f.clone(), 10);
        let png = fake_png(640, 480, 1);
        assert!(h.push_image_at(&png, 10));
        assert!(!h.push_image_at(&png, 11), "same image as latest is a duplicate");
        let e = h.entries()[0].clone();
        assert_eq!(e.kind, ClipKind::Image);
        assert_eq!(e.label(), "Image 640×480");
        let p = h.image_path(&e).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), png);
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // Survives a reload.
        let h2 = ClipboardHistory::load(f, 10);
        assert_eq!(h2.entries().len(), 1);
        assert_eq!(h2.entries()[0].image, e.image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn image_size_cap_and_validation() {
        let root = tmp("imgcap");
        let mut h = ClipboardHistory::load(root.join("clipboard.json"), 10);
        h.set_max_image_bytes(100);
        let mut big = fake_png(1, 1, 0);
        big.extend(vec![0u8; 200]);
        assert!(!h.push_image_at(&big, 1), "over the cap");
        assert!(!h.push_image_at(b"not a png at all, just text...", 1));
        assert!(!h.push_image_at(&fake_png(0, 5, 0), 1), "zero dimension");
        assert!(h.is_empty());
        assert!(!ClipboardHistory::in_memory(5).push_image_at(&fake_png(1, 1, 0), 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn evicted_images_are_deleted_from_disk_but_pinned_ones_stay() {
        let root = tmp("imgevict");
        let mut h = ClipboardHistory::load(root.join("clipboard.json"), 2);
        let keep = fake_png(2, 2, 1);
        assert!(h.push_image_at(&keep, 1));
        let keep_path = h.image_path(&h.entries()[0].clone()).unwrap();
        let id = h.entries()[0].id();
        h.set_pinned(&id, true);
        let victim = fake_png(3, 3, 2);
        assert!(h.push_image_at(&victim, 2));
        let victim_path = h.image_path(&h.entries()[0].clone()).unwrap();
        h.push_text_at("t1", 3);
        h.push_text_at("t2", 4); // two unpinned allowed: t2, t1 -> victim evicted
        assert!(!victim_path.exists(), "evicted image file removed");
        assert!(keep_path.exists(), "pinned image kept");
        h.clear();
        assert!(keep_path.exists(), "clear keeps pinned");
        h.clear_all();
        assert!(!keep_path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tampered_image_names_are_rejected() {
        let root = tmp("imgtamper");
        let outside = root.join("secret.txt");
        std::fs::write(&outside, "x").unwrap();
        let f = root.join("clipboard.json");
        let json =
            r#"[{"text":"","ts":1,"kind":"image","image":{"file":"../secret.txt","width":1,"height":1,"bytes":1}}]"#;
        std::fs::write(&f, json).unwrap();
        let h = ClipboardHistory::load(f, 10);
        assert!(h.is_empty(), "unsafe name dropped on load");
        assert!(outside.exists());
        assert!(!is_safe_image_name("../../etc/passwd"));
        assert!(!is_safe_image_name("0123456789abcdeg.png"));
        assert!(is_safe_image_name("0123456789abcdef.png"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn image_entries_with_missing_files_are_dropped_on_load() {
        let root = tmp("imgmissing");
        let f = root.join("clipboard.json");
        let mut h = ClipboardHistory::load(f.clone(), 10);
        h.push_image_at(&fake_png(2, 2, 7), 1);
        h.push_text_at("kept", 2);
        let p = h.image_path(&h.entries()[1].clone()).unwrap();
        std::fs::remove_file(p).unwrap();
        let h2 = ClipboardHistory::load(f, 10);
        assert_eq!(texts(&h2), vec!["kept"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_lists() {
        let mut h = ClipboardHistory::in_memory(10);
        assert!(!h.push_files_at(&[], 1));
        assert!(!h.push_files_at(&[PathBuf::from("/a\nb")], 1), "newline in a path");
        assert!(h.push_files_at(&[PathBuf::from("/tmp/a.txt")], 1));
        assert_eq!(h.entries()[0].label(), "a.txt");
        assert!(h.push_files_at(&[PathBuf::from("/tmp/a.txt"), PathBuf::from("/x/b.txt")], 2));
        let e = &h.entries()[0];
        assert_eq!(e.kind, ClipKind::Files);
        assert_eq!(e.label(), "a.txt and 1 more");
        assert_eq!(e.files(), vec!["/tmp/a.txt", "/x/b.txt"]);
        // Same list again is a duplicate of the latest.
        assert!(!h.push_files_at(&[PathBuf::from("/tmp/a.txt"), PathBuf::from("/x/b.txt")], 3));
        // Files and text with identical content are distinct entries.
        assert!(h.push_text_at("/tmp/a.txt", 4));
        assert_eq!(h.len(), 3);
        // Searchable by file name.
        assert_eq!(h.search("b.txt", 5).len(), 1);
    }

    #[test]
    fn latest_and_recent_texts_skip_non_text() {
        let mut h = ClipboardHistory::in_memory(10);
        h.push_text_at("one", 1);
        h.push_text_at("two", 2);
        h.push_files_at(&[PathBuf::from("/tmp/f")], 3);
        assert_eq!(h.latest(), Some("two"));
        assert_eq!(h.recent_texts(5), vec!["two", "one"]);
        assert_eq!(h.recent_texts(1), vec!["two"]);
    }

    #[test]
    fn ignore_list_roundtrip() {
        let mut h = ClipboardHistory::in_memory(3);
        assert_eq!(
            h.ignored_apps().len(),
            DEFAULT_IGNORED_APPS.len(),
            "defaults until configured"
        );
        h.set_ignored_apps(vec!["com.x".into()]);
        assert_eq!(h.ignored_apps(), ["com.x".to_string()]);
        h.set_ignored_apps(Vec::new());
        assert!(
            h.ignored_apps().is_empty(),
            "an explicit empty list disables the defaults"
        );
    }

    #[test]
    fn png_dimension_parsing() {
        assert_eq!(png_dimensions(&fake_png(800, 600, 0)), Some((800, 600)));
        assert_eq!(png_dimensions(&[]), None);
        assert_eq!(png_dimensions(&PNG_MAGIC), None);
    }
}
