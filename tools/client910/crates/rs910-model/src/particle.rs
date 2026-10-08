//! The particle stack.
//!
//! * Config: the emitter and effector types from the `particles` archive,
//!   group 0 (emitters) and group 1 (effectors), cached by id.
//! * Runtime: a global owner ([`Runtime`]) with one particle system per model
//!   owner, each with its emitters, effectors and moving particles, advanced
//!   once per client cycle and culled into per-system draw lists.
//!
//! Model owners hand the runtime their current emitter triangles and effector
//! vertices ([`crate::gpumodel::GpuModel::particle_anchors`]) through
//! [`Runtime::bind`]; the draw lists are consumed by the particle renderer.
//!
//! The random source is the reproducible 48-bit LCG stream of
//! [`AnimationRandom`]; a global seed is not part of scene state.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::animation_random::AnimationRandom;
use crate::cache::Pack;
use rs910_core::reader::{Eof, Reader as CoreReader};

/// The particle archive name in the pack.
pub const PARTICLE_ARCHIVE: &str = "particles";

/// The previous-cycle live particle count above which no emitter spawns, per
/// particle detail level.
pub const PARTICLE_LIMITS: [i32; 3] = [2047, 16383, 65535];
/// The slots of a system's particle ring.
pub const SYSTEM_SLOTS: usize = 8192;
/// Emitters per system.
pub const MAX_SYSTEM_EMITTERS: usize = 64;
/// Effectors per system.
pub const MAX_SYSTEM_EFFECTORS: usize = 16;
/// Global (type 1) effectors the runtime keeps at most.
pub const MAX_GLOBAL_EFFECTORS: i32 = 32;
/// Unbound systems die after this many cycles.
pub const SYSTEM_TIMEOUT: i64 = 750;
/// The recycled-system ring mask per particle detail level.
pub const SYSTEM_POOL_MASKS: [usize; 3] = [3, 7, 15];
/// The length of the released-particle ring.
const PARTICLE_POOL: usize = 1024;
/// Owner key of a system no owner references any more (the owner replaced
/// its dead system).
const UNOWNED: u64 = u64::MAX;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// One `rs910_core::reader` read at `pos`; `pos` follows the read.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|_| anyhow::anyhow!("particle config truncated"))
    }
    fn g1(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g1).map(i32::from)
    }
    fn g1b(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g1b).map(i32::from)
    }
    fn g2(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }
    fn g2s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2s).map(i32::from)
    }
    fn g4s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g4s)
    }
    fn list(&mut self) -> anyhow::Result<Vec<i32>> {
        let n = self.g1()?;
        (0..n).map(|_| self.g2()).collect()
    }
}

/// An emitter type: the fields decoded from the emitter config file, with
/// the defaults the client starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitterType {
    /// Spawn yaw range (`<< 3`, shorts).
    pub yaw_min: i16,
    pub yaw_max: i16,
    /// Spawn pitch range (`<< 3`, shorts).
    pub pitch_min: i16,
    pub pitch_max: i16,
    /// Initial speed range.
    pub speed_min: i32,
    pub speed_max: i32,
    /// Speed damping mode (0 none, 1 distance, 2 distance²).
    pub damping_mode: i32,
    /// Damping factor (signed byte).
    pub damping: i32,
    /// Target speed and % of lifetime to reach it.
    pub target_speed: i32,
    pub target_speed_percent: i32,
    /// Initial size range (`<< 14`).
    pub size_min: i32,
    pub size_max: i32,
    /// Target size and % of lifetime to reach it.
    pub target_size: i32,
    pub target_size_percent: i32,
    /// Spawn ARGB range.
    pub colour_min: i32,
    pub colour_max: i32,
    /// One shared random for RGB (opcode 24 clears).
    pub uniform_colour: bool,
    /// Target ARGB (0 = none).
    pub target_colour: i32,
    /// % of lifetime for the RGB / alpha fade.
    pub colour_percent: i32,
    pub alpha_percent: i32,
    /// Particle material, -1 for untextured.
    pub texture: i32,
    /// Lifetime range in cycles.
    pub lifetime_min: i32,
    pub lifetime_max: i32,
    /// Spawn accumulator rate (64 = one per cycle).
    pub rate_min: i32,
    pub rate_max: i32,
    /// Local (same-system) effector type ids.
    pub local_effectors: Option<Vec<i32>>,
    /// Global (type 1) effector type ids.
    pub global_effectors: Option<Vec<i32>>,
    /// Constant (type 2) effector type ids.
    pub constant_effectors: Option<Vec<i32>>,
    /// Upper/lower collision planes (-2 = none).
    pub ceiling_level: i32,
    pub floor_level: i32,
    /// Initial burst emit calls on the first update.
    pub initial_burst: i32,
    /// Active before (`true`) or after the period threshold.
    pub active_before_threshold: bool,
    /// Period threshold.
    pub period_threshold: i32,
    /// Activation period (-1 = always).
    pub period: i32,
    /// The period repeats.
    pub period_repeat: bool,
    /// Minimum particle detail setting.
    pub min_setting: i32,
    /// Substitute type on renderers without texture support.
    pub low_detail_type: i32,
    /// Opcode 26 flag passed to `MovingParticle` and unused there.
    pub unused_flag: bool,
    /// Angular speed range (`* 8`).
    pub spin_min: i32,
    pub spin_max: i32,
    /// Initial rotation range and steps.
    pub angle_min: i32,
    pub angle_max: i32,
    pub angle_steps: i32,
    /// Keep the material on renderers without texture support.
    pub keep_texture: bool,
    /// Sun-ambient lit batches.
    pub lit: bool,
    /// Particles die inside tile entity bounds.
    pub entity_collision: bool,
    /// Particles die below level-0 ground.
    pub ground_collision: bool,
    /// The carrier model face is not drawn.
    pub remove_face: bool,
    // -- fields derived after decoding --
    /// Collides with the level's tiles.
    pub level_collision: bool,
    /// Minimum R/G/B/A.
    pub red: i32,
    pub green: i32,
    pub blue: i32,
    pub alpha: i32,
    /// R/G/B/A range.
    pub red_range: i32,
    pub green_range: i32,
    pub blue_range: i32,
    pub alpha_range: i32,
    /// Colour / alpha fade cycles.
    pub colour_cycles: i32,
    pub alpha_cycles: i32,
    /// Per-cycle R/G/B/A step (8.8).
    pub red_step: i32,
    pub green_step: i32,
    pub blue_step: i32,
    pub alpha_step: i32,
    /// Speed fade cycles and step.
    pub speed_cycles: i32,
    pub speed_step: i32,
    /// Size fade cycles and step.
    pub size_cycles: i32,
    pub size_step: i32,
    /// Rotation and spin ranges.
    pub angle_range: i32,
    pub spin_range: i32,
}

impl Default for EmitterType {
    /// The defaults, before the derived fields are computed.
    fn default() -> Self {
        Self {
            yaw_min: 0,
            yaw_max: 0,
            pitch_min: 0,
            pitch_max: 0,
            speed_min: 0,
            speed_max: 0,
            damping_mode: 0,
            damping: 0,
            target_speed: -1,
            target_speed_percent: 100,
            size_min: 0,
            size_max: 0,
            target_size: -1,
            target_size_percent: 100,
            colour_min: 0,
            colour_max: 0,
            uniform_colour: true,
            target_colour: 0,
            colour_percent: 100,
            alpha_percent: 100,
            texture: -1,
            lifetime_min: 0,
            lifetime_max: 0,
            rate_min: 0,
            rate_max: 0,
            local_effectors: None,
            global_effectors: None,
            constant_effectors: None,
            ceiling_level: -2,
            floor_level: -2,
            initial_burst: 0,
            active_before_threshold: true,
            period_threshold: -1,
            period: -1,
            period_repeat: true,
            min_setting: 0,
            low_detail_type: -1,
            unused_flag: true,
            spin_min: 0,
            spin_max: 0,
            angle_min: 0,
            angle_max: 0,
            angle_steps: 0,
            keep_texture: false,
            lit: true,
            entity_collision: false,
            ground_collision: true,
            remove_face: false,
            level_collision: false,
            red: 0,
            green: 0,
            blue: 0,
            alpha: 0,
            red_range: 0,
            green_range: 0,
            blue_range: 0,
            alpha_range: 0,
            colour_cycles: 0,
            alpha_cycles: 0,
            red_step: 0,
            green_step: 0,
            blue_step: 0,
            alpha_step: 0,
            speed_cycles: 0,
            speed_step: 0,
            size_cycles: 0,
            size_step: 0,
            angle_range: 0,
            spin_range: 0,
        }
    }
}

