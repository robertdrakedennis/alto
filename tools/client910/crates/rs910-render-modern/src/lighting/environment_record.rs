//! The environment record's colour grading and sky fog as the modern client applies them: the
//! look values of `lighting::look` assume them. Every rule here is traced in the modern client
//! itself.
//!
//! # Colour remapping
//!
//! - **The record holds one remap.** The environment record's decoder sizes the id and weight
//!   vectors to three (ids -1, weights 0) and then reads the file's two `(s16 id, f32 weight)`
//!   pairs **both into element 0** (each store goes through the vectors' begin pointer,
//!   re-read, with no index). The second pair wins: the modern client grades by the file's
//!   second slot alone (Lumbridge: `27692 @ 0.85`, not the classic `22216 @ 1.0`; a square whose
//!   second slot is empty is not graded), and an id is a signed 16-bit value (ids above 32767
//!   read negative). See [`record_remaps`]. The 910 file-6 layout is aligned with the modern
//!   record, as for the tone-map block (layout-aligned: the 910 format itself is not in the
//!   modern client).
//! - **The environment apply** writes the three pairs into the top post config; when they
//!   differ from the current ones it starts a transition over the apply's duration (5000 ms,
//!   `crate::post::TONE_MAP_FADE_MS`): `from` = current, `to`, the times, and whether each end
//!   is the empty triple (set to `(-1, 0) x 3` at start-up). No duration: `to` and current are
//!   the new pairs.
//! - **The per-frame fade** (run by the frame orchestrator before the config is copied to the
//!   effect): past the end, current = `to`; else with `t` the elapsed share, fading to the empty
//!   triple: `from` with its weights times `1 - t`; from the empty triple: `to` times `t`;
//!   between two remap sets: the list of `from`'s pairs (id >= 0) times `1 - t`, then `to`'s
//!   times `t`, each added to an equal id among the list's first three entries or appended;
//!   more than three: sorted by weight (descending, stable), the first three kept and scaled by
//!   the list's sum over theirs; the slots past the list's length are `(-1, 0)`. See
//!   [`RemapFade`].
//! - **The LUTs**: a slot with an id >= 0 loads a 2D texture of the sprite archive (js5 index 8,
//!   **sprites**) and decodes frame 0 as RGBA; an id < 0 has no texture. The colour correction
//!   samples it as a 256 x 16 strip (half-texel offsets `0.5/256`, `0.5/16`; blue selects a
//!   16-texel slice): the `post::grading` lookup. See [`crate::post::grading::LutCache`] (a
//!   sprite that is not 256 x 16 is not loaded: its weight is dropped).
//! - **The weights**: with colour correction on (1 in the post initialiser's config, never
//!   written by the environment apply) the slot count is 3 when id 2 >= 0, else 2 when id 1 >=
//!   0, else 1; the colour remap weightings are `(1 - Σ w, w0, w1, w2)` over the counted slots.
//!   See [`grading`].
//!
//! # Angle fog
//!
//! The fog parameters pack `z = pow(start / end, e) * s` and `w = offset` (the three are the
//! record's floats `s`, `e` and `offset`, each a linear transition; `start`/`end` the fog's),
//! which the angle fog raises `1 - clamp(dir.y + w, 0, 1)` to. The modern decoder reads four
//! floats where the 910 file has `f_27[0..3]` (record defaults `0, 32, 8, 0`);
//! [`angle_fog_params`] replaces `atmosphere::sky`'s chosen power 6 and offset 0 in the sky
//! pass (when the distance fog is on).
//!
//! The remapping applies with the verified look (`ModernSettings::look`: the data weights
//! unscaled); with the calibrated look the environment's classic grading stays. The angle fog
//! always applies.

use rs910_config::nxt::map_environment::MapEnvironment;

/// Three `(id, weight)` remap slots.
pub type Remaps = [(i32, f32); 3];

/// The empty triple.
pub const NO_REMAPS: Remaps = [(-1, 0.0); 3];

