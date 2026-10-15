//! Clipboard history and snippets.
//!
//! Clipboard history is sensitive; it is stored with mode 0600. The store
//! cannot tell whether a source marked an entry as concealed (password
//! managers), so users should keep history off for such apps.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::write_private;
use crate::fuzzy;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipEntry {
    pub text: String,
    pub ts: u64,
}

#[derive(Debug, Default)]
pub struct ClipboardHistory {
    entries: Vec<ClipEntry>, // newest first
    max: usize,
    path: Option<PathBuf>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl ClipboardHistory {
    pub fn load(path: PathBuf, max: usize) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        ClipboardHistory { entries, max, path: Some(path) }
    }

    pub fn in_memory(max: usize) -> Self {
        ClipboardHistory { entries: Vec::new(), max, path: None }
    }

    /// Record a clipboard value. Returns false for empty/duplicate-of-latest.
    pub fn push(&mut self, text: &str) -> bool {
        if text.trim().is_empty() || self.entries.first().is_some_and(|e| e.text == text) {
            return false;
        }
        self.entries.retain(|e| e.text != text);
        self.entries.insert(0, ClipEntry { text: text.to_string(), ts: now() });
        self.entries.truncate(self.max.max(1));
        if let Some(p) = &self.path {
            if let Ok(b) = serde_json::to_vec(&self.entries) {
                let _ = write_private(p, &b);
            }
        }
        true
    }

    pub fn latest(&self) -> Option<&str> {
        self.entries.first().map(|e| e.text.as_str())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<ClipEntry> {
        let mut v: Vec<(f64, usize, &ClipEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| fuzzy::score(query, e.text.lines().next().unwrap_or("")).or_else(|| {
                // match anywhere in the text, but rank below title-style matches
                e.text.to_lowercase().contains(&query.trim().to_lowercase()).then_some(1.0)
            }).map(|s| (s, i, e)))
            .collect();
        // Higher score first; for equal scores keep recency order.
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        v.into_iter().take(limit).map(|(_, _, e)| e.clone()).collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        if let Some(p) = &self.path {
            let _ = write_private(p, b"[]");
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub keyword: String,
    pub text: String,
}

pub fn load_snippets(path: &PathBuf) -> Vec<Snippet> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Expand placeholders: `{clipboard}`, `{date}`, `{time}`, `{datetime}`,
/// `{cursor}` (removed; cursor placement needs the paste layer).
pub fn expand_snippet(text: &str, clipboard: &str) -> String {
    let secs = now();
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    let tod = secs % 86_400;
    let date = format!("{y:04}-{m:02}-{d:02}");
    let time = format!("{:02}:{:02}:{:02}", tod / 3600, (tod / 60) % 60, tod % 60);
    text.replace("{clipboard}", clipboard)
        .replace("{datetime}", &format!("{date} {time}"))
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{cursor}", "")
}

/// Days since 1970-01-01 to (year, month, day) in UTC (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ignores_blank() {
        let mut h = ClipboardHistory::in_memory(3);
        assert!(!h.push("   "));
    }

    #[test]
    fn snippet_placeholders() {
        let s = expand_snippet("a {clipboard} b{cursor}", "XYZ");
        assert_eq!(s, "a XYZ b");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
