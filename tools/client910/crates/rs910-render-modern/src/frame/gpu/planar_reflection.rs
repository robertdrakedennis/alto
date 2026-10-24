//! The planar water reflection's draw list (lane P4-GPU): the frame's model
//! draws whose boxes meet the reflected frustum (`models::bounds`), in the
//! frame's order.
//!
//! The reflection pass used to draw every draw of the frame (the modern
//! client draws its own gather of the reflected view; the port drew the main
//! view's list). A draw wholly outside the reflected clip
//! volume (outside the mirrored view's sides or far plane, or below the
//! water: the oblique near plane is the water plane) writes no texel of the
//! reflection, so the culled list gives the same reflection image. Draws
//! without a box (floors or unknown geometry) are always drawn. Far merged
//! containers carry boxes of only the selected models in each compatible run.
//!
//! Models smaller than [`REFLECTION_MIN_EXTENT`] are left out too: in a
//! rippled, distorted reflection small props are invisible, and they are most
//! of the scene's draws.
//!
//! The water reads the reflection only on its planar surface, at its own
//! pixel moved by at most the distortion's reach ([`reflection_margin`]).
//! The screen rectangles of the planar water's visible boxes
//! ([`water_boxes`]) widened by that reach mark the cells of a
//! [`WaterMask`]; their bounding rectangle is the pass's scissor, and a draw
//! whose reflected box meets no marked cell is left out. A texel outside
//! them is never read.

use crate::frame::*;
use crate::models::bounds::{outside_clip, Bounds};
use crate::models::mesh::Vertex;

/// The smallest model (longest box side, fine units; a tile is 512) the
/// water reflection draws.
pub(crate) const REFLECTION_MIN_EXTENT: f32 = 512.0;

/// The side of a cell of [`water_boxes`] (fine units: four tiles).
pub(crate) const WATER_BOX_SIDE: f32 = 2048.0;

/// The largest screen length of the water's distortion direction: the
/// difference of two unit normals (`water.wgsl`'s `screen_normal`).
const DISTORTION_DIRECTION_MAX: f32 = 2.0;

/// The bilinear footprint of a reflection sample, in reflection texels.
const SAMPLE_FOOTPRINT_TEXELS: f32 = 1.0;

/// The boxes (the vertices' space) of a triangle list's triangles by
/// the [`WATER_BOX_SIDE`] cell of the XZ plane their centre is in.
pub(crate) fn water_boxes(vertices: &[Vertex]) -> Vec<Bounds> {
    let mut cells: std::collections::BTreeMap<(i32, i32), Bounds> = Default::default();
    for triangle in vertices.chunks_exact(3) {
        let Some(b) = Bounds::of(triangle) else {
            continue;
        };
        let cell = |k: usize| ((b.min[k] + b.max[k]) * 0.5 / WATER_BOX_SIDE).floor() as i32;
        cells
            .entry((cell(0), cell(2)))
            .and_modify(|c| {
                for k in 0..3 {
                    c.min[k] = c.min[k].min(b.min[k]);
                    c.max[k] = c.max[k].max(b.max[k]);
                }
            })
            .or_insert(b);
    }
    cells.into_values().collect()
}

/// How far (pixels of a viewport `height` tall, reflection texels of
/// `divisor` pixels) from its own pixel the water samples the reflection:
/// the distortion's reach, the blur's and a sample's footprint.
pub(crate) fn reflection_margin(height: f32, divisor: f32) -> f32 {
    use crate::water_body::{
        DISTORTION_REFERENCE_HEIGHT, LOOK, REFLECTION_FILTER_RADIUS, REFLECTION_OFFSET_LIMIT,
    };
    let reach = DISTORTION_DIRECTION_MAX * REFLECTION_OFFSET_LIMIT * LOOK.distortion
        + REFLECTION_FILTER_RADIUS;
    reach * height / DISTORTION_REFERENCE_HEIGHT + SAMPLE_FOOTPRINT_TEXELS * divisor
}

/// The sides and the near plane of a wgpu clip volume (`-w <= x, y <= w`,
/// `0 <= z`) as homogeneous plane coefficients: inside where the dot
/// product with a clip-space point is not negative.
const CLIP_SIDES: [glam::Vec4; 5] = [
    glam::Vec4::new(1.0, 0.0, 0.0, 1.0),
    glam::Vec4::new(-1.0, 0.0, 0.0, 1.0),
    glam::Vec4::new(0.0, 1.0, 0.0, 1.0),
    glam::Vec4::new(0.0, -1.0, 0.0, 1.0),
    glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
];

/// The most vertices a box face clipped by [`CLIP_SIDES`] can have (four,
/// one more per plane).
const CLIPPED_FACE_VERTICES: usize = 4 + CLIP_SIDES.len();

/// The corner indices (bit 0: x, bit 1: y, bit 2: z at the maximum) of
/// each face of a box, in order around the face.
const BOX_FACES: [[usize; 4]; 6] = [
    [0, 2, 6, 4],
    [1, 3, 7, 5],
    [0, 1, 5, 4],
    [2, 3, 7, 6],
    [0, 1, 3, 2],
    [4, 5, 7, 6],
];

