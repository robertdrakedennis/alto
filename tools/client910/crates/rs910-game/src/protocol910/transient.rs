//! Projectile, spot-animation and other transient zone packets.
use super::{Context, Error, Packet, Result};
use crate::entities910::{
    animation_state::{Config as AnimConfig, Node},
    transient::*,
};
use std::collections::BTreeMap;
pub struct Config<'a> {
    pub cycle: i32,
    pub cutscene: bool,
    pub animation: &'a AnimConfig,
    pub height: &'a dyn Fn(i32, i32, i32) -> i32,
    pub attachment_y: Option<&'a BTreeMap<(i32, i32), i32>>,
}

/// The `PROJANIM_SPECIFIC` packet. This uses the distinct
/// 23-byte coordinate/entity form and feeds the same retained projectile
/// physics state as the zone projectile forms.
pub fn decode_specific(
    bytes: &[u8],
    prior: &Transients,
    map: &Context,
    c: &Config,
) -> Result<Transients> {
    let mut p = Packet::new(bytes);
    let start_x_raw = p.g2_alt1()?;
    let delta_x = p.g1b_alt2()?;
    let start_delay = p.g2_alt3()?;
    let end_delay = p.g2()?;
    let arc = p.g2_alt2()? * 4;
    let pitch = p.g1_alt2()?;
    let source = p.g2s_alt1()?;
    let flags = p.g1_alt3()?;
    let targeted = p.byte()? as i32;
    let target = p.g2s_alt1()?;
    let start_z_raw = p.g2()?;
    let delta_z = p.g1b_alt3()?;
    let end_offset = p.g1_alt1()? * 4;
    let start_height = p.byte()? as i32;
    let effect = p.g2_alt1()?;
    if p.pos != bytes.len() {
        return Err(Error::Invalid("PROJANIM_SPECIFIC length"));
    }
    let mut s = prior.clone();
    let start_x = start_x_raw - map.base_x * 2;
    let start_z = start_z_raw - map.base_z * 2;
    let end_x = start_x + delta_x;
    let end_z = start_z + delta_z;
    let valid = start_x >= 0
        && start_z >= 0
        && end_x >= 0
        && end_z >= 0
        && start_x < map.width * 2
        && start_z < map.height * 2
        && end_x < map.width * 2
        && end_z < map.height * 2;
    if !c.cutscene && valid && effect != 65535 {
        let slot = if flags & 2 != 0 { flags >> 2 } else { -1 };
        let mut offset_start = if flags & 2 != 0 {
            (start_height as i8 as i32) * 4
        } else {
            start_height * 4
        };
        if source != 0 && slot != -1 {
            offset_start = offset_start.wrapping_sub(
                *c.attachment_y
                    .ok_or(Error::UnsupportedContext("projectile attachment context"))?
                    .get(&(source, slot))
                    .unwrap_or(&0),
            );
        }
        let t = c
            .animation
            .effects
            .get(&effect)
            .ok_or(Error::UnsupportedContext("projectile effect"))?;
        let mut node = Node::default();
        node.set(t.sequence, 0, 0, c.animation)?;
        let cycle_start = c.cycle.wrapping_add(start_delay);
        let mut projectile = Projectile {
            effect,
            level: 0,
            offset_start,
            offset_end: end_offset * 4,
            start: cycle_start,
            end: c.cycle.wrapping_add(end_delay),
            pitch: if pitch == 255 { -1 } else { pitch },
            arc,
            source,
            target,
            slot,
            targeted,
            follow_ground: flags & 1 != 0,
            mobile: false,
            position: [
                (start_x * 256) as f32,
                (c.height)(start_x * 256, start_z * 256, 0).wrapping_sub(offset_start) as f32,
                (start_z * 256) as f32,
            ],
            rotation: [0., 0., 0., 1.],
            vx: 0.,
            vz: 0.,
            speed: 0.,
            vy: 0.,
            ay: 0.,
            animation: node,
        };
        projectile.velocity(
            end_x * 256,
            end_z * 256,
            (c.height)(end_x * 256, end_z * 256, 0).wrapping_sub(projectile.offset_end),
            cycle_start,
            c.height,
        );
        s.projectiles.push(projectile);
    }
    Ok(s)
}

