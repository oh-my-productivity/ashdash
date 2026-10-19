//! A frame-to-frame cache for the laid-out text of the result rows.
//!
//! Every frame the list used to rebuild the text of all nine rows: clone each item, build the layout jobs, measure the path
//! subtitles again and again to find where to put the `…` (`rowtext::elide_path` asks the font for a width a dozen times per row),
//! and hand every job to egui's galley cache to be hashed. None of it changes while the user types in the field, because the rows
//! do not change until a new list arrives. [`RowCache`] keeps what a row looked like under the exact inputs that decide its text
//! ([`RowKey`]) and hands it back the next frame.
//!
//! Entries that were not asked for during a whole frame are dropped (two generations), so the cache never holds more than the rows of
//! two frames and needs no size limit or clock.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Everything that decides how a row's text looks. Two rows with the same key lay out identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowKey(u64);

impl RowKey {
    /// Mix the inputs of one row into a key. `inputs` are fed in order; use the same order everywhere.
    pub fn of(inputs: impl FnOnce(&mut KeyHasher)) -> RowKey {
        let mut h = KeyHasher(std::collections::hash_map::DefaultHasher::new());
        inputs(&mut h);
        RowKey(h.0.finish())
    }
}

/// Feeds the pieces of a [`RowKey`]. Floats go in as their bit patterns (a width that changed by any amount is another layout).
pub struct KeyHasher(std::collections::hash_map::DefaultHasher);

impl KeyHasher {
    pub fn str(&mut self, s: &str) -> &mut Self {
        s.hash(&mut self.0);
        self
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        v.to_bits().hash(&mut self.0);
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        v.hash(&mut self.0);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        v.hash(&mut self.0);
        self
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        v.hash(&mut self.0);
        self
    }
}

/// Values by [`RowKey`], two generations deep.
#[derive(Debug)]
pub struct RowCache<V> {
    current: HashMap<RowKey, V>,
    previous: HashMap<RowKey, V>,
    hits: u64,
    misses: u64,
}

impl<V> Default for RowCache<V> {
    fn default() -> Self {
        RowCache {
            current: HashMap::new(),
            previous: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }
}

impl<V: Clone> RowCache<V> {
    /// The value for `key`: the one kept from this frame or the last, or the result of `make` (which is then kept).
    pub fn get_or_insert_with(&mut self, key: RowKey, make: impl FnOnce() -> V) -> V {
        if let Some(v) = self.current.get(&key) {
            self.hits += 1;
            return v.clone();
        }
        if let Some(v) = self.previous.remove(&key) {
            self.hits += 1;
            self.current.insert(key, v.clone());
            return v;
        }
        self.misses += 1;
        let v = make();
        self.current.insert(key, v.clone());
        v
    }

    /// The end of a frame: what was not asked for in this frame goes.
    pub fn end_frame(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    /// Entries kept (this frame's and the last frame's).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }

    /// `(hits, misses)` since the start.
    #[cfg(test)]
    pub fn counts(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn key(title: &str, width: f32, selected: bool) -> RowKey {
        RowKey::of(|h| {
            h.str(title).f32(width).bool(selected);
        })
    }

    #[test]
    fn the_same_inputs_make_the_same_key_and_any_change_makes_another() {
        let base = key("Safari", 442.0, false);
        assert_eq!(base, key("Safari", 442.0, false));
        assert_ne!(base, key("Safari ", 442.0, false), "the text");
        assert_ne!(base, key("Safari", 442.5, false), "the width, by any amount");
        assert_ne!(base, key("Safari", 442.0, true), "the selection");
        assert_ne!(
            key("ab", 1.0, false),
            key("a", 1.0, false),
            "no two strings run together"
        );
        // 'a' + 'bc' is not 'ab' + 'c'
        let a = RowKey::of(|h| {
            h.str("a").str("bc");
        });
        let b = RowKey::of(|h| {
            h.str("ab").str("c");
        });
        assert_ne!(a, b);
    }

    #[test]
    fn a_row_is_made_once_while_it_keeps_being_asked_for() {
        let made = Cell::new(0);
        let mut cache = RowCache::default();
        let k = key("Safari", 442.0, false);
        for _ in 0..50 {
            let v = cache.get_or_insert_with(k, || {
                made.set(made.get() + 1);
                "galley".to_string()
            });
            assert_eq!(v, "galley");
            cache.end_frame();
        }
        assert_eq!(made.get(), 1, "49 frames reused the first layout");
        assert_eq!(cache.counts(), (49, 1));
    }

    #[test]
    fn an_entry_not_asked_for_during_a_frame_is_dropped_the_frame_after() {
        let mut cache = RowCache::default();
        let (a, b) = (key("a", 1.0, false), key("b", 1.0, false));
        cache.get_or_insert_with(a, || 1);
        cache.get_or_insert_with(b, || 2);
        cache.end_frame();
        assert_eq!(cache.len(), 2);
        // next frame only `a` is on screen
        cache.get_or_insert_with(a, || unreachable!("a is kept"));
        cache.end_frame();
        assert_eq!(cache.len(), 1, "b was not used in the last frame");
        // `b` comes back: laid out again
        let made = Cell::new(false);
        cache.get_or_insert_with(b, || {
            made.set(true);
            2
        });
        assert!(made.get());
    }

    #[test]
    fn the_cache_does_not_grow_with_the_number_of_lists_shown() {
        let mut cache = RowCache::default();
        for list in 0..200u32 {
            for row in 0..9u32 {
                let k = RowKey::of(|h| {
                    h.u32(list).u32(row);
                });
                cache.get_or_insert_with(k, || row);
            }
            cache.end_frame();
        }
        assert!(cache.len() <= 18, "{} entries after 200 lists of 9 rows", cache.len());
    }
}
