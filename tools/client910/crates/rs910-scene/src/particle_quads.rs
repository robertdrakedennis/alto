//! One frame's particle quads, built on the CPU from the particle runtime's
//! lists (`rs910_model::particle`): what the scene renderer and the
//! interface-model painter upload and draw.
//!
//! Each list is bucketed by view depth (1600 buckets of 64 plus 64 overflow
//! lists of 768, far buckets first) and split into batches on material and
//! lit changes; every particle becomes a camera-facing (optionally rotated)
//! quad. Positions are camera-target relative (the target is the origin).

use crate::particle::DrawParticle;

/// Depth buckets.
const BUCKETS: usize = 1600;
/// Particles per bucket before the overflow lists.
const BUCKET_SIZE: usize = 64;
/// Overflow lists and their capacity.
const OVERFLOW_LISTS: usize = 64;
const OVERFLOW_SIZE: usize = 768;
/// The most quads one batch holds (the renderers' 16-bit quad index buffer).
pub const MAX_BATCH_QUADS: usize = 8191;

/// One quad corner (stride 24): position, R, G, B, A bytes, texture
/// coordinate.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub pos: [f32; 3],
    pub colour: [u8; 4],
    pub uv: [f32; 2],
}

/// `quads` consecutive quads from `first_vertex` sharing a material and the
/// lit flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticleBatch {
    /// Material id (`-1` untextured).
    pub texture: i32,
    /// Whether the batch is sun-lit (it only splits batches).
    pub lit: bool,
    pub first_vertex: u32,
    pub quads: u32,
}

/// One particle owner's batches, drawn right after the owner entity's
/// models in the opaque (`list` 0) or transparent (`list` 1) pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParticleSegment {
    pub list: u8,
    /// The owner's position in the frame plan's list (`usize::MAX`: after
    /// the list).
    pub entity: usize,
    /// `ParticleFrame::batches` of this owner.
    pub batches: std::ops::Range<usize>,
}

/// Every particle list of one frame, ready for upload.
#[derive(Clone, Debug, Default)]
pub struct ParticleFrame {
    pub vertices: Vec<ParticleVertex>,
    pub batches: Vec<ParticleBatch>,
    /// Particles dropped by the bucket capacities.
    pub dropped: usize,
    /// The owners' draw positions; empty means every batch draws after the
    /// transparent list.
    pub segments: Vec<ParticleSegment>,
}

impl ParticleFrame {
    /// The quads of the frame.
    #[must_use]
    pub fn quads(&self) -> usize {
        self.batches.iter().map(|b| b.quads as usize).sum()
    }
}
/// The float cos/sin tables over 16384 steps.
fn trig_matrix(angle: i32) -> (f32, f32) {
    let a = f64::from(angle & 0x3FFF) * crate::trig::STEP;
    (a.cos() as f32, a.sin() as f32)
}

struct Buckets {
    counts: Vec<usize>,
    slots: Vec<[usize; BUCKET_SIZE]>,
    overflow_counts: [usize; OVERFLOW_LISTS],
    overflow: Vec<Vec<usize>>,
    used_overflow: usize,
}

impl Buckets {
    fn new() -> Self {
        Self {
            counts: vec![0; BUCKETS],
            slots: vec![[0; BUCKET_SIZE]; BUCKETS],
            overflow_counts: [0; OVERFLOW_LISTS],
            overflow: (0..OVERFLOW_LISTS)
                .map(|_| Vec::with_capacity(OVERFLOW_SIZE))
                .collect(),
            used_overflow: 0,
        }
    }

    fn clear(&mut self, range: usize) {
        self.used_overflow = 0;
        for c in &mut self.counts[..range.min(BUCKETS)] {
            *c = 0;
        }
        self.overflow_counts = [0; OVERFLOW_LISTS];
        for o in &mut self.overflow {
            o.clear();
        }
    }

    /// Add a particle to its bucket or overflow list. False when dropped.
    fn push(&mut self, bucket: usize, particle: usize) -> bool {
        if bucket >= BUCKETS {
            return false;
        }
        if self.counts[bucket] < BUCKET_SIZE {
            self.slots[bucket][self.counts[bucket]] = particle;
            self.counts[bucket] += 1;
            return true;
        }
        if self.counts[bucket] == BUCKET_SIZE {
            if self.used_overflow == OVERFLOW_LISTS {
                return false;
            }
            self.counts[bucket] += self.used_overflow + 1;
            self.used_overflow += 1;
        }
        let list = self.counts[bucket] - BUCKET_SIZE - 1;
        // The original client would overrun the 768-entry array here.
        if self.overflow_counts[list] == OVERFLOW_SIZE {
            return false;
        }
        self.overflow[list].push(particle);
        self.overflow_counts[list] += 1;
        true
    }
}

/// One material/lit run of a particle list whose particles are already in
/// the buckets, ready to emit quads.
struct Run<'a> {
    list: &'a [DrawParticle],
    /// Buckets in use (view-depth range after shifting).
    range: usize,
    texture: i32,
    lit: bool,
    view: &'a [f32; 16],
    /// Moves a particle's scene-local position into the view's frame.
    local: &'a dyn Fn(&DrawParticle) -> [f32; 3],
}

/// Builds [`ParticleFrame`]s from particle lists.
pub struct Builder {
    buckets: Buckets,
    depths: Vec<i32>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            buckets: Buckets::new(),
            depths: Vec::new(),
        }
    }
}

