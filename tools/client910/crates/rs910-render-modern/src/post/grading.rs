//! The modern renderer's colour grading (the grading half of
//! renderer plan M8): the environment's levels and colour remapping applied
//! to the tonemapped display colour in the final composite.
//!
//! # Why
//!
//! The Lumbridge environments grade the whole frame through a colour
//! remapping LUT at weight 1.0 (sprite 22216 in map square 50,50: trailer
//! op 3, file 6 slot 0). The faithful GPU toolkit applies it in its post
//! chain (levels and remapping always installed; `rs910_render_gpu::postprocess`),
//! whenever the game interface opens the post capture. The LUT turns the raw
//! scene's dark browns into cool grey-blues and its yellow-greens into
//! green: on the online Lumbridge frame the faithful ground's mean hue moves
//! from 40° (raw) to 192° and the grass's from 69° to 115°. Without it the
//! modern frame showed the raw colours: brown ground, brown-grey water, olive
//! grass (the look the user reported). With `CLIENT910_POSTFX_OFF=1` the
//! faithful frame looks the same.
//!
//! The modern client grades as well, from the environment record's colour
//! remappings (a default remapping when a slot is empty): the same sprites,
//! the same weights and slot rules, the same lookup.
//!
//! # What
//!
//! - [`Grading::from_env`] applies the environment's levels and colour
//!   remapping over the snapshot's `EnvFrame` (the fade and overrides already
//!   applied) with the toolkit's slot rules: weighted empty slots dropped
//!   towards the front, the positive weights counted, the source colour
//!   weighted by the remainder. A LUT is a `256x16` sprite, anything else is
//!   no remapper.
//! - The lookup: the clamped colour weighted by the base plus each counted
//!   LUT's trilinear sample at `0.03125 + c * 0.9375`, which lands on texel
//!   coordinate `15 c` of the 16-texel axes (red along the sprite's columns
//!   inside a 16-column slice, green down the rows, blue across the slices).
//!   The composite does the trilinear blend itself from exact texel loads
//!   (no sampler, so frames repeat byte for byte); [`Grading::apply`] is its
//!   CPU mirror.
//! - Levels come before the remap, in the classic chain order (bloom,
//!   levels, remapping).
//!
//! Unlike the faithful toolkit this backend grades every frame, offline
//! views included (grading is part of the modern post chain, not of an
//! interface layer). Bloom (the third classic effect, off unless the bloom
//! preference adds it) is not applied here: M8.

use std::collections::HashMap;
use std::sync::Arc;

/// The LUT's edge.
pub const LUT_SIZE: usize = 16;
/// A LUT sprite's width and height.
pub const LUT_WIDTH: u32 = 256;

/// One frame's grading: levels and up to three remapping LUTs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grading {
    /// `[gamma, inputMin, inputMax, outputMin, outputMax]`, `None` when a
    /// no-op.
    pub levels: Option<[f32; 5]>,
    /// The sprite of each slot after the slot shifts (`-1` none).
    pub luts: [i32; 3],
    pub weights: [f32; 3],
    /// Slots the lookup reads.
    pub count: u32,
    /// The source colour's weight.
    pub base: f32,
}

impl Default for Grading {
    fn default() -> Self {
        Self {
            levels: None,
            luts: [-1; 3],
            weights: [0.0; 3],
            count: 0,
            base: 1.0,
        }
    }
}

impl Grading {
    /// The grading of `env`; `has_lut(id)` says whether sprite `id` is a
    /// LUT (see the module docs).
    #[must_use]
    pub fn from_env(
        env: &rs910_scene::env::EnvFrame,
        mut has_lut: impl FnMut(i32) -> bool,
    ) -> Self {
        let levels = env.levels;
        let levels = (levels != [1.0, 0.0, 1.0, 0.0, 1.0]).then_some(levels);
        let mut slot = |i: usize| {
            let (map, weight) = env.colour_remap[i];
            ((map > -1 && has_lut(map)).then_some(map), weight)
        };
        let ((mut a, mut aw), (mut b, mut bw), (mut c, mut cw)) = (slot(0), slot(1), slot(2));
        // The toolkit's slot rules.
        if c.is_none() && cw > 0.0 {
            cw = 0.0;
        }
        if b.is_none() && bw > 0.0 {
            b = c.take();
            bw = cw;
            cw = 0.0;
        }
        if a.is_none() && aw > 0.0 {
            a = b.take();
            b = c.take();
            aw = bw;
            bw = cw;
            cw = 0.0;
        }
        let weights = [aw, bw, cw];
        let count = weights.iter().filter(|w| **w > 0.0).count() as u32;
        let base = 1.0 - (aw + bw + cw);
        Self {
            levels,
            luts: [a, b, c].map(|l| l.unwrap_or(-1)),
            weights,
            count,
            base,
        }
    }

