//! The modern renderer's environment lighting: the sun and ambient colours of the forward pass,
//! and the per-map-square environment of map file 6.
//!
//! # Lighting from the environment
//!
//! The modern client turns a map environment record into a `Sunlight` uniform block:
//!
//! - the sun colour `c / 255`, raised to 2.2 on the gamma-correct path;
//! - the sun colour times the record's **diffuse** intensity;
//! - the ambient colour: the colour times the **ambient** intensity;
//! - the negated normalised sun direction. All three fade over the transition time (the
//!   direction by a slerp).
//!
//! [`Lighting::from_env`] is that derivation over the classic environment the snapshot carries
//! (`EnvFrame`: `sun_rgb`, the ambient `setSunAmbientIntensity` value, which includes the
//! brightness preference, and `diffuse_half * 2`). The earlier constants (sun
//! `1.35 * diffuse/2`, ambient `1.05`, an sRGB-decoded colour) are gone: with the 2.2 display
//! transfer (`post::tonemap`) the formula alone puts a sunlit flat tile within 5% of the
//! faithful toolkit's brightness (the classic renderer: `ambient + N.L * diffuse/2` on display
//! values; Lumbridge 1.36, the modern `(ambient + N.L * diffuse)^(1/2.2)` = 1.29). The ambient
//! is split into a two-colour hemisphere ([`look`]) until light probes replace it.
//!
//! # Map file 6
//!
//! Map file 6 holds one record per map square (`rs910_config::nxt::map_environment`). Its
//! proven fields (sun colour, direction and intensities, fog colour and depth, bloom, skybox,
//! sampler, colour remap slot 0) equal the classic LAND trailer's in every square, which is
//! what the snapshot's `EnvFrame` is built from, with the classic chunk fade (`CHUNK_FADE_MS`)
//! and the server's environment overrides applied. So the colours, intensities, fog and grading
//! come from `EnvFrame`; what file 6 adds is the **per-square sun direction**: the classic
//! environment pushes one direction, refreshed only when the map is rebuilt
//! (`EnvState::sun_direction`), while the modern client takes each square's own direction and
//! slerps to it. [`EnvironmentState`] reads the record of the square under the camera target
//! and, when the record describes the environment the classic renderer is showing (its sun
//! colour, fog colour and intensities equal the frame's; not during a fade, an override, or in
//! an instanced copy of another square), turns the sun towards the record's direction with the
//! slerp over the classic chunk fade time. Otherwise the classic direction stands.
//!
//! Reads the record's tone-map block (`flag_136`, `u8_137`, `f_138`: [`ToneMapBlock`]).
//! Unused and flagged: the record's scattering block (inferred, `@44`/`@60`; its shader body is
//! not known), the floats and flags without a known meaning (`f_16`, `f_27`, `raw_96`,
//! `raw_128`, `raw_170`), the shadow intensity (no use of it in the sunlight or shadow
//! constants was found), and colour remap slot 1 (no classic source; the modern client reads
//! both slots, but slot 0 alone at weight 1.0 already replaces the whole colour in the classic
//! grade).

use std::collections::HashMap;

use rs910_config::nxt::map_environment::{decode_environment, MapEnvironment};

use crate::post::tonemap::display_to_linear;
use crate::scene_snapshot::SceneSnapshot;

/// This crate's balance of the environment's sun and ambient (M6's light
/// probes replace the hemisphere). Calibrated on the Lumbridge environment
/// (ambient 248, diffuse 312): a sunlit flat tile keeps the light of the
/// plain `ambient + N.L * diffuse` (within 5% of the faithful brightness,
/// module docs), while a tile in the sun's shadow keeps 0.73 of that on the
/// display, the darkening of a classic floor hard shadow (the mask's set
/// texels are alpha 68 of black, `SRC_ALPHA` blended: `1 - 68/255`;
/// `rs910_render_gpu::floorpass`). The plain ambient alone gave 0.77.
pub mod look {
    /// The upper hemisphere's ambient relative to the environment's.
    pub const SKY_AMBIENT: f32 = 0.9;
    /// The lower hemisphere's ambient relative to the environment's.
    pub const GROUND_AMBIENT: f32 = 0.45;
    /// The sun relative to the environment's diffuse intensity.
    pub const SUN: f32 = 1.125;
}

/// The forward pass's lighting (the `Sunlight` block): linear colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    /// Towards the sun (classic axes, y down), normalised.
    pub sun_dir: [f32; 3],
    pub sun_colour: [f32; 3],
    pub sky_ambient: [f32; 3],
    pub ground_ambient: [f32; 3],
}

