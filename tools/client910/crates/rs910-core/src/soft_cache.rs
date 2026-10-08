//! A keyed cache whose entries go idle and can be dropped under memory
//! pressure.
//!
//! The client keeps its derived data (models, textures, icons) in caches that
//! only grow while the player moves about. Two things keep them in check:
//!
//! - [`SoftMap::clean`] runs once per frame. It ages every entry; an entry
//!   that was not read for more than `age` cleans becomes *soft*. A soft
//!   entry is still served, and any read makes it ordinary again.
//! - [`SoftMap::clear_soft`] runs when the process is short of memory, or on
//!   the player's request. It drops the soft entries and nothing else, so what
//!   the scene drew this frame stays.
use std::collections::HashMap;
use std::hash::Hash;

struct Slot<V> {
    value: V,
    /// Cleans since the last read.
    idle: u32,
    soft: bool,
}

/// A map of values by key that ages its entries (see the module docs).
pub struct SoftMap<K, V> {
    slots: HashMap<K, Slot<V>>,
}

impl<K, V> Default for SoftMap<K, V> {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq, V> SoftMap<K, V> {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value for `key`; reading it makes the entry ordinary again.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let slot = self.slots.get_mut(key)?;
        slot.idle = 0;
        slot.soft = false;
        Some(&slot.value)
    }

    /// [`SoftMap::get`] for a value that is changed in place.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let slot = self.slots.get_mut(key)?;
        slot.idle = 0;
        slot.soft = false;
        Some(&mut slot.value)
    }

    /// Whether `key` has an entry. This is not a read: the entry does not
    /// become ordinary.
    #[must_use]
    pub fn contains_key(&self, key: &K) -> bool {
        self.slots.contains_key(key)
    }

    /// Store `value` as an ordinary entry; the old value for `key` comes back.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.slots
            .insert(
                key,
                Slot {
                    value,
                    idle: 0,
                    soft: false,
                },
            )
            .map(|slot| slot.value)
    }

    /// Take the entry for `key` out.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.slots.remove(key).map(|slot| slot.value)
    }

    /// Age every entry by one clean; an entry idle for more than `age`
    /// cleans becomes soft.
    pub fn clean(&mut self, age: u32) {
        for slot in self.slots.values_mut() {
            slot.idle = slot.idle.saturating_add(1);
            if slot.idle > age {
                slot.soft = true;
            }
        }
    }

    /// Drop the soft entries; how many went.
    pub fn clear_soft(&mut self) -> usize {
        let before = self.slots.len();
        self.slots.retain(|_, slot| !slot.soft);
        before - self.slots.len()
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// Keep the entries `keep` is true for.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        self.slots.retain(|key, slot| keep(key, &slot.value));
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The number of soft entries.
    #[must_use]
    pub fn soft_len(&self) -> usize {
        self.slots.values().filter(|slot| slot.soft).count()
    }
}

impl<K: Hash + Eq, V> std::ops::Index<&K> for SoftMap<K, V> {
    type Output = V;

    /// The value for `key` without touching the entry; panics without one.
    fn index(&self, key: &K) -> &V {
        &self.slots[key].value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry unused for more than the age goes soft, a read makes it
    /// ordinary again, and pressure drops only the soft ones.
    #[test]
    fn idle_entries_go_soft_and_pressure_drops_them() {
        let mut cache = SoftMap::new();
        cache.insert("a", 1);
        cache.insert("b", 2);
        for _ in 0..5 {
            cache.clean(5);
        }
        assert_eq!(cache.soft_len(), 0, "five cleans is not more than five");
        assert_eq!(cache.get(&"a"), Some(&1));
        cache.clean(5);
        assert_eq!((cache.len(), cache.soft_len()), (2, 1), "only b is idle");
        assert_eq!(cache.clear_soft(), 1);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(&1));
        // Soft entries are still served until pressure drops them.
        cache.clean(0);
        assert_eq!(cache.soft_len(), 1);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.soft_len(), 0);
    }
}