impl EmitterType {
    /// Decodes an emitter type and computes its derived fields.
    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        let mut r = Reader { data, pos: 0 };
        let mut t = Self::default();
        loop {
            let op = r.g1()?;
            if op == 0 {
                break;
            }
            t.decode_opcode(&mut r, op)?;
        }
        t.finish();
        Ok(t)
    }

    /// A missing file: the defaults, then the derived fields
    #[must_use]
    pub fn missing() -> Self {
        let mut t = Self::default();
        t.finish();
        t
    }

    /// Reads the opcodes. Unknown opcodes consume nothing.
    fn decode_opcode(&mut self, r: &mut Reader<'_>, op: i32) -> anyhow::Result<()> {
        let shl3 = |v: i32| ((v as i16 as i32) << 3) as i16;
        match op {
            1 => {
                self.yaw_min = shl3(r.g2()?);
                self.yaw_max = shl3(r.g2()?);
                self.pitch_min = shl3(r.g2()?);
                self.pitch_max = shl3(r.g2()?);
            }
            2 => {
                r.g1()?;
            }
            3 => {
                self.speed_min = r.g4s()?;
                self.speed_max = r.g4s()?;
            }
            4 => {
                self.damping_mode = r.g1()?;
                self.damping = r.g1b()?;
            }
            5 => {
                self.size_min = r.g2()? << 14;
                self.size_max = self.size_min;
            }
            6 => {
                self.colour_min = r.g4s()?;
                self.colour_max = r.g4s()?;
            }
            7 => {
                self.lifetime_min = r.g2()?;
                self.lifetime_max = r.g2()?;
            }
            8 => {
                self.rate_min = r.g2()?;
                self.rate_max = r.g2()?;
            }
            9 => self.local_effectors = Some(r.list()?),
            10 => self.constant_effectors = Some(r.list()?),
            12 => self.ceiling_level = r.g1b()?,
            13 => self.floor_level = r.g1b()?,
            14 => self.initial_burst = r.g2()?,
            15 => self.texture = r.g2()?,
            16 => {
                self.active_before_threshold = r.g1()? == 1;
                self.period_threshold = r.g2()?;
                self.period = r.g2()?;
                self.period_repeat = r.g1()? == 1;
            }
            17 => self.low_detail_type = r.g2()?,
            18 => self.target_colour = r.g4s()?,
            19 => self.min_setting = r.g1()?,
            20 => self.colour_percent = r.g1()?,
            21 => self.alpha_percent = r.g1()?,
            22 => self.target_speed = r.g4s()?,
            23 => self.target_speed_percent = r.g1()?,
            24 => self.uniform_colour = false,
            25 => self.global_effectors = Some(r.list()?),
            26 => self.unused_flag = false,
            27 => self.target_size = r.g2()? << 14,
            28 => self.target_size_percent = r.g1()?,
            29 => {
                if r.g1()? == 0 {
                    let v = r.g2s()?;
                    self.spin_max = v * 8;
                    self.spin_min = v * 8;
                } else {
                    self.spin_min = r.g2s()? * 8;
                    self.spin_max = r.g2s()? * 8;
                }
            }
            30 => self.keep_texture = true,
            31 => {
                self.size_min = r.g2()? << 14;
                self.size_max = r.g2()? << 14;
            }
            32 => self.lit = false,
            33 => self.entity_collision = true,
            34 => self.ground_collision = false,
            35 => {
                if r.g1()? == 0 {
                    let v = r.g2s()?;
                    self.angle_max = v * 8;
                    self.angle_min = v * 8;
                } else {
                    self.angle_min = r.g2s()? * 8;
                    self.angle_max = r.g2s()? * 8;
                    self.angle_steps = r.g1()?;
                }
            }
            36 => self.remove_face = true,
            _ => {}
        }
        Ok(())
    }

    /// Computes the derived fields.
    fn finish(&mut self) {
        if self.ceiling_level > -2 || self.floor_level > -2 {
            self.level_collision = true;
        }
        let (a, b) = (self.colour_min, self.colour_max);
        self.red = a >> 16 & 0xFF;
        self.red_range = (b >> 16 & 0xFF) - self.red;
        self.green = a >> 8 & 0xFF;
        self.green_range = (b >> 8 & 0xFF) - self.green;
        self.blue = a & 0xFF;
        self.blue_range = (b & 0xFF) - self.blue;
        self.alpha = a >> 24 & 0xFF;
        self.alpha_range = (b >> 24 & 0xFF) - self.alpha;
        if self.target_colour != 0 {
            self.colour_cycles = self.lifetime_max * self.colour_percent / 100;
            self.alpha_cycles = self.lifetime_max * self.alpha_percent / 100;
            if self.colour_cycles == 0 {
                self.colour_cycles = 1;
            }
            let t = self.target_colour;
            self.red_step =
                (((t >> 16 & 0xFF) - (self.red_range / 2 + self.red)) << 8) / self.colour_cycles;
            self.green_step =
                (((t >> 8 & 0xFF) - (self.green_range / 2 + self.green)) << 8) / self.colour_cycles;
            self.blue_step =
                (((t & 0xFF) - (self.blue_range / 2 + self.blue)) << 8) / self.colour_cycles;
            if self.alpha_cycles == 0 {
                self.alpha_cycles = 1;
            }
            self.alpha_step =
                (((t >> 24 & 0xFF) - (self.alpha_range / 2 + self.alpha)) << 8) / self.alpha_cycles;
            for step in [
                &mut self.red_step,
                &mut self.green_step,
                &mut self.blue_step,
                &mut self.alpha_step,
            ] {
                *step += if *step > 0 { -4 } else { 4 };
            }
        }
        if self.target_speed != -1 {
            self.speed_cycles = self.lifetime_max * self.target_speed_percent / 100;
            if self.speed_cycles == 0 {
                self.speed_cycles = 1;
            }
            self.speed_step = (self.target_speed
                - (self.speed_max.wrapping_sub(self.speed_min) / 2 + self.speed_min))
                / self.speed_cycles;
        }
        if self.target_size != -1 {
            self.size_cycles = self.target_size_percent * self.lifetime_max / 100;
            if self.size_cycles == 0 {
                self.size_cycles = 1;
            }
            self.size_step = (self.target_size
                - ((self.size_max - self.size_min) / 2 + self.size_min))
                / self.size_cycles;
        }
        self.angle_range = self.angle_max - self.angle_min;
        self.spin_range = self.spin_max - self.spin_min;
    }
}

/// An effector type definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectorType {
    /// Type id.
    pub id: i32,
    /// 0 local, 1 global, 2 constant.
    pub kind: i32,
    /// Cone half-angle (`<< 3` into the cosine table).
    pub cone: i32,
    /// Force vector.
    pub force: [i32; 3],
    /// Falloff mode (0 none, 1 linear, 2 quadratic).
    pub falloff_mode: i32,
    /// Falloff factor.
    pub falloff: i32,
    /// Opcode 8 applies the force to the position.
    pub positional: i32,
    /// Opcode 9 radial force.
    pub radial: i32,
    /// Opcode 10 inverts the magnitude.
    pub invert: bool,
    /// Force magnitude (negated when inverted).
    pub magnitude: i32,
    /// Squared/linear range limit.
    pub range: i64,
    /// The cosine table entry at `cone << 3`.
    pub cone_cos: i32,
}

impl EffectorType {
    /// Decodes an effector type.
    pub fn decode(id: i32, data: Option<&[u8]>) -> anyhow::Result<Self> {
        let mut t = Self {
            id,
            ..Self::default()
        };
        if let Some(data) = data {
            let mut r = Reader { data, pos: 0 };
            loop {
                match r.g1()? {
                    0 => break,
                    1 => t.cone = r.g2()?,
                    2 => {
                        r.g1()?;
                    }
                    3 => t.force = [r.g4s()?, r.g4s()?, r.g4s()?],
                    4 => {
                        t.falloff_mode = r.g1()?;
                        t.falloff = r.g4s()?;
                    }
                    6 => t.kind = r.g1()?,
                    8 => t.positional = 1,
                    9 => t.radial = 1,
                    10 => t.invert = true,
                    _ => {}
                }
            }
        }
        // The reference indexes the cosine table at `cone << 3` unmasked; cone
        // values above 2047 would be out of range there. The ordinary table mask keeps them finite.
        t.cone_cos = crate::trig::cos(t.cone << 3);
        let [x, y, z] = t.force.map(i64::from);
        t.magnitude = ((z * z + x * x + y * y) as f64).sqrt() as i32;
        if t.falloff == 0 {
            t.falloff = 1;
        }
        match t.falloff_mode {
            0 => t.range = 2_147_483_647,
            1 => {
                t.range = i64::from(t.magnitude.wrapping_mul(8) / t.falloff);
                t.range *= t.range;
            }
            2 => t.range = i64::from(t.magnitude.wrapping_mul(8) / t.falloff),
            _ => {}
        }
        if t.invert {
            t.magnitude = -t.magnitude;
        }
        Ok(t)
    }
}

/// All emitter types by id; missing ids read as the finished default type
/// as the emitter type list does.
#[derive(Clone, Debug)]
pub struct EmitterStore {
    entries: BTreeMap<u32, Arc<EmitterType>>,
    missing: Arc<EmitterType>,
}

impl Default for EmitterStore {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            missing: Arc::new(EmitterType::missing()),
        }
    }
}

impl EmitterStore {
    /// Load group 0 of the `particles` archive.
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        rs910_core::profile::scope!("load emitters");
        let files = pack.read_group(PARTICLE_ARCHIVE, 0)?;
        let mut entries = BTreeMap::new();
        for (id, bytes) in files {
            entries.insert(
                id,
                Arc::new(
                    EmitterType::decode(&bytes)
                        .map_err(|e| anyhow::anyhow!("emitter {id}: {e:#}"))?,
                ),
            );
        }
        Ok(Self {
            entries,
            ..Self::default()
        })
    }

    /// `get(id)`.
    #[must_use]
    pub fn get(&self, id: i32) -> &EmitterType {
        self.shared(id).as_ref()
    }

    /// `get(id)` as the shared handle a live emitter retains.
    #[must_use]
    pub fn shared(&self, id: i32) -> &Arc<EmitterType> {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.entries.get(&id))
            .unwrap_or(&self.missing)
    }

    /// Decoded types in id order.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = (u32, &EmitterType)> {
        self.entries.iter().map(|(id, t)| (*id, t.as_ref()))
    }

    /// Number of decoded types.
    #[must_use]
    #[cfg(test)] // test-only helper
    #[allow(
        clippy::len_without_is_empty,
        reason = "a test-only type count; nothing asks whether the store is empty"
    )]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Replace one type (the golden tests' spawn-rate override,
    /// `client910/src/model_goldens.rs`).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn insert(&mut self, id: u32, t: Arc<EmitterType>) {
        self.entries.insert(id, t);
    }
}