/// Merge the normalized device point of clip-space `p` into `rect`.
fn extend_rect(rect: &mut Option<[f32; 4]>, p: glam::Vec4) {
    if p.w <= f32::EPSILON {
        return;
    }
    let (x, y) = (p.x / p.w, p.y / p.w);
    let r = rect.get_or_insert([x, y, x, y]);
    *r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
}

/// The screen as a normalized device rectangle (`[x0, y0, x1, y1]`).
const SCREEN: [f32; 4] = [-1.0, -1.0, 1.0, 1.0];

/// The normalized device rectangle `[x0, y0, x1, y1]` the part of `b`
/// inside the clip volume of `view_proj` projects to, merged into `rect`:
/// a box wholly before the eye and the near plane by its corners (the
/// bounds of its whole projection), one across them by its faces clipped
/// to the volume's sides and near plane (by its corners before the eye
/// when the clipping leaves only rounding: [`Bounds::outside`] keeps it).
fn extend_screen_rect(rect: &mut Option<[f32; 4]>, b: &Bounds, view_proj: &glam::Mat4) {
    let corners = b.clip_corners(view_proj);
    if outside_clip(&corners) {
        return;
    }
    if corners.iter().all(|c| c.z >= 0.0 && c.w > f32::EPSILON) {
        for c in corners {
            extend_rect(rect, c);
        }
        return;
    }
    let mut clipped_rect = None;
    for face in BOX_FACES {
        let mut polygon = [glam::Vec4::ZERO; CLIPPED_FACE_VERTICES];
        let mut len = face.len();
        for (slot, &corner) in polygon.iter_mut().zip(&face) {
            *slot = corners[corner];
        }
        for plane in CLIP_SIDES {
            let mut clipped = [glam::Vec4::ZERO; CLIPPED_FACE_VERTICES];
            let mut out = 0;
            for i in 0..len {
                let (a, b) = (polygon[i], polygon[(i + 1) % len]);
                let (da, db) = (plane.dot(a), plane.dot(b));
                if da >= 0.0 {
                    clipped[out] = a;
                    out += 1;
                }
                if (da >= 0.0) != (db >= 0.0) {
                    clipped[out] = a + (b - a) * (da / (da - db));
                    out += 1;
                }
            }
            polygon = clipped;
            len = out;
        }
        for &p in &polygon[..len] {
            extend_rect(&mut clipped_rect, p);
        }
    }
    match clipped_rect {
        Some([x0, y0, x1, y1]) => {
            extend_rect(rect, glam::Vec4::new(x0, y0, 0.0, 1.0));
            extend_rect(rect, glam::Vec4::new(x1, y1, 0.0, 1.0));
        }
        None => {
            for c in corners {
                extend_rect(rect, c);
            }
        }
    }
}

/// The texels (`x, y, width, height`, at least one) of a `size` texture
/// the normalized device rectangle `rect` covers, rounded outwards.
fn texel_rect(rect: [f32; 4], size: [u32; 2]) -> [u32; 4] {
    let [w, h] = size.map(|s| s as f32);
    let x0 = ((rect[0] + 1.0) * 0.5 * w).floor().clamp(0.0, w - 1.0);
    let x1 = ((rect[2] + 1.0) * 0.5 * w).ceil().clamp(x0 + 1.0, w);
    let y0 = ((1.0 - rect[3]) * 0.5 * h).floor().clamp(0.0, h - 1.0);
    let y1 = ((1.0 - rect[1]) * 0.5 * h).ceil().clamp(y0 + 1.0, h);
    [x0, y0, x1 - x0, y1 - y0].map(|v| v as u32)
}

/// Cells per side of a [`WaterMask`] (one row is a `u64`).
const MASK_CELLS: usize = u64::BITS as usize;

/// The screen cells (a [`MASK_CELLS`] square grid over the normalized
/// device square) the water reads the reflection in.
pub(crate) struct WaterMask {
    rows: [u64; MASK_CELLS],
}

impl WaterMask {
    /// The cell columns and rows (inclusive) normalized device rectangle
    /// `rect` touches: a column mask and the rows.
    fn cells(rect: [f32; 4]) -> (u64, std::ops::RangeInclusive<usize>) {
        let cell = |v: f32| {
            let t = (v - SCREEN[0]) / (SCREEN[2] - SCREEN[0]);
            ((t * MASK_CELLS as f32).floor().max(0.0) as usize).min(MASK_CELLS - 1)
        };
        let (c0, c1) = (cell(rect[0]), cell(rect[2]));
        let columns = (u64::MAX >> (MASK_CELLS - 1 - c1)) & (u64::MAX << c0);
        (columns, cell(rect[1])..=cell(rect[3]))
    }

    fn mark(&mut self, rect: [f32; 4]) {
        let (columns, rows) = Self::cells(rect);
        for row in &mut self.rows[rows] {
            *row |= columns;
        }
    }

