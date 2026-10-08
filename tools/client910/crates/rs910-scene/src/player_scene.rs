//! Normal player scene insertion. The current server's
//! actor stream supplies players; NPC gameplay and cutscene insertion stay outside
//! this adapter. Scene retains typed temporary identities and tile-list order.
use crate::{
    entities910::Player,
    protocol910::{
        terrain::{self, Terrain},
        Players,
    },
    scene::{Scene, TemporaryEntity},
};
use anyhow::{Context, Result};
pub struct Settings<'a> {
    pub local: usize,
    pub idle_detail: i32,
    pub draw_order: i32,
    pub active_target: i32,
    pub hints: &'a [usize],
    /// Scene state 0 (cutscene): `players` holds the cutscene entities.
    pub cutscene: bool,
}
impl Default for Settings<'_> {
    fn default() -> Self {
        Self {
            local: 0,
            idle_detail: 1,
            draw_order: 0,
            active_target: 0,
            hints: &[],
            cutscene: false,
        }
    }
}
fn centered(e: &Player) -> bool {
    centered_fine(e.size, e.fine_x, e.fine_z)
}
/// The tile-centre test for entity pushes: even sizes sit on a tile corner,
/// odd sizes on a tile centre.
pub fn centered_fine(size: i32, fine_x: f32, fine_z: f32) -> bool {
    let offset = if size & 1 == 0 { 0 } else { 256 };
    (fine_x as i32 & 511) == offset && (fine_z as i32 & 511) == offset
}
/// Entity pushes while in the cutscene scene state: no priorities or entity
/// markers; the tile-centred push runs for levels 0..=3 without its level
/// test, so every tile-centred cutscene entity is added once per level
/// (four scene entries, each drawn), then the off-centre push adds the others
/// once.
pub fn cutscene_pushes(size: i32, fine_x: f32, fine_z: f32) -> usize {
    if centered_fine(size, fine_x, fine_z) {
        4
    } else {
        1
    }
}
fn footprint(e: &Player, margin: i32) -> [i32; 4] {
    let r = e
        .size
        .wrapping_sub(1)
        .wrapping_mul(256)
        .wrapping_add(margin);
    let (x, z) = (e.fine_x as i32, e.fine_z as i32);
    [
        x.wrapping_sub(r) >> 9,
        x.wrapping_add(r) >> 9,
        z.wrapping_sub(r) >> 9,
        z.wrapping_add(r) >> 9,
    ]
}
fn append(
    scene: &mut Scene,
    terrain: Option<&Terrain>,
    e: &mut Player,
    id: usize,
    cycle: i32,
) -> Result<()> {
    if (e.forced[6] <= cycle && e.forced[7] < cycle) || e.forced[5] == e.forced[4] {
        e.motion.y = terrain::height(terrain, e.fine_x as i32, e.fine_z as i32, e.level)
            .map_err(|e| anyhow::anyhow!("player scene height: {e:?}"))?
            as f32;
    }
    e.actor.scene.bounds = crate::player_body::tile_bounds(e);
    let s = &e.actor.scene;
    scene.add_temporary(TemporaryEntity {
        player: id,
        npc_index: None,
        location_key: None,
        pick: None,
        transient: false,
        level: e.level,
        occlude_level: e.occlude_level,
        position: [e.fine_x, e.motion.y, e.fine_z],
        bounds: s.bounds,
        overlay_height: if s.min_y == -32768 { 0 } else { s.min_y },
        transparent: s.transparent,
        spot_shadow: false,
        model: None,
    });
    Ok(())
}
/// Entity pushes for a normal player scene. The count array deliberately
/// persists across planes; only maximum-priority markers are cleared per plane.
pub fn insert(
    players: &mut Players,
    scene: &mut Scene,
    terrain: Option<&Terrain>,
    cycle: i32,
    c: &Settings,
) -> Result<()> {
    scene.clear_temporary();
    if c.cutscene {
        // Cutscene entities keep the draw priority they were assigned, but the
        // cutscene scene state never reads it.
        for _level in 0..=3 {
            for &id in &players.high_indices {
                let e = players.players[id]
                    .as_mut()
                    .context("cutscene player missing")?;
                if centered(e) {
                    e.actor.scene.deferred = false;
                    append(scene, terrain, e, id, cycle)?;
                }
            }
        }
        for &id in &players.high_indices {
            let e = players.players[id]
                .as_mut()
                .context("cutscene player missing")?;
            if !centered(e) {
                append(scene, terrain, e, id, cycle)?;
            }
        }
        return Ok(());
    }
    let crowded = (c.idle_detail == 1 && players.high_indices.len() > 200)
        || (c.idle_detail == 0 && players.high_indices.len() > 50);
    for &id in &players.high_indices {
        let e = players.players[id]
            .as_mut()
            .context("high-resolution player missing")?;
        let bounds = crate::player_body::tile_bounds(e);
        let s = &mut e.actor.scene;
        if e.appearance.model.is_none() || e.appearance.visibility == Some(1) {
            s.priority = -1;
            continue;
        }
        s.bounds = bounds;
        if bounds[0] < 0
            || bounds[2] < 0
            || bounds[1] >= scene.max_x as i32
            || bounds[3] >= scene.max_z as i32
        {
            s.priority = -1;
            continue;
        }
        s.use_idle = e.actor.walk.idle && crowded;
        if id == c.local {
            s.priority = i32::MAX;
            continue;
        }
        let n = i32::from(!s.deferred)
            + if e.combat.as_ref().is_some_and(|c| !c.bars.is_empty()) {
                2
            } else {
                0
            };
        let mut priority = n + (5 - e.size).wrapping_shl(2);
        priority += if e.partner == 0 && !e.suppress_partner {
            if c.draw_order == 0 {
                32 + 256
            } else {
                128 + 256
            }
        } else {
            512
        };
        if -(id as i32) - 1 == c.active_target {
            priority += 2047;
        }
        s.priority = priority + 1;
    }
    for &id in c.hints {
        if id != c.local {
            if let Some(e) = players.players.get_mut(id).and_then(Option::as_mut) {
                if e.actor.scene.priority >= 0 {
                    e.actor.scene.priority = e.actor.scene.priority.wrapping_add(2048);
                }
            }
        }
    }
    let mut maximum = vec![0i32; scene.max_x * scene.max_z];
    let mut count = maximum.clone();
    let stride = scene.max_z;
    for plane in 0..4 {
        maximum.fill(0);
        for &id in &players.high_indices {
            let e = players.players[id].as_ref().unwrap();
            let s = &e.actor.scene;
            if e.level != plane || s.priority < 0 || s.force_show || !centered(e) {
                continue;
            }
            let b = if e.size == 1 {
                [
                    (e.fine_x as i32) >> 9,
                    (e.fine_x as i32) >> 9,
                    (e.fine_z as i32) >> 9,
                    (e.fine_z as i32) >> 9,
                ]
            } else {
                footprint(e, 60)
            };
            for x in b[0]..=b[1] {
                for z in b[2]..=b[3] {
                    let i = x as usize * stride + z as usize;
                    if s.priority > maximum[i] {
                        maximum[i] = s.priority;
                        count[i] = 1;
                    } else if s.priority == maximum[i] {
                        count[i] = count[i].wrapping_add(1);
                    }
                }
            }
        }
        for &id in &players.high_indices {
            let e = players.players[id].as_mut().unwrap();
            if e.level != plane {
                continue;
            }
            if e.actor.scene.priority < 0 || !centered(e) {
                e.actor.scene.deferred = false;
                continue;
            }
            let p = e.actor.scene.priority;
            if !e.actor.scene.force_show {
                if e.size == 1 {
                    let i = (e.fine_x as usize >> 9) * stride + (e.fine_z as usize >> 9);
                    if p != maximum[i] {
                        e.actor.scene.deferred = true;
                        continue;
                    }
                    if count[i] > 1 {
                        count[i] -= 1;
                        e.actor.scene.deferred = true;
                        continue;
                    }
                } else {
                    let b = footprint(e, 252);
                    let mut available = false;
                    for x in b[0]..=b[1] {
                        for z in b[2]..=b[3] {
                            let i = x as usize * stride + z as usize;
                            available |= maximum[i] == p && count[i] <= 1;
                        }
                    }
                    if !available {
                        for x in b[0]..=b[1] {
                            for z in b[2]..=b[3] {
                                let i = x as usize * stride + z as usize;
                                if maximum[i] == p {
                                    count[i] -= 1;
                                }
                            }
                        }
                        e.actor.scene.deferred = true;
                        continue;
                    }
                }
            }
            e.actor.scene.deferred = false;
            append(scene, terrain, e, id, cycle)?;
        }
    }
    for &id in &players.high_indices {
        let e = players.players[id].as_mut().unwrap();
        if e.actor.scene.priority >= 0 && !centered(e) {
            append(scene, terrain, e, id, cycle)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(x: f32, z: f32, priority: i32, deferred: bool) -> Player {
        let mut e = Player {
            size: 1,
            fine_x: x,
            fine_z: z,
            ..Player::default()
        };
        e.actor.scene.priority = priority;
        e.actor.scene.deferred = deferred;
        e
    }

    /// While a cutscene draws, entity placement never reads the draw
    /// priority: a tile-centred actor enters the scene once per level (four
    /// entries) whatever its priority (a negative one, a deferred one), an
    /// actor between tiles enters once, and a deferred flag left by the last
    /// ordinary frame is cleared.
    #[test]
    fn cutscene_placement_ignores_priority_and_pushes_once_per_level() {
        let mut players = Players {
            high_indices: vec![1, 2, 3],
            ..Default::default()
        };
        players.players.resize_with(4, || None);
        players.players[1] = Some(actor(6.0 * 512.0 + 256.0, 6.0 * 512.0 + 256.0, 40, false));
        players.players[2] = Some(actor(7.0 * 512.0 + 256.0, 6.0 * 512.0 + 256.0, -1, true));
        players.players[3] = Some(actor(8.0 * 512.0 + 61.0, 6.0 * 512.0 + 256.0, 900, false));
        let mut scene = Scene::new(9, 4, 32, 32);
        scene.reset();
        let settings = Settings {
            cutscene: true,
            ..Settings::default()
        };
        insert(&mut players, &mut scene, None, 0, &settings).unwrap();
        // The tile-centred actors once per level, level by level, then the
        // actor between tiles.
        let owners: Vec<usize> = scene.temporary.iter().map(|t| t.player).collect();
        assert_eq!(owners, [1, 2, 1, 2, 1, 2, 1, 2, 3]);
        assert_eq!(
            cutscene_pushes(1, 6.0 * 512.0 + 256.0, 6.0 * 512.0 + 256.0),
            4
        );
        assert_eq!(
            cutscene_pushes(1, 8.0 * 512.0 + 61.0, 6.0 * 512.0 + 256.0),
            1
        );
        let deferred = players.players[2].as_ref().unwrap().actor.scene.deferred;
        assert!(!deferred, "the deferred flag is cleared");
        // Priorities stay as assigned.
        let priorities: Vec<i32> = (1..=3)
            .map(|i| players.players[i].as_ref().unwrap().actor.scene.priority)
            .collect();
        assert_eq!(priorities, [40, -1, 900]);
    }
}
