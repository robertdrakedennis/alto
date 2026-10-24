//! When the client cleans its caches.
//!
//! Once loading is over, every redraw ages the caches ([`Frame::clean`]); a
//! cache entry that nothing read for a while turns soft (see
//! [`crate::soft_cache`]). Soft entries are given back only when the process
//! is short of memory: the schedule looks at the process's memory every
//! [`CHECK_INTERVAL_MS`] and, above its limit, asks for the soft entries to
//! be dropped ([`Frame::clear_soft`]). After a drop it leaves the caches
//! alone for [`SETTLE_MS`], so a process that stays large for other reasons
//! does not rebuild its caches every few seconds.

/// How often the memory is looked at, in milliseconds.
pub const CHECK_INTERVAL_MS: i64 = 5_000;

/// How long the caches are left alone after a drop, in milliseconds.
pub const SETTLE_MS: i64 = 30_000;

/// The cache age a frame's clean uses for the interface caches (sprites,
/// masks, fonts).
pub const INTERFACE_AGE: u32 = 50;

/// The cache age a frame's clean uses for every other cache (models,
/// textures, icons, map chunks).
pub const MODEL_AGE: u32 = 5;

/// What one redraw does to the caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Age every cache by one clean.
    pub clean: bool,
    /// Drop the soft entries: memory is short.
    pub clear_soft: bool,
}

/// The cleaning schedule and its memory limit.
#[derive(Clone, Debug)]
pub struct Schedule {
    /// The process memory above which soft entries are dropped; none when the
    /// machine's memory is not known (nothing is dropped on its own then).
    limit_bytes: Option<u64>,
    next_check: Option<i64>,
}

impl Schedule {
    /// The schedule for a machine with `physical_mb` megabytes of memory
    /// (0 when unknown). The limit is four fifths of a quarter of the
    /// memory, and never under 800 MB.
    #[must_use]
    pub fn new(physical_mb: u64) -> Self {
        let limit_bytes = (physical_mb > 0)
            .then(|| (physical_mb * 1024 * 1024 / 4 * 4 / 5).max(800 * 1024 * 1024));
        Self {
            limit_bytes,
            next_check: None,
        }
    }

    /// A schedule with an explicit limit in bytes (tools and tests that need
    /// a pressure point without a machine of that size).
    #[must_use]
    pub fn with_limit(limit_bytes: u64) -> Self {
        Self {
            limit_bytes: Some(limit_bytes),
            next_check: None,
        }
    }

    /// The memory limit in bytes.
    #[must_use]
    pub fn limit_bytes(&self) -> Option<u64> {
        self.limit_bytes
    }

    /// One redraw at `now` (milliseconds on the client's monotonic clock).
    /// `used` reads the process memory in bytes; it is only called when the
    /// check is due.
    pub fn frame(&mut self, now: i64, used: impl FnOnce() -> Option<u64>) -> Frame {
        let mut frame = Frame {
            clean: true,
            clear_soft: false,
        };
        let due = *self.next_check.get_or_insert(now + CHECK_INTERVAL_MS);
        if now < due {
            return frame;
        }
        self.next_check = Some(now + CHECK_INTERVAL_MS);
        if let (Some(limit), Some(used)) = (self.limit_bytes, used()) {
            if used > limit {
                frame.clear_soft = true;
                self.next_check = Some(now + SETTLE_MS);
            }
        }
        frame
    }
}