    /// Whether the remap changes nothing.
    #[must_use]
    pub fn remap_is_noop(&self) -> bool {
        self.count == 0 || self.base == 1.0 || self.weights.iter().sum::<f32>() == 0.0
    }

    /// The graded display colour of `rgb` (the composite's CPU mirror);
    /// `luts[i]` is slot `i`'s sprite ([`LutCache::get`]).
    #[must_use]
    pub fn apply(&self, luts: [Option<&[u8]>; 3], rgb: [f32; 3]) -> [f32; 3] {
        let mut c = rgb.map(|v| v.clamp(0.0, 1.0));
        if let Some([gamma, in_min, in_max, out_min, out_max]) = self.levels {
            c = c.map(|v| {
                let x = ((v - in_min) / (in_max - in_min)).clamp(0.0, 1.0);
                out_min + x.powf(gamma).clamp(0.0, 1.0) * (out_max - out_min)
            });
        }
        if self.remap_is_noop() {
            return c;
        }
        let mut out = c.map(|v| v * self.base);
        let counted = (self.count as usize).min(3);
        for (lut, weight) in luts.iter().zip(self.weights).take(counted) {
            let Some(lut) = lut else { continue };
            let s = sample(lut, c);
            for (o, v) in out.iter_mut().zip(s) {
                *o += v * weight;
            }
        }
        out
    }
}

/// The trilinear sample of a LUT sprite (`R, G, B` bytes of `256x16`
/// pixels, row-major) at colour `c` (texel coordinate `15 c`).
#[must_use]
pub fn sample(lut: &[u8], c: [f32; 3]) -> [f32; 3] {
    let axis = |v: f32| {
        let t = v.clamp(0.0, 1.0) * 15.0;
        let i0 = (t.floor() as usize).min(15);
        (i0, (i0 + 1).min(15), t - i0 as f32)
    };
    let (r0, r1, fr) = axis(c[0]);
    let (g0, g1, fg) = axis(c[1]);
    let (b0, b1, fb) = axis(c[2]);
    let texel = |r: usize, g: usize, b: usize| {
        let at = (g * LUT_WIDTH as usize + b * LUT_SIZE + r) * 3;
        [lut[at], lut[at + 1], lut[at + 2]].map(|v| f32::from(v) / 255.0)
    };
    let mut out = [0.0_f32; 3];
    for (b, wb) in [(b0, 1.0 - fb), (b1, fb)] {
        for (g, wg) in [(g0, 1.0 - fg), (g1, fg)] {
            for (r, wr) in [(r0, 1.0 - fr), (r1, fr)] {
                let t = texel(r, g, b);
                for i in 0..3 {
                    out[i] += t[i] * wr * wg * wb;
                }
            }
        }
    }
    out
}

/// The LUT sprites by id (kept for the renderer's life, they are 12 KB
/// each).
#[derive(Debug, Default)]
pub struct LutCache {
    luts: HashMap<i32, Option<Arc<[u8]>>>,
}

impl LutCache {
    /// Sprite `id` as `R, G, B` bytes when it is a `256x16` sprite
    /// (`None`: missing, undecodable or another size).
    pub fn get(&mut self, pack: Option<&crate::cache::Pack>, id: i32) -> Option<Arc<[u8]>> {
        if id < 0 {
            return None;
        }
        if let Some(hit) = self.luts.get(&id) {
            return hit.clone();
        }
        let pack = pack?;
        let lut = load(pack, id);
        if lut.is_none() {
            log::warn!("[modern] colour remapping sprite {id} is not a 256x16 LUT");
        }
        self.luts.insert(id, lut.clone());
        lut
    }
}

fn load(pack: &crate::cache::Pack, id: i32) -> Option<Arc<[u8]>> {
    let bytes = rs910_js5::js5_fetch::fetch_file(pack, "sprites", id as u32).ok()??;
    let frames = rs910_config::sprite_data::Data::decode(&bytes).ok()?;
    let data = frames.first()?;
    if data.width != LUT_WIDTH as i32 || data.height != LUT_SIZE as i32 {
        return None;
    }
    let argb = data.argb(false).ok()?;
    Some(
        argb.iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect(),
    )
}

/// The composite's `Post` uniform block ([`crate::shaders`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostUniforms {
    /// x exposure, y 1 = encode (a non-sRGB target), z remap slot count
    /// (0: no remap), w levels on.
    pub params: [f32; 4],
    /// x base, yzw slot weights.
    pub weights: [f32; 4],
    /// Levels: gamma, input min, input max, output min.
    pub levels: [f32; 4],
    /// x levels output max.
    pub levels_max: [f32; 4],
}

