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