impl Lighting {
    /// The derivation (module docs) over `env`, with the sun towards
    /// `sun_dir`.
    #[must_use]
    pub fn from_env(env: &rs910_scene::env::EnvFrame, sun_dir: [f32; 3]) -> Self {
        let colour = env.sun_rgb.map(|c| display_to_linear(c.clamp(0.0, 1.0)));
        let scaled = |k: f32| colour.map(|c| c * k);
        let ambient = env.sun.ambient;
        Self {
            sun_dir,
            sun_colour: scaled(env.sun.diffuse_half * 2.0 * look::SUN),
            sky_ambient: scaled(ambient * look::SKY_AMBIENT),
            ground_ambient: scaled(ambient * look::GROUND_AMBIENT),
        }
    }

    /// The linear light on a surface of unit normal `n` (classic axes) with
    /// the sun's visibility `sun` (the forward pass's `shade` without the
    /// specular and normal-map terms; tests and the look calibration).
    #[must_use]
    pub fn light_on(&self, n: [f32; 3], sun: f32) -> [f32; 3] {
        let n_dot_l = (n[0] * self.sun_dir[0] + n[1] * self.sun_dir[1] + n[2] * self.sun_dir[2])
            .max(0.0)
            * sun;
        let up = 0.5 - 0.5 * n[1];
        std::array::from_fn(|i| {
            self.ground_ambient[i]
                + (self.sky_ambient[i] - self.ground_ambient[i]) * up
                + self.sun_colour[i] * n_dot_l
        })
    }
}

/// Which direction the sun was turned to (diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SunSource {
    /// The classic environment's (`EnvFrame.sun.dir`).
    #[default]
    Classic,
    /// The camera square's map file 6 record.
    MapFile6,
}

/// A slerp of the sun direction (module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    from: glam::Vec3,
    to: glam::Vec3,
    start_ms: i64,
}

/// The tone-map block of an environment record as the modern client reads
/// it. Its decoder reads, between the two colours and the bloom block, a
/// flag (tone mapping enabled), a byte (the operator) and five floats (min
/// black, max white, key, max auto exposure, and one more); on each square
/// change the client puts them in the top post config, fading the floats.
/// The 910 file-6 record has the same block at the same place (`flag_136`,
/// `u8_137`, `f_138`; the modern format adds three fields elsewhere).
/// `min_auto` is not in the file (the record default 0 stays).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneMapBlock {
    pub enabled: bool,
    pub operator: i32,
    /// `[min black, max white, key, min auto, max auto]`.
    pub values: [f32; 5],
}

impl ToneMapBlock {
    /// The modern client's default record (also the post initialiser's
    /// config): a square without a record.
    pub const DEFAULT: Self = Self {
        enabled: true,
        operator: crate::post::RECORD_OPERATOR,
        values: [
            crate::post::RECORD_MIN_BLACK_LUM,
            crate::post::RECORD_MAX_WHITE_LUM,
            crate::post::RECORD_EXPOSURE_KEY,
            crate::post::RECORD_MIN_AUTO_EXPOSURE,
            crate::post::RECORD_MAX_AUTO_EXPOSURE,
        ],
    };

    /// The block of a 910 file-6 record.
    #[must_use]
    pub fn of(record: &MapEnvironment) -> Self {
        let f = record.f_138;
        Self {
            enabled: record.flag_136,
            operator: i32::from(record.u8_137),
            values: [
                f[0],
                f[1],
                f[2],
                crate::post::RECORD_MIN_AUTO_EXPOSURE,
                f[3],
            ],
        }
    }
}

/// A fade of the tone-map values (a linear transition of each config
/// entry over [`crate::post::TONE_MAP_FADE_MS`]); the flag and the
/// operator switch at once.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToneFade {
    from: [f32; 5],
    to: ToneMapBlock,
    start_ms: i64,
}

impl ToneFade {
    fn at(&self, now_ms: i64) -> ToneMapBlock {
        let t = ((now_ms - self.start_ms) as f32 / crate::post::TONE_MAP_FADE_MS as f32)
            .clamp(0.0, 1.0);
        if t >= 1.0 {
            return self.to;
        }
        let mut out = self.to;
        for (v, from) in out.values.iter_mut().zip(self.from) {
            *v = from + (*v - from) * t;
        }
        out
    }
}

/// The per-square environment state (M5): the decoded file-6 records by
/// map group, and the sun's current turn.
#[derive(Debug, Default)]
pub struct EnvironmentState {
    records: HashMap<u32, Option<MapEnvironment>>,
    turn: Option<Turn>,
    tone: Option<ToneFade>,
    /// The last frame's target and its source.
    pub source: SunSource,
}

/// The sun's transition time: the classic chunk environment fade, which
/// the modern client also uses as the transition of a square change.
pub const TURN_MS: i64 = rs910_scene::env::CHUNK_FADE_MS as i64;