/// Effector types by id; group 1.
#[derive(Clone, Debug, Default)]
pub struct EffectorStore {
    entries: BTreeMap<u32, Arc<EffectorType>>,
    missing: HashMap<i32, Arc<EffectorType>>,
}

impl EffectorStore {
    /// Load group 1 of the `particles` archive. A pack without the group
    /// has no effectors (every lookup is the finished default type).
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        let mut entries = BTreeMap::new();
        if let Ok(files) = pack.read_group(PARTICLE_ARCHIVE, 1) {
            for (id, bytes) in files {
                entries.insert(
                    id,
                    Arc::new(
                        EffectorType::decode(id as i32, Some(&bytes))
                            .map_err(|e| anyhow::anyhow!("effector {id}: {e:#}"))?,
                    ),
                );
            }
        }
        Ok(Self {
            entries,
            missing: HashMap::new(),
        })
    }

    /// The type `id`, loaded on first use.
    pub fn get(&mut self, id: i32) -> Arc<EffectorType> {
        if let Some(t) = u32::try_from(id).ok().and_then(|id| self.entries.get(&id)) {
            return t.clone();
        }
        self.missing
            .entry(id)
            .or_insert_with(|| Arc::new(EffectorType::decode(id, None).expect("default effector")))
            .clone()
    }
}

// -- model anchors ----------------------------------------------------------

/// What one effector does to a particle this step: the effector's facing
/// (x and z), the offset from the effector to the particle, its length and the
/// time step.
#[derive(Clone, Copy)]
struct EffectorPush {
    direction: [f32; 2],
    delta: [f64; 3],
    dist: f64,
    step: f64,
}

/// The rotation part of a model's 4x4 matrix in entry order
/// `[e0, e1, e2, e4, e5, e6, e8, e9, e10]`, used to turn the effector force
/// into the owner's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rotation(pub [f32; 9]);

impl Default for Rotation {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Rotation {
    pub const IDENTITY: Self = Self([1., 0., 0., 0., 1., 0., 0., 0., 1.]);

    /// Rotates `(x, y, z)` without translation.
    #[must_use]
    pub fn apply(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let e = &self.0;
        [
            e[6] * z + e[0] * x + e[3] * y,
            e[7] * z + e[1] * x + e[4] * y,
            e[8] * z + e[2] * x + e[5] * y,
        ]
    }

    /// The Y rotation `GpuModel::rotate_y_keep_normals`/`rotate_y` bake into vertices.
    #[must_use]
    pub fn yaw(angle: i32) -> Self {
        let s = crate::trig::sin(angle) as f32 / 16384.0;
        let c = crate::trig::cos(angle) as f32 / 16384.0;
        Self([c, 0., -s, 0., 1., 0., s, 0., c])
    }

    /// The X rotation `GpuModel::rotate_x` bakes into vertices.
    #[must_use]
    pub fn pitch(angle: i32) -> Self {
        let s = crate::trig::sin(angle) as f32 / 16384.0;
        let c = crate::trig::cos(angle) as f32 / 16384.0;
        Self([1., 0., 0., 0., c, s, 0., -s, c])
    }

    /// `next` applied after `self`.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        let cols = [
            next.apply(self.0[0], self.0[1], self.0[2]),
            next.apply(self.0[3], self.0[4], self.0[5]),
            next.apply(self.0[6], self.0[7], self.0[8]),
        ];
        Self([
            cols[0][0], cols[0][1], cols[0][2], cols[1][0], cols[1][1], cols[1][2], cols[2][0],
            cols[2][1], cols[2][2],
        ])
    }

    /// The rotation of an actor `Matrix4x3` (`actor_matrix::Matrix`).
    #[must_use]
    pub fn from_matrix(m: &crate::actor_matrix::Matrix) -> Self {
        Self([
            m.0[0], m.0[1], m.0[2], m.0[3], m.0[4], m.0[5], m.0[6], m.0[7], m.0[8],
        ])
    }
}

/// A translation-only owner matrix (dynamic locs and owners whose rotation is
/// baked into the model vertices).
#[must_use]
pub fn translation(position: [f32; 3]) -> crate::actor_matrix::Matrix {
    let mut m = crate::actor_matrix::Matrix::default();
    m.0[9..12].copy_from_slice(&position);
    m
}

/// An emitter anchor after the model's anchor pass: its identity within
/// the owning system and the carrier triangle in scene-local fine units
///.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmitterAnchor {
    pub id: u64,
    pub particle: i32,
    pub vertices: [[i32; 3]; 3],
}

/// An effector anchor after the model's anchor pass: identity, type,
/// vertex in scene-local fine units and the model
/// rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectorAnchor {
    pub id: u64,
    pub effector: i32,
    pub position: [i32; 3],
    pub rotation: Rotation,
}

/// Owner keys for the particle system references entities keep (actors,
/// dynamic locs, projectiles, spot animations) and the
/// emitter id namespaces of the models one owner binds together.
pub mod keys {
    /// Body model ids (`idk[0]`).
    pub const BODY: u64 = 0;
    /// Spot-animation model ids of an actor (`idk[1 + slot]`).
    #[must_use]
    pub fn spot(slot: usize) -> u64 {
        (slot as u64 + 1) << 40
    }
    #[must_use]
    pub fn player(index: usize) -> u64 {
        4 << 56 | index as u64
    }
    #[must_use]
    pub fn npc(index: usize) -> u64 {
        5 << 56 | index as u64
    }
    #[must_use]
    pub fn projectile(index: usize) -> u64 {
        6 << 56 | index as u64
    }
    #[must_use]
    pub fn map_spot(index: usize) -> u64 {
        9 << 56 | index as u64
    }
    /// `kind`: 0 scenery, 1 wall, 2 wall decoration, 3 ground decoration.
    #[must_use]
    pub fn loc(kind: u64, index: usize) -> u64 {
        (10 + kind) << 56 | index as u64
    }
    /// `Component.particleSystem` of the interface component with this
    /// construction serial.
    #[must_use]
    pub fn component(serial: u64) -> u64 {
        15 << 56 | (serial & ((1 << 56) - 1))
    }
    /// The dynamic loc of a zone loc change that
    /// is drawn as its own scene entity, by level, layer and tile.
    #[must_use]
    pub fn location(level: i32, layer: i32, x: i32, z: i32) -> u64 {
        14 << 56
            | (level as u64 & 3) << 40
            | (layer as u64 & 3) << 32
            | (x as u64 & 0xFFFF) << 16
            | (z as u64 & 0xFFFF)
    }
}

/// One owner's anchors for this frame: the models an entity draws
/// (an actor's body and spot models merge into one system) with the owner's
/// level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Binding {
    pub key: u64,
    pub level: i32,
    pub emitters: Vec<EmitterAnchor>,
    pub effectors: Vec<EffectorAnchor>,
}

impl Binding {
    /// Model anchors for one of the owner's models.
    pub fn add_model(
        &mut self,
        model: &crate::gpumodel::GpuModel,
        matrix: &crate::actor_matrix::Matrix,
        rotation: Rotation,
        id_base: u64,
    ) {
        let (emitters, effectors) = model.particle_anchors(matrix, rotation, id_base);
        self.emitters.extend(emitters);
        self.effectors.extend(effectors);
    }

    /// Append another model set of the same owner.
    pub fn merge(&mut self, other: &Self) {
        self.emitters.extend_from_slice(&other.emitters);
        self.effectors.extend_from_slice(&other.effectors);
    }
}

// -- runtime ------------------------------------------------------------------

/// The emitter
/// triangle and the centre it last spawned from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Triangle {
    v: [[i32; 3]; 3],
    /// The triangle's centre.
    centre: [i32; 3],
}

/// `MovingParticle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MovingParticle {
    /// `x` / `y` / `z`: 20.12 fixed scene-local fine units.
    pos: [i32; 3],
    /// ARGB.
    colour: i32,
    /// The low 8 bits of each 8.8 colour channel.
    colour_frac: i32,
    /// Lifetime and remaining cycles.
    lifetime: i16,
    remaining: i16,
    /// Half size (`<< 12`).
    size: i32,
    /// Rotation and angular speed.
    angle: i16,
    spin: i16,
    /// Material (-1 untextured).
    texture: i32,
    /// Lit by the sun.
    lit: bool,
    /// Direction.
    dir: [i16; 3],
    /// Speed.
    speed: i32,
    /// Slot in the system's particle ring.
    slot: u16,
    /// Identity of the slot occupant (replaces an object reference).
    serial: u32,
}

/// One particle in a system's draw list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawParticle {
    /// 20.12 fixed scene-local fine units.
    pub pos: [i32; 3],
    pub colour: i32,
    pub size: i32,
    pub angle: i16,
    pub texture: i32,
    pub lit: bool,
}

