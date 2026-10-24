//! The post-process chain's game state: the effects the toolkit installs
//! (bloom, levels, colour remapping), their parameters fed from the
//! environment each frame, and whether a capture opens. The UI traversal
//! reads the capture answer (`ui_backend::PostRegion`); the modern renderer
//! applies the effects itself from the environment.
use std::rc::Rc;

/// The effects the toolkit installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Bloom,
    Levels,
    ColourRemapping,
}

impl Effect {
    /// The chain position key: bloom first, then levels, then colour
    /// remapping.
    pub fn order(self) -> i32 {
        match self {
            Self::Bloom => 0,
            Self::Levels => 1,
            Self::ColourRemapping => 2,
        }
    }
}

/// The bloom effect's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomParams {
    pub threshold: f32,
    pub white_point_sq: f32,
    pub intensity: f32,
    pub sample_scale: f32,
}

impl Default for BloomParams {
    /// The neutral bloom parameters.
    fn default() -> Self {
        Self {
            threshold: 1.0,
            white_point_sq: 1.0,
            intensity: 0.25,
            sample_scale: 1.0,
        }
    }
}

/// The levels effect's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsParams {
    pub gamma: f32,
    pub input_min: f32,
    pub input_max: f32,
    pub output_min: f32,
    pub output_max: f32,
}

impl Default for LevelsParams {
    /// The identity levels.
    fn default() -> Self {
        Self::from_array([1.0, 0.0, 1.0, 0.0, 1.0])
    }
}

impl LevelsParams {
    /// `[gamma, inputMin, inputMax, outputMin, outputMax]`, the
    /// environment's levels order.
    pub fn from_array(v: [f32; 5]) -> Self {
        Self {
            gamma: v[0],
            input_min: v[1],
            input_max: v[2],
            output_min: v[3],
            output_max: v[4],
        }
    }
    /// Whether the levels leave every colour unchanged.
    pub fn is_noop(&self) -> bool {
        self.gamma == 1.0
            && self.input_min == 0.0
            && self.input_max == 1.0
            && self.output_min == 0.0
            && self.output_max == 1.0
    }
}

/// A colour remapper: the `256x16` remapping sprite behind a colour
/// remapping map id.
#[derive(Clone, Debug)]
pub struct Remapper {
    pub id: i32,
    /// The sprite's ARGB pixels, row-major `256x16`.
    pub argb: Rc<[i32]>,
}

impl PartialEq for Remapper {
    /// The original client compares the cached objects by identity; the
    /// cache keys them by sprite id.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// The colour remapping effect's parameters: up to three remappers with
/// their weights, the number of samples, and the source colour's weight.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourRemapParams {
    pub remappers: [Option<Remapper>; 3],
    pub weights: [f32; 3],
    pub count: i32,
    pub base: f32,
}

impl Default for ColourRemapParams {
    /// No remappers, source colour only.
    fn default() -> Self {
        Self {
            remappers: [None, None, None],
            weights: [0.0; 3],
            count: 1,
            base: 1.0,
        }
    }
}

impl ColourRemapParams {
    /// Drop weighted empty slots towards the front, count the positive
    /// weights, and give the source colour the remainder.
    pub fn set(
        mut a: Option<Remapper>,
        mut aw: f32,
        mut b: Option<Remapper>,
        mut bw: f32,
        mut c: Option<Remapper>,
        mut cw: f32,
    ) -> Self {
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
        let count = [aw, bw, cw].iter().filter(|w| **w > 0.0).count() as i32;
        Self {
            remappers: [a, b, c],
            weights: [aw, bw, cw],
            count,
            base: 1.0 - (aw + bw + cw),
        }
    }
    /// Whether the remapping leaves every colour unchanged.
    pub fn is_noop(&self) -> bool {
        self.count == 0
            || self.base == 1.0
            || self.weights[0] + self.weights[1] + self.weights[2] == 0.0
            || self.remappers.iter().all(Option::is_none)
    }
}

/// The effect statics one frame reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    pub bloom: BloomParams,
    pub levels: LevelsParams,
    pub remap: ColourRemapParams,
}

impl Params {
    /// Whether an effect leaves the frame unchanged; bloom is never a no-op.
    pub fn is_noop(&self, effect: Effect) -> bool {
        match effect {
            Effect::Bloom => false,
            Effect::Levels => self.levels.is_noop(),
            Effect::ColourRemapping => self.remap.is_noop(),
        }
    }
}

/// The ordered effect vector and the widest framebuffer data type any added
/// effect asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    effects: Vec<Effect>,
}

