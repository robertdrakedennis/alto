//! The 48-bit linear congruential generator the animation code draws its
//! doubles from
//! (the animation nodes' random stand-in; the particle system uses
//! the same stream). Split out of `animation_playback` (Phase 2.7) and moved to
//! rs910-core at the 2.6/2.7 merge so `particle` (model) and
//! `animation_playback` (game) share one copy.

/// A seedable stream of doubles in `[0, 1)` (the 48-bit LCG with the
/// 0x5DEECE66D multiplier). The global random seed is not part of scene
/// state; oracle fixtures feed the same seeded draws to the original routines.
#[derive(Clone, Debug)]
pub struct AnimationRandom {
    pub state: u64,
}

impl AnimationRandom {
    pub fn new(seed: u64) -> Self {
        Self {
            state: (seed ^ 0x5deece66d) & ((1 << 48) - 1),
        }
    }
    fn bits(&mut self, n: u32) -> u64 {
        self.state = self.state.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1 << 48) - 1);
        self.state >> (48 - n)
    }
    #[allow(
        clippy::should_implement_trait,
        reason = "a double draw, not an iterator"
    )]
    pub fn next(&mut self) -> f64 {
        ((self.bits(26) << 27) + self.bits(27)) as f64 / (1u64 << 53) as f64
    }
}