/// A live emitter.
#[derive(Clone, Debug)]
struct Emitter {
    /// Identity and the current model triangle.
    id: u64,
    model: [[i32; 3]; 3],
    /// The emitter type.
    ty: Arc<EmitterType>,
    /// The spawn accumulator.
    accumulator: i32,
    /// Live particles, oldest first.
    particles: Vec<MovingParticle>,
    /// Live particle count.
    count: i32,
    /// The cycle it was created.
    created: i64,
    /// Stopped spawning; waiting for its particles to die.
    dying: bool,
    /// The triangle now and the previous update's.
    current: Triangle,
    previous: Triangle,
    /// The triangle normal.
    normal: [i32; 3],
    /// Yaw base and the spawn frame data.
    yaw_base: i32,
    yaw_range: i32,
    pitch_base: i32,
    pitch_range: i32,
    /// All three vertices coincide.
    degenerate: bool,
    /// The normal is valid.
    normal_valid: bool,
    /// Constant effector registry slots.
    constants: Vec<i32>,
}

/// A live effector.
#[derive(Clone, Debug)]
struct Effector {
    id: u64,
    ty: Arc<EffectorType>,
    /// Position.
    pos: [i32; 3],
    /// Rotated force X / Z.
    dir_x: f32,
    dir_z: f32,
    /// Linked into the global effector list.
    global: bool,
}

impl Effector {
    /// A new effector from its anchor.
    fn new(anchor: &EffectorAnchor, ty: Arc<EffectorType>, global: bool) -> Self {
        let mut e = Self {
            id: anchor.id,
            ty,
            pos: [0; 3],
            dir_x: 0.,
            dir_z: 0.,
            global,
        };
        e.refresh(anchor);
        e
    }

    /// Refreshes the position and direction from the anchor.
    fn refresh(&mut self, anchor: &EffectorAnchor) {
        self.pos = anchor.position;
        let [x, y, z] = self.ty.force.map(|v| v as f32);
        let d = anchor.rotation.apply(x, y, z);
        self.dir_x = d[0];
        self.dir_z = d[2];
    }
}

/// A live particle system.
#[derive(Clone, Debug)]
struct System {
    /// Killed. Nothing clears it, so a dead system taken back out of the
    /// recycle ring stays dead and is never bound again.
    dead: bool,
    /// Spawning suspended this update.
    no_spawn: bool,
    /// Occupant serials of the particle ring; allocated on the first spawn.
    slots: Vec<u32>,
    /// The colour fraction of each occupant, kept current so an eviction or a
    /// kill can hand it to the particle pool.
    fracs: Vec<i32>,
    /// The next ring slot.
    next_slot: usize,
    next_serial: u32,
    /// The cycles of the last update and the last bind.
    last_update: i64,
    last_bind: i64,
    /// The emitters.
    emitters: Vec<Emitter>,
    /// The effectors.
    effectors: Vec<Effector>,
    /// No update has run yet.
    first_update: bool,
    /// Owned by the world scene.
    scene: bool,
    /// The owner's level.
    level: i32,
    /// The draw list.
    draw: Vec<DrawParticle>,
}

impl System {
    /// A new system created at `cycle`.
    fn new(cycle: i64, scene: bool) -> Self {
        Self {
            dead: false,
            no_spawn: false,
            slots: Vec::new(),
            fracs: Vec::new(),
            next_slot: 0,
            next_serial: 1,
            last_update: cycle,
            last_bind: cycle,
            emitters: Vec::new(),
            effectors: Vec::new(),
            first_update: true,
            scene,
            level: 0,
            draw: Vec::new(),
        }
    }

    fn live(&self, p: &MovingParticle) -> bool {
        self.slots.get(p.slot as usize) == Some(&p.serial)
    }

    /// Releases the particle's slot and hands the object to the released
    /// ring. The caller drops it from the emitter queue.
    fn release(slots: &mut [u32], pool: &mut ParticlePool, p: &MovingParticle) {
        if let Some(s) = slots.get_mut(p.slot as usize) {
            if *s == p.serial {
                *s = 0;
                pool.release(p.colour_frac);
            }
        }
    }

    /// Mark the system dead,
    /// release every slot occupant in slot order and drop the emitters and
    /// effectors. Global effectors leave the table with their system but
    /// the global effector count is not decremented.
    fn kill(&mut self, pool: &mut ParticlePool) {
        self.dead = true;
        for (slot, serial) in self.slots.iter_mut().enumerate() {
            if *serial != 0 {
                pool.release(self.fracs[slot]);
                *serial = 0;
            }
        }
        self.next_slot = 0;
        self.emitters.clear();
        self.effectors.clear();
        self.draw.clear();
    }

    fn particle_count(&self) -> usize {
        self.emitters
            .iter()
            .map(|e| e.particles.iter().filter(|p| self.live(p)).count())
            .sum()
    }
}

/// The ring of released particle objects. An emitter reuses one, which resets
/// every field except the colour fraction bytes, so a recycled particle starts from the
/// fraction its previous life ended with. Only that field is kept here.
struct ParticlePool {
    fracs: Box<[i32; PARTICLE_POOL]>,
    /// Read index.
    read: usize,
    /// Write index.
    write: usize,
}

impl ParticlePool {
    fn new() -> Self {
        Self {
            fracs: Box::new([0; PARTICLE_POOL]),
            read: 0,
            write: 0,
        }
    }

    /// Releases a particle into the ring. The write index advances without
    /// checking the read index.
    fn release(&mut self, frac: i32) {
        self.fracs[self.write] = frac;
        self.write = (self.write + 1) & (PARTICLE_POOL - 1);
    }

    /// A new particle (colour fraction zero) when
    /// the ring is empty, otherwise the next released one's fraction.
    fn take(&mut self) -> i32 {
        if self.read == self.write {
            return 0;
        }
        let frac = self.fracs[self.read];
        self.read = (self.read + 1) & (PARTICLE_POOL - 1);
        frac
    }
}

/// A type-1 (global) effector as seen through the global list.
#[derive(Clone, Debug)]
struct GlobalEffector {
    pos: [i32; 3],
    dir_x: f32,
    dir_z: f32,
    ty: Arc<EffectorType>,
}

/// Scene data the particle kill test consults (height maps, level tiles and
/// entity bounds). Coordinates are scene-local.
pub trait ParticleScene {
    /// `Scene.size` (`log2` of the fine tile size, 9).
    fn size(&self) -> i32;
    /// `Scene.maxTileX` / `maxTileZ`.
    fn max_tile_x(&self) -> i32;
    fn max_tile_z(&self) -> i32;
    /// `Scene.maxLevel`.
    fn max_level(&self) -> i32;
    /// `levelHeightmaps[level].getTileHeight(x, z)`.
    fn tile_height(&self, level: i32, x: i32, z: i32) -> i32;
    /// `levelTiles[level][x][z].level` when the tile exists.
    fn tile_level(&self, level: i32, x: i32, z: i32) -> Option<i32>;
    /// `levelTiles[0][x][z].bridge != null`.
    fn bridged(&self, x: i32, z: i32) -> bool;
    /// `levelTiles[plane][x][z] = new Tile(level)`.
    fn create_tile(&mut self, plane: i32, x: i32, z: i32, level: i32);
    /// Whether `levelTiles[plane][x][z]` holds
    /// a wall, dynamic wall, ground decoration or primary entity whose
    /// bounds contain the point.
    fn bounds_contain(&self, plane: i32, x: i32, z: i32, point: [i32; 3]) -> bool;
}

/// Per-cycle counters for diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub systems: usize,
    pub emitters: usize,
    pub effectors: usize,
    /// Particles counted in the last update.
    pub counted: i32,
    /// Live particles.
    pub live: usize,
    /// Particles in this frame's draw lists.
    pub drawn: usize,
}

/// The global particle owner together with the owner-keyed system references
/// entities keep.
pub struct Runtime {
    emitter_types: EmitterStore,
    effector_types: EffectorStore,
    /// Systems in creation order with the owner that holds each
    /// ([`UNOWNED`] once its owner replaced it). Dead systems stay listed
    /// until they time out.
    systems: Vec<(u64, System)>,
    /// Owner key -> the system it holds (live or dead).
    index: HashMap<u64, usize>,
    /// The particle detail setting.
    detail: i32,
    /// Recycle ring indices (read and write). The
    /// pooled objects are all dead systems, so only the indices matter.
    pool_read: usize,
    pool_write: usize,
    /// Released particles.
    particles: ParticlePool,
    /// Particle totals of the previous and the current update.
    previous_total: i32,
    current_total: i32,
    /// The registry of constant effector types.
    constant_index: HashMap<i32, i32>,
    constant_types: Vec<Arc<EffectorType>>,
    /// Global effector count.
    global_count: i32,
    rng: AnimationRandom,
}

impl Runtime {
    /// A runtime over the pack's emitter and effector types.
    #[must_use]
    pub fn new(emitter_types: EmitterStore, effector_types: EffectorStore, seed: u64) -> Self {
        Self {
            emitter_types,
            effector_types,
            systems: Vec::new(),
            index: HashMap::new(),
            detail: 2,
            pool_read: 0,
            pool_write: 0,
            particles: ParticlePool::new(),
            previous_total: 0,
            current_total: 0,
            constant_index: HashMap::new(),
            constant_types: Vec::new(),
            global_count: 0,
            rng: AnimationRandom::new(seed),
        }
    }

