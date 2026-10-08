//! The sun-projected hard shadow of a model.

use super::GpuModel;

impl GpuModel {
    /// The model's hard shadow. Every unique vertex is projected along the
    /// sun (the sun's per-256-units-of-height offsets, then down to shadow
    /// texels) relative to the origin its bounds give, and every drawn face
    /// with alpha up to 128 and positive signed area is rasterised. `None`
    /// when the model has no vertices.
    pub fn hard_shadow(
        &mut self,
        sun: &crate::floor::SunLighting,
    ) -> Option<crate::hardshadow::HardShadow> {
        use crate::hardshadow::{HardShadow, SHADOW_TEXEL_SHIFT};
        if self.unique_count == 0 {
            return None;
        }
        self.ensure_bounds();
        let (offset_x, offset_z) = (sun.shadow_offset_x, sun.shadow_offset_z);
        let (min_texel_x, max_texel_x) = if offset_x > 0 {
            (
                (self.min_x - ((offset_x * self.max_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
                (self.max_x - ((offset_x * self.min_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
            )
        } else {
            (
                (self.min_x - ((offset_x * self.min_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
                (self.max_x - ((offset_x * self.max_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
            )
        };
        let (min_texel_z, max_texel_z) = if offset_z > 0 {
            (
                (self.min_z - ((offset_z * self.max_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
                (self.max_z - ((offset_z * self.min_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
            )
        } else {
            (
                (self.min_z - ((offset_z * self.min_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
                (self.max_z - ((offset_z * self.max_y) >> 8)) >> SHADOW_TEXEL_SHIFT,
            )
        };
        let width = max_texel_x - min_texel_x + 1;
        let height = max_texel_z - min_texel_z + 1;
        let mut shadow = HardShadow::new(width, height);
        shadow.set_bounds(min_texel_x, min_texel_z, max_texel_x, max_texel_z);
        // Project every unique vertex onto the shadow grid.
        let n = self.unique_count as usize;
        let mut projected_x = vec![0_i32; n];
        let mut projected_z = vec![0_i32; n];
        for vertex in 0..self.vertex_count as usize {
            let texel_x = ((self.vx[vertex] - ((self.vy[vertex] * offset_x) >> 8))
                >> SHADOW_TEXEL_SHIFT)
                - shadow.origin_x;
            let texel_z = ((self.vz[vertex] - ((self.vy[vertex] * offset_z) >> 8))
                >> SHADOW_TEXEL_SHIFT)
                - shadow.origin_y;
            let first_slot = self.vertex_offsets[vertex] as usize;
            let end_slot = self.vertex_offsets[vertex + 1] as usize;
            let mut slot = first_slot;
            while slot < end_slot && self.vertex_slots[slot] != 0 {
                let unique = (self.vertex_slots[slot] as u16 as usize) - 1;
                projected_x[unique] = texel_x;
                projected_z[unique] = texel_z;
                slot += 1;
            }
        }
        for face in 0..self.draw_face_count as usize {
            // The alpha is a signed byte compared with 128, so this is always
            // true; kept as the client has it.
            if self.face_alpha.is_empty() || i32::from(self.face_alpha[face]) <= 128 {
                let a = self.idx1[face] as u16 as usize;
                let b = self.idx2[face] as u16 as usize;
                let c = self.idx3[face] as u16 as usize;
                let (xa, xb, xc) = (projected_x[a], projected_x[b], projected_x[c]);
                let (za, zb, zc) = (projected_z[a], projected_z[b], projected_z[c]);
                if (xa - xb) * (zb - zc) - (xc - xb) * (zb - za) > 0 {
                    shadow.raster_triangle(za, zb, zc, xa, xb, xc);
                }
            }
        }
        Some(shadow)
    }
}
