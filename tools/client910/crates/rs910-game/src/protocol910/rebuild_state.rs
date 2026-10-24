//! CPU REBUILD_NORMAL transaction. The login initializes players BEFORE the
//! rebuild tail is read. No Scene/GPU types. Missing loc or map-index context is an error, not a default.
//! TODO(#gap-G-rebuild-effects): apply returned invalidation/offset requests in
//! camera, sound, particles, minimap and map-loading adapters before acknowledgment.
use super::{appearance, live, Context, Error, Packet, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Region,
    Cutscene,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    pub base_x: i32,
    pub base_z: i32,
    pub region_x: i32,
    pub region_z: i32,
    pub width: i32,
    pub height: i32,
    pub area: Option<i32>,
    pub last_kind: Kind,
    pub npc_bits: usize,
    /// Allocated arrays retain trailing zero entries when fewer groups exist.
    pub map_squares: Vec<i32>,
    pub groups: Vec<i32>,
    pub group_count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneBounds {
    pub width: i32,
    pub height: i32,
    pub level_tiles: bool,
}
pub struct Config<'a> {
    pub prior: &'a World,
    pub old_map: &'a Context,
    pub appearance: Option<&'a appearance::Config>,
    /// Explicit login framing, never guessed from surplus payload length.
    pub login: bool,
    /// Whether a group holds a LAND file: membership from the real archive index.
    pub land_groups: &'a BTreeSet<i32>,
    pub loc_sizes: &'a BTreeMap<i32, (i32, i32)>,
    pub scene: Option<SceneBounds>,
}
/// Destination chunk templates from a region rebuild.
///
/// Entries are laid out in plane, destination chunk X, then
/// destination chunk Z.  A negative entry is an empty destination chunk;
/// otherwise the 26-bit template carries source plane/chunk and rotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionLayout {
    pub chunks_x: usize,
    pub chunks_z: usize,
    pub templates: Vec<i32>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub rebased: bool,
    /// The rebase mode; selects the external camera-offset behavior.
    pub mode: i32,
    pub delta_x: i32,
    pub delta_z: i32,
    pub resized: bool,
    pub cleared_npcs: bool,
    pub removed_npcs: Vec<usize>,
    pub remove_scene_stacks: Vec<(i32, i32, i32)>,
    /// When rebased: clear map spots/projectiles/text-coordinates/particles,
    /// positional loc sounds/menu/minimap; offset hint arrows and camera.
    /// No acknowledgment is implied by completion of these CPU calculations.
    pub reset_environment_fade: bool,
    /// Instanced-region source templates, consumed by the scene builder.
    pub region: Option<RegionLayout>,
}
#[derive(Clone, Debug)]
pub struct Decoded {
    pub state: live::State,
    pub world: World,
    pub effects: Effects,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub bytes: usize,
    pub initial_bits: Option<usize>,
}
/// Build area edge in tiles by area id (including size 256, not a four-size approximation).
pub fn area_size(id: i32) -> Result<i32> {
    [104, 120, 136, 168, 72, 256]
        .get(id as usize)
        .copied()
        .ok_or(Error::Invalid("build area id"))
}
pub fn decode_normal(bytes: &[u8], prior: &live::State, c: &Config) -> Result<Decoded> {
    if (
        c.prior.base_x,
        c.prior.base_z,
        c.prior.width,
        c.prior.height,
    ) != (
        c.old_map.base_x,
        c.old_map.base_z,
        c.old_map.width,
        c.old_map.height,
    ) {
        return Err(Error::Invalid("stale rebuild map context"));
    }
    let mut s = prior.clone();
    let mut at = 0;
    let mut initial_bits = None;
    if c.login {
        // The session state reset that map preparation performs.
        if let Some(varps) = &mut s.varps {
            varps.reset();
        }
        // Map preparation clears actors/world queues but retains the
        // player-info cache/NSN/speed arrays. Initialization overwrites the
        // scan range; local index zero consumes one additional low slot.
        if c.old_map.local >= 2048 {
            return Err(Error::Invalid("local player index"));
        }
        let bits = 30 + 18 * (2047 - usize::from(c.old_map.local != 0));
        let n = bits.div_ceil(8);
        let prefix = bytes.get(..n).ok_or(Error::Truncated {
            bit: bytes.len() * 8,
        })?;
        s.players.players.fill(None);
        let d = super::initialize_cached(prefix, &s.players, c.old_map, c.appearance)?;
        s.players = d.state;
        s.initialized = true;
        at = n;
        initial_bits = Some(d.bit_pos);
        s.npcs.entities.clear();
        s.npcs.slots.clear();
        s.npcs.snapshot.clear();
        s.zones.objects = Default::default();
        s.zones.locations.clear();
        s.zones.customisations.clear();
        s.zones.loc_animations.clear();
        s.zones.transients = Default::default();
    } else if !s.initialized {
        return Err(Error::Invalid("normal rebuild before login"));
    }
    let mut p = Packet::new(&bytes[at..]);
    let rx = p.alt2()?;
    let npc_bits = p.byte()? as usize;
    let count = (128u8.wrapping_sub(p.byte()? as u8)) as usize;
    let area = p.byte()? as i32;
    let rz = p.alt2()?;
    let force = 128u8.wrapping_sub(p.byte()? as u8) == 1;
    if p.pos + at != bytes.len() {
        return Err(Error::Invalid("normal rebuild length"));
    }
    let size = area_size(area)?;
    let mut w = c.prior.clone();
    let mut effects = Effects::default();
    if w.last_kind != Kind::Cutscene {
        if w.last_kind != Kind::Normal {
            s.npcs.entities.clear();
            s.npcs.slots.clear();
            s.npcs.snapshot.clear();
            effects.cleared_npcs = true;
        }
        w.last_kind = Kind::Normal;
    }
    effects.resized = w.area != Some(area);
    if effects.resized {
        w.width = size;
        w.height = size;
        w.area = Some(area);
    }
    w.npc_bits = npc_bits;
    w.groups = vec![0; count];
    w.map_squares = vec![0; count];
    w.group_count = 0;
    for mx in (rx - (w.width >> 4)) / 8..=(rx + (w.width >> 4)) / 8 {
        for mz in (rz - (w.height >> 4)) / 8..=(rz + (w.height >> 4)) / 8 {
            let group = mx | (mz << 7);
            if c.land_groups.contains(&group) {
                if w.group_count >= count {
                    return Err(Error::Invalid("rebuild map array capacity"));
                }
                w.groups[w.group_count] = group;
                w.map_squares[w.group_count] = (mx << 8) + mz;
                w.group_count += 1;
            }
        }
    }
    if force || (w.region_x, w.region_z) != (rx, rz) {
        let bx = (rx - (w.width >> 4)) * 8;
        let bz = (rz - (w.height >> 4)) * 8;
        effects = rebase(
            &mut s,
            &Rebase {
                old_x: w.base_x,
                old_z: w.base_z,
                base_x: bx,
                base_z: bz,
                width: w.width,
                height: w.height,
                mode: 3,
                preserve_outside: false,
                loc_sizes: c.loc_sizes,
                scene: c.scene,
            },
            effects,
        )?;
        w.base_x = bx;
        w.base_z = bz;
        w.region_x = rx;
        w.region_z = rz;
    }
    s.world = Some(w.clone());
    Ok(Decoded {
        state: s,
        world: w,
        effects,
        bytes: bytes.len(),
        initial_bits,
    })
}

