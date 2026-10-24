//! Point-light shadows, the CPU half (the tables are
//! [`crate::shadows::presets`], the GPU half [`crate::frame::gpu::point_shadows`],
//! the WGSL `point.wgsl`).
//!
//! Each shadowed light owns six 90 degree faces (a cube) in a shared 2-D depth atlas,
//! sampled with hardware depth compare; the stored depth is the squared distance
//! to the light over the squared radius. This module holds the decisions:
//!
//! - **Candidates** ([`candidates`]): a light that casts shadows, is on, reaches at
//!   least [`MIN_RADIUS`], has its sphere (0.8 of the radius) in the camera's
//!   frustum and lies within the quality's maximum distance.
//! - **Priority** ([`priority`]): a 32-bit key from the distance to the sphere's
//!   near side, the radius and the intensity, in `f32` arithmetic (the rounding
//!   quantises the key to steps of 256, about 430 units of distance, which
//!   swamps the small radius and intensity terms), sorted descending.
//! - **Slots** ([`SlotPool`]): a pool of as many slots as the quality allows. A
//!   light takes a free slot in priority order and keeps it: a better-scoring
//!   newcomer never evicts a slotted light.
//! - **Levels** ([`lod_level`], [`ViewFrustum::projected_size`]): each slot holds
//!   every face at every resolution level (level `k` is `base >> k`); the level
//!   follows how large the light's sphere projects.
//! - **Atlas layout** ([`AtlasLayout`]) and **face culling**
//!   ([`ViewFrustum::meets_face`]: a face is drawn only when its frustum meets the
//!   camera's).
//!
//! # Grades
//!
//! Proven: the candidate rules and their numbers, the score, the descending order,
//! the slot counts, the no-eviction rule, the level rule and its numbers, the face
//! camera (90 degrees, near 0.25, far the radius), the face culling against the
//! camera frustum, the depth written and the biases.
//!
//! Stand-ins, each chosen here because the evidence stops short of it:
//!
//! - **The view-axis sign of the score.** The score adds the radius along the
//!   camera's view axis; the sign is taken as the one that makes a light in
//!   front of the camera closer (the distance to the sphere's near side).
//! - **Ties.** Equal keys order by the true distance, then the light's id.
//! - **Slot release.** A slot whose light has left the candidate set is freed
//!   after [`RELEASE_FRAMES`] frames without it (a destroyed or replaced light
//!   frees its slot at once: the pool is emptied when the scene changes).
//! - **The cast-shadows flag and the intensity.** The 910 map data carries no
//!   per-light flag, so every static light has it set; the intensity is the
//!   light's flicker and fade value, and the score's second factor is 1.
//! - **The atlas shape.** A power-of-two rectangle holding every slot's block
//!   ([`AtlasLayout`]); the packing is this crate's.
//! - **Face reuse across frames.** The reference redraws every visible face every
//!   frame; this crate redraws a face only when its casters could have changed
//!   (`crate::frame::gpu::point_shadows`), which gives the pixels of redrawing.

use glam::{DMat4, DVec3, DVec4};

/// The smallest light radius that casts a shadow (world units).
pub const MIN_RADIUS: f32 = 350.0;
/// The share of the radius whose sphere must meet the camera's frustum.
pub const SPHERE_SCALE: f32 = 0.8;
/// A face camera's near distance.
pub const FACE_NEAR: f32 = 0.25;
/// Added to the projected size before the level is chosen.
pub const LOD_OFFSET: f32 = -0.1;
/// Frames a slotted light may stay out of the candidate set before its slot
/// is freed.
pub const RELEASE_FRAMES: u32 = 8;
/// The most resolution levels of any quality.
pub const MAX_LEVELS: usize = 4;
/// The priority score's weights: distance, radius, intensity.
const WEIGHT_DISTANCE: f32 = 0.6;
const WEIGHT_RADIUS: f32 = 0.1;
const WEIGHT_INTENSITY: f32 = 0.3;

/// One point light as the selection sees it (camera-local).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightInput {
    pub pos: [f32; 3],
    pub radius: f32,
    /// The light's current intensity (flicker times fade); 0 is off.
    pub intensity: f32,
    /// The light's cast-shadows flag.
    pub casts: bool,
}

/// A light that qualifies for a shadow this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Index into the frame's lights.
    pub light: usize,
    /// The priority key; higher is better.
    pub key: u32,
    /// The distance the key was made from.
    pub distance: f32,
}

/// The camera's view frustum and the matrices the selection reads, in
/// camera-local coordinates.
#[derive(Clone, Debug)]
pub struct ViewFrustum {
    eye: DVec3,
    /// The camera's view axis (the view matrix's third row, unit length).
    forward: DVec3,
    /// Inward planes `(n, d)` with `n . p + d >= 0` inside, normalised:
    /// left, right, bottom, top, near, far.
    planes: [DVec4; 6],
    /// The eight corners: the near plane's four, then the far plane's.
    corners: [DVec3; 8],
    view_proj: DMat4,
}

