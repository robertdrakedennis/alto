//! Real CPU player appearance update, with explicit config inputs.
//! TODO(#gap-G-effects): caller must apply icon-cache, particle, audio and local-name effects.
use super::{Error, Packet, Result};
use crate::entities910::{
    appearance::{CachedPacket, Customisation, Model},
    Player,
};
#[cfg(test)]
use std::collections::BTreeMap;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::appearance::*;
pub fn apply(cache: &mut CachedPacket, e: &mut Player, c: &Config) -> Result<()> {
    // An appearance change: the held particle system replays
    // its initial burst (applied by the particle owner before its next tick).
    e.actor.particle_resets = e.actor.particle_resets.wrapping_add(1);
    let mut p = Packet::new(&cache.data);
    let flags = p.byte()? as i32;
    let gender = (flags & 1) as i8;
    let old_base = e.appearance.base_size;
    let base = ((flags >> 3) & 7) + 1;
    e.appearance.base_size = base;
    let old_npc = e.appearance.model.as_ref().map_or(-1, |m| m.npc);
    e.size = effective_size(old_npc, base, c)?;
    let delta = e.size.wrapping_sub(old_base).wrapping_shl(8) as f32;
    e.fine_x += delta;
    e.fine_z += delta;
    if flags & 64 != 0 {
        let id = p.smart1()?;
        e.appearance.title_id = id;
        e.appearance.title = Some(
            c.titles
                .get(&(gender, id))
                .unwrap_or(&c.default_titles[gender as usize])
                .clone(),
        );
    } else {
        e.appearance.title_id = -1;
        e.appearance.title = None;
    }
    let visibility = p.byte()? as i8;
    e.appearance.visibility = if c.staff_live_override {
        Some(0)
    } else if (0..=2).contains(&visibility) {
        Some(visibility)
    } else {
        None
    };
    let mut npc = -1;
    let mut team = 0;
    let mut kits = vec![0; c.wear.len()];
    let mut item_ids = vec![None; c.wear.len()];
    let mut custom = vec![None; c.wear.len()];
    for i in 0..c.wear.len() {
        if c.wear[i] == 1 {
            continue;
        }
        let hi = p.byte()? as i32;
        if hi == 0 {
            continue;
        }
        let v = (hi << 8) + p.byte()? as i32;
        if i == 0 && v == 65535 {
            npc = p.smart2()?;
            team = p.byte()? as i32;
            break;
        }
        if v >= 2048 {
            let id = v - 2048;
            kits[i] = id | 0x40000000;
            item_ids[i] = Some(id);
            let t = c
                .items
                .get(&id)
                .ok_or(Error::UnsupportedContext("appearance item config"))?;
            if t.team != 0 {
                team = t.team;
            }
        } else {
            kits[i] = (v - 256) | i32::MIN;
        }
    }
    if npc == -1 {
        let mask = p.g2()?;
        let mut bit = 0;
        for i in 0..c.wear.len() {
            if c.wear[i] != 0 {
                continue;
            }
            if mask & 1i32.wrapping_shl(bit) != 0 {
                let id = item_ids[i].ok_or(Error::Invalid("customisation requires item"))?;
                custom[i] = Some(read_custom(&mut p, &c.items[&id].defaults)?)
            }
            bit += 1;
        }
    }
    let mut colours = [0; 10];
    let mut textures = [0; 10];
    for (out, lens) in [
        (&mut colours, &c.colour_lengths),
        (&mut textures, &c.texture_lengths),
    ] {
        for i in 0..10 {
            let v = p.byte()? as usize;
            out[i] = if v < lens[i] { v as i32 } else { 0 };
        }
    }
    let bas = p.g2()?;
    let mut model = Model {
        bas,
        kits,
        custom,
        colours,
        textures,
        female: gender == 1,
        npc,
        hash: 0,
    };
    model.update_hash();
    e.appearance.bas = bas;
    e.appearance.gender = gender;
    e.appearance.team = team;
    e.appearance.model = Some(model);
    e.size = effective_size(npc, base, c)?;
    if npc != old_npc {
        e.fine_x = e.x[0].wrapping_shl(9).wrapping_add(e.size.wrapping_shl(8)) as f32;
        e.fine_z = e.z[0].wrapping_shl(9).wrapping_add(e.size.wrapping_shl(8)) as f32;
    }
    e.appearance.name = Some(p.string()?);
    e.appearance.combat = p.byte()? as i32;
    if flags & 4 != 0 {
        let skill = p.g2()?;
        e.appearance.skill = if skill == 65535 { -1 } else { skill };
        e.appearance.max_combat = e.appearance.combat;
        e.appearance.wilderness = -1;
    } else {
        e.appearance.skill = 0;
        e.appearance.max_combat = p.byte()? as i32;
        let w = p.byte()?;
        e.appearance.wilderness = if w == 255 { -1 } else { w as i32 };
    }
    e.appearance.sound_range = p.byte()? as i32;
    if e.appearance.sound_range != 0 {
        for v in &mut e.appearance.sound_ids {
            *v = p.g2()?
        }
        e.appearance.sound_volume = p.byte()? as i32;
    }
    cache.consumed = p.pos;
    Ok(())
}

