//! Pure field masks of the player and NPC info packets.
use super::{Packet, Result};
use crate::entities910::Player;
/// start x/z, end x/z, start/end plane, approach-end cycle, movement-end cycle, angle.
pub(super) fn force(p: &mut Packet, e: &mut Player, cycle: i32, npc: bool) -> Result<()> {
    let kinds = if npc {
        [2, 1, 1, 2, 3, 3]
    } else {
        [2, 0, 2, 1, 1, 0]
    };
    for (i, k) in kinds.into_iter().enumerate() {
        e.forced[i] = signed(p, k)?;
    }
    e.forced[6] = (if npc { p.g2()? } else { p.alt3()? }).wrapping_add(cycle);
    e.forced[7] = p.le2()?.wrapping_add(cycle);
    e.forced[8] = p.le2()?;
    e.route_length = 1;
    e.steps_remaining = 0;
    for i in [0, 2] {
        e.forced[i] = e.forced[i].wrapping_add(e.x[0]);
        e.forced[i + 1] = e.forced[i + 1].wrapping_add(e.z[0]);
    }
    e.forced[4] = e.forced[4].wrapping_add(e.level);
    e.forced[5] = e.forced[5].wrapping_add(e.level);
    Ok(())
}
pub(super) fn tint(p: &mut Packet, e: &mut Player, cycle: i32, npc: bool) -> Result<()> {
    let kinds = if npc { [2, 1, 0] } else { [2, 3, 2] };
    for (i, k) in kinds.into_iter().enumerate() {
        e.tint[i] = signed(p, k)?;
    }
    e.tint[3] = if npc {
        p.byte()? as i8 as i32
    } else {
        p.byte()?.wrapping_sub(128) as i8 as i32
    };
    e.tint[4] = cycle.wrapping_add(p.g2()?);
    e.tint[5] = cycle.wrapping_add(if npc { p.le2()? } else { p.g2()? });
    Ok(())
}
fn signed(p: &mut Packet, k: u8) -> Result<i32> {
    let v = p.byte()? as u8;
    Ok(match k {
        0 => v,
        1 => v.wrapping_sub(128),
        2 => v.wrapping_neg(),
        _ => 128u8.wrapping_sub(v),
    } as i8 as i32)
}
pub(super) fn wear(p: &mut Packet, e: &mut Player, slots: usize, npc: bool) -> Result<()> {
    let count = if npc {
        128u32.wrapping_sub(p.byte()?) & 255
    } else {
        p.byte()?.wrapping_sub(128) & 255
    };
    let mut map = vec![-1; slots];
    for _ in 0..count {
        let mut id = if npc { p.alt2()? } else { p.le2()? };
        if id & 0xc000 == 0xc000 {
            id = (id << 16) | if npc { p.alt3()? } else { p.le2()? };
        }
        let mut bits = if npc { p.g2()? } else { p.alt2()? };
        for v in &mut map {
            if bits & 1 != 0 {
                *v = id
            }
            bits >>= 1;
        }
    }
    e.wear = Some(map);
    Ok(())
}