impl ViewFrustum {
    /// The frustum of a camera: `view` and `view_proj` are the frame block's
    /// column-major matrices (depth range 0..1), `eye` its position.
    #[must_use]
    pub fn new(view: &[[f32; 4]; 4], view_proj: &[[f32; 4]; 4], eye: [f32; 3]) -> Self {
        let vp = DMat4::from_cols_array_2d(&view_proj.map(|c| c.map(f64::from)));
        let row = |i: usize| DVec4::new(vp.col(0)[i], vp.col(1)[i], vp.col(2)[i], vp.col(3)[i]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let normalise = |p: DVec4| {
            let n = p.truncate().length();
            if n > 0.0 {
                p / n
            } else {
                p
            }
        };
        let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(normalise);
        let inverse = vp.inverse();
        let corner = |x: f64, y: f64, z: f64| inverse.project_point3(DVec3::new(x, y, z));
        let corners = [
            corner(-1.0, -1.0, 0.0),
            corner(1.0, -1.0, 0.0),
            corner(1.0, 1.0, 0.0),
            corner(-1.0, 1.0, 0.0),
            corner(-1.0, -1.0, 1.0),
            corner(1.0, -1.0, 1.0),
            corner(1.0, 1.0, 1.0),
            corner(-1.0, 1.0, 1.0),
        ];
        let forward = DVec3::new(
            f64::from(view[0][2]),
            f64::from(view[1][2]),
            f64::from(view[2][2]),
        )
        .normalize_or_zero();
        Self {
            eye: DVec3::from(eye.map(f64::from)),
            forward,
            planes,
            corners,
            view_proj: vp,
        }
    }

    /// The camera's position.
    #[must_use]
    pub fn eye(&self) -> [f32; 3] {
        self.eye.as_vec3().to_array()
    }

    /// Whether the sphere meets the frustum (no plane has it entirely
    /// outside).
    #[must_use]
    pub fn sphere_visible(&self, centre: [f32; 3], radius: f32) -> bool {
        let c = DVec3::from(centre.map(f64::from));
        let r = f64::from(radius);
        self.planes.iter().all(|p| p.truncate().dot(c) + p.w >= -r)
    }

    /// `2 * ` this is the size the level rule reads: the distance between the
    /// projected centre and the projected point `radius` further along the
    /// view depth, in normalised device coordinates (x, y and the depth of
    /// the -1..1 range). Infinite when either point is not in front of the
    /// camera (a light that close fills the view).
    #[must_use]
    pub fn projected_size(&self, centre: [f32; 3], radius: f32) -> f32 {
        let c = DVec3::from(centre.map(f64::from));
        let behind = c + self.forward * f64::from(radius);
        let (a, b) = (
            self.view_proj * c.extend(1.0),
            self.view_proj * behind.extend(1.0),
        );
        if a.w < 1e-3 || b.w < 1e-3 {
            return f32::INFINITY;
        }
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        let d = b - a;
        // The -1..1 depth range is twice the 0..1 range's.
        (d.x * d.x + d.y * d.y + 4.0 * d.z * d.z).sqrt() as f32
    }

    /// Whether face `face` (0-5, [`face_axes`]) of a light at `light` with
    /// radius `radius` (its 90 degree frustum from [`FACE_NEAR`] to the
    /// radius) meets this frustum. Conservative: a separating plane of either
    /// frustum proves a miss, anything else counts as a meeting.
    #[must_use]
    pub fn meets_face(&self, light: [f32; 3], face: usize, radius: f32) -> bool {
        let l = DVec3::from(light.map(f64::from));
        let (axis, u, v) = face_axes(face);
        let (near, far) = (f64::from(FACE_NEAR), f64::from(radius));
        let mut corners = [DVec3::ZERO; 8];
        for (k, c) in corners.iter_mut().enumerate() {
            let depth = if k < 4 { near } else { far };
            let (su, sv) = match k & 3 {
                0 => (-1.0, -1.0),
                1 => (1.0, -1.0),
                2 => (1.0, 1.0),
                _ => (-1.0, 1.0),
            };
            *c = l + axis * depth + u * (su * depth) + v * (sv * depth);
        }
        // The camera's planes against the face's corners.
        if self
            .planes
            .iter()
            .any(|p| corners.iter().all(|c| p.truncate().dot(*c) + p.w < 0.0))
        {
            return false;
        }
        // The face's planes against the camera's corners: four sides
        // (45 degrees), the near and the far.
        let inward = [
            (axis + u) * std::f64::consts::FRAC_1_SQRT_2,
            (axis - u) * std::f64::consts::FRAC_1_SQRT_2,
            (axis + v) * std::f64::consts::FRAC_1_SQRT_2,
            (axis - v) * std::f64::consts::FRAC_1_SQRT_2,
        ];
        for n in inward {
            if self.corners.iter().all(|c| n.dot(*c - l) < 0.0) {
                return false;
            }
        }
        if self.corners.iter().all(|c| axis.dot(*c - l) < near) {
            return false;
        }
        !self.corners.iter().all(|c| axis.dot(*c - l) > far)
    }
}

/// A face's frustum axes in camera-local coordinates: the direction it looks
/// along and its two side directions. The faces are +X, -X, +Y, -Y, +Z, -Z of
/// the light-to-point vector with its z negated (the lookup's convention,
/// `point.wgsl`).
#[must_use]
pub fn face_axes(face: usize) -> (DVec3, DVec3, DVec3) {
    // Axes of the vector with z negated, mapped back: z flips sign.
    let flip = |v: DVec3| DVec3::new(v.x, v.y, -v.z);
    let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
    let (a, u, v) = match face / 2 {
        0 => (DVec3::X, DVec3::Y, DVec3::Z),
        1 => (DVec3::Y, DVec3::X, DVec3::Z),
        _ => (DVec3::Z, DVec3::X, DVec3::Y),
    };
    (flip(a * sign), flip(u), flip(v))
}

/// The priority key of a light (see the module docs), with the distance it
/// was made from. `forward` is the camera's unit view axis.
#[must_use]
pub fn priority(light: &LightInput, eye: [f32; 3], forward: [f32; 3]) -> (u32, f32) {
    let r = light.radius;
    // The vector to the centre, moved back along the view axis by the
    // radius: its length is the distance to the sphere's near side along
    // the view.
    let d = [
        light.pos[0] - eye[0] - forward[0] * r,
        light.pos[1] - eye[1] - forward[1] * r,
        light.pos[2] - eye[2] - forward[2] * r,
    ];
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    // The bitwise complement of the rounded distance: nearer is larger.
    let near = !((distance + 0.5) as i32) as u32;
    let key =
        near as f32 * WEIGHT_DISTANCE + (r * WEIGHT_RADIUS + light.intensity * WEIGHT_INTENSITY);
    (key as u32, distance)
}

/// The lights that qualify for a shadow, best first (see the module docs).
/// `max_distance` is the quality's maximum view distance.
#[must_use]
pub fn candidates(lights: &[LightInput], view: &ViewFrustum, max_distance: f32) -> Vec<Candidate> {
    let eye = view.eye();
    let forward = view.forward.as_vec3().to_array();
    let mut out: Vec<Candidate> = lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.casts && l.intensity > 0.0 && l.radius >= MIN_RADIUS)
        .filter(|(_, l)| view.sphere_visible(l.pos, l.radius * SPHERE_SCALE))
        .filter(|(_, l)| {
            let d = [l.pos[0] - eye[0], l.pos[1] - eye[1], l.pos[2] - eye[2]];
            let to_centre = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            (to_centre - l.radius).abs() < max_distance
        })
        .map(|(light, l)| {
            let (key, distance) = priority(l, eye, forward);
            Candidate {
                light,
                key,
                distance,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.key
            .cmp(&a.key)
            .then(a.distance.total_cmp(&b.distance))
            .then(a.light.cmp(&b.light))
    });
    out
}

/// The resolution level for a projected size (see the module docs): 0 (the
/// largest) when the light's sphere fills the view, the lowest level when it
/// is tiny.
#[must_use]
pub fn lod_level(size: f32, levels: usize) -> usize {
    let s = size + LOD_OFFSET;
    let t = if s <= 0.0 {
        1.0
    } else if s < 1.0 {
        1.0 - s
    } else {
        0.0
    };
    (((levels.saturating_sub(1)) as f32) * t + 0.5) as usize
}

/// The level of a light for a camera: inside its sphere the size is 1,
/// otherwise twice the projected size of the sphere.
#[must_use]
pub fn level_for(light: &LightInput, view: &ViewFrustum, levels: usize) -> usize {
    let eye = view.eye();
    let d = [
        light.pos[0] - eye[0],
        light.pos[1] - eye[1],
        light.pos[2] - eye[2],
    ];
    let inside = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < light.radius * light.radius;
    let size = if inside {
        1.0
    } else {
        2.0 * view.projected_size(light.pos, light.radius)
    };
    lod_level(size, levels)
}

#[derive(Clone, Copy, Debug)]
struct Held {
    light: usize,
    /// Frames since the light was last a candidate.
    absent: u32,
}

/// A light that holds a slot and is a candidate this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub slot: usize,
    pub light: usize,
}

