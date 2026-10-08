//! The modern renderer's static point lights (renderer plan M4): the frame's
//! light set from the snapshot, the per-tile light index grid the floors
//! look their lights up in, and the per-model light lists.
//!
//! # The modern client's model (reference only, nothing copied)
//!
//! - **Data.** The modern client reads the map's light list from the same
//!   trailer the classic client does (`docs/renderer/modern-renderer.md`) and
//!   derives the same flicker table. So the light set here is the classic one
//!   the snapshot already carries: `live.model_lights`
//!   (`rs910_scene::model_lights::ModelLights`: the scene's static lights with
//!   the flicker intensity of this frame times the group fade scale, and the
//!   group colour fade). The trailer is not decoded again.
//! - **Index grid.** The modern client builds, per map square, a 256 x 128
//!   texture: the left half holds each tile's (per level, levels in 2 x 2
//!   quadrants of 64 x 64 tiles) up to four light ids in the four bytes of a
//!   texel, filled first come first served in light order over the light's
//!   span runs (its level only, or from level 0 with the "below" flag and up
//!   to level 3 with "above"); id 0 is "no light", so a square holds at most
//!   255. The right half holds each light's position, radius (or its
//!   reciprocal with float textures) and colour. A 256-entry uniform array
//!   binds each light's intensity this frame. That per-tile table is exactly
//!   the classic `ModelLights` table (four slots per tile, the same spans and
//!   level flags), so the grid here is read from it
//!   ([`ModelLights::tile_slots`]), for the whole scene at once (ids up to
//!   65,535; the classic table stops at 65,253) instead of per map square.
//! - **Shading.** The modern client generates its point light evaluation in
//!   two variants:
//!   - terrain (the world-position variant): the fragment's own tile
//!     (`floor(worldPos.xz / 512)` within the square) at the patch's level,
//!     up to four lights; attenuation `1 - clamp(d / r, 0, 1)^2`;
//!   - models (the tile-position variant): the model's tile from a vertex
//!     varying, up to four lights; attenuation `1 / max(1e-6, d^2 / r^2)`
//!     (inverse square, as the classic model shader's point lights). The same
//!     shader can add eight per-draw lights, with the same attenuation.
//!
//!   Both add `clamp(colour * intensity, 0, 1) * clamp(dot(N, L), 0, 1) *
//!   attenuation` into a point light term: diffuse only. No specular term
//!   (the specular BRDF is the sun's), and no shadows (the only shadow maps
//!   are the sun's cascades). How the modern shader combines the term is
//!   not known exactly; this crate adds it to the diffuse light beside the
//!   sun and the ambient.
//!
//! # What this crate does
//!
//! - **Floors** use the grid: each fragment's tile under its floor's level,
//!   up to four lights, the terrain attenuation.
//! - **Models** use the lights the classic scene selected for the entity
//!   (`ModelLights::selected`: up to four lights from the entity's tiles,
//!   with the classic wall and diagonal rules), the lights the faithful model
//!   shader receives, with the model attenuation. The modern client looks a
//!   model's tile up in the same grid; the classic selection covers the
//!   model's whole footprint.
//! - **Colour.** The modern client packs the light's colour as `c / 255`
//!   without the 2.2 decode it applies to environment colours, and so does
//!   this crate: the classic floor light pass multiplies the display colour
//!   by `1 + c * f` (`GL_DST_COLOR, GL_ONE`), whose linear equivalent is
//!   close to linear in `c`, so the undecoded colour keeps the lamp pools'
//!   hue. The colour is times the intensity and clamped, then times
//!   [`STRENGTH`], this crate's calibration.
//! - **Cap.** Four lights per tile and four per model, the same cap the
//!   classic table has. Further lights on a full tile are not in its list
//!   (the classic table drops them the same way when it is built).
//!   [`GridStats`] counts the full tiles.
//! - **Double counting.** The faithful backends draw the classic baked static
//!   floor lights (`snapshot.lights`, passes that multiply the floor by
//!   `1 + light`) as separate geometry after the floor; the floor vertex
//!   colours carry no point light (only the sun's lambert when a floor has no
//!   normals). This crate never draws `snapshot.lights`: its per-fragment
//!   lookup replaces the baked passes, so each light reaches a floor once.

use rs910_scene::model_lights::ModelLights;

/// The side of a tile in fine units.
pub const TILE: f32 = 512.0;

/// Lights per tile and per model.
pub const MAX_PER_TILE: usize = 4;

/// This crate's scale of the modern point light term (see the module docs).
/// The modern client adds `clamp(colour * intensity, 0, 1)` as is; at 1.0 the
/// lamp pools at Draynor at night showed 84-89% of the faithful frame's mean
/// value, at 1.5 96-105% (hue within 1°; renderer plan §4(f), three online
/// spots, strength runs).
pub const STRENGTH: f32 = 1.5;