impl EnvironmentState {
    /// The file-6 record of map group `group` (decoded once; `None` when
    /// the square has none or it does not decode).
    fn record(&mut self, pack: &crate::cache::Pack, group: u32) -> Option<&MapEnvironment> {
        self.records
            .entry(group)
            .or_insert_with(|| {
                let files = pack
                    .read_group(rs910_config::nxt::MAP_ARCHIVE, group)
                    .ok()?;
                let bytes = files.get(&rs910_config::nxt::ENVIRONMENT_FILE)?;
                match decode_environment(group, bytes) {
                    Ok(record) => {
                        log::debug!(
                            "[modern] map file 6 group {group}: unused fields f_16 {} f_27 {:?} scattering {:?} {:?} f_138 {:?} remap slot 1 {:?}",
                            record.f_16,
                            record.f_27,
                            record.scattering_params,
                            record.scattering,
                            record.f_138,
                            record.colour_remap[1]
                        );
                        Some(record)
                    }
                    Err(err) => {
                        log::warn!("[modern] map file 6 group {group}: {err}");
                        None
                    }
                }
            })
            .as_ref()
    }

    /// The camera square's file-6 scattering block
    /// (`scattering_params`, `[tint, out, in]`; inferred fields, see
    /// `atmosphere`), `None` without a record.
    pub fn scattering(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
    ) -> Option<([f32; 4], [[f32; 3]; 3])> {
        let pack = snapshot.pack?;
        let [x, _, z] = snapshot.camera.target;
        if x < 0 || z < 0 {
            return None;
        }
        let group = rs910_config::nxt::map_group((x >> 15) as u32, (z >> 15) as u32);
        self.record(pack, group)
            .map(|r| (r.scattering_params, r.scattering))
    }

    /// The camera square's file-6 record (`None` without one), the record
    /// the NXT environment apply reads (`lighting::environment_record`).
    pub fn camera_record(&mut self, snapshot: &SceneSnapshot<'_>) -> Option<&MapEnvironment> {
        let pack = snapshot.pack?;
        let [x, _, z] = snapshot.camera.target;
        if x < 0 || z < 0 {
            return None;
        }
        let group = rs910_config::nxt::map_group((x >> 15) as u32, (z >> 15) as u32);
        self.record(pack, group)
    }

    /// The camera square's global environment cube, a
    /// material id (`sampler_material`: `lighting::env_reflections`);
    /// `None` without a record or id.
    pub fn global_cube(&mut self, snapshot: &SceneSnapshot<'_>) -> Option<u16> {
        self.camera_record(snapshot)?.sampler_material
    }

    /// The tone-map block of the camera square's record at
    /// `now_ms` ([`ToneMapBlock`]; [`ToneMapBlock::DEFAULT`] without a
    /// record), fading from the previous square's over the modern transition.
    pub fn tone_map(&mut self, snapshot: &SceneSnapshot<'_>, now_ms: i64) -> ToneMapBlock {
        let target = snapshot
            .pack
            .and_then(|pack| {
                let [x, _, z] = snapshot.camera.target;
                if x < 0 || z < 0 {
                    return None;
                }
                let group = rs910_config::nxt::map_group((x >> 15) as u32, (z >> 15) as u32);
                self.record(pack, group).map(ToneMapBlock::of)
            })
            .unwrap_or(ToneMapBlock::DEFAULT);
        let fade = match self.tone {
            Some(fade) if fade.to != target => ToneFade {
                from: fade.at(now_ms).values,
                to: target,
                start_ms: now_ms,
            },
            Some(fade) => fade,
            None => ToneFade {
                from: target.values,
                to: target,
                start_ms: now_ms,
            },
        };
        self.tone = Some(fade);
        fade.at(now_ms)
    }

    /// The sun direction of this frame at `now_ms`: towards the camera
    /// square's file-6 direction when its record describes `env` (module
    /// docs), else the classic one; changes of target turn the sun over
    /// [`TURN_MS`].
    pub fn sun_dir(&mut self, snapshot: &SceneSnapshot<'_>, now_ms: i64) -> [f32; 3] {
        let env = snapshot.env;
        let classic = glam::Vec3::from(env.sun.dir).normalize_or(glam::Vec3::new(0.0, -1.0, 0.0));
        let target = snapshot.pack.and_then(|pack| {
            let [x, _, z] = snapshot.camera.target;
            if x < 0 || z < 0 {
                return None;
            }
            let group = rs910_config::nxt::map_group((x >> 15) as u32, (z >> 15) as u32);
            self.record(pack, group)
                .and_then(|r| record_direction(r, env))
        });
        self.source = if target.is_some() {
            SunSource::MapFile6
        } else {
            SunSource::Classic
        };
        let target = target.unwrap_or(classic);
        let turn = match self.turn {
            None => Turn {
                from: target,
                to: target,
                start_ms: now_ms,
            },
            Some(turn) if (turn.to - target).length_squared() > 1e-12 => Turn {
                from: turn.at(now_ms),
                to: target,
                start_ms: now_ms,
            },
            Some(turn) => turn,
        };
        self.turn = Some(turn);
        turn.at(now_ms).to_array()
    }
}