    /// Whether `rect` touches a marked cell.
    fn meets(&self, rect: [f32; 4]) -> bool {
        let (columns, rows) = Self::cells(rect);
        self.rows[rows].iter().any(|row| row & columns != 0)
    }
}

/// Where the frame's water reads its reflection: the [`WaterMask`] and the
/// marked rectangles' bounds (normalized device `[x0, y0, x1, y1]`).
pub(crate) struct WaterScreen {
    pub(crate) mask: WaterMask,
    pub(crate) rect: [f32; 4],
}

impl ModernRenderer {
    /// Where the frame's planar water (its boxes within the tolerance of
    /// the plane at camera-local `plane`) under `view_proj` reads its
    /// reflection: each visible box's screen rectangle widened by `margin`
    /// pixels of a `size` viewport; `None`: no box is on screen.
    pub(crate) fn planar_water_screen(
        &self,
        view_proj: &glam::Mat4,
        origin: [f32; 3],
        plane: f32,
        margin: f32,
        size: [f32; 2],
    ) -> Option<WaterScreen> {
        let water = &self.frame_resources.water;
        let tolerance = crate::frame::gpu::water::PLANE_TOLERANCE;
        let (mx, my) = (2.0 * margin / size[0], 2.0 * margin / size[1]);
        let mut mask = WaterMask {
            rows: [0; MASK_CELLS],
        };
        let mut bounds: Option<[f32; 4]> = None;
        for d in &water.draws {
            let Some(boxes) = water
                .levels
                .get(d.level)
                .and_then(Option::as_ref)
                .and_then(|l| l.mesh.as_ref())
                .and_then(|m| m.boxes.get(&d.batch))
            else {
                continue;
            };
            for b in boxes {
                let local = Bounds {
                    min: std::array::from_fn(|k| b.min[k] - origin[k]),
                    max: std::array::from_fn(|k| b.max[k] - origin[k]),
                };
                if local.min[1] > plane + tolerance || local.max[1] < plane - tolerance {
                    continue;
                }
                let mut rect = None;
                extend_screen_rect(&mut rect, &local, view_proj);
                let Some([x0, y0, x1, y1]) = rect else {
                    continue;
                };
                let r = [
                    (x0 - mx).max(SCREEN[0]),
                    (y0 - my).max(SCREEN[1]),
                    (x1 + mx).min(SCREEN[2]),
                    (y1 + my).min(SCREEN[3]),
                ];
                mask.mark(r);
                let u = bounds.get_or_insert(r);
                *u = [
                    u[0].min(r[0]),
                    u[1].min(r[1]),
                    u[2].max(r[2]),
                    u[3].max(r[3]),
                ];
            }
        }
        bounds.map(|rect| WaterScreen { mask, rect })
    }

    /// This frame's reflection list and scissor under the reflected
    /// camera's `clip` (camera-local to clip) for the water's `screen`
    /// (`None`: it reads no texel) and a reflection of `size` texels.
    pub(crate) fn cull_reflection(
        &mut self,
        clip: &glam::Mat4,
        screen: Option<WaterScreen>,
        size: [u32; 2],
    ) {
        self.frame_resources.water.reflected.clear();
        #[cfg(test)]
        let cull = !self.frame_resources.water.test_no_reflection_cull;
        #[cfg(not(test))]
        let cull = true;
        // Without culling the whole reflection is drawn (the tests'
        // reference); with it, nothing when the water reads no texel.
        let rect = if cull {
            screen.as_ref().map(|s| s.rect)
        } else {
            Some(SCREEN)
        };
        self.frame_resources.water.reflection_scissor = rect.map(|r| texel_rect(r, size));
        let reflected = &mut self.frame_resources.water.reflected;
        let mut ranges = self.frame_resources.draw_bounds.iter().peekable();
        let mut culled = 0;
        let mut out = None;
        for i in 0..self.frame_resources.draws.len() as u32 {
            // The box of the entity whose range holds draw `i`, tested
            // once per entity.
            while ranges.next_if(|r| r.1 <= i).is_some() {
                out = None;
            }
            // Batched locs reflect from their container; small props are
            // never reflected. Neither counts as culled.
            let skip = self.frame_resources.draws[i as usize].batched
                || matches!(ranges.peek(), Some(&&(start, _, bounds))
                    if start <= i && bounds.extent() < REFLECTION_MIN_EXTENT);
            if skip {
                continue;
            }
            // A box hides its draws when its part in the reflected volume
            // meets no cell the water reads.
            let hidden = match (ranges.peek(), &rect) {
                (_, None) => true,
                (Some(&&(start, _, bounds)), Some(_)) if cull && start <= i => *out
                    .get_or_insert_with(|| {
                        let mut reflected = None;
                        extend_screen_rect(&mut reflected, &bounds, clip);
                        reflected.is_none_or(|r| screen.as_ref().is_some_and(|s| !s.mask.meets(r)))
                    }),
                _ => false,
            };
            if hidden {
                culled += 1;
            } else {
                reflected.push(i);
            }
        }
        self.frame_resources.water.stats.reflection_draws = reflected.len();
        self.frame_resources.water.stats.reflection_culled = culled;
    }
}