/// A reconnect the server resumed in place (login reply 15): the character
/// never left its world, so the client keeps its map, npcs, zones and
/// variables. What it drops is the player list: every player is forgotten and
/// the list starts again from the server's player-positions block (the same
/// block a login opens its first rebuild with), and no npc keeps a target.
/// The retained appearance cache still dresses the local player.
pub fn resume_players(
    bytes: &[u8],
    prior: &live::State,
    c: &Context,
    appearance: Option<&appearance::Config>,
) -> Result<live::State> {
    if c.local >= 2048 {
        return Err(Error::Invalid("local player index"));
    }
    let mut s = prior.clone();
    s.players.players.fill(None);
    s.players = super::initialize_cached(bytes, &s.players, c, appearance)?.state;
    s.initialized = true;
    for npc in s.npcs.entities.values_mut() {
        npc.path.target = -1;
    }
    Ok(s)
}

/// The region rebuild.  The header is byte
/// aligned, followed by four planes of presence bits and 26-bit source
/// templates.  Source map squares are retained in first-seen destination
/// order (a hash-set-backed collection loop), with the real
/// LAND archive membership supplied by the production pack index.
pub fn decode_region(bytes: &[u8], prior: &live::State, c: &Config) -> Result<Decoded> {
    let mut p = Packet::new(bytes);
    let npc_bits = p.byte()? as usize;
    let area = p.byte()? as i32;
    let rebuild_kind = p.byte()? as i32;
    if !(1..=4).contains(&rebuild_kind) {
        return Err(Error::Invalid("region rebuild kind"));
    }
    let region_x = p.g2()?;
    let region_z = p.g2_alt1()?;
    let flags = p.g1_alt1()?;
    let preserve_outside = flags & 1 != 0;
    let size = area_size(area)?;
    let chunks_x = usize::try_from(size / 8).map_err(|_| Error::Invalid("region width"))?;
    let chunks_z = chunks_x;
    p.access_bits();
    let mut templates = Vec::with_capacity(4 * chunks_x * chunks_z);
    let mut source_squares = Vec::new();
    for _level in 0..4 {
        for _cx in 0..chunks_x {
            for _cz in 0..chunks_z {
                let present = p.bits(1)?;
                if present == 0 {
                    templates.push(-1);
                    continue;
                }
                let template = p.bits(26)?;
                templates.push(template);
                let src_x = (template >> 14) & 0x3ff;
                let src_z = (template >> 3) & 0x7ff;
                let packed = ((src_x >> 3) << 8) | (src_z >> 3);
                if !source_squares.contains(&packed) {
                    source_squares.push(packed);
                }
            }
        }
    }
    p.access_bytes();
    if p.pos != bytes.len() {
        return Err(Error::Invalid("region rebuild length"));
    }

    let mut s = prior.clone();
    let mut w = c.prior.clone();
    w.last_kind = Kind::Region;
    w.width = size;
    w.height = size;
    w.area = Some(area);
    w.npc_bits = npc_bits;
    w.region_x = region_x;
    w.region_z = region_z;
    w.groups.clear();
    w.map_squares.clear();
    for packed in source_squares {
        let mx = packed >> 8;
        let mz = packed & 0xff;
        let group = mx | (mz << 7);
        if c.land_groups.contains(&group) {
            w.map_squares.push(packed);
            w.groups.push(group);
        }
    }
    w.group_count = w.groups.len();
    let base_x = (region_x - (size >> 4)) * 8;
    let base_z = (region_z - (size >> 4)) * 8;
    let mut effects = Effects {
        region: Some(RegionLayout {
            chunks_x,
            chunks_z,
            templates,
        }),
        ..Effects::default()
    };
    effects.resized = c.prior.area != Some(area);
    if effects.resized {
        s.npcs.entities.clear();
        s.npcs.slots.clear();
        s.npcs.snapshot.clear();
    }
    effects = rebase(
        &mut s,
        &Rebase {
            old_x: w.base_x,
            old_z: w.base_z,
            base_x,
            base_z,
            width: size,
            height: size,
            mode: 3,
            preserve_outside,
            loc_sizes: c.loc_sizes,
            scene: c.scene,
        },
        effects,
    )?;
    w.base_x = base_x;
    w.base_z = base_z;
    s.world = Some(w.clone());
    Ok(Decoded {
        state: s,
        world: w,
        effects,
        bytes: bytes.len(),
        initial_bits: None,
    })
}
/// The inputs of one rebase of the live entity state from one build origin to
/// the next. `mode == 3` keeps all NPCs; other modes filter fine position and
/// ALL ten waypoints, rebuilding hash-bucket snapshot order. `preserve_outside`
/// is the rebuild type's keep-outside flag, not a renderer preference.
#[derive(Clone, Copy)]
pub struct Rebase<'a> {
    pub old_x: i32,
    pub old_z: i32,
    pub base_x: i32,
    pub base_z: i32,
    pub width: i32,
    pub height: i32,
    pub mode: i32,
    pub preserve_outside: bool,
    pub loc_sizes: &'a BTreeMap<i32, (i32, i32)>,
    pub scene: Option<SceneBounds>,
}

