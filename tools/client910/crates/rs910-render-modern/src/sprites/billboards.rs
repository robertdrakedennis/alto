//! The modern renderer's model billboards (renderer plan M9), CPU half: which
//! camera-facing quads the frame's models carry (lamp glows, fire and
//! candle sprites), where they are, their colour, material and depth state,
//! and after which entity draw they go. No GPU types, so the shell's
//! `CLIENT910_MODERN_CHECK` compares the set with the faithful backend's
//! (`rs910_render_gpu::billboard_render::Frame`).
//!
//! # Which billboards, and where (the faithful behaviour, kept)
//!
//! A billboard is a model face the model's billboard table replaces by a
//! sprite (`rs910_model::billboard`). The classic renderer draws them at the
//! end of a model's draw, right after the model's own batches, and so does
//! this module:
//!
//! - the model's frustum early-out first (its bounding cylinder against the
//!   toolkit's frustum planes): [`culled`], [`frustum_planes`];
//! - billboards whose type is flagged bloom-hidden are skipped while the
//!   classic toolkit's bloom is on (the glow comes from the bloom there), so
//!   the set follows the faithful toolkit's bloom state ([`View::bloom`]);
//! - each quad is placed by the classic billboard maths
//!   (`rs910_model::billboard::place`: the face centroid through the
//!   model-view matrix, pulled `depth_offset` units towards the eye, the unit
//!   quad scaled by the billboard size and its animated scale, rolled and
//!   offset in view space) and taken back to the scene frame through the
//!   inverse camera, so its corners are camera-local like every other
//!   modern draw;
//! - its colour is the face colour and alpha (the diffuse colour), its
//!   texture the billboard type's material, its depth writes
//!   `!hasTransparency` of the owning model, its alpha test the reference
//!   the model's last batch left (`MeshBillboards::alpha_ref`);
//! - a segment per model: its quads belong right after that model's draw
//!   in the same list (0 opaque, 1 transparent), `at` being the model's
//!   position in the list plus one, as the faithful scene lists place them
//!   (the check compares this; the renderer draws each segment where the
//!   modern forward groups put it, `crate::frame::sprites`).
//!
//! The billboards of a rotated or scaled loc are taken from the model with
//! its scale-rotate transform applied to the vertices, as the faithful
//! meshes are built (`rs910_scene::scene::srt_model`), so the centroids, the
//! cull bounds and the quads are the faithful ones.
//!
//! Where they draw and how they are shaded (the modern forward groups, HDR,
//! blending, fog, no depth writes) is the renderer's
//! (`crate::frame::sprites`, [`crate::shaders`]).

use std::ops::Range;

use crate::actor_matrix::Matrix;
use crate::billboard::QUAD_CORNERS;
use crate::mesh_billboards::MeshBillboards;
use crate::models::draw_list::{DrawList, EntityDraw};
use crate::scene_snapshot::SceneSnapshot;

/// The classic toolkit's frustum planes of the `view * projection` entries:
/// near, far, left, right, bottom, top, each normalised by its normal's
/// length.
#[must_use]
pub fn frustum_planes(vp: &[f32; 16]) -> [[f32; 4]; 6] {
    let plane = |col: usize, sign: f32| -> [f32; 4] {
        let nx = vp[3] + sign * vp[col];
        let ny = vp[7] + sign * vp[4 + col];
        let nz = vp[11] + sign * vp[8 + col];
        let length = f64::from(nz * nz + nx * nx + ny * ny).sqrt();
        [
            (f64::from(nx) / length) as f32,
            (f64::from(ny) / length) as f32,
            (f64::from(nz) / length) as f32,
            (f64::from(vp[15] + sign * vp[12 + col]) / length) as f32,
        ]
    };
    [
        plane(2, 1.0),
        plane(2, -1.0),
        plane(0, 1.0),
        plane(0, -1.0),
        plane(1, 1.0),
        plane(1, -1.0),
    ]
}

/// The classic model draw's early-out: a model without
/// drawn vertices, or whose bounding cylinder (the model's y range at its
/// origin, the horizontal radius) lies outside one of `planes`, draws no
/// billboard. `world` is the model matrix in the camera-local frame.
#[must_use]
pub fn culled(b: &MeshBillboards, world: &[f32; 16], planes: &[[f32; 4]; 6]) -> bool {
    if !b.drawn {
        return true;
    }
    let (min_y, max_y, radius) = b.bounds;
    let point = |y: f32| -> [f32; 3] { std::array::from_fn(|i| world[4 + i] * y + world[12 + i]) };
    let low = point(min_y as f32);
    let high = point(max_y as f32);
    planes.iter().any(|p| {
        let a = p[2] * low[2] + p[0] * low[0] + p[1] * low[1] + p[3] + radius as f32;
        let b = p[2] * high[2] + p[0] * high[0] + p[1] * high[1] + p[3] + radius as f32;
        a < 0.0 && b < 0.0
    })
}