impl Chain {
    /// Levels and colour remapping are always installed; bloom is added by
    /// [`Chain::add`] once the bloom preference is reconciled.
    pub fn with_filters() -> Self {
        let mut chain = Self::default();
        chain.add(Effect::Levels);
        chain.add(Effect::ColourRemapping);
        chain
    }
    /// Insert before the first effect with a larger position key, raising
    /// the chain's data type, unless the effect is already in the chain
    /// (returns whether it was added). Every effect's program compiles on
    /// this device.
    pub fn add(&mut self, effect: Effect) -> bool {
        if self.contains(effect) {
            return false;
        }
        let at = self
            .effects
            .iter()
            .position(|e| effect.order() < e.order())
            .unwrap_or(self.effects.len());
        self.effects.insert(at, effect);
        true
    }
    /// Remove an effect; the chain's data type is not lowered.
    pub fn remove(&mut self, effect: Effect) {
        self.effects.retain(|e| *e != effect);
    }
    /// Whether the effect is in the chain.
    pub fn contains(&self, effect: Effect) -> bool {
        self.effects.contains(&effect)
    }
    /// Open a capture when some enabled effect is live; the capture's own
    /// empty-chain early-out then cannot trigger.
    pub fn capture(&self, params: &Params) -> bool {
        self.effects.iter().any(|e| !params.is_noop(*e))
    }
    /// When any effect is a no-op, only the live ones run (in chain order).
    pub fn live(&self, params: &Params) -> Vec<Effect> {
        self.effects
            .iter()
            .copied()
            .filter(|e| !params.is_noop(*e))
            .collect()
    }
}

/// A small LRU cache (8 entries) of remappers by sprite id: only a `256x16`
/// sprite yields a remapper, and only hits are cached.
pub struct RemapperCache {
    pack: Option<crate::cache::Pack>,
    cache: Vec<Remapper>,
}

impl RemapperCache {
    const CAPACITY: usize = 8;
    pub fn new(pack: Option<crate::cache::Pack>) -> Self {
        Self {
            pack,
            cache: Vec::new(),
        }
    }
    pub fn get(&mut self, id: i32) -> Option<Remapper> {
        if let Some(at) = self.cache.iter().position(|r| r.id == id) {
            let hit = self.cache.remove(at);
            self.cache.push(hit.clone());
            return Some(hit);
        }
        let pack = self.pack.as_ref()?;
        let bytes = crate::js5_fetch::fetch_file(pack, "sprites", id as u32).ok()??;
        let frames = crate::sprite_data::Data::decode(&bytes).ok()?;
        let data = frames.first()?;
        if data.width != 256 || data.height != 16 {
            return None;
        }
        let remapper = Remapper {
            id,
            argb: data.argb(false).ok()?.into(),
        };
        if self.cache.len() == Self::CAPACITY {
            self.cache.remove(0);
        }
        self.cache.push(remapper.clone());
        Some(remapper)
    }
}

/// Feed the chain's parameters from the current environment. The override
/// levels and remapping are only ever reset, never set, so the current
/// environment always wins.
pub fn update_full(
    chain: &Chain,
    params: &mut Params,
    env: &crate::env::EnvFrame,
    remappers: &mut RemapperCache,
) {
    // The sample scale keeps its default of 1.
    params.bloom = BloomParams {
        threshold: env.bloom[2],
        white_point_sq: env.bloom[0],
        intensity: env.bloom[1],
        sample_scale: 1.0,
    };
    if chain.contains(Effect::Levels) {
        params.levels = LevelsParams::from_array(env.levels);
    }
    if chain.contains(Effect::ColourRemapping) {
        let mut get = |slot: usize| {
            let (map, _) = env.colour_remap[slot];
            (map > -1).then(|| remappers.get(map)).flatten()
        };
        let (a, b, c) = (get(0), get(1), get(2));
        params.remap = ColourRemapParams::set(
            a,
            env.colour_remap[0].1,
            b,
            env.colour_remap[1].1,
            c,
            env.colour_remap[2].1,
        );
    }
}

/// The installed chain and this frame's parameters.
pub struct PostProcessor {
    pub chain: Chain,
    pub params: Params,
}

impl Default for PostProcessor {
    fn default() -> Self {
        Self {
            chain: Chain::with_filters(),
            params: Params::default(),
        }
    }
}

impl PostProcessor {
    /// Whether the UI traversal opens a capture (some effect is live).
    pub fn capture(&self) -> bool {
        self.chain.capture(&self.params)
    }
}
