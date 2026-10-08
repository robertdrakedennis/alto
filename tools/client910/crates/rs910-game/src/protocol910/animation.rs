//! Animation update masks: wire order and byte variants for players and NPCs.
//! Playback is deliberately excluded.
use super::{Error, Packet, Result};
use crate::entities910::{
    animation_state::{Config, SpotRequest},
    Player,
};
pub(super) fn modes(p: &mut Packet, e: &mut Player, c: Option<&Config>, npc: bool) -> Result<()> {
    let c = c.ok_or(Error::UnsupportedContext("animation config"))?;
    let mut modes = vec![];
    for _ in 0..4 {
        modes.push(p.smart2()?)
    }
    let delay = (p.byte()?.wrapping_neg() & 255) as i32;
    e.animation
        .select_modes(modes, delay, npc, e.route_length, &mut e.steps_remaining, c)
}
pub(super) fn overlays(
    p: &mut Packet,
    e: &mut Player,
    c: Option<&Config>,
    npc: bool,
) -> Result<()> {
    let c = c.ok_or(Error::UnsupportedContext("animation config"))?;
    let n = 128u32.wrapping_sub(p.byte()?) & 255;
    for _ in 0..n {
        let id = p.smart2()?;
        let delay = if npc {
            p.byte()?
        } else {
            p.byte()?.wrapping_sub(128) & 255
        } as i32;
        let mask = if npc { p.alt2()? } else { p.alt3()? };
        e.animation.overlay(id, delay, mask, c)?;
    }
    Ok(())
}
pub(super) fn spot(
    p: &mut Packet,
    e: &mut Player,
    c: Option<&Config>,
    index: usize,
    id_mode: u8,
    param_mode: u8,
    flag_mode: u8,
) -> Result<()> {
    let c = c.ok_or(Error::UnsupportedContext("animation config"))?;
    let id = match id_mode {
        0 => p.g2()?,
        1 => p.le2()?,
        _ => p.alt3()?,
    };
    let id = if id == 65535 { -1 } else { id };
    let b = [p.byte()?, p.byte()?, p.byte()?, p.byte()?];
    let order = match param_mode {
        0 => [0, 1, 2, 3],
        1 => [3, 2, 1, 0],
        2 => [2, 3, 0, 1],
        _ => [1, 0, 3, 2],
    };
    let mut v = 0u32;
    for i in order {
        v = (v << 8) | b[i]
    }
    let f = match flag_mode {
        0 => p.byte()?,
        1 => p.byte()?.wrapping_sub(128) & 255,
        2 => p.byte()?.wrapping_neg() & 255,
        _ => 128u32.wrapping_sub(p.byte()?) & 255,
    };
    let delay = ((f >> 3) & 15) as i32;
    e.animation.spot(
        index,
        SpotRequest {
            id,
            param: v as i32,
            orientation: (f & 7) as i32,
            delay: if delay == 15 { -1 } else { delay },
            flag: f & 128 != 0,
        },
        c,
    )
}
