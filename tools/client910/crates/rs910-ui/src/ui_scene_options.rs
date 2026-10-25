//! The scene viewport's ground pick and the menu-action packets built from
//! it: the "Walk here" entry (unproject the mouse through the inverted
//! view-projection matrix, raycast the heightmap), the 4x4 matrix helpers
//! that needs, the fine terrain height, and the byte layout of the walk and
//! operation packets.
//!
//! NPCs and players use the retained runtime's scene pick frame; ground-object
//! stacks and installed loc snapshots use the retained zone and scene paths in
//! `ui_runtime`, whose `use_menu_option` owns every menu action branch. The
//! developer teleport uses the existing client-cheat writer; ground face-here
//! uses server menu state and action 60.
//!
//! Operation packets:
//! - `build_opnpc` feeds the retained dispatcher for actions 9-13/1003 ->
//!   `OPNPC1`..`OPNPC6`. The retained runtime resolves rendered NPC body hits,
//!   applies the server op mask and emits ordinary operation/target entries.
//!   The server-controlled NPC attack-priority policy, `multinpc` selection,
//!   the default Examine slot and the cursor order (`defaultMenuCursor`,
//!   `getCursor`, `cursorattack`) follow the NPC menu-entry rules.
//! - `encode_loc_id` packs a loc reference and `build_oploc` owns actions
//!   3-6/1001 -> `OPLOC1`..`OPLOC5`. The ordinary menu consumes captured scene
//!   loc snapshots, emits the installed loc operations (including the default
//!   Examine slot -> OPLOC6) and target action 2 with multiloc/param
//!   resolution and per-loc cursors.
//! - `build_opobj` feeds actions 18/19/20/21/22/1004 -> `OPOBJ1`..`OPOBJ6`.
//!   Ground-stack menu construction reads the retained object-stack state at
//!   the picked tile, emits operation order/colours/cursors, and supports
//!   target action 17.
//! - The on-target builders: `build_opobjt` for action 17 -> `OPOBJT`,
//!   `build_opnpct` action 8 -> `OPNPCT`, `build_oploct` action 2 -> `OPLOCT`,
//!   `build_apcoordt` action 59 -> `APCOORDT`. Target selection state (the
//!   active component fields) stays partial: no picker is built here, only
//!   the dispatcher and the wire bytes. `IF_BUTTONT` (action 58) and
//!   `OPPLAYERT` (actions 15/16) live in `ui_player_options.rs`; their live
//!   `IF_BUTTONT` dispatch in `ui_interaction.rs` is untouched.
use crate::ui_cam2::Heightmap;
use rs910_core::terrain::{blend_corners, height_level, TILE_SHIFT};

/// Ground walking in the native minimenu action contract.
pub const WALK_ACTION: i32 = 23;

/// Installed location identity consumed by the cache menu builder. It carries
/// no claim that a renderer hit or screen capsule exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocTarget {
    pub id: i32,
    pub shape: i32,
    pub angle: i32,
    pub tile: [i32; 2],
}

/// One menu option queued for the scene viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneOption {
    pub detail: Option<String>,
    pub op: String,
    pub target: Option<String>,
    pub cursor: i32,
    pub action: i32,
    pub obj_id: i32,
    pub entity_id: i64,
    pub tile: [i32; 2],
    pub enabled: bool,
    pub has_arrow: bool,
    pub sub_id: i64,
    pub force_submenu: bool,
}

/// The view and projection of the last scene draw (unflipped, scene-local fine
/// units), kept for the scene pick. The scene owner publishes them after each
/// scene draw that was not blacked out, so the pick follows whichever camera
/// drew the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnView {
    pub view: [f32; 16],
    pub projection: [f32; 16],
    /// The view depth (fine units) the scene was drawn to past the
    /// projection's own far plane: the modern far scene's far plane while it
    /// draws. The ground pick reaches it; `None` keeps the projection's.
    pub far_plane: Option<f32>,
}