    /// The particle detail preference consumer. The setter resizes (and so empties) the recycled-system pool; the level
    /// gates spawning through [`PARTICLE_LIMITS`]. The caller
    /// applies the preference every frame, so only a change is a setter call.
    pub fn set_detail(&mut self, level: i32) {
        let level = if (0..=2).contains(&level) { level } else { 0 };
        if level != self.detail {
            self.detail = level;
            self.pool_read = 0;
            self.pool_write = 0;
        }
    }

    /// The live particle count of the last update.
    #[must_use]
    pub fn detail(&self) -> i32 {
        self.detail
    }

    fn pool_mask(&self) -> usize {
        SYSTEM_POOL_MASKS[self.detail as usize]
    }

    /// The global effector table is
    /// replaced and every system killed. The dead systems stay
    /// in the system list and with their owners until they time out; an owner
    /// that draws again replaces its dead system.
    pub fn reset(&mut self) {
        self.global_count = 0;
        for (_, system) in &mut self.systems {
            system.kill(&mut self.particles);
        }
    }

    /// Advances every system to `cycle` (the client loop cycle); the ones that
    /// timed out are killed and pushed onto the recycle ring.
    pub fn tick(&mut self, cycle: i64) {
        self.previous_total = self.current_total;
        self.current_total = 0;
        let globals: Vec<(i32, GlobalEffector)> = self
            .systems
            .iter()
            .flat_map(|(_, s)| s.effectors.iter())
            .filter(|e| e.global)
            .map(|e| {
                (
                    e.ty.id,
                    GlobalEffector {
                        pos: e.pos,
                        dir_x: e.dir_x,
                        dir_z: e.dir_z,
                        ty: e.ty.clone(),
                    },
                )
            })
            .collect();
        let mut ctx = Tick {
            detail: self.detail,
            previous_total: self.previous_total,
            current_total: 0,
            globals: &globals,
            constants: &self.constant_types,
            rng: &mut self.rng,
            pool: &mut self.particles,
        };
        let mask = SYSTEM_POOL_MASKS[self.detail as usize];
        let mut pool_write = self.pool_write;
        let mut removed = false;
        self.systems.retain_mut(|(_, system)| {
            let alive = system.update(cycle, &mut ctx);
            if !alive {
                system.kill(ctx.pool);
                pool_write = (pool_write + 1) & mask;
                removed = true;
            }
            alive
        });
        self.pool_write = pool_write;
        self.current_total = ctx.current_total;
        if removed {
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        self.index = self
            .systems
            .iter()
            .enumerate()
            .filter(|(_, (k, _))| *k != UNOWNED)
            .map(|(i, (k, _))| (*k, i))
            .collect();
    }

    /// A new system, or the next dead one from the recycle ring (still dead:
    /// creating one does not clear the flag).
    fn create(&mut self, key: u64, cycle: i64, scene: bool) -> usize {
        let mut system = System::new(cycle, scene);
        if self.pool_read != self.pool_write {
            self.pool_read = (self.pool_read + 1) & self.pool_mask();
            system.dead = true;
        }
        if let Some(&old) = self.index.get(&key) {
            self.systems[old].0 = UNOWNED;
        }
        self.systems.push((key, system));
        let i = self.systems.len() - 1;
        self.index.insert(key, i);
        i
    }

    /// An owner draws its models (an actor, a dynamic loc, a spot animation or
    /// a projectile): a new system when it holds none or a dead one and the
    /// models carry emitters or effectors, then the emitter and effector bind
    /// (a no-op on a dead system) and the level update.
    pub fn bind(
        &mut self,
        key: u64,
        cycle: i64,
        level: i32,
        emitters: &[EmitterAnchor],
        effectors: &[EffectorAnchor],
    ) {
        self.bind_owned(key, cycle, level, emitters, effectors, true);
    }

    fn bind_owned(
        &mut self,
        key: u64,
        cycle: i64,
        level: i32,
        emitters: &[EmitterAnchor],
        effectors: &[EffectorAnchor],
        scene: bool,
    ) {
        let held = self.index.get(&key).copied();
        let index = match held {
            Some(i) if !self.systems[i].1.dead => i,
            _ if !emitters.is_empty() || !effectors.is_empty() => self.create(key, cycle, scene),
            Some(i) => i,
            None => return,
        };
        let system = &mut self.systems[index].1;
        system.level = level;
        if system.dead {
            return;
        }
        system.last_bind = cycle;
        self.bind_emitters(index, emitters);
        self.bind_effectors(index, effectors);
    }

    /// Restarts the system `key` holds: its emitters replay their bursts at
    /// the next update.
    pub fn restart(&mut self, key: u64) {
        if let Some(&i) = self.index.get(&key) {
            self.systems[i].1.first_update = true;
        }
    }

    /// The owner is culled but keeps whatever
    /// system it holds alive, dead or not (the cull has no dead test).
    pub fn touch(&mut self, key: u64, cycle: i64, level: i32) {
        if let Some(&i) = self.index.get(&key) {
            let system = &mut self.systems[i].1;
            system.last_bind = cycle;
            system.level = level;
        }
    }

    /// Binds the emitters of `anchors`.
    fn bind_emitters(&mut self, index: usize, anchors: &[EmitterAnchor]) {
        let mut matched = [false; MAX_SYSTEM_EMITTERS];
        let system = &mut self.systems[index].1;
        let created = system.last_bind;
        system.emitters.retain_mut(|emitter| {
            for (j, anchor) in anchors.iter().enumerate().take(MAX_SYSTEM_EMITTERS) {
                if anchor.id == emitter.id {
                    matched[j] = true;
                    emitter.model = anchor.vertices;
                    emitter.refresh();
                    emitter.dying = false;
                    return true;
                }
            }
            if emitter.count == 0 {
                false
            } else {
                emitter.dying = true;
                true
            }
        });
        for (j, anchor) in anchors.iter().enumerate().take(MAX_SYSTEM_EMITTERS) {
            if self.systems[index].1.emitters.len() == MAX_SYSTEM_EMITTERS {
                break;
            }
            if matched[j] {
                continue;
            }
            let ty = self.emitter_types.shared(anchor.particle).clone();
            let constants = ty
                .constant_effectors
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|&id| self.register_constant(id))
                .collect();
            let emitter = Emitter::new(anchor, ty, created, constants, &mut self.rng);
            self.systems[index].1.emitters.push(emitter);
            matched[j] = true;
        }
    }

    /// Binds the effectors of `anchors`.
    ///
    /// A removed effector leaves the list once; the second release frees its
    /// global table entry and the global count.
    fn bind_effectors(&mut self, index: usize, anchors: &[EffectorAnchor]) {
        let mut matched = [false; MAX_SYSTEM_EFFECTORS];
        let mut released = 0;
        let system = &mut self.systems[index].1;
        system.effectors.retain_mut(|effector| {
            for (j, anchor) in anchors.iter().enumerate().take(MAX_SYSTEM_EFFECTORS) {
                if anchor.id == effector.id {
                    matched[j] = true;
                    effector.refresh(anchor);
                    return true;
                }
            }
            if effector.global {
                released += 1;
            }
            false
        });
        self.global_count -= released;
        for (j, anchor) in anchors.iter().enumerate().take(MAX_SYSTEM_EFFECTORS) {
            if self.systems[index].1.effectors.len() == MAX_SYSTEM_EFFECTORS {
                break;
            }
            if matched[j] {
                continue;
            }
            let ty = self.effector_types.get(anchor.effector);
            let global = ty.kind == 1 && self.global_count < MAX_GLOBAL_EFFECTORS;
            if global {
                self.global_count += 1;
            }
            self.systems[index]
                .1
                .effectors
                .push(Effector::new(anchor, ty, global));
            matched[j] = true;
        }
    }

    /// The registry slot of a constant effector type. The table holds 16
    /// entries and a non-constant type returns -1 (which `MovingParticle`
    /// would then index); both are skipped here.
    fn register_constant(&mut self, id: i32) -> i32 {
        if let Some(&i) = self.constant_index.get(&id) {
            return i;
        }
        let ty = self.effector_types.get(id);
        if ty.kind != 2 || self.constant_types.len() >= 16 {
            return -1;
        }
        let i = self.constant_types.len() as i32;
        self.constant_types.push(ty);
        self.constant_index.insert(id, i);
        i
    }

    /// Rebuilds every scene system's draw list,
    /// killing particles that left the scene or collided.
    pub fn collect(&mut self, scene: &mut dyn ParticleScene) {
        for (_, system) in &mut self.systems {
            if system.scene {
                system.collect(scene, &mut self.particles);
            }
        }
    }

    /// Scene systems' draw lists with their owner keys.
    pub fn lists(&self) -> impl Iterator<Item = (u64, &[DrawParticle])> {
        self.systems
            .iter()
            .filter(|(k, s)| *k != UNOWNED && s.scene && !s.draw.is_empty())
            .map(|(k, s)| (*k, s.draw.as_slice()))
    }

    /// The draw list of the system `key` holds, empty when it
    /// holds none.
    #[must_use]
    pub fn list(&self, key: u64) -> &[DrawParticle] {
        self.index
            .get(&key)
            .map_or(&[], |&i| self.systems[i].1.draw.as_slice())
    }

    /// An interface model's system
    /// (created outside the scene pass)
    /// bound to the component's current model anchors.
    pub fn bind_interface(
        &mut self,
        key: u64,
        cycle: i64,
        emitters: &[EmitterAnchor],
        effectors: &[EffectorAnchor],
    ) {
        self.bind_owned(key, cycle, 0, emitters, effectors, false);
    }

