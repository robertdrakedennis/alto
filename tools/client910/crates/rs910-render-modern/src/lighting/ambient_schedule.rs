//! The per-square ambient capture's schedule: which face of which map square
//! is captured next, the cache of finished blocks and the cross-fade that
//! shows them ([`Schedule`]); and the grid of cells the shader reads
//! ([`Table`]). Renderer-neutral: no GPU types, the clock is an argument.
//!
//! # The model
//!
//! - **Capture.** Every loaded map square is captured from above its bounds
//!   in six axis directions (the face geometry and the render state are
//!   `crate::frame::gpu::ambient`'s); the schedule hands out at most one face
//!   per call, squares in load order, six faces to a square. The faces come
//!   back asynchronously and the six of a square are projected off the
//!   render thread ([`crate::lighting::ambient::project`]).
//! - **Cache.** A result is kept under the environment's key (its sun,
//!   ambient, fog and sky values: anything a capture depends on) and the map
//!   square. A changed key drops the pending captures and requests every
//!   loaded square again, answering from the cache where it can.
//! - **Blend.** A square shows its current blend: on a new result the block
//!   shown at that moment and the new one are mixed, every packed float,
//!   linearly over [`FADE_MS`]. A square's first result blends from the
//!   default block.
//! - **Cells.** The shader reads a grid of [`Table::SIDE`] squares a side
//!   around the camera. A cell holds the block of its square; a square that
//!   is not loaded (the far scene's, which has no geometry to capture) takes
//!   the block of the nearest loaded one (stand-in: the reference loads
//!   every square it draws).

use std::collections::{HashMap, VecDeque};

use crate::lighting::ambient::{Faces, Irradiance, FADE_MS};

/// A map square: its coordinates (64 tiles a side).
pub type Square = (i32, i32);

/// Fine units per map square side.
pub const SQUARE_SIZE: i32 = 64 * 512;

/// How many finished blocks the cache keeps (the oldest go first).
pub const CACHE_LIMIT: usize = 256;

/// The cache key: the environment's key and the map square.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub env: u64,
    pub square: Square,
}

/// One face to capture: which round of capture work it belongs to (an
/// environment change or a dropped square starts a new one, and late faces
/// of the old one are dropped), the square and the cube face (`0..6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub round: u32,
    pub square: Square,
    pub face: u8,
}

/// A face's texels as the capture delivers them: any payload the
/// projection can read (the renderer keeps the readback bytes and converts
/// them on the worker; the tests use colours).
pub type Colours = Vec<[f32; 3]>;

/// Six captured faces of a square, ready to project.
#[derive(Clone, Debug)]
pub struct Projection<F = Colours> {
    pub key: Key,
    /// The faces' size in texels.
    pub size: usize,
    pub faces: [F; 6],
}

impl Projection {
    /// The faces as a projection input.
    #[must_use]
    pub fn into_faces(self) -> Faces {
        Faces {
            size: self.size,
            texels: self.faces,
        }
    }
}

/// A square's display: the block shown at `start_ms` mixed into the new one
/// by `end_ms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    pub from: Irradiance,
    pub to: Irradiance,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Blend {
    /// Shows `block` at once.
    #[must_use]
    pub fn settled(block: Irradiance) -> Self {
        Self {
            from: block,
            to: block,
            start_ms: 0,
            end_ms: 0,
        }
    }

    /// The block at `now_ms`: the new one from the end on, the old one
    /// before the start, the linear mix between.
    #[must_use]
    pub fn value(&self, now_ms: i64) -> Irradiance {
        if now_ms >= self.end_ms {
            self.to
        } else if now_ms <= self.start_ms {
            self.from
        } else {
            let t = (now_ms - self.start_ms) as f32 / (self.end_ms - self.start_ms) as f32;
            self.from.lerp(&self.to, t)
        }
    }

    /// Whether the blend is still moving at `now_ms`.
    #[must_use]
    pub fn running(&self, now_ms: i64) -> bool {
        now_ms < self.end_ms
    }
}

/// The square being captured: its faces so far.
struct Active<F> {
    square: Square,
    next: u8,
    faces: [Option<F>; 6],
    size: usize,
}

/// The capture schedule, the cache and the blends (module docs).
pub struct Schedule<F = Colours> {
    env: Option<u64>,
    round: u32,
    /// The requested squares, in load order.
    wanted: Vec<Square>,
    /// The squares still to capture, in order.
    queue: VecDeque<Square>,
    active: Option<Active<F>>,
    cache: HashMap<Key, Irradiance>,
    cache_order: VecDeque<Key>,
    blends: HashMap<Square, Blend>,
    /// Projections handed out and not yet completed.
    projecting: usize,
}