/// The camera inputs of one frame's billboards (the faithful
/// `billboard_render::View`'s).
#[derive(Clone, Debug)]
pub struct View {
    /// The camera's view entries with the camera target as
    /// the origin (the camera-local frame).
    pub view: [f32; 16],
    /// Its inverse.
    pub inverse: Matrix,
    /// The frustum planes ([`frustum_planes`]).
    pub planes: [[f32; 4]; 6],
    /// The faithful toolkit's bloom is on (the toolkit's bloom check): the
    /// bloom-hidden billboards are skipped.
    pub bloom: bool,
}

impl View {
    /// The view of `snapshot`'s camera (its viewport and projection, the
    /// target as the origin), with the faithful toolkit's `bloom` state.
    #[must_use]
    pub fn new(snapshot: &SceneSnapshot<'_>, bloom: bool) -> Self {
        let mut local = snapshot.camera.clone();
        local.target = [0; 3];
        let view = local.view_entries();
        let inverse = Matrix(std::array::from_fn(|i| view[i / 3 * 4 + i % 3])).inverse();
        let vp = crate::camera::multiply(&view, &local.projection());
        Self {
            view,
            inverse,
            planes: frustum_planes(&vp),
            bloom,
        }
    }
}

/// One billboard quad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    /// Camera-local corners in [`QUAD_CORNERS`] order (the fan the classic
    /// toolkit uploads; each corner's texture coordinate is its unit-quad
    /// position).
    pub corners: [[f32; 3]; 4],
    /// The face colour as R, G, B, A bytes (the diffuse colour, display
    /// referred; A is the face's opacity).
    pub colour: [u8; 4],
    /// The billboard type's material (`-1`: untextured).
    pub material: i32,
    /// The alpha reference (`0`: alpha blended with the `> 0` test;
    /// otherwise alpha tested against `ref / 255`, unblended).
    pub alpha_ref: u8,
    /// Whether the quad writes depth: `!hasTransparency` of the owning model.
    pub depth_write: bool,
    /// The placed quad centre's view depth.
    pub view_depth: f32,
}

/// The quads of one model, drawn after entity draw `at - 1` of list `list`
/// (0 opaque, 1 transparent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub list: u8,
    pub at: usize,
    pub quads: Range<usize>,
}

/// Every billboard quad of one frame.
#[derive(Clone, Debug, Default)]
pub struct Billboards {
    pub quads: Vec<Quad>,
    pub segments: Vec<Segment>,
}

/// The model matrix of scene-local classic entries `m` in the camera-local
/// frame (`origin`: the scene-local camera target).
fn camera_local(m: &[f32; 16], origin: [f32; 3]) -> [f32; 16] {
    let mut out = *m;
    out[12] -= origin[0];
    out[13] -= origin[1];
    out[14] -= origin[2];
    out
}

/// The scene-local camera target of `snapshot` (the NXT frame's origin).
#[must_use]
pub fn origin(snapshot: &SceneSnapshot<'_>) -> [f32; 3] {
    [
        (snapshot.camera.target[0] - snapshot.floor_base[0] * 512) as f32,
        snapshot.camera.target[1] as f32,
        (snapshot.camera.target[2] - snapshot.floor_base[1] * 512) as f32,
    ]
}

/// The billboards `entity`'s model carries and the scene-local matrix they
/// are placed with (`None`: no billboards). A rotated or scaled loc's come
/// from its model with the scale-rotate transform applied (the faithful mesh's
/// model), placed with its translation only; every other draw's from its
/// model under its matrix.
#[must_use]
pub fn of_entity(
    snapshot: &SceneSnapshot<'_>,
    entity: &EntityDraw<'_>,
) -> Option<(MeshBillboards, [f32; 16])> {
    entity.model.billboards.as_ref()?;
    let materials = snapshot.materials?;
    let srt = entity
        .key
        .and_then(|key| crate::scene::entity_srt(snapshot.scene?, key.source));
    match srt {
        Some(srt) => {
            let model = crate::scene::srt_model(entity.model, Some(srt));
            let b = MeshBillboards::from_model(&model, materials, [0.0; 3])?;
            let m = &entity.matrix;
            let translation = [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., m[12], m[13], m[14], 1.,
            ];
            Some((b, translation))
        }
        None => Some((
            MeshBillboards::from_model(entity.model, materials, [0.0; 3])?,
            entity.matrix,
        )),
    }
}

impl Billboards {
    /// The classic model draw's billboard tail for one model (see the module docs):
    /// `world` is its model matrix in the camera-local frame.
    pub fn add(&mut self, list: u8, at: usize, b: &MeshBillboards, world: &[f32; 16], view: &View) {
        if culled(b, world, &view.planes) {
            return;
        }
        let mv = crate::camera::multiply(world, &view.view);
        let start = self.quads.len();
        for instance in &b.instances {
            if instance.face.bloom_hidden && view.bloom {
                continue;
            }
            let placement = crate::billboard::place(instance, &mv);
            let quad = Matrix(placement.quad);
            let c = instance.state.colour;
            let corners = QUAD_CORNERS.map(|[x, y]| {
                let v = quad.point(x, y, 0.0);
                view.inverse.point(v[0], v[1], v[2])
            });
            self.quads.push(Quad {
                corners,
                colour: [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8],
                material: instance.face.material,
                alpha_ref: b.alpha_ref,
                depth_write: !b.has_transparency,
                view_depth: placement.view[2],
            });
        }
        if self.quads.len() > start {
            self.segments.push(Segment {
                list,
                at,
                quads: start..self.quads.len(),
            });
        }
    }