    /// The live slot occupants in slot order, the list an interface model's
    /// particle draw takes (no scene collision pass).
    #[must_use]
    pub fn slot_list(&self, key: u64) -> Vec<DrawParticle> {
        let Some(system) = self.index.get(&key).map(|&i| &self.systems[i].1) else {
            return Vec::new();
        };
        let mut by_slot: Vec<(u16, DrawParticle)> = system
            .emitters
            .iter()
            .flat_map(|e| e.particles.iter())
            .filter(|p| system.live(p))
            .map(|p| {
                (
                    p.slot,
                    DrawParticle {
                        pos: p.pos,
                        colour: p.colour,
                        size: p.size,
                        angle: p.angle,
                        texture: p.texture,
                        lit: p.lit,
                    },
                )
            })
            .collect();
        by_slot.sort_by_key(|(slot, _)| *slot);
        by_slot.into_iter().map(|(_, p)| p).collect()
    }

    /// Diagnostic counters.
    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            systems: self.systems.len(),
            emitters: self.systems.iter().map(|(_, s)| s.emitters.len()).sum(),
            effectors: self.systems.iter().map(|(_, s)| s.effectors.len()).sum(),
            counted: self.current_total,
            live: self.systems.iter().map(|(_, s)| s.particle_count()).sum(),
            drawn: self.systems.iter().map(|(_, s)| s.draw.len()).sum(),
        }
    }

    /// Whether `key` currently holds a live system.
    #[cfg(test)]
    #[must_use]
    pub fn has_system(&self, key: u64) -> bool {
        self.index
            .get(&key)
            .is_some_and(|&i| !self.systems[i].1.dead)
    }
}

struct Tick<'a> {
    detail: i32,
    previous_total: i32,
    current_total: i32,
    globals: &'a [(i32, GlobalEffector)],
    constants: &'a [Arc<EffectorType>],
    rng: &'a mut AnimationRandom,
    pool: &'a mut ParticlePool,
}

impl System {
    /// Ages the system. Returns false when the system timed out.
    fn update(&mut self, cycle: i64, ctx: &mut Tick<'_>) -> bool {
        self.no_spawn = self.last_update != self.last_bind;
        if cycle - self.last_bind > SYSTEM_TIMEOUT {
            return false;
        }
        let elapsed = (cycle - self.last_update) as i32;
        let spawn = !self.no_spawn;
        let Self {
            emitters,
            effectors,
            slots,
            fracs,
            next_slot,
            next_serial,
            first_update,
            ..
        } = self;
        let mut state = SlotState {
            slots,
            fracs,
            next_slot,
            next_serial,
        };
        if *first_update {
            for emitter in emitters.iter_mut() {
                for _ in 0..emitter.ty.initial_burst {
                    emitter.update(cycle, 1, spawn, effectors, &mut state, ctx);
                }
            }
            *first_update = false;
        }
        for emitter in emitters.iter_mut() {
            emitter.update(cycle, elapsed, spawn, effectors, &mut state, ctx);
        }
        self.last_update = cycle;
        true
    }

    /// Rebuilds the draw list, killing particles that left the scene.
    fn collect(&mut self, scene: &mut dyn ParticleScene, pool: &mut ParticlePool) {
        self.draw.clear();
        let level = self.level;
        let Self {
            emitters,
            slots,
            draw,
            ..
        } = self;
        for emitter in emitters.iter_mut() {
            let ty = emitter.ty.clone();
            emitter.particles.retain(|p| {
                if slots.get(p.slot as usize) != Some(&p.serial) {
                    return false;
                }
                if collide(p, &ty, level, scene) {
                    System::release(slots, pool, p);
                    return false;
                }
                draw.push(DrawParticle {
                    pos: p.pos,
                    colour: p.colour,
                    size: p.size,
                    angle: p.angle,
                    texture: p.texture,
                    lit: p.lit,
                });
                true
            });
        }
    }
}

/// The particle kill conditions; true when the particle dies.
/// A particle that survives the height tests allocates the empty tiles the
/// client creates for its column, entity collision
/// or not.
fn collide(
    p: &MovingParticle,
    ty: &EmitterType,
    level: i32,
    scene: &mut dyn ParticleScene,
) -> bool {
    let size = scene.size();
    let tx = p.pos[0] >> (size + 12);
    let tz = p.pos[2] >> (size + 12);
    let y = p.pos[1] >> 12;
    if !(-262_144..=262_144).contains(&y)
        || tx < 0
        || tx >= scene.max_tile_x()
        || tz < 0
        || tz >= scene.max_tile_z()
    {
        return true;
    }
    let max_level = scene.max_level();
    let height = |l: i32| {
        if (0..max_level).contains(&l) {
            scene.tile_height(l, tx, tz)
        } else {
            0
        }
    };
    let tile_level = scene.tile_level(level, tx, tz).unwrap_or(level);
    let ground = height(tile_level);
    let ceiling = if tile_level < max_level - 1 {
        height(tile_level + 1)
    } else {
        ground - (8 << size)
    };
    if ty.level_collision {
        if ty.ceiling_level == -1 && y > ground {
            return true;
        }
        if ty.ceiling_level >= 0 && y > height(ty.ceiling_level) {
            return true;
        }
        if ty.floor_level == -1 && y < ceiling {
            return true;
        }
        if ty.floor_level >= 0 && y < height(ty.floor_level + 1) {
            return true;
        }
    }
    let mut plane = max_level - 1;
    while plane > 0 && y > height(plane) {
        plane -= 1;
    }
    if ty.ground_collision && plane == 0 && y > height(0) {
        return true;
    }
    if max_level - 1 == plane && height(plane) - y > 8 << size {
        return true;
    }
    let tested = allocate_tiles(scene, plane, tx, tz);
    ty.entity_collision && scene.bounds_contain(tested, tx, tz, [p.pos[0] >> 12, y, p.pos[2] >> 12])
}

/// When the level's tile table is missing, the client
/// allocates plane 0 (when `plane` is 0 or it is missing) and every missing
/// plane `1..=plane` (bridged columns stop at plane 2 and bump the new
/// tiles' `level`), then tests the last tile it allocated, or
/// `levelTiles[plane]` when it allocated none. Returns the tested plane.
fn allocate_tiles(scene: &mut dyn ParticleScene, plane: i32, x: i32, z: i32) -> i32 {
    if scene.tile_level(plane, x, z).is_some() {
        return plane;
    }
    let mut tested = None;
    if plane == 0 || scene.tile_level(0, x, z).is_none() {
        scene.create_tile(0, x, z, 0);
        tested = Some(0);
    }
    let bridged = scene.bridged(x, z);
    let mut plane = plane;
    if plane == 3 && bridged {
        plane -= 1;
    }
    for l in 1..=plane {
        if scene.tile_level(l, x, z).is_none() {
            scene.create_tile(l, x, z, if bridged { l + 1 } else { l });
            tested = Some(l);
        }
    }
    tested.unwrap_or(plane)
}

struct SlotState<'a> {
    slots: &'a mut Vec<u32>,
    fracs: &'a mut Vec<i32>,
    next_slot: &'a mut usize,
    next_serial: &'a mut u32,
}

impl SlotState<'_> {
    /// Takes the next ring slot, evicting its occupant.
    fn claim(&mut self, frac: i32, pool: &mut ParticlePool) -> (u16, u32) {
        if self.slots.is_empty() {
            self.slots.resize(SYSTEM_SLOTS, 0);
            self.fracs.resize(SYSTEM_SLOTS, 0);
        }
        let slot = *self.next_slot;
        if self.slots[slot] != 0 {
            pool.release(self.fracs[slot]);
        }
        let serial = *self.next_serial;
        *self.next_serial = self.next_serial.wrapping_add(1).max(1);
        self.slots[slot] = serial;
        self.fracs[slot] = frac;
        *self.next_slot = (slot + 1) & 0x1FFF;
        (slot as u16, serial)
    }

    /// Keep an occupant's colour fraction current for a later release.
    fn store(&mut self, p: &MovingParticle) {
        self.fracs[p.slot as usize] = p.colour_frac;
    }
}

impl Emitter {
    /// A new emitter from its anchor, keeping the type rather than the
    /// low detail substitute.
    fn new(
        anchor: &EmitterAnchor,
        ty: Arc<EmitterType>,
        created: i64,
        constants: Vec<i32>,
        rng: &mut AnimationRandom,
    ) -> Self {
        let mut e = Self {
            id: anchor.id,
            model: anchor.vertices,
            ty,
            accumulator: (0.0 + rng.next() * 64.0) as i32,
            particles: Vec::new(),
            count: 0,
            created,
            dying: false,
            current: Triangle::default(),
            previous: Triangle::default(),
            normal: [0; 3],
            yaw_base: 0,
            yaw_range: 0,
            pitch_base: 0,
            pitch_range: 0,
            degenerate: false,
            normal_valid: false,
            constants,
        };
        e.refresh();
        e.previous.v = e.current.v;
        e
    }

    /// Refreshes the triangle from the anchor.
    fn refresh(&mut self) {
        self.current.v = self.model;
        let [a, b, c] = self.current.v;
        if a[0] == b[0]
            && c[0] == b[0]
            && b[1] == a[1]
            && c[1] == b[1]
            && b[2] == a[2]
            && b[2] == c[2]
        {
            self.degenerate = true;
        } else if self.degenerate {
            self.degenerate = false;
            self.previous.v = self.current.v;
        }
    }