/// One light as the forward pass reads it (`PointLight` in the WGSL).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    /// Camera-local position (classic axes, y down) and the radius.
    pub pos_radius: [f32; 4],
    /// Linear colour times intensity and [`STRENGTH`]; `w` unused.
    pub colour: [f32; 4],
}

/// The `PointGrid` uniform block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridUniforms {
    /// The scene-local camera origin (camera-local + origin = scene-local).
    pub origin: [f32; 4],
    /// x tiles, z tiles, levels, light count.
    pub dims: [u32; 4],
}

/// A light of the frame on the CPU (scene-local).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub pos: [f32; 3],
    pub radius: f32,
    /// `clamp(colour * intensity, 0, 1)`, before [`STRENGTH`].
    pub colour: [f32; 3],
    /// The light's intensity this frame (its flicker and group fade; 0 is off).
    pub intensity: f32,
    /// Whether the light casts shadows. The map's light records carry no
    /// such flag, so every static light has it set (the point-light shadows'
    /// candidate rule reads it, `crate::shadows::point`).
    pub casts_shadows: bool,
}

impl Light {
    /// The colour term: `c / 255` per channel times `intensity`,
    /// clamped to 0..1.
    #[must_use]
    pub fn colour_term(rgb: i32, intensity: f32) -> [f32; 3] {
        [16, 8, 0].map(|s| (((rgb >> s) & 0xFF) as f32 / 255.0 * intensity).clamp(0.0, 1.0))
    }

    /// The GPU record, camera-local about `origin`, the colour times
    /// `strength` ([`STRENGTH`] unless a calibration run overrides it).
    #[must_use]
    pub fn gpu(&self, origin: [f32; 3], strength: f32) -> GpuLight {
        GpuLight {
            pos_radius: [
                self.pos[0] - origin[0],
                self.pos[1] - origin[1],
                self.pos[2] - origin[2],
                self.radius,
            ],
            colour: [
                self.colour[0] * strength,
                self.colour[1] * strength,
                self.colour[2] * strength,
                0.0,
            ],
        }
    }
}

/// The terrain attenuation: `1 - clamp(d / r, 0, 1)^2`.
#[must_use]
pub fn floor_attenuation(d: f32, r: f32) -> f32 {
    let ratio = (d / r).clamp(0.0, 1.0);
    1.0 - ratio * ratio
}

/// The model attenuation: `1 / max(1e-6, d^2 / r^2)`.
#[must_use]
pub fn model_attenuation(d: f32, r: f32) -> f32 {
    1.0 / (d * d / (r * r)).max(1e-6)
}

/// The per-tile light index grid (see the module docs): the entry at
/// `(level * nz + z) * nx + x` holds tile `(x, z)`'s up to four ids plus
/// one (0: none); the forward pass reads it from a storage buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub levels: usize,
    pub nx: usize,
    pub nz: usize,
    /// Row-major entries, `(level * nz + z) * nx + x`.
    pub entries: Vec<[u16; 4]>,
}

/// Grid diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridStats {
    /// Tiles (over every level) with at least one light.
    pub lit_tiles: usize,
    /// Tiles holding the cap of four.
    pub full_tiles: usize,
}

impl Grid {
    /// The table `lights` holds.
    #[must_use]
    pub fn from_model_lights(lights: &ModelLights) -> Self {
        let (levels, nx, nz) = lights.grid_size();
        let mut entries = Vec::with_capacity(levels * nx * nz);
        for level in 0..levels {
            for z in 0..nz {
                for x in 0..nx {
                    entries.push(lights.tile_slots(level, x, z));
                }
            }
        }
        Self {
            levels,
            nx,
            nz,
            entries,
        }
    }

    /// The slots of tile `(level, x, z)` (all empty outside the grid).
    #[must_use]
    pub fn slots(&self, level: usize, x: i32, z: i32) -> [u16; 4] {
        if level >= self.levels || x < 0 || z < 0 {
            return [0; 4];
        }
        let (x, z) = (x as usize, z as usize);
        if x >= self.nx || z >= self.nz {
            return [0; 4];
        }
        self.entries[(level * self.nz + z) * self.nx + x]
    }

    #[must_use]
    pub fn stats(&self) -> GridStats {
        let mut stats = GridStats::default();
        for t in &self.entries {
            if t[0] != 0 {
                stats.lit_tiles += 1;
            }
            if t[MAX_PER_TILE - 1] != 0 {
                stats.full_tiles += 1;
            }
        }
        stats
    }
}