impl DrawnView {
    /// The matrices a pick frame of the drawn scene was built from.
    pub fn of_frame(frame: &crate::player_picking::Frame) -> Self {
        Self {
            view: frame.view,
            projection: frame.projection,
            far_plane: None,
        }
    }
}

/// The component a target operation was picked from: its parent layer, the
/// inventory object in it and its id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveTarget {
    pub parentlayer: i32,
    pub invobject: i32,
    pub id: i32,
}

pub use crate::animation_matrix::transform;

/// The fine terrain height at `(x, z)` over the level heightmap (tile shift 9,
/// 512 units per tile); 0 outside the tile grid.
pub fn fine_height(h: &Heightmap, level: i32, x: i32, z: i32) -> i32 {
    let (tx, tz) = (x >> TILE_SHIFT, z >> TILE_SHIFT);
    if tx < 0 || tz < 0 || tx > h.width as i32 - 1 || tz > h.height as i32 - 1 {
        return 0;
    }
    let get = |x: i32, z: i32| h.get(level, x, z).map(|v| v as i32).unwrap_or(0);
    let fine = (1 << TILE_SHIFT) - 1;
    let corners = [
        get(tx, tz),
        get(tx + 1, tz),
        get(tx, tz + 1),
        get(tx + 1, tz + 1),
    ];
    blend_corners(corners, x & fine, z & fine, TILE_SHIFT)
}

/// Raycasts the ground from `a` to `b` in the
/// scene-local fine units the view matrix works in; `level` is the local
/// player's level (raised over a link-below tile).
pub fn raycast_ground(h: &Heightmap, level: i32, a: [f32; 3], b: [f32; 3], depth: i32) -> f32 {
    let mut t = 0.0_f32;
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    let (mut prev_x, mut prev_y, mut prev_z) = (0.0_f32, 0.0_f32, 0.0_f32);
    while t < 1.1 {
        let x = t * dx + a[0];
        let y = t * dy + a[1];
        let z = t * dz + a[2];
        let tile_x = (x as i32) >> 9;
        let tile_z = (z as i32) >> 9;
        if tile_x > 0 && tile_z > 0 && tile_x < h.width as i32 && tile_z < h.height as i32 {
            let level = height_level(level, h.is_link_below(tile_x, tile_z));
            let ground = fine_height(h, level, x as i32, z as i32);
            if (ground as f32) < y {
                if depth >= 2 {
                    return t - 0.1
                        + raycast_ground(h, level, [prev_x, prev_y, prev_z], [x, y, z], depth - 1)
                            * 0.1;
                }
                return t;
            }
        }
        prev_x = x;
        prev_y = y;
        prev_z = z;
        t += 0.1;
    }
    -1.0
}