impl PostUniforms {
    /// The block for `grading` on a target that `encode`s its own display
    /// values.
    #[must_use]
    pub fn new(grading: &Grading, encode: bool) -> Self {
        let levels = grading.levels.unwrap_or([1.0, 0.0, 1.0, 0.0, 1.0]);
        Self {
            params: [
                crate::post::tonemap::EXPOSURE,
                if encode { 1.0 } else { 0.0 },
                if grading.remap_is_noop() {
                    0.0
                } else {
                    grading.count as f32
                },
                if grading.levels.is_some() { 1.0 } else { 0.0 },
            ],
            weights: [
                grading.base,
                grading.weights[0],
                grading.weights[1],
                grading.weights[2],
            ],
            levels: [levels[0], levels[1], levels[2], levels[3]],
            levels_max: [levels[4], 0.0, 0.0, 0.0],
        }
    }
}

/// The LUT texture's rows of `lut` for slot `slot` (RGBA, `256 x 16`).
#[must_use]
pub fn lut_rows(lut: &[u8]) -> Vec<u8> {
    lut.chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(remap: [(i32, f32); 3], levels: [f32; 5]) -> rs910_scene::env::EnvFrame {
        let env = rs910_scene::env::Environment {
            colour_remap: remap,
            levels,
            ..Default::default()
        };
        rs910_scene::env::EnvFrame::build(
            &env,
            rs910_scene::env::SunSettings {
                direction: env.sun_dir,
                brightness_pref: 3,
                anti_macro: 0.0,
            },
            true,
            rs910_scene::env::FogReference {
                far: 1000.0,
                near_min: 50.0,
                view: &glam::Mat4::IDENTITY.to_cols_array(),
            },
        )
    }

    /// `setColourRemapping`'s slot rules: a weighted slot without a LUT
    /// hands its place to the next, the base takes the remainder.
    #[test]
    fn slots_follow_set_colour_remapping() {
        let identity = [1.0, 0.0, 1.0, 0.0, 1.0];
        let g = Grading::from_env(
            &env_with([(22216, 1.0), (-1, 0.0), (-1, 0.0)], identity),
            |_| true,
        );
        assert_eq!(
            (g.luts, g.weights, g.count, g.base),
            ([22216, -1, -1], [1.0, 0.0, 0.0], 1, 0.0)
        );
        assert!(!g.remap_is_noop() && g.levels.is_none());
        let g = Grading::from_env(
            &env_with([(7, 0.5), (8, 0.25), (-1, 0.0)], identity),
            |id| id != 7,
        );
        assert_eq!(
            (g.luts, g.weights, g.count),
            ([8, -1, -1], [0.25, 0.0, 0.0], 1)
        );
        assert!((g.base - 0.75).abs() < 1e-6);
        let g = Grading::from_env(&env_with([(-1, 0.0); 3], [0.8, 0.0, 1.0, 0.0, 1.0]), |_| {
            true
        });
        assert!(g.remap_is_noop());
        assert_eq!(g.levels, Some([0.8, 0.0, 1.0, 0.0, 1.0]));
    }

    /// The Lumbridge LUT (sprite 22216) through the CPU mirror: the raw
    /// scene's dark brown ground turns cool grey and olive grass green, the
    /// shift measured on the faithful online frame (module docs).
    #[test]
    #[cfg_attr(feature = "no-pack", ignore)]
    fn lumbridge_lut_cools_brown_and_greens_grass() {
        let pack = crate::test_support::require_pack("client.sprites.js5");
        let mut cache = LutCache::default();
        let lut = cache
            .get(Some(&pack), 22216)
            .expect("sprite 22216 is a LUT");
        assert_eq!(lut.len(), 256 * 16 * 3);
        let g = Grading {
            luts: [22216, -1, -1],
            weights: [1.0, 0.0, 0.0],
            count: 1,
            base: 0.0,
            levels: None,
        };
        let hue = |c: [f32; 3]| {
            let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            let d = mx - mn;
            let h = if mx == c[0] {
                ((c[1] - c[2]) / d).rem_euclid(6.0)
            } else if mx == c[1] {
                (c[2] - c[0]) / d + 2.0
            } else {
                (c[0] - c[1]) / d + 4.0
            };
            h * 60.0
        };
        let brown = [40.0, 30.0, 20.0].map(|v| v / 255.0);
        let out = g.apply([Some(&lut), None, None], brown);
        assert!(
            hue(out) > 180.0 && hue(out) < 300.0,
            "brown -> {out:?} ({})",
            hue(out)
        );
        let olive = [70.0, 80.0, 40.0].map(|v| v / 255.0);
        let out = g.apply([Some(&lut), None, None], olive);
        assert!(
            hue(out) > hue(olive) + 10.0,
            "olive -> {out:?} ({})",
            hue(out)
        );
        // Not every sprite is a LUT.
        assert!(cache.get(Some(&pack), 0).is_none());
    }
}