/// The frame's lights from `lights` (every static light of the scene, its
/// intensity and colour this frame), scene-local.
#[must_use]
pub fn frame_lights(lights: &ModelLights) -> Vec<Light> {
    lights
        .lights
        .iter()
        .enumerate()
        .map(|(id, l)| {
            let intensity = lights.intensities.get(id).copied().unwrap_or(1.0);
            let rgb = lights.colour(id).unwrap_or(l.colour);
            Light {
                pos: [l.x as f32, l.y as f32, l.z as f32],
                radius: l.radius as f32,
                colour: Light::colour_term(rgb, intensity),
                intensity,
                casts_shadows: true,
            }
        })
        .collect()
}

/// A model draw's light slots for the instance record: the entity's classic
/// selection, ids plus one, 0 for none.
#[must_use]
pub fn model_slots(lights: &ModelLights, entity: usize) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (slot, &id) in lights
        .selected(entity)
        .iter()
        .take(MAX_PER_TILE)
        .enumerate()
    {
        out[slot] = (id + 1) as f32;
    }
    out
}

/// The light a floor fragment at scene-local `p` with unit normal `n` of
/// `level` receives (the forward shader's floor path on the CPU, before
/// [`STRENGTH`]; tests).
#[must_use]
pub fn floor_light_at(
    grid: &Grid,
    lights: &[Light],
    level: usize,
    p: [f32; 3],
    n: [f32; 3],
) -> [f32; 3] {
    let tx = (p[0] / TILE).floor() as i32;
    let tz = (p[2] / TILE).floor() as i32;
    let mut out = [0.0; 3];
    for id in grid.slots(level, tx, tz) {
        if id == 0 {
            break;
        }
        let Some(l) = lights.get(usize::from(id) - 1) else {
            continue;
        };
        let v = [l.pos[0] - p[0], l.pos[1] - p[1], l.pos[2] - p[2]];
        let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if d <= 0.0 {
            continue;
        }
        let n_dot_l = ((n[0] * v[0] + n[1] * v[1] + n[2] * v[2]) / d).clamp(0.0, 1.0);
        let a = floor_attenuation(d, l.radius) * n_dot_l;
        for (o, c) in out.iter_mut().zip(l.colour) {
            *o += c * a;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(level: u8, x: i32, z: i32, span: u8) -> rs910_scene::env::StaticLight {
        rs910_scene::env::StaticLight {
            level,
            above: false,
            below: false,
            x,
            y: -400,
            z,
            radius: (i32::from(span) << 9) + 256,
            colour: 0xFF_C080,
            flicker: 0,
            phase: 0,
            wave: 0,
            offset: 0,
            amplitude: 2048,
            speed: 2048,
            group: -1,
            span_runs: vec![u16::from(2 * span + 1); 2 * usize::from(span) + 1],
        }
    }

    /// The grid is the classic table (`ModelLights::new`): a light's span of
    /// tiles on its level, four per tile, later lights of a full tile
    /// dropped; the frame's lights carry the intensity and colour.
    #[test]
    fn grid_is_the_classic_tile_table() {
        let scene = rs910_scene::scene::Scene::new(9, 2, 16, 16);
        let centre = 8 * 512 + 256;
        let mut lights: Vec<_> = (0..5).map(|_| light(0, centre, centre, 1)).collect();
        lights.push(light(1, centre, centre, 0));
        let table = ModelLights::new(&scene, &lights, 0);
        let grid = Grid::from_model_lights(&table);
        assert_eq!((grid.levels, grid.nx, grid.nz), (2, 16, 16));
        // Span 1: tiles 7..=9 on both axes; four of the five lights.
        assert_eq!(grid.slots(0, 8, 8), [1, 2, 3, 4]);
        assert_eq!(grid.slots(0, 7, 9), [1, 2, 3, 4]);
        assert_eq!(grid.slots(0, 6, 8), [0; 4]);
        assert_eq!(grid.slots(1, 8, 8), [6, 0, 0, 0]);
        assert_eq!(grid.slots(1, 7, 8), [0; 4]);
        assert_eq!(grid.slots(0, -1, 8), [0; 4]);
        assert_eq!(
            grid.stats(),
            GridStats {
                lit_tiles: 10,
                full_tiles: 9
            }
        );
        let frame = frame_lights(&table);
        assert_eq!(frame.len(), 6);
        assert_eq!(frame[0].pos, [centre as f32, -400.0, centre as f32]);
        assert_eq!(frame[0].radius, 768.0);
        assert_eq!(frame[0].colour, Light::colour_term(0xFF_C080, 1.0));
        // Under the light, straight down onto a flat floor: attenuation
        // 1 - (400/768)^2 of four lights.
        let at = floor_light_at(
            &grid,
            &frame,
            0,
            [centre as f32, 0.0, centre as f32],
            [0.0, -1.0, 0.0],
        );
        let a = floor_attenuation(400.0, 768.0);
        assert!((at[0] - 4.0 * a).abs() < 1e-5, "{at:?}");
    }
}
