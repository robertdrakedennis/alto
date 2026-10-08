//! Sequence selection and lifecycle, independent of GPU.
use crate::{
    animation_assets::{AnimationAssets, Playback},
    animation_playback::AnimationRandom,
    config::Loc,
};

/// What sequence selection reads besides the state it updates: the loc's own
/// type, the type it currently resolves to (through its varbit or varp), the
/// animation assets, the client cycle and the animation detail preference.
#[derive(Clone, Copy)]
pub struct LocContext<'a> {
    pub base: &'a Loc,
    pub resolved: Option<&'a Loc>,
    pub assets: &'a AnimationAssets,
    pub cycle: i32,
    pub detail: i32,
}

/// How a sequence is picked: keep a playing one that is still valid
/// (`preserve`), force a given one (`explicit`, -1 for the weighted pick), the
/// playback mode and the start delay.
#[derive(Clone, Copy, Debug)]
pub struct Pick {
    pub preserve: bool,
    pub explicit: i32,
    pub mode: i32,
    pub delay: i32,
}

impl Pick {
    /// A weighted pick from the loc type's animations.
    pub fn automatic(preserve: bool) -> Self {
        Self {
            preserve,
            explicit: -1,
            mode: 0,
            delay: 0,
        }
    }
}

pub struct DynamicLoc {
    pub animation: Playback,
    pub delayed: Option<Playback>,
    pub last_cycle: i32,
    pub selected: i32,
    pub overlay_height: i32,
    pub forced: bool,
    pub shadow_current: bool,
    pub cache_revision: u64,
}
impl Default for DynamicLoc {
    fn default() -> Self {
        Self {
            animation: Playback::default(),
            delayed: None,
            last_cycle: 0,
            selected: -1,
            overlay_height: 0,
            forced: false,
            shadow_current: false,
            cache_revision: 0,
        }
    }
}
/// The caller supplies the resolved local varbit/varp.
pub fn morph_id(loc: &Loc, value: i32) -> i32 {
    if !loc.has_multiloc {
        return loc.id as i32;
    }
    if value >= 0 && (value as usize) < loc.multiloc.len() - 1 {
        loc.multiloc[value as usize]
    } else {
        *loc.multiloc.last().expect("multiloc fallback")
    }
}
fn weighted(loc: &Loc, rng: &mut AnimationRandom) -> i32 {
    if !loc.has_anim {
        return -1;
    }
    if loc.anims.len() <= 1 {
        return loc.anims.first().copied().unwrap_or(-1);
    }
    let mut value = (rng.next() * 65535.) as i32;
    for (&id, &weight) in loc.anims.iter().zip(&loc.anim_weights) {
        if value <= weight {
            return id;
        }
        value -= weight;
    }
    -1
}
impl DynamicLoc {
    fn invalidate(&mut self) {
        self.shadow_current = false;
        self.cache_revision = self.cache_revision.wrapping_add(1);
    }
    /// Choose the loc's sequence: `pick.explicit` when given, else a weighted
    /// pick among the animations of the resolved (or, failing that, the base)
    /// loc type.
    pub fn select(
        &mut self,
        ctx: &LocContext<'_>,
        pick: Pick,
        rng: &mut AnimationRandom,
    ) -> anyhow::Result<()> {
        let LocContext {
            base,
            resolved,
            assets,
            cycle,
            detail,
        } = *ctx;
        let Pick {
            preserve,
            explicit,
            mut mode,
            delay,
        } = pick;
        let mut id = explicit;
        let mut random = false;
        if id == -1 {
            let Some(resolved) = resolved else {
                return Ok(());
            };
            let source = if resolved.has_anim {
                Some(resolved)
            } else if resolved.id != base.id && base.has_anim {
                Some(base)
            } else {
                None
            };
            if let Some(source) = source.filter(|l| !l.disable_anim_low_detail || detail == 1) {
                if preserve
                    && self.animation.node.id() != -1
                    && source.anims.contains(&self.animation.node.id())
                {
                    return Ok(());
                }
                if self.selected != resolved.id as i32 {
                    random = source.random_anim_frame;
                }
                id = weighted(source, rng);
                mode = if source.anims.len() > 1 { 0 } else { 1 };
            }
        }
        self.animation.start(assets, id, delay, mode, random, rng)?;
        if id != -1 {
            self.last_cycle = cycle;
            self.invalidate();
        }
        Ok(())
    }
    /// Start an animation: a delayed replacement does not stop the current pose.
    pub fn start(
        &mut self,
        ctx: &LocContext<'_>,
        id: i32,
        delay: i32,
        rng: &mut AnimationRandom,
    ) -> anyhow::Result<()> {
        self.delayed = None;
        if delay > 0 {
            let mut p = Playback::default();
            p.start(ctx.assets, id, delay, 1, false, rng)?;
            self.delayed = Some(p);
        } else {
            self.forced = true;
            let pick = Pick {
                preserve: false,
                explicit: id,
                mode: 1,
                delay: 0,
            };
            self.select(ctx, pick, rng)?;
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        ctx: &LocContext<'_>,
        shadows: i32,
        rng: &mut AnimationRandom,
    ) -> anyhow::Result<()> {
        let LocContext { assets, cycle, .. } = *ctx;
        if let Some(delayed) = self.delayed.as_mut().filter(|p| p.node.id() != -1) {
            delayed.advance(assets, cycle.wrapping_sub(self.last_cycle), rng);
            if delayed.node.finished {
                delayed.start(assets, -1, 0, 0, false, rng)?;
            }
            if delayed.node.delay == 0 {
                self.animation = delayed.clone();
                self.forced = true;
                self.last_cycle = cycle;
                return Ok(());
            }
        }
        if self.animation.node.id() == -1 {
            self.select(ctx, Pick::automatic(false), rng)?;
        } else if self
            .animation
            .advance(assets, cycle.wrapping_sub(self.last_cycle), rng)
        {
            if shadows == 2 {
                self.shadow_current = false;
            }
            if self.animation.node.finished {
                self.animation.start(assets, -1, 0, 0, false, rng)?;
                self.forced = false;
                self.select(ctx, Pick::automatic(false), rng)?;
            }
        }
        self.last_cycle = cycle;
        Ok(())
    }
    /// The first portion of getModel, before its shadow-only early return.
    pub fn begin(
        &mut self,
        ctx: &LocContext<'_>,
        shadows: i32,
        rng: &mut AnimationRandom,
    ) -> anyhow::Result<bool> {
        let Some(loc) = ctx.resolved else {
            self.selected = -1;
            return Ok(false);
        };
        if !self.forced && self.selected != loc.id as i32 {
            self.select(ctx, Pick::automatic(true), rng)?;
            self.invalidate();
        }
        self.advance(ctx, shadows, rng)?;
        Ok(true)
    }
}
