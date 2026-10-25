//! A small map that keeps at most `CAPACITY` entries, evicting the least
//! recently used one. Recency is a linear scan, which suits the few hundred
//! entries the client's model and sprite caches hold.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

#[derive(Debug)]
pub struct Lru<K, V, const CAPACITY: usize> {
    values: HashMap<K, V>,
    /// Keys from least to most recently used.
    order: VecDeque<K>,
}

impl<K, V, const CAPACITY: usize> Default for Lru<K, V, CAPACITY> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<K: Hash + Eq + Clone, V, const CAPACITY: usize> Lru<K, V, CAPACITY> {
    /// Looks up `key` and marks it most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.touch(key);
        self.values.get(key)
    }

    /// Looks up `key` without changing its recency.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.values.get(key)
    }

    /// Marks `key` most recently used if it is present.
    pub fn touch(&mut self, key: &K) {
        if self.values.contains_key(key) {
            self.promote(key);
        }
    }

    /// Stores `value` as the most recently used entry, then evicts the least
    /// recently used entries beyond `CAPACITY`.
    pub fn insert(&mut self, key: K, value: V) {
        self.promote(&key);
        self.values.insert(key, value);
        while self.order.len() > CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.values.retain(|k, _| keep(k));
        let values = &self.values;
        self.order.retain(|k| values.contains_key(k));
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn promote(&mut self, key: &K) {
        if let Some(at) = self.order.iter().position(|k| k == key) {
            self.order.remove(at);
        }
        self.order.push_back(key.clone());
    }
}
