//! Particle anchors and billboards of a lit model.

use super::GpuModel;
use crate::colour::hsl_tables;

impl GpuModel {
    /// The emitter triangles and effector vertices through the owner's
    /// matrix (float order `e8 * z + e0 * x + e4 * y + e12`, truncated to
    /// int), keyed by `id_base | index` so the owner's particle system
    /// matches them across frames. `rotation` is the rotation the owner's
    /// matrix would carry when the owner bakes its turn into the vertices
    /// ([`GpuModel::rotate_y_keep_normals`]); it only turns effector forces.
    #[must_use]
    pub fn particle_anchors(
        &self,
        matrix: &crate::actor_matrix::Matrix,
        rotation: crate::particle::Rotation,
        id_base: u64,
    ) -> (
        Vec<crate::particle::EmitterAnchor>,
        Vec<crate::particle::EffectorAnchor>,
    ) {
        let m = &matrix.0;
        let point = |v: usize| -> Option<[i32; 3]> {
            let (x, y, z) = (
                *self.vx.get(v)? as f32,
                *self.vy.get(v)? as f32,
                *self.vz.get(v)? as f32,
            );
            Some(std::array::from_fn(|i| {
                (m[6 + i] * z + m[i] * x + m[3 + i] * y + m[9 + i]) as i32
            }))
        };
        let emitters = self
            .particle_emitters
            .iter()
            .enumerate()
            .filter_map(|(index, e)| {
                Some(crate::particle::EmitterAnchor {
                    id: id_base | index as u64,
                    particle: e.particle,
                    vertices: [
                        point(e.vertices[0])?,
                        point(e.vertices[1])?,
                        point(e.vertices[2])?,
                    ],
                })
            })
            .collect();
        let rotation = rotation.then(&crate::particle::Rotation::from_matrix(matrix));
        let effectors = self
            .particle_effectors
            .iter()
            .enumerate()
            .filter_map(|(index, e)| {
                Some(crate::particle::EffectorAnchor {
                    id: id_base | index as u64,
                    effector: e.effector,
                    position: point(e.vertex)?,
                    rotation,
                })
            })
            .collect();
        (emitters, effectors)
    }

    /// Refreshes the palette entry of every billboard from its face colour
    /// after a recolour, a tint or animation op 7. A retexture leaves them.
    pub(crate) fn refresh_billboard_palette(&mut self) {
        let colours = &self.face_colour;
        if let Some(b) = &mut self.billboards {
            b.refresh_palette(|face| i32::from(colours[face] as u16));
        }
    }

    /// Refreshes the colour of every billboard from its face colour; every
    /// face-colour edit ends with it.
    pub(crate) fn refresh_billboard_colours(&mut self) {
        let colours = &self.face_colour;
        if let Some(b) = &mut self.billboards {
            let rgb = &hsl_tables().rgb;
            b.refresh_colours(|face| rgb[(colours[face] as u16) as usize]);
        }
    }

    /// Refreshes the alpha of every billboard from its face after animation
    /// op 5.
    pub(crate) fn refresh_billboard_alphas(&mut self) {
        let alphas = &self.face_alpha;
        if let Some(b) = &mut self.billboards {
            b.refresh_alphas(|face| i32::from(alphas[face] as u8));
        }
    }

    /// The billboards placed against the current vertices: each with its
    /// face centroid.
    #[must_use]
    pub fn billboard_instances(&self) -> Vec<crate::billboard::BillboardInstance> {
        self.billboards.as_ref().map_or_else(Vec::new, |b| {
            b.faces
                .iter()
                .zip(&b.states)
                .map(|(face, state)| crate::billboard::BillboardInstance {
                    centroid: crate::billboard::centroid(&self.vx, &self.vy, &self.vz, face),
                    face: *face,
                    state: *state,
                })
                .collect()
        })
    }
}
