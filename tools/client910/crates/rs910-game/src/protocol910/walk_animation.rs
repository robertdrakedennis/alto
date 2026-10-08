//! Walk/idle animation selection from the BAS type.
//! Selection is independent of pose loading. The caller supplies Math.random.
//! Moved from `entities910::walk_animation` (which keeps the `Walk` state)
//! in Phase 2.2: it reads the base animation set config.
use super::{bas_types::Bas, Error};
use crate::entities910::animation_state::Config;
use crate::entities910::walk_animation::Walk;
pub struct Input {
    pub speed: i32,
    pub direction: i32,
    pub turn: i32,
    pub angle: i32,
    pub desired: i32,
    pub turn_ticks: i32,
    pub target: i32,
}
fn is_idle(b: &Bas, id: i32) -> bool {
    id != -1
        && (b.readyanim == id
            || b.extra_seq_ids
                .as_ref()
                .is_some_and(|ids| ids.contains(&id)))
}
pub fn idle_animation(b: &Bas, random: &mut impl FnMut() -> f64) -> Result<i32, Error> {
    if b.readyanim != -1 {
        return Ok(b.readyanim);
    }
    let Some(ids) = &b.extra_seq_ids else {
        return Ok(-1);
    };
    let weights = b
        .idle_weights
        .as_ref()
        .ok_or(Error::Invalid("idle weights missing"))?;
    let mut pick = (random() * b.idle_weight_total as f64) as i32;
    for (index, &weight) in weights.iter().enumerate() {
        if pick < weight {
            return ids
                .get(index)
                .copied()
                .ok_or(Error::Invalid("idle sequence index"));
        }
        pick = pick.wrapping_sub(weight);
    }
    Err(Error::Invalid("idle weight selection exhausted"))
}
fn set(walk: &mut Walk, id: i32, config: &Config) -> Result<(), Error> {
    let changed = walk.node.id() != id;
    walk.node.set(id, 0, 0, config)?;
    // The final flag changes only when a different, valid sequence is
    // installed.
    if changed && walk.node.sequence.is_some() {
        walk.node.flag = true;
    }
    Ok(())
}
pub fn update(
    walk: &mut Walk,
    b: &Bas,
    c: &Input,
    config: &Config,
    random: &mut impl FnMut() -> f64,
) -> Result<(), Error> {
    let delta = c.desired.wrapping_sub(c.angle) & 16383;
    if c.speed == -1 {
        let turn = if delta == 0 && c.turn_ticks <= 25 {
            None
        } else if c.turn < 0 && b.readyanim_l != -1 {
            Some(b.readyanim_l)
        } else if c.turn > 0 && b.readyanim_r != -1 {
            Some(b.readyanim_r)
        } else {
            None
        };
        if let Some(id) = turn {
            set(walk, id, config)?;
            walk.idle = false;
        } else if !walk.idle || !is_idle(b, walk.node.id()) {
            set(walk, idle_animation(b, random)?, config)?;
            walk.idle = walk.node.sequence.is_some();
        }
        return Ok(());
    }
    let (base, back, left, right, turn_left, turn_right) = if c.speed == 2 && b.runanim != -1 {
        (
            b.runanim,
            b.runanim_b,
            b.runanim_l,
            b.runanim_r,
            b.run_turn_left,
            b.run_turn_right,
        )
    } else if c.speed == 0 && b.crawlanim != -1 {
        (
            b.crawlanim,
            b.crawlanim_b,
            b.crawlanim_l,
            b.crawlanim_r,
            b.crawl_turn_left,
            b.crawl_turn_right,
        )
    } else {
        (
            b.walkanim,
            b.walkanim_b,
            b.walkanim_l,
            b.walkanim_r,
            b.walk_turn_left,
            b.walk_turn_right,
        )
    };
    let id = if c.target != -1 && (delta >= 10240 || delta <= 2048) {
        // Turn-angle table, indexed by the movement routine's direction bits.
        let angle = *[
            -1i32, 8192, 0, -1, 12288, 10240, 14336, -1, 4096, 6144, 2048,
        ]
        .get(c.direction as usize)
        .ok_or(Error::Invalid("walk direction"))?;
        let relative = angle.wrapping_sub(c.angle) & 16383;
        if relative > 2048 && relative <= 6144 && right != -1 {
            right
        } else if (10240..14336).contains(&relative) && left != -1 {
            left
        } else if relative > 6144 && relative < 10240 && back != -1 {
            back
        } else {
            base
        }
    } else if delta == 0 && c.turn_ticks <= 25 {
        base
    } else if c.turn < 0 && turn_left != -1 {
        turn_left
    } else if c.turn > 0 && turn_right != -1 {
        turn_right
    } else {
        base
    };
    set(walk, id, config)?;
    walk.idle = false;
    Ok(())
}