/// The pool of shadow slots (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct SlotPool {
    slots: Vec<Option<Held>>,
}

impl SlotPool {
    /// Free every slot.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// The light in `slot`, if any.
    #[must_use]
    pub fn holder(&self, slot: usize) -> Option<usize> {
        self.slots.get(slot).copied().flatten().map(|h| h.light)
    }

    /// One frame: the pool has `capacity` slots; slotted lights absent from
    /// `candidates` (best first) for more than [`RELEASE_FRAMES`] frames are
    /// freed; candidates without a slot take the free slots in order; the
    /// result is the slotted lights that are candidates, by slot.
    pub fn update(&mut self, capacity: usize, candidates: &[Candidate]) -> Vec<Assignment> {
        self.slots.resize(capacity, None);
        for held in self.slots.iter_mut() {
            if let Some(h) = held {
                if candidates.iter().any(|c| c.light == h.light) {
                    h.absent = 0;
                } else {
                    h.absent += 1;
                    if h.absent > RELEASE_FRAMES {
                        *held = None;
                    }
                }
            }
        }
        for c in candidates {
            if self.slots.iter().flatten().any(|h| h.light == c.light) {
                continue;
            }
            match self.slots.iter_mut().find(|s| s.is_none()) {
                Some(free) => {
                    *free = Some(Held {
                        light: c.light,
                        absent: 0,
                    });
                }
                None => break,
            }
        }
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, h)| {
                h.filter(|h| candidates.iter().any(|c| c.light == h.light))
                    .map(|h| Assignment {
                        slot,
                        light: h.light,
                    })
            })
            .collect()
    }
}