/// Move an actor's route waypoints by a build-origin change.
fn shift_route(p: &mut crate::entities910::Player, dx: i32, dz: i32) {
    for v in &mut p.x {
        *v = v.wrapping_sub(dx);
    }
    for v in &mut p.z {
        *v = v.wrapping_sub(dz);
    }
}

/// Move an actor's fine position by a build-origin change.
fn shift_fine(p: &mut crate::entities910::Player, dx: i32, dz: i32) {
    p.fine_x -= dx.wrapping_mul(512) as f32;
    p.fine_z -= dz.wrapping_mul(512) as f32;
}

/// Carry the NPCs over a build-origin change of `delta` tiles into a build
/// area of `size` tiles. Mode 3 keeps every NPC; the other modes drop the NPCs
/// whose fine position or any waypoint falls outside the new area, rebuild the
/// slot list from the survivors and return the dropped indices.
pub fn rebase_npcs(
    npcs: &mut super::npc::Npcs,
    (dx, dz): (i32, i32),
    (width, height): (i32, i32),
    mode: i32,
) -> Result<Vec<usize>> {
    let mut removed = Vec::new();
    if mode != 3 {
        npcs.slots.clear();
    }
    for id in npcs.snapshot.clone() {
        let n = npcs
            .entities
            .get_mut(&id)
            .ok_or(Error::Invalid("missing rebuild NPC"))?;
        if mode == 3 {
            shift_route(&mut n.path, dx, dz);
            shift_fine(&mut n.path, dx, dz);
            continue;
        }
        shift_fine(&mut n.path, dx, dz);
        let x = n.path.fine_x as i32;
        let z = n.path.fine_z as i32;
        let mut keep = x >= 0
            && x <= width.wrapping_mul(512).wrapping_sub(512)
            && z >= 0
            && z <= height.wrapping_mul(512).wrapping_sub(512);
        if keep {
            shift_route(&mut n.path, dx, dz);
            keep = n
                .path
                .x
                .iter()
                .zip(n.path.z)
                .all(|(&x, z)| x >= 0 && x < width && z >= 0 && z < height);
        }
        if keep {
            npcs.slots.push(id);
        } else {
            removed.push(id);
        }
    }
    for id in &removed {
        npcs.entities.remove(id);
    }
    if !removed.is_empty() {
        npcs.snapshot.retain(|id| npcs.entities.contains_key(id));
        npcs.snapshot.sort_by_key(|id| id & 63);
    }
    Ok(removed)
}