/// The modern record decode of a 910 file-6 record's remaps (module docs): the
/// second pair in slot 0, its id as a signed 16-bit value.
#[must_use]
pub fn record_remaps(record: &MapEnvironment) -> Remaps {
    let (id, weight) = record.colour_remap[1];
    [
        (id.map_or(-1, |v| i32::from(v as i16)), weight),
        (-1, 0.0),
        (-1, 0.0),
    ]
}

/// The environment's remap transition in the top post config and its
/// per-frame fade (module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemapFade {
    pub current: Remaps,
    from: Remaps,
    to: Remaps,
    start_ms: i64,
    end_ms: i64,
    from_empty: bool,
    to_empty: bool,
}

impl Default for RemapFade {
    /// The post initialiser's config.
    fn default() -> Self {
        Self {
            current: NO_REMAPS,
            from: NO_REMAPS,
            to: NO_REMAPS,
            start_ms: 0,
            end_ms: 0,
            from_empty: true,
            to_empty: true,
        }
    }
}

impl RemapFade {
    /// The environment apply's write of `new` at `now_ms` with a transition
    /// of `duration_ms` (0: none).
    pub fn set(&mut self, new: Remaps, now_ms: i64, duration_ms: i64) {
        if duration_ms == 0 {
            self.to = new;
            self.current = new;
            return;
        }
        if new == self.current {
            return;
        }
        self.from = self.current;
        self.start_ms = now_ms;
        self.to = new;
        self.end_ms = now_ms + duration_ms;
        self.from_empty = self.from == NO_REMAPS;
        self.to_empty = self.to == NO_REMAPS;
    }

    /// The frame's slots at `now_ms` (the orchestrator's fade).
    pub fn advance(&mut self, now_ms: i64) -> Remaps {
        if now_ms > self.end_ms {
            self.current = self.to;
            return self.current;
        }
        let t = (now_ms - self.start_ms) as f32 / (self.end_ms - self.start_ms) as f32;
        let scaled = |r: Remaps, k: f32| r.map(|(id, w)| (id, w * k));
        if self.to_empty {
            self.current = scaled(self.from, 1.0 - t);
            return self.current;
        }
        if self.from_empty {
            self.current = scaled(self.to, t);
            return self.current;
        }
        let mut list: Vec<(i32, f32)> = self
            .from
            .iter()
            .filter(|(id, _)| *id >= 0)
            .map(|&(id, w)| (id, w * (1.0 - t)))
            .collect();
        for &(id, w) in self.to.iter().filter(|(id, _)| *id >= 0) {
            let w = w * t;
            let head = list.len().min(3);
            match list[..head].iter_mut().find(|e| e.0 == id) {
                Some(e) => e.1 += w,
                None => list.push((id, w)),
            }
        }
        if list.len() > 3 {
            let total = list[1..].iter().fold(list[0].1, |s, e| s + e.1);
            // Insertion sort, descending, moving an entry only past a
            // strictly lighter one.
            for i in 1..list.len() {
                let mut j = i;
                while j > 0 && list[j].1 > list[j - 1].1 {
                    list.swap(j, j - 1);
                    j -= 1;
                }
            }
            let scale = total / (list[0].1 + list[1].1 + list[2].1);
            self.current = std::array::from_fn(|k| (list[k].0, list[k].1 * scale));
        } else {
            self.current = std::array::from_fn(|k| list.get(k).copied().unwrap_or((-1, 0.0)));
        }
        self.current
    }
}

/// The slots for `remaps` as this crate's grading (the
/// composite's `crate::post::grading::Grading`: no levels): the counted slots, the
/// base `1 - Σ w` over them, each slot's LUT when its id is a 256 x 16
/// sprite (`has_lut`), else no sample (weight 0; the modern client would sample a
/// texture this crate does not have).
#[must_use]
pub fn grading(
    remaps: &Remaps,
    mut has_lut: impl FnMut(i32) -> bool,
) -> crate::post::grading::Grading {
    let count = if remaps[2].0 >= 0 {
        3
    } else if remaps[1].0 >= 0 {
        2
    } else {
        1
    };
    let base = 1.0 - remaps[..count].iter().map(|r| r.1).sum::<f32>();
    let mut luts = [-1; 3];
    let mut weights = [0.0; 3];
    for (k, &(id, w)) in remaps.iter().enumerate().take(count) {
        if id >= 0 && has_lut(id) {
            luts[k] = id;
            weights[k] = w;
        }
    }
    crate::post::grading::Grading {
        levels: None,
        luts,
        weights,
        count: count as u32,
        base,
    }
}