impl Builder {
    /// Build one frame from every scene system's list when no frame plan
    /// places them (offline scenes): the lists are ordered by their farthest
    /// particle and drawn after the transparent list.
    pub fn build(
        &mut self,
        lists: &[&[DrawParticle]],
        view: &[f32; 16],
        offset: [i32; 3],
    ) -> ParticleFrame {
        let depth = |p: &DrawParticle| {
            let [x, y, z]: [f32; 3] =
                std::array::from_fn(|i| ((p.pos[i] >> 12) + offset[i]) as f32);
            z * view[10] + y * view[6] + x * view[2] + view[14]
        };
        let mut order: Vec<(f32, usize)> = lists
            .iter()
            .enumerate()
            .map(|(i, l)| (l.iter().map(depth).fold(f32::MIN, f32::max), i))
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut frame = ParticleFrame::default();
        for (_, i) in order {
            self.add_list(&mut frame, lists[i], view, offset);
        }
        frame
    }

    /// One owner's `drawParticles` at plan position `entity` of `list`.
    pub fn add_segment(
        &mut self,
        frame: &mut ParticleFrame,
        list: u8,
        entity: usize,
        particles: &[DrawParticle],
        view: &[f32; 16],
        offset: [i32; 3],
    ) {
        let start = frame.batches.len();
        self.add_list(frame, particles, view, offset);
        if frame.batches.len() > start {
            frame.segments.push(ParticleSegment {
                list,
                entity,
                batches: start..frame.batches.len(),
            });
        }
    }

    /// Add one particle list: `view` is the camera's view matrix with the
    /// target as origin and `offset` moves a scene-local fine position into
    /// that frame (`base * 512 - target`).
    pub fn add_list(
        &mut self,
        frame: &mut ParticleFrame,
        list: &[DrawParticle],
        view: &[f32; 16],
        offset: [i32; 3],
    ) {
        if list.is_empty() {
            return;
        }
        let local = |p: &DrawParticle| -> [f32; 3] {
            std::array::from_fn(|i| ((p.pos[i] >> 12) + offset[i]) as f32)
        };
        self.depths.clear();
        let mut min = i32::MAX;
        let mut max = 0;
        for p in list {
            let [x, y, z] = local(p);
            let d = (z * view[10] + y * view[6] + x * view[2] + view[14]) as i32;
            max = max.max(d);
            min = min.min(d);
            self.depths.push(d);
        }
        let mut range = max - min;
        let shift = if range + 2 > BUCKETS as i32 {
            let s = ilog(range) + 1 - ilog(BUCKETS as i32);
            range = (range >> s) + 2;
            s
        } else {
            range += 2;
            0
        };
        let range = range as usize;
        // Split into material/lit runs, each bucketed and drawn.
        let mut start = 0;
        while start < list.len() {
            self.buckets.clear(range);
            let texture = list[start].texture;
            let lit = list[start].lit;
            let mut end = start;
            while end < list.len() {
                let p = &list[end];
                if end > 0 && end > start && (p.texture != texture || p.lit != lit) {
                    break;
                }
                let bucket = (self.depths[end] - min) >> shift;
                if !self.buckets.push(bucket.max(0) as usize, end) {
                    frame.dropped += 1;
                }
                end += 1;
            }
            self.emit(
                frame,
                &Run {
                    list,
                    range,
                    texture,
                    lit,
                    view,
                    local: &local,
                },
            );
            start = end;
        }
    }

    /// Emit one run's quads: far buckets first, each bucket newest-first,
    /// then its overflow list newest-first.
    fn emit(&self, frame: &mut ParticleFrame, run: &Run<'_>) {
        let Run {
            list,
            range,
            texture,
            lit,
            view,
            local,
        } = *run;
        let right = [view[0], view[4], view[8]];
        let up = [view[1], view[5], view[9]];
        let first_vertex = frame.vertices.len() as u32;
        let mut quads = 0_usize;
        let mut put = |p: &DrawParticle, frame: &mut ParticleFrame| {
            if quads == MAX_BATCH_QUADS {
                frame.dropped += 1;
                return;
            }
            let c = p.colour;
            let colour = [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8];
            let pos = local(p);
            let s = (p.size >> 12) as f32;
            let (r, u) = if p.angle == 0 {
                (right.map(|v| v * s), up.map(|v| v * s))
            } else {
                // rotZ(angle) * scale(s) * inverse view rotation.
                let (cos, sin) = trig_matrix(i32::from(p.angle));
                (
                    std::array::from_fn(|i| s * cos * right[i] + s * sin * up[i]),
                    std::array::from_fn(|i| s * -sin * right[i] + s * cos * up[i]),
                )
            };
            let corner = |a: f32, b: f32| -> [f32; 3] {
                std::array::from_fn(|i| pos[i] + a * r[i] + b * u[i])
            };
            for (position, uv) in [
                (corner(-1., -1.), [0., 0.]),
                (corner(-1., 1.), [0., 1.]),
                (corner(1., 1.), [1., 1.]),
                (corner(1., -1.), [1., 0.]),
            ] {
                frame.vertices.push(ParticleVertex {
                    pos: position,
                    colour,
                    uv,
                });
            }
            quads += 1;
        };
        for bucket in (0..range.min(BUCKETS)).rev() {
            let count = self.buckets.counts[bucket];
            for i in (0..count.min(BUCKET_SIZE)).rev() {
                put(&list[self.buckets.slots[bucket][i]], frame);
            }
            if count > BUCKET_SIZE {
                let overflow = &self.buckets.overflow[count - BUCKET_SIZE - 1];
                for &i in overflow.iter().rev() {
                    put(&list[i], frame);
                }
            }
        }
        if quads > 0 {
            frame.batches.push(ParticleBatch {
                texture,
                lit,
                first_vertex,
                quads: quads as u32,
            });
        }
    }
}

/// The bit length.
fn ilog(v: i32) -> i32 {
    32 - (v as u32).leading_zeros() as i32
}