    /// Advances the emitter one cycle.
    fn update(
        &mut self,
        cycle: i64,
        elapsed: i32,
        mut spawn: bool,
        locals: &[Effector],
        slots: &mut SlotState<'_>,
        ctx: &mut Tick<'_>,
    ) {
        let ty = self.ty.clone();
        if self.dying
            || ctx.detail < ty.min_setting
            || ctx.previous_total > PARTICLE_LIMITS[ctx.detail as usize]
            || self.degenerate
        {
            spawn = false;
        } else if ty.period != -1 {
            let mut t = (cycle - self.created) as i32;
            if ty.period_repeat || t <= ty.period {
                // The reference divides by the period unguarded; a zero period is a
                // division by zero there.
                t = if ty.period == 0 { 0 } else { t % ty.period };
            } else {
                spawn = false;
            }
            if !ty.active_before_threshold && t < ty.period_threshold {
                spawn = false;
            }
            if ty.active_before_threshold && t >= ty.period_threshold {
                spawn = false;
            }
        }
        self.count = 0;
        let centre = self.current.centre;
        let mut particles = std::mem::take(&mut self.particles);
        particles.retain_mut(|p| {
            if slots.slots.get(p.slot as usize) != Some(&p.serial) {
                return false;
            }
            self.count += 1;
            let alive = p.advance(elapsed, &ty, centre, locals, &self.constants, ctx);
            if alive {
                slots.store(p);
            } else {
                System::release(slots.slots, ctx.pool, p);
            }
            alive
        });
        self.particles = particles;
        if spawn {
            self.spawn(elapsed, &ty, locals, slots, ctx);
        }
        if self.current.centre != self.previous.centre {
            std::mem::swap(&mut self.current, &mut self.previous);
            self.current.v = self.model;
            self.current.centre = self.previous.centre;
        }
        ctx.current_total += self.count;
    }

    /// The spawn branch of [`Self::update`].
    fn spawn(
        &mut self,
        elapsed: i32,
        ty: &EmitterType,
        locals: &[Effector],
        slots: &mut SlotState<'_>,
        ctx: &mut Tick<'_>,
    ) {
        let [v1, v2, v3] = self.current.v;
        let centre = [
            (v3[0] + v1[0] + v2[0]) / 3,
            (v3[1] + v2[1] + v1[1]) / 3,
            (v3[2] + v2[2] + v1[2]) / 3,
        ];
        if self.current.centre != centre || !self.normal_valid {
            self.current.centre = centre;
            let e1 = [v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]];
            let e2 = [v3[0] - v1[0], v3[1] - v1[1], v3[2] - v1[2]];
            let mut n = [
                e1[1]
                    .wrapping_mul(e2[2])
                    .wrapping_sub(e1[2].wrapping_mul(e2[1])),
                e1[2]
                    .wrapping_mul(e2[0])
                    .wrapping_sub(e1[0].wrapping_mul(e2[2])),
                e1[0]
                    .wrapping_mul(e2[1])
                    .wrapping_sub(e1[1].wrapping_mul(e2[0])),
            ];
            while n.iter().any(|v| *v > 32767 || *v < -32767) {
                n = n.map(|v| v >> 1);
            }
            let sq = n[2]
                .wrapping_mul(n[2])
                .wrapping_add(n[1].wrapping_mul(n[1]))
                .wrapping_add(n[0].wrapping_mul(n[0]));
            let mut len = f64::from(sq).sqrt() as i32;
            if len <= 0 {
                len = 1;
            }
            n = n.map(|v| v * 32767 / len);
            self.normal = n;
            if ty.yaw_max > 0 || ty.pitch_max > 0 {
                let yaw = (f64::from(n[2]).atan2(f64::from(n[0])) * 2607.5945876176133) as i32;
                let flat = f64::from(
                    n[2].wrapping_mul(n[2])
                        .wrapping_add(n[0].wrapping_mul(n[0])),
                );
                let pitch = (f64::from(n[1]).atan2(flat.sqrt()) * 2607.5945876176133) as i32;
                self.yaw_range = i32::from(ty.yaw_max) - i32::from(ty.yaw_min);
                self.yaw_base = i32::from(ty.yaw_min) + yaw - (self.yaw_range >> 1);
                self.pitch_range = i32::from(ty.pitch_max) - i32::from(ty.pitch_min);
                self.pitch_base = i32::from(ty.pitch_min) + pitch - (self.pitch_range >> 1);
            }
            self.normal_valid = true;
        }
        let rate = f64::from(ty.rate_min) + ctx.rng.next() * f64::from(ty.rate_max - ty.rate_min);
        self.accumulator = self
            .accumulator
            .wrapping_add((f64::from(elapsed) * rate) as i32);
        if self.accumulator <= 63 {
            return;
        }
        let count = self.accumulator >> 6;
        self.accumulator &= 0x3F;
        let step = (elapsed << 8) / count;
        let mut advance = 0;
        let [v1, v2, v3] = self.current.v;
        for _ in 0..count {
            let dir = if ty.yaw_max <= 0 && ty.pitch_max <= 0 {
                self.normal
            } else {
                let yaw =
                    (self.yaw_base + (f64::from(self.yaw_range) * ctx.rng.next()) as i32) & 0x3FFF;
                let (ys, yc) = (crate::trig::sin(yaw), crate::trig::cos(yaw));
                let pitch = (self.pitch_base
                    + (f64::from(self.pitch_range) * ctx.rng.next()) as i32)
                    & 0x1FFF;
                let (ps, pc) = (crate::trig::sin(pitch), crate::trig::cos(pitch));
                [(yc * ps) >> 13, -(pc << 1), (ys * ps) >> 13]
            };
            let mut s = ctx.rng.next() as f32;
            let mut t = ctx.rng.next() as f32;
            if s + t > 1.0 {
                s = 1.0 - s;
                t = 1.0 - t;
            }
            let u = 1.0 - (s + t);
            let x = (v3[0] as f32 * u + v1[0] as f32 * s + v2[0] as f32 * t) as i32;
            let y = (v3[1] as f32 * u + v2[1] as f32 * t + v1[1] as f32 * s) as i32;
            let z = (v3[2] as f32 * u + v2[2] as f32 * t + v1[2] as f32 * s) as i32;
            let speed = ty.speed_min
                + (ctx.rng.next() * f64::from(ty.speed_max.wrapping_sub(ty.speed_min))) as i32;
            let lifetime = ty.lifetime_min
                + (ctx.rng.next() * f64::from(ty.lifetime_max - ty.lifetime_min)) as i32;
            let size = ty.size_min + (ctx.rng.next() * f64::from(ty.size_max - ty.size_min)) as i32;
            let mut angle = ty.angle_min;
            if ty.angle_range != 0 {
                if ty.angle_steps == 0 {
                    angle += (ctx.rng.next() * f64::from(ty.angle_range + 1)) as i32;
                } else {
                    angle += (ctx.rng.next() * f64::from(ty.angle_steps + 1)) as i32
                        * (ty.angle_range / ty.angle_steps);
                }
            }
            let mut spin = ty.spin_min;
            if ty.spin_range != 0 {
                spin += (ctx.rng.next() * f64::from(ty.spin_range + 1)) as i32;
            }
            let colour = if ty.uniform_colour {
                let r = ctx.rng.next();
                ((f64::from(ty.red_range) * r + f64::from(ty.red)) as i32) << 16
                    | ((f64::from(ty.green_range) * r + f64::from(ty.green)) as i32) << 8
                    | (f64::from(ty.blue_range) * r + f64::from(ty.blue)) as i32
                    | ((f64::from(ty.alpha) + ctx.rng.next() * f64::from(ty.alpha_range)) as i32)
                        << 24
            } else {
                let r = (f64::from(ty.red) + ctx.rng.next() * f64::from(ty.red_range)) as i32;
                let g = (f64::from(ty.green) + ctx.rng.next() * f64::from(ty.green_range)) as i32;
                let b = (f64::from(ty.blue) + ctx.rng.next() * f64::from(ty.blue_range)) as i32;
                let a = (f64::from(ty.alpha) + ctx.rng.next() * f64::from(ty.alpha_range)) as i32;
                r << 16 | g << 8 | b | a << 24
            };
            // A released particle object keeps its colour fraction.
            let colour_frac = ctx.pool.take();
            let (slot, serial) = slots.claim(colour_frac, ctx.pool);
            let mut p = MovingParticle {
                pos: [x << 12, y << 12, z << 12],
                colour,
                colour_frac,
                lifetime: lifetime as i16,
                remaining: lifetime as i16,
                size,
                angle: angle as i16,
                spin: spin as i16,
                texture: ty.texture,
                lit: ty.lit,
                dir: dir.map(|v| v as i16),
                speed,
                slot,
                serial,
            };
            let mut alive = true;
            if advance > 256 {
                alive = p.advance(
                    advance >> 8,
                    ty,
                    self.current.centre,
                    locals,
                    &self.constants,
                    ctx,
                );
            }
            if alive {
                slots.store(&p);
                self.particles.push(p);
            } else {
                System::release(slots.slots, ctx.pool, &p);
            }
            self.count += 1;
            advance += step;
        }
    }
}

