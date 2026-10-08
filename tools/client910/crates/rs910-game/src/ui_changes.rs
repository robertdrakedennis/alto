//! Delayed state changes. One cached node belongs to at most
//! one queue. Server writes coalesce behind the client's 500 ms edit deadline.
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub key: i64,
    pub timing: i64,
    pub ints: [i32; 3],
    pub string: Option<Vec<u16>>,
}
impl Change {
    pub fn kind(&self) -> i32 {
        ((self.key as u64) >> 56) as i32
    }
    pub fn target(&self) -> i64 {
        self.key & 0x00ff_ffff_ffff_ffff
    }
    pub fn time(&self) -> i64 {
        self.timing & i64::MAX
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub cache: BTreeMap<i64, Change>,
    pub client: VecDeque<i64>,
    pub server: VecDeque<i64>,
    pub last_push_new: bool,
}
impl Changes {
    pub fn cache(&mut self, kind: i32, target: i64) -> &mut Change {
        let key = (kind as i64).wrapping_shl(56) | target;
        self.last_push_new = !self.cache.contains_key(&key);
        self.cache.entry(key).or_insert_with(|| Change {
            key,
            ..Default::default()
        })
    }
    fn detach(&mut self, key: i64) {
        self.client.retain(|&v| v != key);
        self.server.retain(|&v| v != key);
    }
    pub fn push_client(&mut self, kind: i32, target: i64, now: i64) {
        let change = self.cache(kind, target);
        change.timing = (change.timing & i64::MIN) | now.wrapping_add(500);
        let key = change.key;
        self.detach(key);
        self.client.push_back(key);
    }
    pub fn push_server(&mut self, kind: i32, target: i64) -> &mut Change {
        let change = self.cache(kind, target);
        change.timing |= i64::MIN;
        let key = change.key;
        if change.time() == 0 {
            self.detach(key);
            self.server.push_back(key);
        }
        self.cache.get_mut(&key).unwrap()
    }
    pub fn poll(&mut self, mut now: impl FnMut() -> i64) -> Option<Change> {
        if let Some(key) = self.server.pop_front() {
            return self.cache.remove(&key);
        }
        loop {
            let key = *self.client.front()?;
            if self.cache[&key].time() > now() {
                return None;
            }
            self.client.pop_front();
            let change = self.cache.remove(&key).unwrap();
            if change.timing & i64::MIN != 0 {
                return Some(change);
            }
        }
    }
    /// removeAll does not alter lastPushNew.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "remove-all; exercised by tests only")
    )]
    pub fn clear(&mut self) {
        self.cache.clear();
        self.client.clear();
        self.server.clear();
    }
}
