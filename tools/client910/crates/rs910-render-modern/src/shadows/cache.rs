//! The sun shadows' cascade cache (lane P3-SHADOWS, performance plan
//! bottleneck #2): which cascades this frame must redraw, and which fit
//! each cascade's map holds. CPU only; the GPU half (the static atlas, the
//! fill pipelines and the passes) is `crate::frame::gpu::sun_shadows`.
//!
//! # Why
//!
//! The shadow pass costs CPU, not GPU: every caster was re-encoded into
//! every cascade it meets, every frame (3.7-7.3 ms of encode at MED, 14.3 ms
//! online at ULTRA, against 0.5-1.0 ms of GPU). A cascade's depth map is a
//! function of its fit (the orthographic box, [`Fit`]), the light frame and
//! the casters inside it; while none of them changes, the map is the same.
//!
//! # What this crate does
//!
//! - **Static and dynamic casters.** A caster is *dynamic* when its shape
//!   is posed this frame (the per-frame arena: NPCs, players, projectiles,
//!   spot anims), when it is a dynamic loc whose model changed within the
//!   last [`DYNAMIC_SETTLE`] frames (an animating loc), or when its material
//!   scrolls (the caster pass's alpha test reads the scrolled texture). The
//!   rest are *static*: their draws are summarised per cascade by an
//!   order-independent [`Signature`] of what the caster pass reads (the
//!   mesh's key, material, index range, instance parameters, the model
//!   matrix in scene-local coordinates), plus the terrain's and the
//!   roof-hidden tiles' selections.
//! - **Static maps.** A cascade whose fit and static signature are unchanged
//!   keeps its map. With dynamic casters, the static casters are kept in a
//!   second atlas (the *static atlas*); each frame the cascade's tile is
//!   restored from it and the dynamic casters drawn on top. A depth map
//!   keeps the nearest depth of all its fragments, whatever their order, so
//!   the composite is the map of drawing every caster at once, bit for bit.
//! - **Amortised fits** (the modern client refreshes cascade *i* every
//!   1, 1, 2, 3 frames while the camera is still; the quality record holds
//!   the same 1, 1, 2, 3, but its use is not proven): when the camera moves,
//!   cascade *k* keeps its previous fit
//!   for up to [`UPDATE_PERIOD`]`[k] - 1` frames, so the far cascades are
//!   redrawn every second or third frame. The receivers read the map through
//!   the fit it was drawn with, re-expressed for this frame's camera (the
//!   fit is in scene-local light space), so a kept fit does not move the
//!   shadows; what lags is the cascade's coverage, by the camera's
//!   displacement over those frames, at the far edge of the far cascades.
//!   A large displacement ([`LAG_REACH`] of the cascade's radius), a zoom or
//!   a turned sun refits at once. Near cascades (period 1) never lag, and a
//!   still camera never lags (the modern client amortises only there; here a
//!   still camera redraws nothing at all).
//!
//! The modern client re-renders every caster of a refreshed cascade; the
//! static atlas is this crate's choice. Every value the frame samples keeps the
//! shadow presets (`crate::shadows::presets`) exactly.

use crate::shadows::{Fit, MAX_CASCADES};

/// Frames a cascade keeps its fit while the camera moves (see the module
/// docs): cascade `k` is refitted when its fit is this many frames old.
pub const UPDATE_PERIOD: [u64; MAX_CASCADES] = [1, 1, 2, 3];

/// A kept fit's centre may lie at most this share of the cascade's radius
/// from this frame's fit (else the cascade is refitted at once).
pub const LAG_REACH: f64 = 1.0 / 16.0;

/// A dynamic loc counts as a dynamic caster for this many frames after its
/// model last changed.
pub const DYNAMIC_SETTLE: u64 = 30;

/// Scene-local translations enter a caster's [`Signature`] quantised to this
/// many steps per fine unit (the draws carry camera-local matrices; the
/// camera origin is added back in `f64`, well within a step).
const TRANSLATION_STEPS: f64 = 4.0;

/// An order-independent summary of a set of static casters: the wrapping
/// sum and the count of their hashes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub sum: u64,
    pub count: u64,
}