/// Decode the identity-kit body carried by `PLAYER_SNAPSHOT`.
///
/// The snapshot packet supplies only the gender and the bytes consumed by
/// the identity-kit setter; the surrounding player metadata is absent.
/// Keep this decoder separate from [`apply`] so the interface model owner can
/// install the same player model selection without manufacturing a live actor.
pub fn decode_snapshot(data: &[u8], gender: i8, c: &Config) -> Result<Model> {
    let mut p = Packet::new(data);
    let mut kits = vec![0; c.wear.len()];
    let mut item_ids = vec![None; c.wear.len()];
    let mut custom = vec![None; c.wear.len()];
    let mut npc = -1;
    for i in 0..c.wear.len() {
        if c.wear[i] == 1 {
            continue;
        }
        let hi = p.byte()? as i32;
        if hi == 0 {
            continue;
        }
        let v = (hi << 8) + p.byte()? as i32;
        if i == 0 && v == 65535 {
            npc = p.smart2()?;
            let _team = p.byte()?;
            break;
        }
        if v >= 2048 {
            let id = v - 2048;
            kits[i] = id | 0x4000_0000;
            item_ids[i] = Some(id);
        } else {
            kits[i] = (v - 256) | i32::MIN;
        }
    }
    if npc == -1 {
        let mask = p.g2()?;
        let mut bit = 0;
        for i in 0..c.wear.len() {
            if c.wear[i] != 0 {
                continue;
            }
            if mask & 1i32.wrapping_shl(bit) != 0 {
                let id = item_ids[i].ok_or(Error::Invalid("customisation requires item"))?;
                let item = c
                    .items
                    .get(&id)
                    .ok_or(Error::UnsupportedContext("appearance item config"))?;
                custom[i] = Some(read_custom(&mut p, &item.defaults)?);
            }
            bit += 1;
        }
    }
    let mut colours = [0; 10];
    let mut textures = [0; 10];
    for (out, lens) in [
        (&mut colours, &c.colour_lengths),
        (&mut textures, &c.texture_lengths),
    ] {
        for i in 0..10 {
            let v = p.byte()? as usize;
            out[i] = if v < lens[i] { v as i32 } else { 0 };
        }
    }
    let bas = p.g2()?;
    if p.pos != data.len() {
        return Err(Error::Invalid("PLAYER_SNAPSHOT trailing bytes"));
    }
    let mut model = Model {
        bas,
        kits,
        custom,
        colours,
        textures,
        female: gender == 1,
        npc,
        hash: 0,
    };
    model.update_hash();
    Ok(model)
}
fn effective_size(npc: i32, base: i32, c: &Config) -> Result<i32> {
    if npc == -1 {
        Ok(base)
    } else {
        c.npc_sizes
            .get(&npc)
            .copied()
            .ok_or(Error::UnsupportedContext("appearance NPC size"))
    }
}
fn read_custom(p: &mut Packet, defaults: &Customisation) -> Result<Customisation> {
    let mut c = defaults.clone();
    let flags = p.byte()?;
    if flags & 1 != 0 {
        c.man[0] = p.smart2()?;
        c.woman[0] = p.smart2()?;
        for i in 1..3 {
            if defaults.man[i] != -1 || defaults.woman[i] != -1 {
                c.man[i] = p.smart2()?;
                c.woman[i] = p.smart2()?;
            }
        }
    }
    if flags & 2 != 0 {
        c.man_head[0] = p.smart2()?;
        c.woman_head[0] = p.smart2()?;
        if defaults.man_head[1] != -1 || defaults.woman_head[1] != -1 {
            c.man_head[1] = p.smart2()?;
            c.woman_head[1] = p.smart2()?;
        }
    }
    if flags & 4 != 0 {
        let mask = p.g2()?;
        for i in 0..4 {
            let at = (mask >> (i * 4)) & 15;
            if at != 15 {
                let v = p.g2()? as i16;
                *c.recolour
                    .as_mut()
                    .and_then(|a| a.get_mut(at as usize))
                    .ok_or(Error::Invalid("custom recolour index"))? = v;
            }
        }
    }
    if flags & 8 != 0 {
        let mask = p.byte()?;
        for i in 0..2 {
            let at = (mask >> (i * 4)) & 15;
            if at != 15 {
                let v = p.g2()? as i16;
                *c.retexture
                    .as_mut()
                    .and_then(|a| a.get_mut(at as usize))
                    .ok_or(Error::Invalid("custom retexture index"))? = v;
            }
        }
    }
    Ok(c)
}
pub fn head_icons(cache: &mut CachedPacket, e: &mut Player) -> Result<()> {
    let mut p = Packet::new(&cache.data);
    let mask = p.byte()?;
    for i in 0..8 {
        if mask & (1 << i) == 0 {
            e.appearance.head_ids[i] = -1;
            e.appearance.head_groups[i] = -1;
        } else {
            e.appearance.head_ids[i] = p.byte()? as i32;
            e.appearance.head_groups[i] = p.g2()?;
        }
    }
    cache.consumed = p.pos;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_decoder_consumes_identity_kit_tail() {
        let config = Config {
            wear: vec![1; 12],
            colour_lengths: [1; 10],
            texture_lengths: [1; 10],
            items: BTreeMap::new(),
            npc_sizes: BTreeMap::new(),
            titles: BTreeMap::new(),
            default_titles: [String::new(), String::new()],
            staff_live_override: false,
        };
        // No wear slots are active: the customisation mask is still read,
        // ten recolour bytes, ten retexture bytes and the BAS id.
        let mut bytes = vec![0, 0];
        bytes.extend([0; 20]);
        bytes.extend_from_slice(&2699u16.to_be_bytes());
        let model = decode_snapshot(&bytes, 1, &config).unwrap();
        assert_eq!(model.bas, 2699);
        assert!(model.female);
        assert_eq!(model.kits, vec![0; 12]);
        assert!(decode_snapshot(&bytes[..bytes.len() - 1], 1, &config).is_err());
    }
}