/// The new loc of a loc-change request that removes the loc.
const REMOVAL: i32 = -1;
/// The size of a loc type the config has no record of (`width` and `length` default to 1).
const DEFAULT_LOC_SIZE: (i32, i32) = (1, 1);

fn rebase_inner(s: &mut live::State, r: &Rebase, mut e: Effects) -> Result<Effects> {
    let Rebase {
        old_x,
        old_z,
        base_x,
        base_z,
        width,
        height,
        mode,
        preserve_outside,
        loc_sizes,
        scene,
    } = *r;
    let dx = base_x.wrapping_sub(old_x);
    let dz = base_z.wrapping_sub(old_z);
    e.rebased = true;
    e.mode = mode;
    e.delta_x = dx;
    e.delta_z = dz;
    let shift = |p: &mut crate::entities910::Player| shift_route(p, dx, dz);
    let fine = |p: &mut crate::entities910::Player| shift_fine(p, dx, dz);
    e.removed_npcs
        .extend(rebase_npcs(&mut s.npcs, (dx, dz), (width, height), mode)?);
    for p in s.players.players.iter_mut().flatten() {
        shift(p);
        fine(p);
    }
    for queue in [&mut s.zones.locations, &mut s.zones.customisations] {
        let mut next = vec![];
        for mut r in queue.iter().cloned() {
            r.x = r.x.wrapping_sub(dx);
            r.z = r.z.wrapping_sub(dz);
            // A removal (new loc -1) is measured by the default type the type
            // list builds for an id it has no record of: 1x1.
            let &(mut w, mut h) = if r.id == REMOVAL {
                &DEFAULT_LOC_SIZE
            } else {
                loc_sizes
                    .get(&r.id)
                    .ok_or(Error::UnsupportedContext("rebuild loc dimensions"))?
            };
            if r.angle & 1 != 0 {
                std::mem::swap(&mut w, &mut h);
            }
            if preserve_outside
                || !(r.x.wrapping_add(w) <= 0
                    || r.z.wrapping_add(h) <= 0
                    || r.x >= width
                    || r.z >= height)
            {
                next.push(r);
            }
        }
        *queue = next;
    }
    if let Some(scene) = scene {
        let mut keys = s.zones.objects.key_order.clone();
        keys.sort_by_key(|k| k & 63);
        for key in keys {
            let level = (key >> 28 & 3) as i32;
            let x = ((key & 16383) as i32).wrapping_sub(base_x);
            let z = ((key >> 14 & 16383) as i32).wrapping_sub(base_z);
            if x >= 0 && z >= 0 && x < width && z < height && x < scene.width && z < scene.height {
                if scene.level_tiles {
                    e.remove_scene_stacks.push((level, x, z));
                }
            } else if !preserve_outside {
                s.zones.objects.stacks.remove(&key);
                s.zones.objects.revision = s.zones.objects.revision.wrapping_add(1);
                s.zones.objects.key_order.retain(|k| *k != key);
            }
        }
    }
    s.zones.loc_animations.clear();
    s.zones.transients = Default::default();
    e.reset_environment_fade =
        mode != 3 && (dx.wrapping_abs() > width || dz.wrapping_abs() > height);
    Ok(e)
}

/// Transactional direct rebase entry point for non-normal rebuild modes. Failure
/// leaves all prior entity/queue state intact.
pub fn rebase(s: &mut live::State, r: &Rebase, effects: Effects) -> Result<Effects> {
    let mut next = s.clone();
    let effects = rebase_inner(&mut next, r, effects)?;
    *s = next;
    Ok(effects)
}