impl Signature {
    pub fn add(&mut self, hash: u64) {
        self.sum = self.sum.wrapping_add(hash);
        self.count += 1;
    }
}

/// A 64-bit hash of words (a multiply-rotate accumulator with the
/// `splitmix64` finaliser; deterministic across runs), also a
/// [`std::hash::Hasher`] for the keys that derive `Hash`.
#[derive(Clone, Copy, Debug)]
pub struct Mix(u64);

impl Default for Mix {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl Mix {
    /// A mix started with `tag` (what kind of thing it hashes).
    #[must_use]
    pub fn tagged(tag: u64) -> Self {
        let mut m = Self::default();
        m.word(tag);
        m
    }

    pub fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x0100_0000_01B3).rotate_left(29);
    }
}

impl std::hash::Hasher for Mix {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut w = [0; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            self.word(u64::from_le_bytes(w));
        }
    }

    fn write_u8(&mut self, i: u8) {
        self.word(u64::from(i));
    }

    fn write_u32(&mut self, i: u32) {
        self.word(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }

    fn write_usize(&mut self, i: usize) {
        self.word(i as u64);
    }

    fn write_isize(&mut self, i: isize) {
        self.word(i as u64);
    }

    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The hash of one static caster draw: `identity` (its geometry, from the
/// caller) with what the caster pass reads of its instance: the index range,
/// the material, `p0` (UV scale, alpha reference, flags) and the model
/// matrix, whose translation is made scene-local with `origin` (the
/// camera-local origin the matrix was built for).
#[must_use]
pub fn draw_hash(
    mut identity: Mix,
    material: i32,
    range: (u32, u32),
    model: &[f32; 16],
    p0: [f32; 4],
    origin: [f32; 3],
) -> u64 {
    let h = &mut identity;
    h.word(u64::from(material as u32));
    h.word(u64::from(range.0) << 32 | u64::from(range.1));
    for v in p0 {
        h.word(u64::from(v.to_bits()));
    }
    for (i, v) in model.iter().enumerate() {
        match i {
            12..=14 => {
                let scene = f64::from(*v) + f64::from(origin[i - 12]);
                h.word((scene * TRANSLATION_STEPS).round() as i64 as u64);
            }
            _ => h.word(u64::from(v.to_bits())),
        }
    }
    std::hash::Hasher::finish(h)
}

/// The hash of a tile selection (`FloorSelection`: the floors', the
/// terrain's and the roof-hidden tiles' plan selection).
pub fn selection_word(h: &mut Mix, selection: &crate::draw::FloorSelection) {
    h.word(u64::from(selection.whole));
    h.word(u64::from(selection.origin[0] as u32) << 32 | u64::from(selection.origin[1] as u32));
    h.word(u64::from(selection.distance as u32));
    for row in &selection.mask {
        h.word(row.len() as u64);
        for chunk in row.chunks(64) {
            h.word(
                chunk
                    .iter()
                    .enumerate()
                    .fold(0_u64, |w, (i, &b)| w | u64::from(b) << i),
            );
        }
    }
}

/// Whether a moving camera may keep cascade `k`'s fit `kept`, taken
/// `age` frames ago, instead of this frame's `fresh` one (see the module
/// docs).
#[must_use]
pub fn may_keep(k: usize, kept: &Fit, fresh: &Fit, age: u64) -> bool {
    if age >= UPDATE_PERIOD[k.min(MAX_CASCADES - 1)] {
        return false;
    }
    if kept.radius != fresh.radius || kept.texel != fresh.texel {
        return false;
    }
    let reach = kept.radius * LAG_REACH;
    let (dx, dy) = (
        kept.centre[0] - fresh.centre[0],
        kept.centre[1] - fresh.centre[1],
    );
    let dz = (kept.depth[0] - fresh.depth[0]).abs();
    dx.hypot(dy) <= reach && dz <= reach
}

/// What a cascade's tile of an atlas holds: its fit's generation and the
/// static casters' signature, and (the frame atlas only) whether dynamic
/// casters were drawn over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileContent {
    pub fit: u64,
    pub statics: Signature,
    pub dynamic: bool,
}

/// How a cascade's tile is brought up to date this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TilePlan {
    /// Redraw the static casters into the static atlas.
    pub statics_to_cache: bool,
    /// Redraw the static casters straight into the frame atlas (no dynamic
    /// casters: the static atlas is not needed).
    pub statics_to_atlas: bool,
    /// Restore the tile from the static atlas.
    pub restore: bool,
    /// Draw the dynamic casters over it.
    pub dynamic: bool,
}