impl<F> Default for Schedule<F> {
    fn default() -> Self {
        Self {
            env: None,
            round: 0,
            wanted: Vec::new(),
            queue: VecDeque::new(),
            active: None,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            blends: HashMap::new(),
            projecting: 0,
        }
    }
}

impl<F> Schedule<F> {
    /// Requests `squares` (load order) under environment key `env` at
    /// `now_ms`. A new key drops the pending work and asks for every square
    /// again; the same key only asks for squares it has not seen. A square
    /// whose block is cached starts its blend to it at once, every other
    /// square is queued.
    pub fn request(&mut self, env: u64, squares: &[Square], now_ms: i64) {
        let changed = self.env != Some(env);
        if changed {
            self.env = Some(env);
            self.queue.clear();
            self.active = None;
            self.wanted.clear();
            self.round += 1;
        }
        // Squares no longer loaded leave the queue (their cache stays), and
        // the faces in flight of a dropped square are dropped with it.
        self.queue.retain(|s| squares.contains(s));
        if self
            .active
            .as_ref()
            .is_some_and(|a| !squares.contains(&a.square))
        {
            self.active = None;
            self.round += 1;
        }
        let fresh: Vec<Square> = squares
            .iter()
            .copied()
            .filter(|s| !self.wanted.contains(s))
            .collect();
        self.wanted = squares.to_vec();
        for square in fresh {
            let key = Key { env, square };
            match self.cache.get(&key).copied() {
                Some(block) => self.start_blend(square, block, now_ms),
                None => self.queue.push_back(square),
            }
        }
    }

    /// The next face to capture, at most one per call: the active square's
    /// next face, else the first queued square's first.
    pub fn next_face(&mut self) -> Option<Ticket> {
        if self.active.is_none() {
            let square = self.queue.pop_front()?;
            self.active = Some(Active {
                square,
                next: 0,
                faces: Default::default(),
                size: 0,
            });
        }
        let active = self.active.as_mut()?;
        if active.next >= 6 {
            return None;
        }
        let ticket = Ticket {
            round: self.round,
            square: active.square,
            face: active.next,
        };
        active.next += 1;
        Some(ticket)
    }

    /// A captured face arrived (`size` texels a side). The sixth face of a
    /// square yields its projection to run; a face of an old round or of a
    /// square no longer active is dropped.
    pub fn face_done(&mut self, ticket: Ticket, size: usize, face: F) -> Option<Projection<F>> {
        if ticket.round != self.round {
            return None;
        }
        let active = self.active.as_mut().filter(|a| a.square == ticket.square)?;
        active.size = size;
        active.faces[usize::from(ticket.face)] = Some(face);
        if active.faces.iter().any(Option::is_none) {
            return None;
        }
        let active = self.active.take()?;
        self.projecting += 1;
        let faces = active.faces.map(|f| f.expect("six faces"));
        Some(Projection {
            key: Key {
                env: self.env?,
                square: active.square,
            },
            size: active.size,
            faces,
        })
    }