/// The ground pick: unproject the mouse through the inverted
/// view-projection matrix of `drawn`, raycast the heightmap, and return the
/// walk tile. Past the projection's far plane the ray goes on to the one the
/// scene was drawn to ([`DrawnView::far_plane`]) in stretches as long as the
/// projection's, so the march stays as fine; the nearest hit wins.
/// `viewport` is the scene viewport's x, y, width and height, `size` the
/// local player's size.
pub fn pick_walk_tile(
    drawn: &DrawnView,
    viewport: [i32; 4],
    mouse: [i32; 2],
    h: &Heightmap,
    level: i32,
    size: i32,
) -> Option<[i32; 2]> {
    let inverse =
        crate::animation_matrix::inverse(&crate::camera::multiply(&drawn.view, &drawn.projection));
    let rel_x = mouse[0] - viewport[0];
    let rel_y = mouse[1] - viewport[1];
    let ndc_x = rel_x as f32 * 2.0 / viewport[2] as f32 - 1.0;
    let ndc_y = rel_y as f32 * 2.0 / viewport[3] as f32 - 1.0;
    let near = transform(&inverse, ndc_x, ndc_y, -1.0);
    let (near_x, near_y, near_z) = (near[0] / near[3], near[1] / near[3], near[2] / near[3]);
    let far = transform(&inverse, ndc_x, ndc_y, 1.0);
    let (far_x, far_y, far_z) = (far[0] / far[3], far[1] / far[3], far[2] / far[3]);
    let span = [far_x - near_x, far_y - near_y, far_z - near_z];
    // View depth runs linearly along the ray, so the drawn far plane is
    // this many projection-long stretches out.
    let stretches = drawn.far_plane.map_or(1, |far_plane| {
        let depth = |p: [f32; 3]| transform(&drawn.view, p[0], p[1], p[2])[2].abs();
        let (near_depth, far_depth) = (
            depth([near_x, near_y, near_z]),
            depth([far_x, far_y, far_z]),
        );
        let reach = ((far_plane - near_depth) / (far_depth - near_depth)).ceil();
        if reach.is_finite() {
            reach.max(1.0) as i32
        } else {
            1
        }
    });
    let (mut start, mut end) = ([near_x, near_y, near_z], [far_x, far_y, far_z]);
    for _ in 0..stretches {
        let hit = raycast_ground(h, level, start, end, 4);
        if hit > 0.0 {
            let fine_x = (hit * span[0] + start[0]) as i32;
            let fine_z = (hit * span[2] + start[2]) as i32;
            // A link-below tile raises the level for the raycast only; the
            // walk tile is unaffected.
            return Some([
                (fine_x + ((size - 1) << 8)) >> 9,
                (fine_z + ((size - 1) << 8)) >> 9,
            ]);
        }
        start = end;
        end = [end[0] + span[0], end[1] + span[1], end[2] + span[2]];
    }
    None
}

/// The bytes of `MOVE_GAMECLICK`
/// (33, fixed size 5): `p2(baseZ + z)`, `p1(ctrl)`, `p2_alt3(baseX + x)`.
/// ISAAC is disabled on this connection.
pub fn move_game_click(base: [i32; 2], tile: [i32; 2], ctrl: bool) -> Vec<u8> {
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    vec![
        crate::proto::client::MOVE_GAMECLICK,
        (z >> 8) as u8,
        z as u8,
        u8::from(ctrl),
        (x + 128) as u8,
        (x >> 8) as u8,
    ]
}

/// The bytes of `MOVE_MINIMAPCLICK` (78, fixed size 18): the shared
/// `p2(baseZ + z)`, `p1(ctrl)`, `p2_alt3(baseX + x)` head, then the
/// minimap-only anticheat tail: `p1(-1)`, `p1(-1)`, `p2(orbitYaw)`, `p1(57)`,
/// `p1(angle)`, `p1(zoom)`, `p1(89)`, `p2(playerX)`, `p2(playerZ)`, `p1(63)`.
/// `yaw` is the orbit camera yaw (14-bit units), `angle` and `zoom` are the
/// minimap anticheat angle and the minimap zoom, and `player` is the local
/// player's x/z as ints. Like [`move_game_click`] the returned bytes include
/// the opcode first.
pub fn move_minimap_click(
    base: [i32; 2],
    tile: [i32; 2],
    ctrl: bool,
    yaw: i32,
    angle: i32,
    zoom: i32,
    player: [i32; 2],
) -> Vec<u8> {
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    vec![
        crate::proto::client::MOVE_MINIMAPCLICK,
        (z >> 8) as u8,
        z as u8,
        u8::from(ctrl),
        (x + 128) as u8,
        (x >> 8) as u8,
        0xFF,
        0xFF,
        (yaw >> 8) as u8,
        yaw as u8,
        57,
        angle as u8,
        zoom as u8,
        89,
        (player[0] >> 8) as u8,
        player[0] as u8,
        (player[1] >> 8) as u8,
        player[1] as u8,
        63,
    ]
}

