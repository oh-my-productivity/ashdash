//! Learned ranking: items you pick for a query rise for that query.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::write_private;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    total: HashMap<String, u32>,
    by_query: HashMap<String, HashMap<String, u32>>,
    #[serde(skip)]
    path: Option<PathBuf>,
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

    pub fn boost(&self, query: &str, id: &str) -> f64 {
        let q = query.trim().to_lowercase();
        let exact = self.by_query.get(&q).and_then(|m| m.get(id)).copied().unwrap_or(0) as f64;
        let total = self.total.get(id).copied().unwrap_or(0) as f64;
        // Query-specific picks matter most; global popularity breaks ties.
        (exact.min(10.0) * 60.0) + (1.0 + total).ln() * 15.0
    }

    pub fn record(&mut self, query: &str, id: &str) {
        let q = query.trim().to_lowercase();
        *self.total.entry(id.to_string()).or_insert(0) += 1;
        if !q.is_empty() {
            *self.by_query.entry(q).or_default().entry(id.to_string()).or_insert(0) += 1;
        }
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
}
