//! World-map chunk sprite caching, idle aging and memory limits.

use crate::ui_sprites::Sprite;

use std::{collections::HashMap, rc::Rc};

/// The most chunk sprites a cache keeps.
pub(super) const CHUNK_CACHE_CAPACITY: usize = 4096;

/// The pixel memory soft entries may hold before the oldest are dropped.
pub(super) const SOFT_CHUNK_BYTES: usize = 128 << 20;

/// The idle `clean` calls after which a chunk sprite turns soft.
pub const CHUNK_IDLE_CLEANS: u32 = 5;

pub(super) struct CachedChunk {
    sprite: Rc<Sprite>,
    /// `clean` calls since the last use.
    idle: u32,
    /// Idle for too long: kept only while memory allows.
    soft: bool,
    /// Position in the least-recently-used order.
    used: u64,
}

/// The chunk-sprite cache: a 4096-entry LRU whose entries turn soft after
/// `clean(5)` has seen them unused five times. A soft entry is still found
/// (and turns hard again) until the pixel memory of soft entries passes
/// [`SOFT_CHUNK_BYTES`], when the least recently used go first. The original
/// leaves that to its garbage collector, which keeps such sprites for minutes
/// on a machine with free memory.
pub struct ChunkCache {
    entries: HashMap<i64, CachedChunk>,
    clock: u64,
    /// Bytes of pixels soft entries may hold.
    soft_limit: usize,
}

impl Default for ChunkCache {
    fn default() -> Self {
        Self::with_soft_limit(SOFT_CHUNK_BYTES)
    }
}

impl ChunkCache {
    pub fn with_soft_limit(soft_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            soft_limit,
        }
    }
    pub(super) fn touch(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
    pub fn get(&mut self, key: i64) -> Option<Rc<Sprite>> {
        let used = self.touch();
        let entry = self.entries.get_mut(&key)?;
        entry.idle = 0;
        entry.soft = false;
        entry.used = used;
        Some(entry.sprite.clone())
    }
    /// The cached width, or -1.
    pub(super) fn width(&mut self, key: i64) -> i32 {
        self.get(key).map_or(-1, |s| s.size[0])
    }
    pub fn put(&mut self, sprite: Rc<Sprite>, key: i64) {
        self.entries.remove(&key);
        while self.entries.len() >= CHUNK_CACHE_CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => self.entries.remove(&k),
                None => break,
            };
        }
        let used = self.touch();
        self.entries.insert(
            key,
            CachedChunk {
                sprite,
                idle: 0,
                soft: false,
                used,
            },
        );
    }
    /// Ages the hard entries, turns those idle for more than `n` calls soft
    /// and drops soft entries beyond the memory allowance.
    pub fn clean(&mut self, n: u32) {
        let mut soft_bytes = 0usize;
        for entry in self.entries.values_mut() {
            if !entry.soft {
                entry.idle += 1;
                entry.soft = entry.idle > n;
            }
            if entry.soft {
                soft_bytes += entry.sprite.argb.len() * 4;
            }
        }
        while soft_bytes > self.soft_limit {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| e.soft)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            let Some(key) = oldest else { break };
            if let Some(entry) = self.entries.remove(&key) {
                soft_bytes = soft_bytes.saturating_sub(entry.sprite.argb.len() * 4);
            }
        }
    }
    pub fn reset(&mut self) {
        self.entries.clear();
    }
}
