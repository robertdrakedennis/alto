//! Local varp arrays and timing.
//! Clock values are explicit monotonic-clock inputs, not game-loop cycles.
//! Unlike per-entity sparse variables these arrays start/reset to zero.
use super::Error;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Error>;
const SERVER: i64 = 0x4000000000000000;
const IMMEDIATE: i64 = SERVER | 1;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cursor {
    None,
    Bucket(usize),
    Entry(i32),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Varps {
    pub server: Vec<i32>,
    pub current: Vec<i32>,
    pending: BTreeMap<i32, i64>,
    buckets: Vec<Vec<i32>>,
    pub iterator_bucket: usize,
    pub cursor: Cursor,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub clock_reads: usize,
    pub ignored_overflow: bool,
}
impl Varps {
    pub fn new(count: usize) -> Self {
        Self {
            server: vec![0; count],
            current: vec![0; count],
            pending: BTreeMap::new(),
            buckets: vec![vec![]; 128],
            iterator_bucket: 0,
            cursor: Cursor::None,
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new(self.current.len());
    }
    pub fn get(&self, id: i32) -> Result<i32> {
        self.current
            .get(id as usize)
            .copied()
            .ok_or(Error::Invalid("varp index"))
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only introspection
    pub fn pending_order(&self) -> Vec<(i32, i64)> {
        self.buckets
            .iter()
            .flatten()
            .map(|id| (*id, self.pending[id]))
            .collect()
    }
    fn write_pending(&mut self, id: i32, value: i64) {
        if self.pending.insert(id, value).is_none() {
            self.buckets[(id & 127) as usize].push(id);
        }
    }
    pub fn set_local(&mut self, id: i32, value: i32, now: i64) -> Result<Outcome> {
        *self
            .current
            .get_mut(id as usize)
            .ok_or(Error::Invalid("varp index"))? = value;
        self.write_pending(id, now.wrapping_add(500));
        Ok(Outcome {
            clock_reads: 1,
            ignored_overflow: false,
        })
    }
    pub fn set_server(&mut self, id: i32, value: i32, now: i64) -> Result<Outcome> {
        *self
            .server
            .get_mut(id as usize)
            .ok_or(Error::Invalid("varp index"))? = value;
        let reads = match self.pending.get(&id) {
            None => {
                self.write_pending(id, IMMEDIATE);
                0
            }
            Some(&old) if old == IMMEDIATE => 0,
            Some(_) => {
                self.write_pending(id, now.wrapping_add(500) | SERVER);
                1
            }
        };
        Ok(Outcome {
            clock_reads: reads,
            ignored_overflow: false,
        })
    }
    fn next(&mut self) -> Result<Option<i32>> {
        if self.iterator_bucket > 0 && self.cursor != Cursor::Bucket(self.iterator_bucket - 1) {
            let Cursor::Entry(id) = self.cursor else {
                return Err(Error::Invalid("hash iterator null cursor"));
            };
            let bucket = (id & 127) as usize;
            let list = &self.buckets[bucket];
            let at = list
                .iter()
                .position(|&n| n == id)
                .ok_or(Error::Invalid("hash iterator detached cursor"))?;
            self.cursor = if at + 1 < list.len() {
                Cursor::Entry(list[at + 1])
            } else {
                Cursor::Bucket(bucket)
            };
            return Ok(Some(id));
        }
        while self.iterator_bucket < 128 {
            let bucket = self.iterator_bucket;
            self.iterator_bucket += 1;
            let list = &self.buckets[bucket];
            if let Some(&id) = list.first() {
                self.cursor = if list.len() > 1 {
                    Cursor::Entry(list[1])
                } else {
                    Cursor::Bucket(bucket)
                };
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
    /// One poll call. Call restart=true at the start of each polling pass,
    /// then false only while the previous call returned a nonnegative varp ID.
    pub fn poll(&mut self, restart: bool, now: i64) -> Result<i32> {
        if restart {
            self.iterator_bucket = 0;
        }
        while let Some(id) = self.next()? {
            let deadline = self.pending[&id];
            if deadline & 0x3fffffffffffffff < now {
                let apply = deadline & SERVER != 0;
                if apply {
                    self.current[id as usize] = self.server[id as usize];
                }
                self.pending.remove(&id);
                self.buckets[(id & 127) as usize].retain(|&n| n != id);
                if apply {
                    return Ok(id);
                }
            }
        }
        Ok(-1)
    }
}
