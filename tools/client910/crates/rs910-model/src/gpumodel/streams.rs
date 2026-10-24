//! The vertex and index streams a renderer uploads for a lit model.

use super::GpuModel;
use crate::texture::MaterialStore;

impl GpuModel {
    /// Position stream: `(x, y, z)` floats per unique vertex.
    #[must_use]
    pub fn position_stream(&self) -> Vec<[f32; 3]> {
        (0..self.unique_count as usize)
            .map(|i| {
                let v = self.unique_vertex[i] as usize;
                [self.vx[v] as f32, self.vy[v] as f32, self.vz[v] as f32]
            })
            .collect()
    }

    /// Colour stream: `(255 - alpha) << 24 | vertex_colour(...)` per unique
    /// vertex, baked at the model's ambient.
    pub fn colour_stream(&self, materials: &MaterialStore) -> anyhow::Result<Vec<i32>> {
        self.colour_stream_at(materials, i32::from(self.ambient))
    }

    /// The colour stream without the model's ambient bake: [`Self::colour_stream`]
    /// at ambient 128, where [`Self::scale_lightness`] leaves each vertex's HSL
    /// lightness unscaled (clamped to 2..126) and a material's grey blend
    /// blends towards white. For a backend that lights models itself.
    pub fn albedo_stream(&self, materials: &MaterialStore) -> anyhow::Result<Vec<i32>> {
        self.colour_stream_at(materials, 128)
    }

    /// `(255 - alpha) << 24 | vertex_colour(hsl, material, ambient)` per unique
    /// vertex.
    fn colour_stream_at(
        &self,
        materials: &MaterialStore,
        ambient: i32,
    ) -> anyhow::Result<Vec<i32>> {
        let mut out = Vec::with_capacity(self.unique_count as usize);
        for vertex in 0..self.unique_count as usize {
            let face = self.unique_face[vertex] as usize;
            let rgb = Self::vertex_colour(
                materials,
                i32::from(self.face_colour[face]) & 0xFFFF,
                self.face_material[face],
                ambient,
            )?;
            let alpha = (255 - i32::from(self.face_alpha[face])).wrapping_shl(24);
            out.push(alpha | rgb);
        }
        Ok(out)
    }

    /// Normal stream: the (merged) normal divided by its face count when that
    /// exceeds 1, then normalised, in single-precision float.
    #[must_use]
    pub fn normal_stream(&self) -> Vec<[f32; 3]> {
        let (nx, ny, nz, nc): (&[i16], &[i16], &[i16], &[i8]) = match &self.merged {
            Some(m) => (&m.nx, &m.ny, &m.nz, &m.count),
            None => (&self.nx, &self.ny, &self.nz, &self.ncount),
        };
        (0..self.unique_count as usize)
            .map(|i| {
                let mut x = f32::from(nx[i]);
                let mut y = f32::from(ny[i]);
                let mut z = f32::from(nz[i]);
                let len = f64::from(z * z + y * y + x * x).sqrt() as f32;
                if len != 0.0 {
                    if nc[i] > 1 {
                        let d = f32::from(nc[i]);
                        x /= d;
                        y /= d;
                        z /= d;
                    }
                    let l2 = f64::from(z * z + y * y + x * x).sqrt() as f32;
                    let inv = 1.0 / l2;
                    x *= inv;
                    y *= inv;
                    z *= inv;
                }
                [x, y, z]
            })
            .collect()
    }

    /// Texture coordinate stream.
    #[must_use]
    pub fn uv_stream(&self) -> Vec<[f32; 2]> {
        (0..self.unique_count as usize)
            .map(|i| [self.u[i], self.v[i]])
            .collect()
    }

    /// Index buffer: the first `draw_face_count` faces as `u16` triples.
    #[must_use]
    pub fn index_stream(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.draw_face_count as usize * 3);
        for i in 0..self.draw_face_count as usize {
            out.push(self.idx1[i] as u16);
            out.push(self.idx2[i] as u16);
            out.push(self.idx3[i] as u16);
        }
        out
    }

    /// Material batches: `(material, first face, face count, min vertex,
    /// vertex span)`.
    #[must_use]
    pub fn batches(&self) -> Vec<(i16, i32, i32, i32, i32)> {
        let mut out = Vec::new();
        for b in 0..self.batch_min_vertex.len() {
            let start = self.batch_face_start[b];
            let end = self.batch_face_start[b + 1];
            out.push((
                self.face_material[start as usize],
                start,
                end - start,
                self.batch_min_vertex[b],
                self.batch_vertex_span[b],
            ));
        }
        out
    }
}