    /// The billboards of `list` (the frame's draw list of `snapshot`), each
    /// model's computed afresh ([`of_entity`]; the renderer caches the loc
    /// models' instead).
    #[must_use]
    pub fn build(snapshot: &SceneSnapshot<'_>, list: &DrawList<'_>, view: &View) -> Self {
        let origin = origin(snapshot);
        let mut out = Self::default();
        for (index, draws) in [&list.opaque, &list.transparent].into_iter().enumerate() {
            for (i, entity) in draws.iter().enumerate() {
                if let Some((b, matrix)) = of_entity(snapshot, entity) {
                    out.add(index as u8, i + 1, &b, &camera_local(&matrix, origin), view);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billboard::{BillboardFace, BillboardInstance, BillboardState};

    fn instance() -> BillboardInstance {
        BillboardInstance {
            centroid: [0.0; 3],
            face: BillboardFace {
                face: 0,
                source_face: 0,
                vertices: [0, 1, 2],
                width: 10,
                height: 20,
                material: -1,
                bloom_hidden: false,
                depth_offset: 0,
                sprite_mode: 1,
                sprite_blend: 2,
                remove_face: false,
            },
            state: BillboardState::new(0x80FF_4020_u32 as i32),
        }
    }

    fn billboards(instances: Vec<BillboardInstance>) -> MeshBillboards {
        MeshBillboards {
            instances,
            has_transparency: false,
            alpha_ref: 0,
            drawn: true,
            bounds: (-10, 10, 10),
            matrix: Matrix::default(),
            origin: [0.0; 3],
        }
    }

    /// An identity camera with a far orthographic box: nothing near the
    /// origin is culled.
    fn view() -> View {
        let identity = Matrix::default().entries();
        let mut projection = identity;
        projection[0] = 1.0 / 4096.0;
        projection[5] = 1.0 / 4096.0;
        projection[10] = 1.0 / 4096.0;
        View {
            view: identity,
            inverse: Matrix::default(),
            planes: frustum_planes(&crate::camera::multiply(&identity, &projection)),
            bloom: false,
        }
    }

    /// The quad corners follow the classic fan around the model's
    /// camera-local position, sized by the billboard's half extents; the
    /// colour bytes are the face colour's; one segment follows the model.
    #[test]
    fn quads_follow_the_fan_the_size_and_the_model() {
        let mut out = Billboards::default();
        let mut world = Matrix::default().entries();
        world[12] = 100.0;
        world[14] = 1000.0;
        out.add(1, 3, &billboards(vec![instance()]), &world, &view());
        assert_eq!(
            out.quads[0].corners,
            [
                [90.0, -20.0, 1000.0],
                [90.0, 20.0, 1000.0],
                [110.0, 20.0, 1000.0],
                [110.0, -20.0, 1000.0],
            ]
        );
        assert_eq!(out.quads[0].colour, [0xFF, 0x40, 0x20, 0x80]);
        assert_eq!(out.quads[0].view_depth, 1000.0);
        assert!(out.quads[0].depth_write);
        assert_eq!(
            out.segments,
            vec![Segment {
                list: 1,
                at: 3,
                quads: 0..1
            }]
        );
    }

    /// Bloom-hidden billboards hide under the faithful bloom; a culled or
    /// undrawn model adds nothing.
    #[test]
    fn bloom_hidden_culled_and_undrawn_models_add_nothing() {
        let mut hidden = instance();
        hidden.face.bloom_hidden = true;
        let identity = Matrix::default().entries();
        let mut v = view();
        v.bloom = true;
        let mut out = Billboards::default();
        out.add(0, 1, &billboards(vec![hidden, instance()]), &identity, &v);
        assert_eq!(out.quads.len(), 1);
        v.bloom = false;
        let mut out = Billboards::default();
        out.add(0, 1, &billboards(vec![hidden, instance()]), &identity, &v);
        assert_eq!(out.quads.len(), 2);
        let mut far = identity;
        far[12] = 1.0e6;
        let mut out = Billboards::default();
        out.add(0, 1, &billboards(vec![instance()]), &far, &view());
        let mut undrawn = billboards(vec![instance()]);
        undrawn.drawn = false;
        out.add(0, 1, &undrawn, &identity, &view());
        assert!(out.quads.is_empty() && out.segments.is_empty());
    }

    #[test]
    fn frustum_planes_are_the_normalised_row_sums() {
        let p = frustum_planes(&Matrix::default().entries());
        assert_eq!(p[0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(p[1], [0.0, 0.0, -1.0, 1.0]);
        assert_eq!(p[2], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(p[5], [0.0, -1.0, 0.0, 1.0]);
    }
}