pub fn decode(
    bytes: &[u8],
    prior: &Transients,
    map: &Context,
    base: (i32, i32, i32),
    op: u8,
    c: &Config,
) -> Result<Transients> {
    let mut p = Packet::new(bytes);
    let mut s = prior.clone();
    let (bx, bz, level) = base;
    if op == 1 {
        let coord = p.byte()? as i32;
        let x = bx + ((coord >> 4) & 7);
        let z = bz + (coord & 7);
        let id = p.g2()?;
        let offset = p.g2()?;
        let delay = p.g2()?;
        let orientation = p.byte()? as i32;
        let targeted = p.g2()? as i16 as i32;
        if !c.cutscene && x >= 0 && z >= 0 && x < map.width && z < map.height {
            let key = ((x << 16) | z) as i64;
            if id == 65535 {
                if let Some(i) = s.spots.iter().position(|s| s.key == key) {
                    s.spots.remove(i);
                }
            } else {
                let t = c
                    .animation
                    .effects
                    .get(&id)
                    .ok_or(Error::UnsupportedContext("map spot effect"))?;
                let animation = if t.sequence == -1 {
                    None
                } else {
                    let mut node = Node::default();
                    node.set(
                        t.sequence,
                        delay,
                        if t.looping { 0 } else { 2 },
                        c.animation,
                    )?;
                    Some(node)
                };
                let fx = x * 512 + 256;
                let fz = z * 512 + 256;
                s.spots.push(Spot {
                    key,
                    effect: id,
                    level,
                    occlude: level + if level < 3 { map.bridge(x, z) } else { 0 },
                    position: [
                        fx as f32,
                        (c.height)(fx, fz, level).wrapping_sub(offset) as f32,
                        fz as f32,
                    ],
                    orientation,
                    targeted,
                    animation,
                });
            }
        }
    } else if op == 7 || op == 12 {
        let coord = p.byte()? as i32;
        let (
            sx,
            sz,
            ex,
            ez,
            follow,
            slot,
            source,
            target,
            id,
            mut offset_start,
            offset_end,
            start,
            end,
            pitch,
            arc,
            targeted,
            valid,
        ) = if op == 7 {
            let sx = bx + ((coord >> 3) & 7);
            let sz = bz + (coord & 7);
            let ex = sx + p.byte()? as i8 as i32;
            let ez = sz + p.byte()? as i8 as i32;
            let target = p.g2()? as i16 as i32;
            let id = p.g2()?;
            let start_h = p.byte()? as i32 * 16;
            let end_h = p.byte()? as i32 * 16;
            let start = p.g2()?;
            let end = p.g2()?;
            let pitch = p.byte()? as i32;
            let arc = p.g2()? * 4;
            let targeted = p.g2()? as i16 as i32;
            (
                sx * 512 + 256,
                sz * 512 + 256,
                ex * 512 + 256,
                ez * 512 + 256,
                coord & 128 != 0,
                -1,
                0,
                target,
                id,
                start_h,
                end_h,
                start,
                end,
                pitch,
                arc,
                targeted,
                sx >= 0
                    && sz >= 0
                    && ex >= 0
                    && ez >= 0
                    && sx < map.width
                    && sz < map.height
                    && ex < map.width
                    && ez < map.height,
            )
        } else {
            let sx = bx * 2 + ((coord >> 4) & 15);
            let sz = bz * 2 + (coord & 15);
            let flags = p.byte()? as i32;
            let slot = if flags & 2 != 0 { flags >> 2 } else { -1 };
            let ex = sx + p.byte()? as i8 as i32;
            let ez = sz + p.byte()? as i8 as i32;
            let source = p.g2()? as i16 as i32;
            let target = p.g2()? as i16 as i32;
            let id = p.g2()?;
            let h = p.byte()? as i32;
            let h = if flags & 2 != 0 {
                (h as i8 as i32) * 4
            } else {
                h * 16
            };
            let end_h = p.byte()? as i32 * 16;
            let start = p.g2()?;
            let end = p.g2()?;
            let pitch = p.byte()? as i32;
            let arc = p.g2()? * 4;
            let targeted = p.g2()? as i16 as i32;
            (
                sx * 256,
                sz * 256,
                ex * 256,
                ez * 256,
                flags & 1 != 0,
                slot,
                source,
                target,
                id,
                h,
                end_h,
                start,
                end,
                pitch,
                arc,
                targeted,
                sx >= 0
                    && sz >= 0
                    && ex >= 0
                    && ez >= 0
                    && sx < map.width * 2
                    && sz < map.width * 2
                    && ex < map.height * 2
                    && ez < map.height * 2,
            )
        };
        if !c.cutscene && valid && id != 65535 {
            if source != 0 && slot != -1 {
                offset_start = offset_start.wrapping_sub(
                    *c.attachment_y
                        .ok_or(Error::UnsupportedContext("projectile attachment context"))?
                        .get(&(source, slot))
                        .unwrap_or(&0),
                );
            }
            let t = c
                .animation
                .effects
                .get(&id)
                .ok_or(Error::UnsupportedContext("projectile effect"))?;
            let mut node = Node::default();
            node.set(t.sequence, 0, 0, c.animation)?;
            let mut e = Projectile {
                effect: id,
                level,
                offset_start,
                offset_end,
                start: c.cycle.wrapping_add(start),
                end: c.cycle.wrapping_add(end),
                pitch: if pitch == 255 { -1 } else { pitch },
                arc,
                source,
                target,
                slot,
                targeted,
                follow_ground: follow,
                mobile: false,
                position: [
                    sx as f32,
                    (c.height)(sx, sz, level).wrapping_sub(offset_start) as f32,
                    sz as f32,
                ],
                rotation: [0., 0., 0., 1.],
                vx: 0.,
                vz: 0.,
                speed: 0.,
                vy: 0.,
                ay: 0.,
                animation: node,
            };
            e.velocity(
                ex,
                ez,
                (c.height)(ex, ez, level).wrapping_sub(offset_end),
                c.cycle.wrapping_add(start),
                c.height,
            );
            s.projectiles.push(e);
        }
    } else {
        return Err(Error::UnsupportedZone(op));
    }
    if p.pos != bytes.len() {
        return Err(Error::Invalid("transient packet length"));
    }
    Ok(s)
}
