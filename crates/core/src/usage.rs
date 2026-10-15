//! Learned ranking: items you pick for a query rise for that query.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::write_private;

/// Queries longer than this are never stored (people paste secrets into launchers; SEC-18).
pub const MAX_QUERY_CHARS: usize = 100;
/// Item ids longer than this are not recorded at all (ids may embed user text).
pub const MAX_ID_BYTES: usize = 200;
/// Distinct queries kept; the least-used are dropped beyond it.
const MAX_QUERIES: usize = 2_000;
const MAX_IDS_PER_QUERY: usize = 50;

#[derive(Debug, Serialize, Deserialize)]
pub struct Usage {
    total: HashMap<String, u32>,
    by_query: HashMap<String, HashMap<String, u32>>,
    #[serde(skip)]
    path: Option<PathBuf>,
    /// Learning switch (opt-out). Not persisted here: the owner (the config) decides at start-up.
    #[serde(skip, default = "enabled_default")]
    enabled: bool,
}

fn enabled_default() -> bool {
    true
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            total: HashMap::new(),
            by_query: HashMap::new(),
            path: None,
            enabled: true,
        }
    }
}

impl Usage {
    pub fn load(path: PathBuf) -> Usage {
        let mut u: Usage = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        u.path = Some(path);
        u
    }

    pub fn in_memory() -> Usage {
        Usage::default()
    }

    /// Turn learning on or off. While off nothing new is recorded and existing data does not
    /// influence ranking (it is kept on disk; see [`Usage::clear`] to delete it).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Forget everything learned, in memory and on disk.
    pub fn clear(&mut self) {
        self.total.clear();
        self.by_query.clear();
        self.persist();
    }

    pub fn boost(&self, query: &str, id: &str) -> f64 {
        if !self.enabled {
            return 0.0;
        }
        let q = query.trim().to_lowercase();
        let exact = self.by_query.get(&q).and_then(|m| m.get(id)).copied().unwrap_or(0) as f64;
        let total = self.total.get(id).copied().unwrap_or(0) as f64;
        // Query-specific picks matter most; global popularity breaks ties.
        (exact.min(10.0) * 60.0) + (1.0 + total).ln() * 15.0
    }

    pub fn record(&mut self, query: &str, id: &str) {
        if !self.enabled || id.len() > MAX_ID_BYTES {
            return;
        }
        let q = query.trim().to_lowercase();
        *self.total.entry(id.to_string()).or_insert(0) += 1;
        if !q.is_empty() && q.chars().count() <= MAX_QUERY_CHARS {
            let ids = self.by_query.entry(q).or_default();
            *ids.entry(id.to_string()).or_insert(0) += 1;
            if ids.len() > MAX_IDS_PER_QUERY
                && let Some(least) = ids
                    .iter()
                    .min_by_key(|(k, c)| (**c, (*k).clone()))
                    .map(|(k, _)| k.clone())
            {
                ids.remove(&least);
            }
            if self.by_query.len() > MAX_QUERIES {
                self.trim_queries();
            }
        }
        self.persist();
    }

    /// Drop the least-used queries (ties by name, for determinism) down to 90% of the cap.
    fn trim_queries(&mut self) {
        let mut ranked: Vec<(u32, String)> = self
            .by_query
            .iter()
            .map(|(q, m)| (m.values().sum::<u32>(), q.clone()))
            .collect();
        ranked.sort();
        let drop = self.by_query.len().saturating_sub(MAX_QUERIES * 9 / 10);
        for (_, q) in ranked.into_iter().take(drop) {
            self.by_query.remove(&q);
        }
    }

    fn persist(&self) {
        if let Some(p) = &self.path
            && let Ok(bytes) = serde_json::to_vec(self)
        {
            let _ = write_private(p, &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_raise_boost_for_that_query() {
        let mut u = Usage::in_memory();
        assert_eq!(u.boost("sa", "app:Safari"), 0.0);
        u.record("sa", "app:Safari");
        u.record("sa", "app:Safari");
        assert!(u.boost("sa", "app:Safari") > u.boost("sa", "app:Sandbox"));
        assert!(u.boost("sa", "app:Safari") > u.boost("zz", "app:Safari"));
    }

    #[test]
    fn long_queries_and_long_ids_are_not_stored() {
        let mut u = Usage::in_memory();
        let long = "x".repeat(MAX_QUERY_CHARS + 1);
        u.record(&long, "app:Safari");
        assert!(u.by_query.is_empty(), "long query stored");
        assert_eq!(u.total["app:Safari"], 1); // the pick itself still counts globally
        let exact = "y".repeat(MAX_QUERY_CHARS);
        u.record(&exact, "app:Safari");
        assert_eq!(u.by_query.len(), 1);
        u.record("q", &"i".repeat(MAX_ID_BYTES + 1));
        assert!(!u.by_query.contains_key("q") && u.total.len() == 1);
    }

    #[test]
    fn opt_out_stops_recording_and_boosting() {
        let mut u = Usage::in_memory();
        u.record("sa", "app:Safari");
        let before = u.boost("sa", "app:Safari");
        assert!(before > 0.0);
        u.set_enabled(false);
        assert!(!u.is_enabled());
        assert_eq!(u.boost("sa", "app:Safari"), 0.0);
        u.record("sa", "app:Other");
        assert!(!u.total.contains_key("app:Other"));
        u.set_enabled(true);
        assert_eq!(u.boost("sa", "app:Safari"), before);
        assert_eq!(u.boost("sa", "app:Other"), 0.0);
    }

    #[test]
    fn tables_are_bounded_and_clear_wipes_the_file() {
        let root = std::env::temp_dir().join(format!("oa-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = root.join("usage.json");
        let mut u = Usage::load(file.clone());
        for i in 0..(MAX_QUERIES + 50) {
            u.record(&format!("q{i}"), "app:A");
        }
        u.record("q0", "app:A");
        assert!(u.by_query.len() <= MAX_QUERIES, "{}", u.by_query.len());
        for i in 0..(MAX_IDS_PER_QUERY + 10) {
            u.record("same", &format!("id{i}"));
        }
        assert!(u.by_query["same"].len() <= MAX_IDS_PER_QUERY);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        u.clear();
        let back = Usage::load(file);
        assert!(back.total.is_empty() && back.by_query.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