/// The bytes of `FACE_SQUARE` (menu action 60).
pub fn face_square(base: [i32; 2], tile: [i32; 2]) -> Vec<u8> {
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    vec![
        crate::proto::client::FACE_SQUARE,
        (z >> 8) as u8,
        (z + 128) as u8,
        (x + 128) as u8,
        (x >> 8) as u8,
    ]
}

/// Menu action 9 -> `OPNPC1`: opcode 51 size 3, `p1_alt3(ctrl)` (byte
/// `128 - v`) then `p2_alt2(index)` (hi, `lo + 128`). Mirrors
/// `ui_player_options::build_opplayer_packet` but with the NPC writer
/// order/transforms; `>= 2000` strips the deprioritised flag before
/// matching. Actions 9-13 and 1003 map to OPNPC1-6
/// (10->56, 11->116, 12->31, 13->2, 1003->36) with the same payload.
pub fn build_opnpc(action: i32, npc_index: u16, ctrl: bool) -> Option<(u8, [u8; 3])> {
    let base = action - if action >= 2000 { 2000 } else { 0 };
    let opcode = match base {
        9 => crate::proto::client::OPNPC1,
        10 => crate::proto::client::OPNPC2,
        11 => crate::proto::client::OPNPC3,
        12 => crate::proto::client::OPNPC4,
        13 => crate::proto::client::OPNPC5,
        1003 => crate::proto::client::OPNPC6,
        _ => return None,
    };
    Some((
        opcode,
        [
            128u8.wrapping_sub(u8::from(ctrl)),
            (npc_index >> 8) as u8,
            (npc_index as u8).wrapping_add(128),
        ],
    ))
}

/// Packs a loc reference for a menu entry:
/// `low = tileX | tileZ << 7 | shape << 14 | angle << 20 | 0x40000000`,
/// with the sign bit set when the loc's `active` is 0 and bit 22 when its
/// `raiseobject` is 1, then `| (long)id << 32`. `tile_x`/`tile_z` are the
/// scene tile offsets, `shape` and `angle` the placed loc's, and the two
/// flags come from the resolved loc type. The menu entry stores the full
/// long as `entityId`; the packet builders send only the high half (see
/// `build_oploc`).
pub fn encode_loc_id(
    loc_id: i32,
    tile_x: i32,
    tile_z: i32,
    shape: i32,
    angle: i32,
    active_zero: bool,
    raiseobject_one: bool,
) -> i64 {
    let mut low: i64 =
        ((tile_x | tile_z << 7 | shape << 14 | angle << 20 | 0x4000_0000) as u32) as i64;
    if active_zero {
        low |= i64::MIN;
    }
    if raiseobject_one {
        low |= 4194304;
    }
    low | ((loc_id as i64) << 32)
}

/// Menu actions 3-6/1001 -> `OPLOC1`..`OPLOC5`: opcode 12 size 9,
/// `p1_alt2(ctrl)` (byte `-v`) then `p2(baseZ + z)` (hi, lo),
/// `p4((int)(entityId >>> 32) & MAX_VALUE)` (4 bytes big-endian: the
/// `encode_loc_id` loc id high half) then `p2_alt3(baseX + x)` (`lo + 128`,
/// hi). `>= 2000` strips the deprioritised flag before matching.
pub fn build_oploc(
    action: i32,
    entity_id: i64,
    base: [i32; 2],
    tile: [i32; 2],
    ctrl: bool,
) -> Option<(u8, [u8; 9])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    let opcode = match base_action {
        3 => crate::proto::client::OPLOC1,
        4 => crate::proto::client::OPLOC2,
        5 => crate::proto::client::OPLOC3,
        6 => crate::proto::client::OPLOC4,
        1001 => crate::proto::client::OPLOC5,
        1002 => crate::proto::client::OPLOC6,
        _ => return None,
    };
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    let id_hash = (((entity_id as u64) >> 32) & 0x7FFF_FFFF) as u32;
    Some((
        opcode,
        [
            (if ctrl { 0u8.wrapping_sub(1) } else { 0 }),
            (z >> 8) as u8,
            z as u8,
            (id_hash >> 24) as u8,
            (id_hash >> 16) as u8,
            (id_hash >> 8) as u8,
            id_hash as u8,
            (x as u8).wrapping_add(128),
            (x >> 8) as u8,
        ],
    ))
}