impl Turn {
    /// The slerp of the direction at `now_ms`.
    fn at(&self, now_ms: i64) -> glam::Vec3 {
        let t = ((now_ms - self.start_ms) as f32 / TURN_MS as f32).clamp(0.0, 1.0);
        if t >= 1.0 {
            return self.to;
        }
        let cos = self.from.dot(self.to).clamp(-1.0, 1.0);
        let angle = cos.acos() * t;
        let ortho = (self.to - self.from * cos).normalize_or_zero();
        if ortho == glam::Vec3::ZERO {
            return self.to;
        }
        (self.from * angle.cos() + ortho * angle.sin()).normalize()
    }
}

/// The record's normalised sun direction when it describes the frame's
/// environment: the sun colour, the fog colour and the diffuse intensity
/// equal the ones `env` was built from (the ambient carries the classic
/// brightness preference, so it is not compared).
fn record_direction(
    record: &MapEnvironment,
    env: &rs910_scene::env::EnvFrame,
) -> Option<glam::Vec3> {
    let sun_rgb = rs910_scene::env::sun_rgb(record.sun_colour as i32);
    let fog = [16, 8, 0].map(|s| ((record.fog_colour >> s) & 0xFF) as f32 / 255.0);
    let diffuse_half = record.sun_intensity()[1] * 0.5;
    let same = sun_rgb == env.sun_rgb && fog == env.clear && diffuse_half == env.sun.diffuse_half;
    if !same {
        return None;
    }
    // The classic environment pushes `(int)dir << 2` and normalises; the
    // shift does not change the direction.
    let d = record.sun_direction.map(f32::from);
    let v = glam::Vec3::from(d);
    (v.length_squared() > 0.0).then(|| v.normalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_turns_along_the_great_circle() {
        let from = glam::Vec3::new(0.0, -1.0, 0.0);
        let to = glam::Vec3::new(1.0, 0.0, 0.0);
        let turn = Turn {
            from,
            to,
            start_ms: 1000,
        };
        assert!((turn.at(1000) - from).length() < 1e-6);
        assert!((turn.at(1000 + TURN_MS) - to).length() < 1e-6);
        let half = turn.at(1000 + TURN_MS / 2);
        assert!((half.length() - 1.0).abs() < 1e-5);
        let expected = (from + to).normalize();
        assert!((half - expected).length() < 1e-3, "{half}");
    }

    /// Lumbridge's record (group 6450, hand-decoded in
    /// `rs910_config::nxt`) describes the classic environment of the same
    /// square, and its direction is the classic one; a changed fog colour (a
    /// fade or override in progress) keeps the classic direction.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore)]
    fn lumbridge_record_describes_the_classic_environment() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let mut state = EnvironmentState::default();
        let record = state
            .record(&pack, 6450)
            .expect("group 6450 file 6")
            .clone();
        let env = rs910_scene::env::Environment {
            sun_colour: record.sun_colour as i32,
            sun_ambient: record.sun_intensity()[0],
            sun_diffuse: record.sun_intensity()[1],
            sun_shadow: record.sun_intensity()[2],
            sun_dir: record.sun_direction.map(f32::from),
            fog_colour: record.fog_colour as i32,
            fog_depth: i32::from(record.fog_depth),
            ..Default::default()
        };
        let mut frame = rs910_scene::env::EnvFrame::build(
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
        );
        let dir = record_direction(&record, &frame).expect("the record matches");
        assert!((dir - glam::Vec3::from(frame.sun.dir)).length() < 1e-5);
        frame.clear[0] += 0.01;
        assert_eq!(record_direction(&record, &frame), None);
    }

    /// Lumbridge's record selects `FILMIC_UNCHARTED2` with
    /// its own tone-map values (not the defaults); a square change fades
    /// the values linearly over the modern transition and switches the operator
    /// at once.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore)]
    fn lumbridge_tone_map_block_fades_in() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let mut state = EnvironmentState::default();
        let block = ToneMapBlock::of(state.record(&pack, 6450).expect("group 6450 file 6"));
        assert!(block.enabled);
        assert_eq!(block.operator, crate::post::OPERATOR_FILMIC_UNCHARTED2);
        assert_eq!(block.values, [0.02, 1.2, 0.5, 0.0, 2.88]);
        let fade = ToneFade {
            from: ToneMapBlock::DEFAULT.values,
            to: block,
            start_ms: 1000,
        };
        let half = fade.at(1000 + crate::post::TONE_MAP_FADE_MS / 2);
        assert!((half.values[1] - 1.35).abs() < 1e-6, "{half:?}");
        assert_eq!(fade.at(1000 + crate::post::TONE_MAP_FADE_MS), block);
    }
}
