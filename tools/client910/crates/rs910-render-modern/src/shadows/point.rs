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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadows::presets::point_preset;
    use crate::shadows::Quality;

    /// A camera at the origin looking along +z with a 90 degree lens, far
    /// 40000 (classic view: the third row is the forward axis).
    fn camera() -> ViewFrustum {
        let view = glam::Mat4::IDENTITY;
        let proj = glam::Mat4::perspective_lh(std::f32::consts::FRAC_PI_2, 1.0, 50.0, 40_000.0);
        ViewFrustum::new(
            &view.to_cols_array_2d(),
            &(proj * view).to_cols_array_2d(),
            [0.0; 3],
        )
    }

    fn light(pos: [f32; 3], radius: f32) -> LightInput {
        LightInput {
            pos,
            radius,
            intensity: 1.0,
            casts: true,
        }
    }

    #[test]
    fn candidates_pass_every_rule() {
        let cam = camera();
        let ok = light([0.0, 0.0, 2000.0], 768.0);
        let off = LightInput {
            intensity: 0.0,
            ..ok
        };
        let no_flag = LightInput { casts: false, ..ok };
        let small = light([0.0, 0.0, 2000.0], 349.0);
        let edge = light([0.0, 0.0, 2000.0], 350.0);
        // Behind the camera, its sphere (0.8 of the radius) out of view.
        let behind = light([0.0, 0.0, -2000.0], 768.0);
        // Behind the camera but its 0.8 sphere reaches the frustum.
        let clipping = light([0.0, 0.0, -500.0], 768.0);
        // Beside the view: the plane at x = z; centre 3000 out at depth
        // 1000, 0.8R = 614 cannot reach.
        let beside = light([3000.0, 0.0, 1000.0], 768.0);
        // Past the maximum distance (Low: 10000).
        let far = light([0.0, 0.0, 12_000.0], 768.0);
        let all = [ok, off, no_flag, small, edge, behind, clipping, beside, far];
        let picked: Vec<usize> = candidates(&all, &cam, 10_000.0)
            .iter()
            .map(|c| c.light)
            .collect();
        assert!(picked.contains(&0), "a lit, casting light in view");
        assert!(!picked.contains(&1), "intensity 0");
        assert!(!picked.contains(&2), "the flag is clear");
        assert!(!picked.contains(&3), "radius 349");
        assert!(picked.contains(&4), "radius 350 is enough");
        assert!(!picked.contains(&5), "its sphere is behind the camera");
        assert!(picked.contains(&6), "the 0.8 sphere meets the frustum");
        assert!(!picked.contains(&7), "beside the view");
        assert!(!picked.contains(&8), "past the maximum distance");
        // The distance rule is to the sphere's surface: a light whose centre
        // is past the maximum but whose surface is within it qualifies.
        let edge_on = light([0.0, 0.0, 10_500.0], 768.0);
        assert_eq!(candidates(&[edge_on], &cam, 10_000.0).len(), 1);
        assert_eq!(candidates(&[edge_on], &cam, 9_000.0).len(), 0);
    }

    #[test]
    fn the_score_orders_nearer_first_and_weighs_intensity() {
        let cam = camera();
        let order = |lights: &[LightInput]| -> Vec<usize> {
            candidates(lights, &cam, 25_000.0)
                .iter()
                .map(|c| c.light)
                .collect()
        };
        let near = light([0.0, 0.0, 1000.0], 768.0);
        let far = light([0.0, 0.0, 6000.0], 768.0);
        assert_eq!(order(&[far, near]), vec![1, 0], "nearer first");
        // The intensity term (0.3 per unit, against 0.6 per unit of
        // distance) outweighs a thousand units of distance at 3000.
        let bright = LightInput {
            intensity: 3000.0,
            ..light([0.0, 0.0, 2000.0], 768.0)
        };
        assert_eq!(order(&[near, bright]), vec![1, 0], "a bright light wins");
        // Lights within a quantisation step of one another tie (the `f32`
        // rounding swallows the small distance and radius terms) and order by
        // the true distance, then the id.
        let a = light([30.0, 0.0, 3000.0], 768.0);
        let b = light([-20.0, 0.0, 3000.0], 768.0);
        let c = candidates(&[a, b], &cam, 25_000.0);
        assert_eq!(c[0].key, c[1].key);
        assert_eq!((c[0].light, c[1].light), (1, 0));
        // A very large radius does (R * 0.1 past 128 shifts the key).
        let huge = light([0.0, 0.0, 3400.0], 2400.0);
        let plain = light([0.0, 0.0, 3400.0], 768.0);
        let (kh, _) = priority(&huge, [0.0; 3], [0.0, 0.0, 1.0]);
        let (kp, _) = priority(&plain, [0.0; 3], [0.0, 0.0, 1.0]);
        assert_ne!(kh, kp);
    }

    fn cand(light: usize, key: u32) -> Candidate {
        Candidate {
            light,
            key,
            distance: 0.0,
        }
    }

    #[test]
    fn a_slotted_light_is_never_evicted() {
        let mut pool = SlotPool::default();
        let first = pool.update(2, &[cand(7, 90), cand(8, 80), cand(9, 70)]);
        assert_eq!(
            first,
            vec![
                Assignment { slot: 0, light: 7 },
                Assignment { slot: 1, light: 8 }
            ],
            "the best two take the two slots"
        );
        // A newcomer scoring above both gets no slot.
        let second = pool.update(2, &[cand(1, 500), cand(7, 90), cand(8, 80)]);
        assert_eq!(second.len(), 2);
        assert!(second.iter().all(|a| a.light != 1));
        assert_eq!(pool.holder(0), Some(7));
        // Slots stay with their lights when the order changes.
        let third = pool.update(2, &[cand(8, 600), cand(7, 90)]);
        assert_eq!(third[0], Assignment { slot: 0, light: 7 });
        assert_eq!(third[1], Assignment { slot: 1, light: 8 });
    }

    #[test]
    fn a_slot_is_freed_after_the_light_stays_away() {
        let mut pool = SlotPool::default();
        pool.update(1, &[cand(3, 10)]);
        // Absent: still held for the grace frames, and the newcomer waits.
        for _ in 0..RELEASE_FRAMES {
            let out = pool.update(1, &[cand(4, 99)]);
            assert!(out.is_empty(), "the held light is not a candidate");
            assert_eq!(pool.holder(0), Some(3));
        }
        // The light coming back in time keeps the slot.
        assert_eq!(
            pool.update(1, &[cand(4, 99), cand(3, 10)]),
            vec![Assignment { slot: 0, light: 3 }]
        );
        // Away for good: freed, and the newcomer takes it.
        for _ in 0..=RELEASE_FRAMES {
            pool.update(1, &[]);
        }
        assert_eq!(pool.holder(0), None);
        assert_eq!(
            pool.update(1, &[cand(4, 99)]),
            vec![Assignment { slot: 0, light: 4 }]
        );
    }

    #[test]
    fn the_pool_follows_the_quality_capacity() {
        let counts: Vec<usize> = Quality::ALL
            .iter()
            .map(|&q| point_preset(q).lights)
            .collect();
        assert_eq!(counts, [2, 2, 3, 4, 4]);
        let cands: Vec<Candidate> = (0..9).map(|i| cand(i, 100 - i as u32)).collect();
        for &n in &counts {
            let mut pool = SlotPool::default();
            assert_eq!(pool.update(n, &cands).len(), n);
        }
        // A smaller pool (a quality change) drops the extra slots' lights.
        let mut pool = SlotPool::default();
        pool.update(4, &cands);
        let out = pool.update(2, &cands);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_level_follows_the_projected_size() {
        // Big on screen: level 0; tiny: the lowest.
        assert_eq!(lod_level(2.0, 4), 0);
        assert_eq!(lod_level(1.1, 4), 0);
        assert_eq!(lod_level(0.05, 4), 3);
        assert_eq!(lod_level(0.0, 4), 3);
        assert_eq!(lod_level(-1.0, 3), 2);
        // Between: 1 - (size - 0.1) scaled to the levels, rounded.
        assert_eq!(lod_level(0.6, 3), 1);
        assert_eq!(lod_level(0.6, 2), 1);
        assert_eq!(lod_level(0.9, 2), 0);
        // A single level is always 0.
        assert_eq!(lod_level(0.01, 1), 0);
        // Levels only ever get smaller with distance, for a light off the
        // view axis (where the projected measure has a lateral part).
        let cam = camera();
        let level = |z: f32| level_for(&light([600.0, 0.0, z], 768.0), &cam, 4);
        let by_distance: Vec<usize> = [900.0, 1500.0, 3000.0, 6000.0, 12_000.0, 25_000.0]
            .map(level)
            .to_vec();
        assert!(
            by_distance.windows(2).all(|w| w[0] <= w[1]),
            "{by_distance:?}"
        );
        assert_eq!(by_distance[5], 3, "{by_distance:?}");
        // Close to the camera and well off the view axis the sphere spans
        // much of the view: the largest level.
        assert_eq!(level_for(&light([1000.0, 0.0, 300.0], 768.0), &cam, 4), 0);
        // Inside the sphere: size 1, level 0.
        assert_eq!(level_for(&light([0.0, 0.0, 300.0], 768.0), &cam, 4), 0);
    }

    #[test]
    fn the_projected_size_grows_as_the_light_nears() {
        let cam = camera();
        let s = |z: f32| cam.projected_size([0.0, 0.0, z], 768.0);
        assert!(s(1000.0) > s(2000.0) && s(2000.0) > s(8000.0));
        assert!(s(8000.0) > 0.0);
        assert_eq!(s(-10.0), f32::INFINITY);
    }

    #[test]
    fn faces_meet_the_camera_only_where_they_can_be_seen() {
        let cam = camera();
        // A light in the view: its apex is in the frustum, every face meets.
        let ahead = [0.0, 0.0, 3000.0];
        assert!((0..6).all(|f| cam.meets_face(ahead, f, 768.0)));
        // A light far to the side, out of view: no face meets.
        let side = [40_000.0, 0.0, 1000.0];
        assert!((0..6).all(|f| !cam.meets_face(side, f, 768.0)));
        // A light just outside the right edge: the face looking further out
        // (+x) cannot be seen, the one looking back into the view (-x) can.
        let beside = [2500.0, 0.0, 1500.0];
        assert!(!cam.meets_face(beside, 0, 768.0));
        assert!(cam.meets_face(beside, 1, 768.0));
        // A light behind the camera with the view axis pointing away.
        let behind = [0.0, 0.0, -3000.0];
        assert!((0..6).all(|f| !cam.meets_face(behind, f, 768.0)));
    }

    #[test]
    fn face_axes_are_orthonormal_and_cover_the_sphere() {
        for f in 0..6 {
            let (a, u, v) = face_axes(f);
            assert!((a.length() - 1.0).abs() < 1e-9);
            assert!(a.dot(u).abs() < 1e-9 && a.dot(v).abs() < 1e-9 && u.dot(v).abs() < 1e-9);
        }
        // Opposite faces look opposite ways.
        for k in 0..3 {
            let (a, _, _) = face_axes(2 * k);
            let (b, _, _) = face_axes(2 * k + 1);
            assert!((a + b).length() < 1e-9);
        }
    }

    #[test]
    fn the_tables_follow_the_quality() {
        use crate::shadows::presets::{PointShadowUniforms, ShadowedLight, POINT_DEPTH_BIAS};
        // (lights, levels, face, maximum distance) of Low..Ultra+.
        let table = [
            (2, 2, 256, 10_000.0),
            (2, 3, 256, 12_500.0),
            (3, 3, 512, 16_666.666),
            (4, 4, 512, 25_000.0),
            (4, 4, 1024, 25_000.0),
        ];
        for (q, (lights, levels, face, max)) in Quality::ALL.iter().zip(table) {
            let p = point_preset(*q);
            assert_eq!(
                (p.lights, p.levels, p.face, p.max_view_distance),
                (lights, levels, face, max),
                "{q:?}"
            );
            assert_eq!((p.fade_from, p.normal_bias), (0.85, -50.0));
        }
        assert_eq!(POINT_DEPTH_BIAS, [-0.0045, -0.0095, -0.0145, -0.0195]);
        // The block: each light's level picks its depth bias and its faces'
        // rectangles; the fade runs from 0.85 of the maximum distance to it.
        let p = point_preset(Quality::Ultra);
        let layout = AtlasLayout::new(p.face, p.levels, p.lights);
        let lights: Vec<ShadowedLight> = [(5_u32, 1, 0), (9, 2, 3)]
            .iter()
            .map(|&(light_slot, slot, level)| {
                let (scale, faces) = layout.uv(slot, level);
                ShadowedLight {
                    light_slot,
                    level,
                    scale,
                    faces,
                }
            })
            .collect();
        let block = PointShadowUniforms::new(&p, &lights);
        assert_eq!(block.ids, [5, 9, 0, 0]);
        assert_eq!(block.params[3], 2.0);
        assert_eq!(block.params[0], 25_000.0);
        assert_eq!(block.lights[0].bias[2], -0.0045);
        assert_eq!(block.lights[1].bias[2], -0.0195);
        assert_eq!(block.lights[0].bias[1], -50.0);
        let f = block.lights[0].fade;
        assert_eq!((f[0], f[1], f[2]), (1.0, 21_250.0, 25_000.0));
        assert!((f[3] - 1.0 / 3_750.0).abs() < 1e-9);
        // Level 3's face is 64 texels of the 4096 x 2048 atlas.
        assert!((block.lights[1].scale[0] - 64.0 / 4096.0).abs() < 1e-9);
        assert!((block.lights[1].scale[2] - 1.0 / 4096.0).abs() < 1e-9);
        // The unused entries are off.
        assert_eq!(block.lights[2].fade[0], 0.0);
    }

    #[test]
    fn the_atlas_holds_every_face_once() {
        for q in Quality::ALL {
            let p = point_preset(q);
            let layout = AtlasLayout::new(p.face, p.levels, p.lights);
            assert!(layout.width.is_power_of_two() && layout.height.is_power_of_two());
            let mut used = vec![0_u8; (layout.width * layout.height / 64) as usize];
            let stride = (layout.width / 8) as usize;
            for slot in 0..p.lights {
                for level in 0..p.levels {
                    for face in 0..6 {
                        let r = layout.rect(slot, level, face);
                        assert_eq!(r.size, p.face >> level);
                        assert!(
                            r.x + r.size <= layout.width && r.y + r.size <= layout.height,
                            "{q:?} {slot}/{level}/{face} outside"
                        );
                        // Faces are multiples of 32 texels: mark 8x8 cells.
                        for y in (r.y..r.y + r.size).step_by(8) {
                            for x in (r.x..r.x + r.size).step_by(8) {
                                let cell = (y / 8) as usize * stride + (x / 8) as usize;
                                assert_eq!(used[cell], 0, "{q:?} {slot}/{level}/{face} overlaps");
                                used[cell] = 1;
                            }
                        }
                    }
                }
            }
        }
        // The sizes: Ultra+ at 1024 with four slots is an 8192 x 4096 atlas.
        let p = point_preset(Quality::UltraPlus);
        let l = AtlasLayout::new(p.face, p.levels, p.lights);
        assert_eq!((l.width, l.height), (8192, 4096));
        let p = point_preset(Quality::Low);
        let l = AtlasLayout::new(p.face, p.levels, p.lights);
        assert_eq!((l.width, l.height), (1024, 1024));
    }
}