/// Where each slot's faces live in the 2-D atlas.
///
/// A slot's block is `4 * base` wide and `2 * base` tall: level 0's six faces
/// in a 3 x 2 grid at the left; level 1 (half the size) in a 2 x 3 grid at the
/// right; level 2 (a quarter) in the row-major cells of a 4-wide grid under
/// it; level 3 (an eighth) in the cells left beside level 2's. Blocks tile the
/// atlas in a grid of one column up to two slots and two columns beyond; the
/// atlas is the next power of two over the blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasLayout {
    /// Level 0's face size.
    pub base: u32,
    pub levels: usize,
    pub slots: usize,
    pub width: u32,
    pub height: u32,
}

/// One face's square in the atlas, in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceRect {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

impl AtlasLayout {
    #[must_use]
    pub fn new(base: u32, levels: usize, slots: usize) -> Self {
        let columns = if slots <= 2 { 1 } else { 2 };
        let rows = slots.max(1).div_ceil(columns) as u32;
        Self {
            base,
            levels: levels.min(MAX_LEVELS),
            slots,
            width: (columns as u32 * 4 * base).next_power_of_two(),
            height: (rows * 2 * base).next_power_of_two(),
        }
    }

    /// Level `level`'s face size.
    #[must_use]
    pub fn face_size(&self, level: usize) -> u32 {
        self.base >> level
    }

    /// Slot `slot`'s block: `[x, y, width, height]` in texels.
    #[must_use]
    pub fn block(&self, slot: usize) -> [u32; 4] {
        let columns = if self.slots <= 2 { 1 } else { 2 };
        [
            (slot % columns) as u32 * 4 * self.base,
            (slot / columns) as u32 * 2 * self.base,
            4 * self.base,
            2 * self.base,
        ]
    }

    /// Face `face` of `slot` at `level`.
    #[must_use]
    pub fn rect(&self, slot: usize, level: usize, face: usize) -> FaceRect {
        let [bx, by, _, _] = self.block(slot);
        let s = self.face_size(level);
        let (ox, oy, columns) = match level {
            0 => (0, 0, 3),
            1 => (3 * self.base, 0, 2),
            2 => (3 * self.base, 3 * self.base / 2, 4),
            _ => (7 * self.base / 2, 7 * self.base / 4, 4),
        };
        FaceRect {
            x: bx + ox + (face as u32 % columns) * s,
            y: by + oy + (face as u32 / columns) * s,
            size: s,
        }
    }

    /// The atlas texture coordinates of a face: its scale and texel size
    /// (`[face/width, face/height, 1/width, 1/height]`) and the six faces'
    /// extents `[u0, v0, u1, v1]`.
    #[must_use]
    pub fn uv(&self, slot: usize, level: usize) -> ([f32; 4], [[f32; 4]; 6]) {
        let (w, h) = (self.width as f32, self.height as f32);
        let s = self.face_size(level) as f32;
        let extents = std::array::from_fn(|f| {
            let r = self.rect(slot, level, f);
            [
                r.x as f32 / w,
                r.y as f32 / h,
                (r.x + r.size) as f32 / w,
                (r.y + r.size) as f32 / h,
            ]
        });
        ([s / w, s / h, 1.0 / w, 1.0 / h], extents)
    }
}