impl MovingParticle {
    /// Advances the particle one cycle. Returns false when the particle
    /// expired.
    fn advance(
        &mut self,
        elapsed: i32,
        ty: &EmitterType,
        centre: [i32; 3],
        locals: &[Effector],
        constants: &[i32],
        ctx: &Tick<'_>,
    ) -> bool {
        self.remaining = (i32::from(self.remaining) - elapsed) as i16;
        if self.remaining <= 0 {
            return false;
        }
        let px = self.pos[0] >> 12;
        let py = self.pos[1] >> 12;
        let pz = self.pos[2] >> 12;
        let age = i32::from(self.lifetime) - i32::from(self.remaining);
        if ty.target_colour != 0 {
            if age <= ty.colour_cycles {
                let clamp = |v: i32| v.clamp(0, 65535);
                let r = clamp(
                    ty.red_step.wrapping_mul(elapsed)
                        + (self.colour_frac >> 16 & 0xFF)
                        + (self.colour >> 8 & 0xFF00),
                );
                let g = clamp(
                    ty.green_step.wrapping_mul(elapsed)
                        + (self.colour_frac >> 8 & 0xFF)
                        + (self.colour & 0xFF00),
                );
                let b = clamp(
                    ty.blue_step.wrapping_mul(elapsed)
                        + ((self.colour & 0xFF) << 8)
                        + (self.colour_frac & 0xFF),
                );
                self.colour &= 0xFF00_0000_u32 as i32;
                self.colour |= (b >> 8 & 0xFF) + ((r & 0xFF00) << 8) + (g & 0xFF00);
                self.colour_frac &= 0xFF00_0000_u32 as i32;
                self.colour_frac |= (b & 0xFF) + ((r & 0xFF) << 16) + ((g & 0xFF) << 8);
            }
            if age <= ty.alpha_cycles {
                let a = (ty.alpha_step.wrapping_mul(elapsed)
                    + (self.colour_frac >> 24 & 0xFF)
                    + (self.colour >> 16 & 0xFF00))
                    .clamp(0, 65535);
                self.colour &= 0xFF_FFFF;
                self.colour |= (a & 0xFF00) << 16;
                self.colour_frac &= 0xFF_FFFF;
                self.colour_frac |= (a & 0xFF) << 24;
            }
        }
        if ty.target_speed != -1 && age <= ty.speed_cycles {
            self.speed = self.speed.wrapping_add(ty.speed_step.wrapping_mul(elapsed));
        }
        if ty.target_size != -1 && age <= ty.size_cycles {
            self.size = self.size.wrapping_add(ty.size_step.wrapping_mul(elapsed));
        }
        if self.spin != 0 {
            self.angle = ((i32::from(self.spin) * elapsed + i32::from(self.angle)) & 0x3FFF) as i16;
        }
        let mut vx = f64::from(self.dir[0]);
        let mut vy = f64::from(self.dir[1]);
        let mut vz = f64::from(self.dir[2]);
        let mut changed = false;
        if ty.damping_mode == 1 {
            let (dx, dy, dz) = (px - centre[0], py - centre[1], pz - centre[2]);
            let sq = dz
                .wrapping_mul(dz)
                .wrapping_add(dx.wrapping_mul(dx))
                .wrapping_add(dy.wrapping_mul(dy));
            let dist = (f64::from(sq).sqrt() as i32) >> 2;
            let k = i64::from(ty.damping.wrapping_mul(dist).wrapping_mul(elapsed));
            self.speed = (i64::from(self.speed) - ((i64::from(self.speed) * k) >> 18)) as i32;
        } else if ty.damping_mode == 2 {
            let (dx, dy, dz) = (px - centre[0], py - centre[1], pz - centre[2]);
            let sq = dz
                .wrapping_mul(dz)
                .wrapping_add(dx.wrapping_mul(dx))
                .wrapping_add(dy.wrapping_mul(dy));
            let k = i64::from(ty.damping.wrapping_mul(sq).wrapping_mul(elapsed));
            self.speed = (i64::from(self.speed) - ((i64::from(self.speed) * k) >> 28)) as i32;
        }
        let t = f64::from(elapsed);
        if let Some(ids) = ty.local_effectors.as_deref() {
            for e in locals {
                let et = &e.ty;
                if et.kind == 1 || !ids.contains(&et.id) {
                    continue;
                }
                let dx = f64::from(px - e.pos[0]);
                let dy = f64::from(py - e.pos[1]);
                let dz = f64::from(pz - e.pos[2]);
                let sq = dz * dz + dx * dx + dy * dy;
                if sq > et.range as f64 {
                    continue;
                }
                let mut dist = sq.sqrt();
                if dist == 0.0 {
                    dist = 1.0;
                }
                let cone = (f64::from(e.dir_z) * dz
                    + f64::from(e.dir_x) * dx
                    + f64::from(et.force[1]) * dy)
                    * 65535.0
                    / (f64::from(et.magnitude) * dist);
                if cone < f64::from(et.cone_cos) {
                    continue;
                }
                self.apply_effector(
                    et,
                    EffectorPush {
                        direction: [e.dir_x, e.dir_z],
                        delta: [dx, dy, dz],
                        dist,
                        step: t,
                    },
                    [&mut vx, &mut vy, &mut vz],
                    &mut changed,
                );
            }
        }
        if let Some(ids) = ty.global_effectors.as_deref() {
            for id in ids {
                for (_, g) in ctx.globals.iter().filter(|(k, _)| k == id) {
                    let et = &g.ty;
                    let dx = f64::from(px - g.pos[0]);
                    let dy = f64::from(py - g.pos[1]);
                    let dz = f64::from(pz - g.pos[2]);
                    let sq = dz * dz + dx * dx + dy * dy;
                    if sq > et.range as f64 {
                        continue;
                    }
                    let mut dist = sq.sqrt();
                    if dist == 0.0 {
                        dist = 1.0;
                    }
                    if et.cone > 0 && et.cone < 2047 {
                        let cone = (f64::from(g.dir_z) * dz
                            + f64::from(g.dir_x) * dx
                            + f64::from(et.force[1]) * dy)
                            * 16384.0
                            / (f64::from(et.magnitude) * dist);
                        if cone < f64::from(et.cone_cos) {
                            continue;
                        }
                    }
                    self.apply_effector(
                        et,
                        EffectorPush {
                            direction: [g.dir_x, g.dir_z],
                            delta: [dx, dy, dz],
                            dist,
                            step: t,
                        },
                        [&mut vx, &mut vy, &mut vz],
                        &mut changed,
                    );
                }
            }
        }
        for &index in constants {
            let Some(et) = usize::try_from(index)
                .ok()
                .and_then(|i| ctx.constants.get(i))
            else {
                continue;
            };
            if et.positional == 0 {
                vx += f64::from(et.force[0].wrapping_mul(elapsed));
                vy += f64::from(et.force[1].wrapping_mul(elapsed));
                vz += f64::from(et.force[2].wrapping_mul(elapsed));
                changed = true;
            } else {
                self.pos[0] = self.pos[0].wrapping_add(et.force[0].wrapping_mul(elapsed));
                self.pos[1] = self.pos[1].wrapping_add(et.force[1].wrapping_mul(elapsed));
                self.pos[2] = self.pos[2].wrapping_add(et.force[2].wrapping_mul(elapsed));
            }
        }
        if changed {
            loop {
                // The test is `!(v > 32767) && !(v < -32767)`: a NaN component
                // counts as in range.
                let overflow = vx > 32767.0
                    || vy > 32767.0
                    || vz > 32767.0
                    || vx < -32767.0
                    || vy < -32767.0
                    || vz < -32767.0;
                if !overflow {
                    self.dir = [vx as i32 as i16, vy as i32 as i16, vz as i32 as i16];
                    break;
                }
                vx /= 2.0;
                vy /= 2.0;
                vz /= 2.0;
                self.speed = self.speed.wrapping_shl(1);
            }
        }
        let speed = i64::from(self.speed.wrapping_shl(2));
        for axis in 0..3 {
            self.pos[axis] = (i64::from(self.pos[axis])
                + ((speed * i64::from(self.dir[axis])) >> 23) * i64::from(elapsed))
                as i32;
        }
        true
    }

    /// The effector body shared by local and global effectors. The float
    /// multiplication order differs between the radial (`t * f`) and the
    /// directional (`f * t`) form and is kept.
    #[allow(
        clippy::if_same_then_else,
        reason = "the radial and directional forms multiply in a different order, which is kept"
    )]
    fn apply_effector(
        &mut self,
        et: &EffectorType,
        push: EffectorPush,
        v: [&mut f64; 3],
        changed: &mut bool,
    ) {
        let EffectorPush {
            direction: [dir_x, dir_z],
            delta,
            dist,
            step: t,
        } = push;
        let falloff = match et.falloff_mode {
            1 => dist / 16.0 * f64::from(et.falloff),
            2 => dist / 16.0 * (dist / 16.0) * f64::from(et.falloff),
            _ => 0.0,
        };
        let [vx, vy, vz] = v;
        let force = if et.radial != 0 {
            let m = f64::from(et.magnitude);
            [
                delta[0] / dist * m,
                delta[1] / dist * m,
                delta[2] / dist * m,
            ]
        } else {
            [
                f64::from(dir_x) - falloff,
                f64::from(et.force[1]) - falloff,
                f64::from(dir_z) - falloff,
            ]
        };
        if et.positional == 0 {
            if et.radial != 0 {
                *vx += t * force[0];
                *vy += t * force[1];
                *vz += t * force[2];
            } else {
                *vx += force[0] * t;
                *vy += force[1] * t;
                *vz += force[2] * t;
            }
            *changed = true;
        } else {
            for (pos, f) in self.pos.iter_mut().zip(force) {
                let add = if et.radial != 0 { t * f } else { f * t };
                *pos = (f64::from(*pos) + add) as i32;
            }
        }
    }
}

#[cfg(test)]
mod tests;