impl TilePlan {
    /// Whether the frame atlas's tile is written at all.
    #[must_use]
    pub fn touches_atlas(&self) -> bool {
        self.statics_to_atlas || self.restore || self.dynamic
    }
}

/// One cascade's cache state.
#[derive(Clone, Copy, Debug, Default)]
pub struct CascadeState {
    /// The fit the cascade's maps are drawn with, the frame it was taken and
    /// its generation (bumped whenever the fit changes).
    pub fit: Option<Fit>,
    pub fitted: u64,
    pub generation: u64,
    /// What the frame atlas's and the static atlas's tiles hold (`None`:
    /// unknown, e.g. a new atlas).
    pub atlas: Option<TileContent>,
    pub cache: Option<TileContent>,
}

/// The sun cascades' cache (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct SunCache {
    /// What the cached fits depend on besides the camera: the profile's
    /// quality and the light direction's bits.
    pub key: Option<(crate::shadows::Quality, [u32; 3])>,
    pub cascades: [CascadeState; MAX_CASCADES],
    /// Fits taken so far (the generations' counter).
    pub generations: u64,
}

impl SunCache {
    /// Forget every map (a new atlas, profile or light direction).
    pub fn reset(&mut self) {
        let generations = self.generations;
        *self = Self {
            generations,
            ..Self::default()
        };
    }

    /// Forget what the atlases hold (new textures), keeping the fits.
    pub fn forget_tiles(&mut self) {
        for c in &mut self.cascades {
            c.atlas = None;
            c.cache = None;
        }
    }

    /// Forget the static atlas's tiles only (a new static atlas).
    pub fn forget_cache(&mut self) {
        for c in &mut self.cascades {
            c.cache = None;
        }
    }

    /// The fits to draw with this frame (`frame`): per cascade its kept fit
    /// when it equals `fresh`'s or may lag behind it ([`may_keep`]), else
    /// `fresh`'s (a new generation). `key` (the quality and the light
    /// direction) resets the cache when it changes.
    pub fn choose_fits(
        &mut self,
        key: (crate::shadows::Quality, [u32; 3]),
        fresh: &[Fit],
        frame: u64,
    ) -> Vec<Fit> {
        if self.key != Some(key) {
            self.reset();
            self.key = Some(key);
        }
        fresh
            .iter()
            .enumerate()
            .map(|(k, fresh)| {
                let c = &mut self.cascades[k];
                match c.fit {
                    Some(kept)
                        if kept == *fresh
                            || may_keep(k, &kept, fresh, frame.saturating_sub(c.fitted)) =>
                    {
                        kept
                    }
                    _ => {
                        self.generations += 1;
                        c.fit = Some(*fresh);
                        c.fitted = frame;
                        c.generation = self.generations;
                        *fresh
                    }
                }
            })
            .collect()
    }

    /// Cascade `k`'s plan for this frame: its static casters' `statics`
    /// signature and whether dynamic casters meet it (`dynamic`); records
    /// what its tiles will hold.
    pub fn plan(&mut self, k: usize, statics: Signature, dynamic: bool) -> TilePlan {
        let c = &mut self.cascades[k];
        let content = TileContent {
            fit: c.generation,
            statics,
            dynamic: false,
        };
        let mut plan = TilePlan::default();
        if dynamic {
            if c.cache != Some(content) {
                plan.statics_to_cache = true;
                c.cache = Some(content);
            }
            plan.restore = true;
            plan.dynamic = true;
            c.atlas = Some(TileContent {
                dynamic: true,
                ..content
            });
        } else if c.atlas != Some(content) {
            if c.cache == Some(content) {
                plan.restore = true;
            } else {
                plan.statics_to_atlas = true;
            }
            c.atlas = Some(content);
        }
        plan
    }
}