/// Object menu actions 18..22/1004 -> `OPOBJ1`..`OPOBJ6`. All six protocols
/// share the same payload: `p2_alt1(objIndex)` (lo, hi) then
/// `p2_alt1(baseX + x)` (lo, hi), `p2(baseZ + z)` (hi, lo) then
/// `p1_alt3((submenu ? 2 : 0) | (ctrl ? 1 : 0))` (byte `128 - v`). `submenu`
/// is set when the option was chosen from an open menu (direct clicks pass
/// false); `ctrl` is whether control is held. `>= 2000` strips the
/// deprioritised flag before matching.
pub fn build_opobj(
    action: i32,
    obj_index: i32,
    base: [i32; 2],
    tile: [i32; 2],
    submenu: bool,
    ctrl: bool,
) -> Option<(u8, [u8; 7])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    let opcode = match base_action {
        18 => crate::proto::client::OPOBJ1,
        19 => crate::proto::client::OPOBJ2,
        20 => crate::proto::client::OPOBJ3,
        21 => crate::proto::client::OPOBJ4,
        22 => crate::proto::client::OPOBJ5,
        1004 => crate::proto::client::OPOBJ6,
        _ => return None,
    };
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    let flag = (if submenu { 2 } else { 0 }) | (if ctrl { 1 } else { 0 });
    Some((
        opcode,
        [
            obj_index as u8,
            (obj_index >> 8) as u8,
            x as u8,
            (x >> 8) as u8,
            (z >> 8) as u8,
            z as u8,
            128u8.wrapping_sub(flag as u8),
        ],
    ))
}

/// Menu action 8 -> `OPNPCT`: opcode 113 size 11, `p4(parentlayer)`
/// (big-endian) then `p2(npcIndex)` (hi, lo), `p1_alt1(ctrl)` (byte
/// `v + 128`), `p2_alt1(activeInvobject)` (lo, hi), `p2(activeId)` (hi, lo).
/// `active*` are the parent layer, inventory object and id of the component
/// the target was picked from. `>= 2000` strips the deprioritised flag
/// before matching.
pub fn build_opnpct(
    action: i32,
    npc_index: u16,
    active: ActiveTarget,
    ctrl: bool,
) -> Option<(u8, [u8; 11])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    if base_action != 8 {
        return None;
    }
    Some((
        crate::proto::client::OPNPCT,
        [
            (active.parentlayer >> 24) as u8,
            (active.parentlayer >> 16) as u8,
            (active.parentlayer >> 8) as u8,
            active.parentlayer as u8,
            (npc_index >> 8) as u8,
            npc_index as u8,
            u8::from(ctrl).wrapping_add(128),
            active.invobject as u8,
            (active.invobject >> 8) as u8,
            (active.id >> 8) as u8,
            active.id as u8,
        ],
    ))
}