/// The composite's block for a [`grading`] result (the slot count as the modern
/// client counts it, the base and weights as they are; no levels).
#[must_use]
pub fn post_uniforms(
    grading: &crate::post::grading::Grading,
    encode: bool,
) -> crate::post::grading::PostUniforms {
    crate::post::grading::PostUniforms {
        params: [
            crate::post::tonemap::EXPOSURE,
            if encode { 1.0 } else { 0.0 },
            grading.count as f32,
            0.0,
        ],
        weights: [
            grading.base,
            grading.weights[0],
            grading.weights[1],
            grading.weights[2],
        ],
        levels: [1.0, 0.0, 1.0, 0.0],
        levels_max: [1.0, 0.0, 0.0, 0.0],
    }
}

/// The record's angle-fog floats (scale, exponent, offset) = 910
/// `f_27[1..3]`.
#[must_use]
pub fn record_angle_fog(record: &MapEnvironment) -> [f32; 3] {
    [record.f_27[1], record.f_27[2], record.f_27[3]]
}

/// The modern record defaults of those floats.
pub const DEFAULT_ANGLE_FOG: [f32; 3] = [32.0, 8.0, 0.0];

/// The angle-fog power and offset for the fog's `start` and `end` and the
/// record's floats: the power `pow(start / end, e) * s` and the offset.
#[must_use]
pub fn angle_fog_params(start: f32, end: f32, record: [f32; 3]) -> [f32; 2] {
    let [s, e, offset] = record;
    [(start / end).powf(e) * s, offset]
}

/// A linear transition of the three angle-fog floats.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade3 {
    from: [f32; 3],
    to: [f32; 3],
    start_ms: i64,
}

impl Fade3 {
    fn at(&self, now_ms: i64) -> [f32; 3] {
        let t = ((now_ms - self.start_ms) as f32 / crate::post::TONE_MAP_FADE_MS as f32)
            .clamp(0.0, 1.0);
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * t)
    }
}

/// The per-frame state: the camera square's remaps and angle fog, with
/// the client's transitions (the first square applies at once, as
/// `lighting::environment`'s tone map; a square change fades over 5000 ms).
#[derive(Clone, Debug, Default)]
pub struct EnvironmentRecord {
    /// Whether the frame grades by [`Self::remaps`] (the verified look).
    pub remap: bool,
    target: Option<(Remaps, [f32; 3])>,
    fade: RemapFade,
    angle: Option<Fade3>,
    /// This frame's slots.
    pub remaps: Remaps,
    /// This frame's angle-fog floats (scale, exponent, offset).
    pub angle_fog: [f32; 3],
}

impl EnvironmentRecord {
    /// The state of look `mode` (module docs).
    #[must_use]
    pub fn for_mode(mode: crate::settings::LookMode) -> Self {
        Self {
            remap: mode == crate::settings::LookMode::Verified,
            angle_fog: DEFAULT_ANGLE_FOG,
            ..Self::default()
        }
    }

    /// The camera square's record (`None`: the default record) at
    /// `now_ms`.
    pub fn update(&mut self, record: Option<&MapEnvironment>, now_ms: i64) {
        let target = record.map_or((NO_REMAPS, DEFAULT_ANGLE_FOG), |r| {
            (record_remaps(r), record_angle_fog(r))
        });
        let first = self.target.is_none();
        if self.target != Some(target) {
            let duration = if first {
                0
            } else {
                crate::post::TONE_MAP_FADE_MS
            };
            self.fade.set(target.0, now_ms, duration);
            self.angle = Some(Fade3 {
                from: if first { target.1 } else { self.angle_fog },
                to: target.1,
                start_ms: now_ms,
            });
            self.target = Some(target);
        }
        self.remaps = self.fade.advance(now_ms);
        self.angle_fog = self.angle.map_or(DEFAULT_ANGLE_FOG, |a| a.at(now_ms));
    }
}
