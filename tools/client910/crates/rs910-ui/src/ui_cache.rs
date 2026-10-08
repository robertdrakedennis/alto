//! Shared font/component soft-LRU cache ownership.
use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};
/// A test snapshot row: `(key, soft, age, cached, weight)`.
#[cfg(any(test, feature = "test-hooks"))]
pub type WeightedEntry = (i64, bool, i64, bool, i32);
struct Entry<T> {
    value: Option<Rc<T>>,
    soft: bool,
    age: i64,
    weight: i32,
}
/// A cache of weighted entries. Soft references retain their values until
/// an explicit reference clear; Rust `Weak` would collect immediately and
/// would not model soft-reference semantics.
pub struct Cache<T> {
    entries: BTreeMap<i64, Entry<T>>,
    order: VecDeque<i64>,
    capacity: i32,
    available: i32,
}
impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self::new(20)
    }
}
impl<T> Cache<T> {
    pub fn new(capacity: i32) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            capacity,
            available: capacity,
        }
    }
    pub fn available(&self) -> i32 {
        self.available
    }
    pub fn capacity(&self) -> i32 {
        self.capacity
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn reset(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.available = self.capacity;
    }

    pub fn remove(&mut self, key: i64) {
        if let Some(e) = self.entries.remove(&key) {
            self.available = self.available.wrapping_add(e.weight);
        }
        self.order.retain(|v| *v != key);
    }
    pub fn get(&mut self, key: i64) -> Option<Rc<T>> {
        let entry = self.entries.get_mut(&key)?;
        let value = entry.value.clone();
        if value.is_none() {
            self.remove(key);
            return None;
        }
        entry.soft = false;
        entry.age = 0;
        self.order.retain(|v| *v != key);
        self.order.push_back(key);
        value
    }
    pub fn put(&mut self, key: i64, value: Rc<T>) -> anyhow::Result<()> {
        self.insert(key, value, 1)
    }
    /// The soft-LRU cache checks capacity before removing an old key.
    /// Component/font caches do not install a removal listener.
    pub fn insert(&mut self, key: i64, value: Rc<T>, weight: i32) -> anyhow::Result<()> {
        anyhow::ensure!(weight <= self.capacity, "entry exceeds cache capacity");
        self.remove(key);
        self.available = self.available.wrapping_sub(weight);
        while self.available < 0 {
            let key = *self
                .order
                .front()
                .ok_or_else(|| anyhow::anyhow!("cache eviction queue empty"))?;
            self.remove(key);
        }
        self.entries.insert(
            key,
            Entry {
                value: Some(value),
                soft: false,
                age: 0,
                weight,
            },
        );
        self.order.push_back(key);
        Ok(())
    }
    pub fn clean(&mut self, age: i32) {
        for key in self.order.clone() {
            let e = self.entries.get_mut(&key).unwrap();
            if e.soft {
                if e.value.is_none() {
                    self.remove(key);
                }
            } else {
                e.age = e.age.wrapping_add(1);
                if e.age > age as i64 {
                    e.soft = true;
                    e.age = 0;
                }
            }
        }
    }
    pub fn clear_soft(&mut self) {
        for key in self.order.clone() {
            if self.entries[&key].soft {
                self.remove(key);
            }
        }
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "cache-control operation kept with its owner; the frame schedule drives `clean` and `clear_soft` (tested)"
        )
    )]
    pub fn clear_soft_referents(&mut self) {
        for e in self.entries.values_mut() {
            if e.soft {
                e.value = None;
            }
        }
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn snapshot(&self) -> Vec<(i64, bool, i64, bool)> {
        self.order
            .iter()
            .map(|k| {
                (
                    *k,
                    self.entries[k].soft,
                    self.entries[k].age,
                    self.entries[k].value.is_some(),
                )
            })
            .collect()
    }
    /// `(key, soft, age, cached, weight)` in LRU order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn weighted_snapshot(&self) -> Vec<WeightedEntry> {
        self.snapshot()
            .into_iter()
            .map(|(k, s, a, v)| (k, s, a, v, self.entries[&k].weight))
            .collect()
    }
}