/// Menu action 17 -> `OPOBJT`: opcode 114 size 15, `p2_alt1(objIndex)`
/// then `p1_alt1(ctrl)`, `p2_alt1(activeInvobject)`,
/// `p2_alt1(baseZ + z)`, `p2_alt1(baseX + x)` (lo, hi; byte `v + 128`),
/// `p4_alt2(activeParentlayer)` (`b8, b0, b24, b16`) then
/// `p2_alt3(activeId)` (`lo + 128`, hi). Unlike `build_opobj` there is no
/// submenu flag: only ctrl is sent. `>= 2000` strips the deprioritised flag
/// before matching.
pub fn build_opobjt(
    action: i32,
    obj_index: i32,
    base: [i32; 2],
    tile: [i32; 2],
    active: ActiveTarget,
    ctrl: bool,
) -> Option<(u8, [u8; 15])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    if base_action != 17 {
        return None;
    }
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    Some((
        crate::proto::client::OPOBJT,
        [
            obj_index as u8,
            (obj_index >> 8) as u8,
            u8::from(ctrl).wrapping_add(128),
            active.invobject as u8,
            (active.invobject >> 8) as u8,
            z as u8,
            (z >> 8) as u8,
            x as u8,
            (x >> 8) as u8,
            (active.parentlayer >> 8) as u8,
            active.parentlayer as u8,
            (active.parentlayer >> 24) as u8,
            (active.parentlayer >> 16) as u8,
            (active.id as u8).wrapping_add(128),
            (active.id >> 8) as u8,
        ],
    ))
}

/// Menu action 2 -> `OPLOCT`: opcode 21 size 17, `p1_alt1(ctrl)` then
/// `p2_alt1(baseX + x)`, `p2_alt1(activeInvobject)`,
/// `p2_alt3(baseZ + z)` (`lo + 128`, hi),
/// `p4_alt1(activeParentlayer)` (little-endian),
/// `p4_alt2((int)(entityId >>> 32) & MAX_VALUE)` (the `encode_loc_id`
/// loc-id high half) then `p2(activeId)`. `>= 2000` strips the
/// deprioritised flag before matching.
pub fn build_oploct(
    action: i32,
    entity_id: i64,
    base: [i32; 2],
    tile: [i32; 2],
    active: ActiveTarget,
    ctrl: bool,
) -> Option<(u8, [u8; 17])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    if base_action != 2 {
        return None;
    }
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    let id_hash = (((entity_id as u64) >> 32) & 0x7FFF_FFFF) as u32;
    Some((
        crate::proto::client::OPLOCT,
        [
            u8::from(ctrl).wrapping_add(128),
            x as u8,
            (x >> 8) as u8,
            active.invobject as u8,
            (active.invobject >> 8) as u8,
            (z as u8).wrapping_add(128),
            (z >> 8) as u8,
            active.parentlayer as u8,
            (active.parentlayer >> 8) as u8,
            (active.parentlayer >> 16) as u8,
            (active.parentlayer >> 24) as u8,
            (id_hash >> 8) as u8,
            id_hash as u8,
            (id_hash >> 24) as u8,
            (id_hash >> 16) as u8,
            (active.id >> 8) as u8,
            active.id as u8,
        ],
    ))
}

/// Menu action 59 -> `APCOORDT`: opcode 59 size 12, `p2_alt2(baseX + x)`
/// (hi, `lo + 128`), `p4_alt1(activeParentlayer)` (little-endian),
/// `p2(activeInvobject)` (hi, lo), `p2_alt2(baseZ + z)`,
/// `p2_alt2(activeId)`. No ctrl byte is sent here; `>= 2000` still strips
/// the deprioritised flag before matching.
pub fn build_apcoordt(
    action: i32,
    base: [i32; 2],
    tile: [i32; 2],
    active: ActiveTarget,
) -> Option<(u8, [u8; 12])> {
    let base_action = action - if action >= 2000 { 2000 } else { 0 };
    if base_action != 59 {
        return None;
    }
    let z = base[1] + tile[1];
    let x = base[0] + tile[0];
    Some((
        crate::proto::client::APCOORDT,
        [
            (x >> 8) as u8,
            (x as u8).wrapping_add(128),
            active.parentlayer as u8,
            (active.parentlayer >> 8) as u8,
            (active.parentlayer >> 16) as u8,
            (active.parentlayer >> 24) as u8,
            (active.invobject >> 8) as u8,
            active.invobject as u8,
            (z >> 8) as u8,
            (z as u8).wrapping_add(128),
            (active.id >> 8) as u8,
            (active.id as u8).wrapping_add(128),
        ],
    ))
}

#[cfg(test)]
mod tests;
