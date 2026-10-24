//! NPC customisation masks. The head branch constructs but does
//! not retain its customisation object; its counter and reads are preserved.
use super::{Packet, Result};
use crate::entities910::npc_custom::Custom;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::npc_custom::*;
pub(super) fn read(
    p: &mut Packet,
    spec: &Spec,
    counter: &mut i32,
    head: bool,
) -> Result<Option<Custom>> {
    let flags = if head {
        p.byte()?
    } else {
        p.byte()?.wrapping_neg() & 255
    };
    if flags & 1 != 0 {
        return Ok(None);
    }
    p.g2()?;
    let mut models = None;
    let mut scales = None;
    let mut rotations = None;
    let mut offsets = None;
    if flags & 2 != 0 {
        let n = if head {
            p.byte()?
        } else {
            p.byte()?.wrapping_neg() & 255
        } as usize;
        let mut ids = vec![0; n];
        if !head && flags & 16 != 0 {
            scales = Some(vec![0u32; n]);
            rotations = Some(vec![[0i32; 3]; n]);
            offsets = Some(vec![[0i32; 3]; n]);
        }
        for (i, id) in ids.iter_mut().enumerate() {
            *id = p.smart2()?;
            if !head && flags & 16 != 0 && *id != -1 {
                scales.as_mut().unwrap()[i] = (p.g2()? as u32) << 16 | p.g2()? as u32;
                for v in &mut rotations.as_mut().unwrap()[i] {
                    *v = p.le2()? as i16 as i32
                }
                for v in &mut offsets.as_mut().unwrap()[i] {
                    *v = p.g2()? as i16 as i32
                }
            }
        }
        models = Some(ids);
    }
    let nc = if head {
        spec.retexture.or(spec.recolour).unwrap_or(0)
    } else {
        spec.recolour.unwrap_or(0)
    };
    let nt = if head { 0 } else { spec.retexture.unwrap_or(0) };
    let mut colours = None;
    let mut textures = None;
    if flags & 4 != 0 {
        let mut a = vec![];
        for _ in 0..nc {
            a.push(if head { p.le2()? } else { p.g2()? } as i16)
        }
        colours = Some(a);
    }
    if flags & 8 != 0 {
        let mut a = vec![];
        for _ in 0..nt {
            a.push(p.g2()? as i16)
        }
        textures = Some(a);
    }
    let salt = *counter as i64;
    *counter = counter.wrapping_add(1);
    Ok(Some(Custom {
        salt,
        models,
        scale_bits: scales,
        rotation: rotations,
        offset: offsets,
        colours,
        textures,
    }))
}
