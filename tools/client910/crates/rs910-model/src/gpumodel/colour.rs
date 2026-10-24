//! Colour edits of a lit model (recolour, retexture, tint) and the bake of a
//! face colour into a vertex colour.

use super::{find_material, GpuModel};
use crate::colour::hsl_tables;
use crate::texture::MaterialStore;

impl GpuModel {
    /// Replaces the face colour `from` by `to`.
    pub fn recolor(&mut self, from: i16, to: i16) {
        for face in 0..self.face_count as usize {
            if self.face_colour[face] == from {
                self.face_colour[face] = to;
            }
        }
        self.refresh_billboard_colours();
        self.refresh_billboard_palette();
    }

    /// Replaces the face material `from` by `to`.
    pub fn retexture(
        &mut self,
        materials: &MaterialStore,
        from: i16,
        to: i16,
    ) -> anyhow::Result<()> {
        for face in 0..self.face_count as usize {
            if self.face_material[face] == from {
                self.face_material[face] = to;
            }
        }
        let mut old_grey_blend = 0_u8;
        let mut old_brightness = 0_u8;
        if from != -1 {
            let old = find_material(materials, from)?;
            (old_grey_blend, old_brightness) = (old.grey_blend, old.brightness_boost);
        }
        let mut new_grey_blend = 0_u8;
        let mut new_brightness = 0_u8;
        if to != -1 {
            let new = find_material(materials, to)?;
            (new_grey_blend, new_brightness) = (new.grey_blend, new.brightness_boost);
            if new.speed_u != 0.0 || new.speed_v != 0.0 {
                self.has_animated_uvs = true;
            }
        }
        // Billboards refresh only when the material brightness parameters
        // change.
        if old_grey_blend != new_grey_blend || old_brightness != new_brightness {
            self.refresh_billboard_colours();
        }
        Ok(())
    }

    /// Sets the transparency of every face (`255` is fully transparent, `0`
    /// opaque), as an entity fading in does.
    pub fn set_alpha(&mut self, alpha: u8) {
        for face in 0..self.face_count as usize {
            self.face_alpha[face] = alpha as i8;
        }
        self.refresh_billboard_alphas();
    }

    /// Moves every face colour `weight / 128` of the way towards the given
    /// hue, saturation and lightness (`-1` leaves a component alone).
    pub fn tint(&mut self, hue: i32, saturation: i32, lightness: i32, weight: i32) {
        for face in 0..self.face_count as usize {
            let colour = i32::from(self.face_colour[face]) & 0xFFFF;
            let mut face_hue = (colour >> 10) & 0x3F;
            let mut face_saturation = (colour >> 7) & 0x7;
            let mut face_lightness = colour & 0x7F;
            if hue != -1 {
                face_hue += ((hue - face_hue) * weight) >> 7;
            }
            if saturation != -1 {
                face_saturation += ((saturation - face_saturation) * weight) >> 7;
            }
            if lightness != -1 {
                face_lightness += ((lightness - face_lightness) * weight) >> 7;
            }
            self.face_colour[face] =
                ((face_hue << 10) | (face_saturation << 7) | face_lightness) as i16;
        }
        self.refresh_billboard_colours();
        self.refresh_billboard_palette();
    }

    /// Scales the 7-bit lightness of an HSL colour by `ambient / 128`,
    /// clamped to 2..=126; hue and saturation (and bit 7) stay.
    #[must_use]
    pub fn scale_lightness(hsl: i32, ambient: i32) -> i32 {
        let mut lightness = ((hsl & 0x7F) * ambient) >> 7;
        lightness = lightness.clamp(2, 126);
        (hsl & 0xFF80) + lightness
    }

    /// The vertex colour of an HSL face colour and material under `ambient`,
    /// packed blue-green-red: the lightness scaled by the ambient, blended
    /// towards the ambient grey by the material's grey blend, then brightened
    /// by its brightness boost.
    pub fn vertex_colour(
        materials: &MaterialStore,
        hsl: i32,
        material: i16,
        ambient: i32,
    ) -> anyhow::Result<i32> {
        let mut colour = hsl_tables().bgr[Self::scale_lightness(hsl, ambient) as usize];
        if material != -1 {
            let material = find_material(materials, material)?;
            let grey_blend = i32::from(material.grey_blend) & 0xFF;
            if grey_blend != 0 {
                let grey = if ambient < 0 {
                    0
                } else if ambient > 127 {
                    16_777_215
                } else {
                    ambient * 131_586
                };
                if grey_blend == 256 {
                    colour = grey;
                } else {
                    let keep = 256 - grey_blend;
                    colour = (((colour & 0xFF00FF)
                        .wrapping_mul(keep)
                        .wrapping_add((grey & 0xFF00FF).wrapping_mul(grey_blend))
                        & 0xFF00_FF00_u32 as i32)
                        .wrapping_add(
                            ((colour & 0xFF00)
                                .wrapping_mul(keep)
                                .wrapping_add((grey & 0xFF00).wrapping_mul(grey_blend)))
                                & 0xFF0000,
                        ))
                        >> 8;
                }
            }
            let mut boost = i32::from(material.brightness_boost) & 0xFF;
            if boost != 0 {
                boost += 256;
                let red = (((colour >> 16) & 0xFF) * boost).min(65535);
                let green = (((colour >> 8) & 0xFF) * boost).min(65535);
                let blue = ((colour & 0xFF) * boost).min(65535);
                colour = (blue >> 8) + ((red & 0xFF00) << 8) + (green & 0xFF00);
            }
        }
        Ok(colour & 0xFF_FFFF)
    }
}