    /// Return the last face issued when its frame never submitted a copy.
    /// Earlier faces and their readbacks keep the same capture round.
    pub fn retry_unsubmitted(&mut self, ticket: Ticket) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if ticket.round == self.round
            && ticket.square == active.square
            && active.next == ticket.face + 1
        {
            active.next = ticket.face;
        }
    }

    /// A face's readback failed: the square goes back to the front of the
    /// queue and its faces in flight are dropped.
    pub fn abandon(&mut self, ticket: Ticket) {
        if ticket.round != self.round
            || self
                .active
                .as_ref()
                .is_none_or(|a| a.square != ticket.square)
        {
            return;
        }
        self.active = None;
        self.round += 1;
        self.queue.push_front(ticket.square);
    }

    /// A projection finished: cache the block and start the square's blend
    /// to it when the key is the current environment's and the square is
    /// still loaded (an older result is only cached, under its own key).
    pub fn complete(&mut self, key: Key, block: Irradiance, now_ms: i64) {
        self.projecting = self.projecting.saturating_sub(1);
        if self.cache.insert(key, block).is_none() {
            self.cache_order.push_back(key);
            while self.cache_order.len() > CACHE_LIMIT {
                if let Some(old) = self.cache_order.pop_front() {
                    self.cache.remove(&old);
                }
            }
        }
        if Some(key.env) == self.env && self.wanted.contains(&key.square) {
            self.start_blend(key.square, block, now_ms);
        }
    }

    fn start_blend(&mut self, square: Square, block: Irradiance, now_ms: i64) {
        let from = self.shown(square, now_ms);
        self.blends.insert(
            square,
            Blend {
                from,
                to: block,
                start_ms: now_ms,
                end_ms: now_ms + FADE_MS,
            },
        );
    }

    /// The block `square` shows at `now_ms` (the default until it has one).
    #[must_use]
    pub fn shown(&self, square: Square, now_ms: i64) -> Irradiance {
        self.blends
            .get(&square)
            .map_or(Irradiance::DEFAULT, |b| b.value(now_ms))
    }

    /// Whether any blend is still moving at `now_ms`.
    #[must_use]
    pub fn blending(&self, now_ms: i64) -> bool {
        self.blends.values().any(|b| b.running(now_ms))
    }

    /// Whether every wanted square has its block (no capture queued, active
    /// or being projected).
    #[must_use]
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.active.is_none() && self.projecting == 0
    }

    /// The squares still waiting for a capture or being captured.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.len() + usize::from(self.active.is_some()) + self.projecting
    }

    /// The squares requested, in load order.
    #[must_use]
    pub fn wanted(&self) -> &[Square] {
        &self.wanted
    }

    /// The finished blocks in the cache.
    #[must_use]
    pub fn cached(&self) -> usize {
        self.cache.len()
    }
}

/// The grid of squares the shader reads, [`Self::SIDE`] a side, its
/// south-west cell at `origin` (map-square coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub origin: Square,
    pub cells: Vec<Irradiance>,
}

impl Table {
    /// Cells per side (the camera square, eight squares each way).
    pub const SIDE: i32 = 16;

    /// The grid centred on the camera's square `camera`.
    #[must_use]
    pub fn origin_for(camera: Square) -> Square {
        (camera.0 - Self::SIDE / 2, camera.1 - Self::SIDE / 2)
    }

    /// The table at `now_ms`: each loaded square's shown block, each other
    /// cell the block of the nearest loaded square (the default with none).
    #[must_use]
    pub fn fill<F>(schedule: &Schedule<F>, origin: Square, now_ms: i64) -> Self {
        let loaded = schedule.wanted();
        let mut cells = Vec::with_capacity((Self::SIDE * Self::SIDE) as usize);
        for dz in 0..Self::SIDE {
            for dx in 0..Self::SIDE {
                let square = (origin.0 + dx, origin.1 + dz);
                let source = if loaded.contains(&square) {
                    Some(square)
                } else {
                    loaded.iter().copied().min_by_key(|s| {
                        let (ex, ez) = (i64::from(s.0 - square.0), i64::from(s.1 - square.1));
                        (ex * ex + ez * ez, s.0, s.1)
                    })
                };
                cells.push(source.map_or(Irradiance::DEFAULT, |s| schedule.shown(s, now_ms)));
            }
        }
        Self { origin, cells }
    }

    /// The cell holding map-square `square` (clamped into the grid, as the
    /// shader clamps).
    #[must_use]
    pub fn cell(&self, square: Square) -> &Irradiance {
        let side = Self::SIDE;
        let x = (square.0 - self.origin.0).clamp(0, side - 1);
        let z = (square.1 - self.origin.1).clamp(0, side - 1);
        &self.cells[(z * side + x) as usize]
    }
}

/// The map square holding scene-local fine position `(x, z)` of a scene
/// whose window starts at tile `base`.
#[must_use]
pub fn square_of(base: [i32; 2], x: i32, z: i32) -> Square {
    let fine = |b: i32, v: i32| (b * 512 + v).div_euclid(SQUARE_SIZE);
    (fine(base[0], x), fine(base[1], z))
}

/// The map squares a scene window of `tiles` (x, z) starting at tile `base`
/// touches, in load order (x, then z).
#[must_use]
pub fn window_squares(base: [i32; 2], tiles: [usize; 2]) -> Vec<Square> {
    if tiles[0] == 0 || tiles[1] == 0 {
        return Vec::new();
    }
    let range = |b: i32, n: usize| (b >> 6)..=((b + n as i32 - 1) >> 6);
    range(base[0], tiles[0])
        .flat_map(|x| range(base[1], tiles[1]).map(move |z| (x, z)))
        .collect()
}
