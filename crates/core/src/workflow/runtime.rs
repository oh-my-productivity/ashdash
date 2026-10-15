//! Mutable per-workflow state that outlives a single query: Script Filter
//! session variables, cached Script Filter output and the user's saved
//! configuration values.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::filter::ParsedFilter;
use super::model::str_map;

#[derive(Debug)]
pub(super) struct CacheEntry {
    pub at: Instant,
    pub ttl: Duration,
    pub parsed: ParsedFilter,
}

#[derive(Debug, Default)]
pub(super) struct Runtime {
    /// Variables a Script Filter returned, per object uid; fed back to the
    /// script as environment variables on the next run ("session variables").
    session: Mutex<HashMap<String, BTreeMap<String, String>>>,
    /// Script Filter results kept by the `cache` key, per (object uid, argument).
    cache: Mutex<HashMap<(String, String), CacheEntry>>,
    /// Lazily loaded user-set configuration values.
    user: Mutex<Option<BTreeMap<String, String>>>,
}

impl Runtime {
    pub fn session_vars(&self, node_uid: &str) -> BTreeMap<String, String> {
        self.session.lock().unwrap().get(node_uid).cloned().unwrap_or_default()
    }

    pub fn merge_session_vars(&self, node_uid: &str, vars: &BTreeMap<String, String>) {
        if vars.is_empty() {
            return;
        }
        self.session
            .lock()
            .unwrap()
            .entry(node_uid.to_string())
            .or_default()
            .extend(vars.clone());
    }

    pub fn cached(&self, node_uid: &str, arg: &str) -> Option<(ParsedFilter, bool)> {
        let map = self.cache.lock().unwrap();
        let e = map.get(&(node_uid.to_string(), arg.to_string()))?;
        Some((e.parsed.clone(), e.at.elapsed() <= e.ttl))
    }

    pub fn store_cache(&self, node_uid: &str, arg: &str, ttl: Duration, parsed: ParsedFilter) {
        self.cache.lock().unwrap().insert(
            (node_uid.to_string(), arg.to_string()),
            CacheEntry {
                at: Instant::now(),
                ttl,
                parsed,
            },
        );
    }

    /// Test hook: pretend the cache entry was stored `age` ago.
    #[cfg(test)]
    pub fn age_cache(&self, age: Duration) {
        for e in self.cache.lock().unwrap().values_mut() {
            e.at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
        }
    }

    pub fn user_values(&self, file: &Path) -> BTreeMap<String, String> {
        let mut slot = self.user.lock().unwrap();
        slot.get_or_insert_with(|| {
            std::fs::read_to_string(file)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|v| str_map(Some(&v)))
                .unwrap_or_default()
        })
        .clone()
    }

    pub fn reset(&self) {
        self.session.lock().unwrap().clear();
        self.cache.lock().unwrap().clear();
        *self.user.lock().unwrap() = None;
    }
}
