//! The billboards an uploaded lit-model mesh carries (what is drawn after the
//! batches): CPU data read by the floor/model upload and the billboard pass.
//! Split out of the billboard renderer to break its cycle with the floor
//! renderer; the renderer re-exports it.

use crate::actor_matrix::Matrix;
use crate::billboard::BillboardInstance;

/// The billboards of one uploaded lit model.
#[derive(Clone, Debug)]
pub struct MeshBillboards {
    pub instances: Vec<BillboardInstance>,
    /// The model has transparency: it gates depth writes.
    pub has_transparency: bool,
    /// The alpha reference the last batch leaves: the ALPHA_TESTED threshold
    /// of its material, else 0. A model with no drawn faces returns before its
    /// batches and leaves the previous draw's state, which the ordered
    /// lists do not model; it reads as 0.
    pub alpha_ref: u8,
    /// `draw` returns before the billboards otherwise.
    pub drawn: bool,
    /// `(min_y, max_y, horizontal_radius)` for the frustum early-out.
    pub bounds: (i32, i32, i32),
    /// Model -> scene frame (identity for baked locs, the actor matrix for
    /// pathing entities).
    pub matrix: Matrix,
    /// Scene frame -> absolute fine units.
    pub origin: [f64; 3],
}

impl MeshBillboards {
    /// The billboard half of an upload; `origin` is the mesh's absolute
    /// fine translation. `None` for models without billboards.
    #[must_use]
    pub fn from_model(
        model: &crate::gpumodel::GpuModel,
        materials: &crate::texture::MaterialStore,
        origin: [f32; 3],
    ) -> Option<Self> {
        model.billboards.as_ref()?;
        Some(Self {
            instances: model.billboard_instances(),
            has_transparency: model.has_transparency,
            alpha_ref: Self::last_alpha_ref(model, materials),
            drawn: model.unique_count != 0,
            bounds: model.draw_bounds(),
            matrix: Matrix::default(),
            origin: origin.map(f64::from),
        })
    }

    /// Re-read an animated model into this mesh's billboards, keeping the
    /// transform.
    pub fn refresh(
        this: &mut Option<Self>,
        model: &crate::gpumodel::GpuModel,
        materials: &crate::texture::MaterialStore,
    ) {
        let (matrix, origin) = this
            .as_ref()
            .map_or((Matrix::default(), [0.0; 3]), |b| (b.matrix, b.origin));
        *this = Self::from_model(model, materials, [0.0; 3]).map(|mut b| {
            b.matrix = matrix;
            b.origin = origin;
            b
        });
    }

    fn last_alpha_ref(
        model: &crate::gpumodel::GpuModel,
        materials: &crate::texture::MaterialStore,
    ) -> u8 {
        if model.draw_face_count == 0 {
            return 0;
        }
        model
            .batches()
            .last()
            .filter(|b| b.0 != -1)
            .and_then(|b| materials.get(u32::from(b.0 as u16)))
            .filter(|m| m.alpha == crate::texture::AlphaMode::AlphaTested)
            .map_or(0, |m| m.alpha_threshold)
    }

    /// The model's world matrix in the camera-target frame: `matrix` with the translation moved by `origin - target`.
    #[must_use]
    pub fn world(&self, target: [f64; 3]) -> [f32; 16] {
        let mut m = self.matrix;
        for ((dst, origin), target) in m.0[9..12].iter_mut().zip(self.origin).zip(target) {
            *dst = (f64::from(*dst) + origin - target) as f32;
        }
        m.entries()
    }
}

/// The six normalised frustum planes of the `view * projection` entries `vp`
/// (the `z`, `-z`, `x`, `-x`, `y` and `-y` clip bounds): f32 sums, then f64
/// normalisation.
#[must_use]
pub fn frustum_planes(vp: &[f32; 16]) -> [[f32; 4]; 6] {
    [
        (2, 1.0),
        (2, -1.0),
        (0, 1.0),
        (0, -1.0),
        (1, 1.0),
        (1, -1.0),
    ]
    .map(|(col, sign): (usize, f32)| {
        let p = [
            vp[3] + sign * vp[col],
            vp[7] + sign * vp[4 + col],
            vp[11] + sign * vp[8 + col],
            vp[15] + sign * vp[12 + col],
        ];
        let length = f64::from(p[2] * p[2] + p[0] * p[0] + p[1] * p[1]).sqrt();
        p.map(|v| (f64::from(v) / length) as f32)
    })
}

/// The model draw's early-out: no drawn vertices, or the bounding cylinder
/// (the model's y range at its origin plus its horizontal radius, under
/// `world`) outside one of `planes`.
#[must_use]
pub fn culled(b: &MeshBillboards, world: &[f32; 16], planes: &[[f32; 4]; 6]) -> bool {
    if !b.drawn {
        return true;
    }
    let (min_y, max_y, radius) = b.bounds;
    let point = |y: f32| -> [f32; 3] { std::array::from_fn(|i| world[4 + i] * y + world[12 + i]) };
    let (low, high) = (point(min_y as f32), point(max_y as f32));
    planes.iter().any(|p| {
        let a = p[2] * low[2] + p[0] * low[0] + p[1] * low[1] + p[3] + radius as f32;
        let b = p[2] * high[2] + p[0] * high[0] + p[1] * high[1] + p[3] + radius as f32;
        a < 0.0 && b < 0.0
    })
}

/// One billboard quad placed facing the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedQuad {
    /// Corners in `QUAD_CORNERS` order, in the frame `inverse` maps back to.
    pub corners: [[f32; 3]; 4],
    /// The face colour as R, G, B, A bytes.
    pub colour: [u8; 4],
    /// The billboard type's material (`-1`: untextured).
    pub material: i32,
    /// The quad centre's view depth.
    pub view_depth: f32,
}

impl MeshBillboards {
    /// The quads of this mesh under model matrix `world`, given the view
    /// entries `view` and its inverse camera; the bloom-hidden ones are
    /// skipped while bloom is on.
    pub fn placed_quads<'a>(
        &'a self,
        world: &[f32; 16],
        view: &[f32; 16],
        inverse: &'a Matrix,
        bloom: bool,
    ) -> impl Iterator<Item = PlacedQuad> + 'a {
        let mv = crate::animation_matrix::multiply(world, view);
        self.instances
            .iter()
            .filter(move |instance| !(instance.face.bloom_hidden && bloom))
            .map(move |instance| {
                let placement = crate::billboard::place(instance, &mv);
                let quad = Matrix(placement.quad);
                let c = instance.state.colour;
                PlacedQuad {
                    corners: crate::billboard::QUAD_CORNERS.map(|[x, y]| {
                        let v = quad.point(x, y, 0.0);
                        inverse.point(v[0], v[1], v[2])
                    }),
                    colour: [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8],
                    material: instance.face.material,
                    view_depth: placement.view[2],
                }
            })
    }
}
