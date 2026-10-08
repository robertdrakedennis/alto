//! Game/UI-coupled CS2 engine commands
//! that are not owned by a narrower host: legacy `cam_*`, cutscene
//! `spline_*`, the disabled `shop_*`/map-loading/3D world-map stubs,
//! `mec_*` map-element queries, object search, inventory stock/category
//! queries, affined clan-channel moderation, local appearance editing,
//! scripted movement and the component commands of the same family
//! (`if_*`/`cc_*` NPC/player models, links, text metrics, debug buttons).
//!
//! It is a child of `ui_runtime` so that the retained `Engine`/`Runtime`
//! owners stay private while the command bodies live in one place. Engine
//! commands are dispatched immediately before `Engine`'s unknown-command
//! fallback; component commands are dispatched by `ui_properties` with the
//! live component store and active-component pair.
use super::{absent, encode_pjstr, ActiveEntity, Engine, Runtime};
use crate::{
    ui_components::{Active, Store},
    ui_properties::Context,
    ui_vars::Variables,
};
use anyhow::Context as _;
use native910::vm::{InstructionContext, Value, VmError, VmResult};
use rs910_core::fault::Fault;

/// The constant floor added to every follow-camera height write.
const FOLLOW_HEIGHT_BASE: i32 = 35;

/// Engine-level commands of this family (routing table used by the partition
/// check and by `dispatch`).
pub(crate) const ENGINE_COMMANDS: &[&str] = &[
    "cam_dec_x",
    "cam_dec_y",
    "cam_followcoord",
    "cam_forceangle",
    "cam_getangle_xa",
    "cam_getangle_ya",
    "cam_getfollowheight",
    "cam_inc_x",
    "cam_inc_y",
    "cam_lookat",
    "cam_movealong",
    "cam_moveto",
    "cam_removeroof",
    "cam_reset",
    "cam_setfollowheight",
    "cam_smoothreset",
    "spline_addpoint",
    "spline_length",
    "spline_new",
    "shop_applypendingtransactions",
    "shop_getcategorycount",
    "shop_getcategorydescription",
    "shop_getcategoryid",
    "shop_getindexforcategoryid",
    "shop_getindexforcategoryname",
    "shop_getproductcount",
    "shop_getproductdetails",
    "shop_isproductavailable",
    "shop_isproductrecommended",
    "shop_open",
    "shop_opencategories",
    "shop_requestdata",
    "shop_requestdatastatus",
    "map_build_complete",
    "map_isowner",
    "map_loadedpercent",
    "map_loadingscreen_isopen",
    "map_loadingscreen_settriggerpercent",
    "map_preload",
    "worldmap_3dview_getloddistance",
    "worldmap_3dview_getscreenposition",
    "worldmap_3dview_gettextfont",
    "worldmap_3dview_settextfont",
    "worldmap_getcategorypriority",
    "worldmap_setcategorypriority",
    "mec_category",
    "mec_param",
    "mec_sprite",
    "mec_text",
    "mec_textsize",
    "oc_find",
    "oc_findnext",
    "oc_findrestart",
    "inv_stockbase",
    "inv_totalcat",
    "affinedclansettings_addbanned_fromchannel",
    "affinedclansettings_setmuted_fromchannel",
    "basecolour",
    "baseidkit",
    "basematerial",
    "gender",
    "setgender",
    "setobj",
    "opcount",
    "movescripted",
    "get_selfyangle",
    "facing_fine",
    "interface_getpickingradius",
    "minimenu_close",
    "npc_find_active_minimenu_entry",
    "player_find_active_minimenu_entry",
    "get_currentcursor",
];

/// Component-level commands dispatched by `ui_properties::Context`. The
/// `if_/cc_setnpchead|setnpcmodel|setplayermodel|setplayerhead_self` model
/// setters live in `ui_properties::SETTERS` beside the other model setters.
pub(crate) const COMPONENT_COMMANDS: &[&str] = &[
    "if_debug_button1",
    "if_debug_button2",
    "if_debug_button3",
    "if_debug_button4",
    "if_debug_button5",
    "if_debug_button6",
    "if_debug_button7",
    "if_debug_button8",
    "if_debug_button9",
    "if_debug_button10",
    "if_debug_getcomcount",
    "if_debug_getcomname",
    "if_debug_getname",
    "if_debug_getopenifid",
    "if_debug_getservertriggers",
    "if_getcharindexatpos",
    "cc_getcharindexatpos",
    "if_getcharposatindex",
    "cc_getcharposatindex",
    "if_getgraphicdimensions",
    "cc_getgraphicdimensions",
    "if_npc_setcustombodymodel",
    "cc_npc_setcustombodymodel",
    "if_npc_setcustombodymodel_transformed",
    "cc_npc_setcustombodymodel_transformed",
    "if_npc_setcustomheadmodel",
    "cc_npc_setcustomheadmodel",
    "if_npc_setcustomrecol",
    "cc_npc_setcustomrecol",
    "if_npc_setcustomretex",
    "cc_npc_setcustomretex",
    "if_resume_pausebutton",
    "cc_resume_pausebutton",
    "if_setlinkfriend",
    "cc_setlinkfriend",
    "if_setlinkfriendchat",
    "cc_setlinkfriendchat",
    "if_setlinkplayergroup",
    "cc_setlinkplayergroup",
    "if_setongamepadaxis",
    "cc_setongamepadaxis",
    "if_setongamepadbutton",
    "cc_setongamepadbutton",
    "if_setongamepadbuttonheld",
    "cc_setongamepadbuttonheld",
    "if_setongamepadtrigger",
    "cc_setongamepadtrigger",
    "if_setonhorizontalpinch",
    "cc_setonhorizontalpinch",
    "if_setonverticalpinch",
    "cc_setonverticalpinch",
    "opplayer",
    "opplayert",
];

/// Retained client state owned by this command family, held by `Engine`.
pub struct State {
    /// The two cutscene spline tables. Each keyframe pair is
    /// `{x, y, z, roll}` then `{x, y, z}`.
    pub cutscene_spline: [Option<Vec<Vec<i32>>>; 2],
    /// Whether the scripted camera path just finished / has reached its end;
    /// process-wide flags, so a camera reset does not clear them.
    pub spline_finished: bool,
    pub spline_end_reached: bool,
    /// Result list and read position of the object search commands (`oc_find*`).
    pub obj_find_results: Option<Vec<i32>>,
    pub obj_find_index: usize,
    /// Orbit pitch/yaw and follow height as last seen on the application's
    /// follow camera, updated by this family's writes.
    pub orbit_pitch: f32,
    pub orbit_yaw: f32,
    pub follow_height: i32,
    /// Follow-camera writes consumed by the application after the VM tick:
    /// orbit nudges (pitch?, positive?), the forced angle and the follow
    /// height.
    pub orbit_inputs: Vec<(bool, bool)>,
    pub orbit_force: Option<(f32, f32)>,
    pub follow_height_changed: bool,
    /// Scene base (in tiles) and size.
    pub base: [i32; 2],
    pub size: [i32; 2],
    /// The current player level, for the legacy camera height samples.
    pub level: i32,
    /// The local player's facing angle and a working copy of its appearance
    /// model; `model_dirty` requests the write-back.
    pub local_angle: Option<i32>,
    pub local_model: Option<crate::entities910::appearance::Model>,
    pub model_dirty: bool,
    /// The active minimenu entry resolved to its NPC (type 4) or player
    /// (type 7) entity during the minimenu update.
    pub minimenu_entity: Option<(i32, ActiveEntity)>,
    /// The current cursor id, mirrored from the window cursor owner.
    pub current_cursor: i32,
    /// The owner name compared by `map_isowner`.
    pub owner: Option<String>,
    /// Whether a game connection exists.
    pub game_connection: bool,
    /// Inventory type definitions (stock tables).
    pub inv_types: Option<crate::config::InvStore>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            cutscene_spline: [None, None],
            spline_finished: false,
            spline_end_reached: false,
            obj_find_results: None,
            obj_find_index: 0,
            orbit_pitch: 1088.0,
            orbit_yaw: 0.0,
            follow_height: FOLLOW_HEIGHT_BASE + 200,
            orbit_inputs: Vec::new(),
            orbit_force: None,
            follow_height_changed: false,
            base: [0; 2],
            size: [0; 2],
            level: 0,
            local_angle: None,
            local_model: None,
            model_dirty: false,
            minimenu_entity: None,
            current_cursor: -1,
            owner: None,
            game_connection: false,
            inv_types: None,
        }
    }
}

/// Retained script-visible state held by the component property owner
/// (`ui_properties::State.game`), where the component commands run.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    /// Friends list load state and display names.
    pub friends_list_state: i32,
    pub friends: Vec<String>,
    /// Friend chat user names (`None` when no channel is joined).
    pub friend_chat: Option<Vec<String>>,
    /// Current player group member display names and banned list.
    pub player_group: Option<(Vec<String>, Vec<String>)>,
    /// Players in high-resolution index order with each one's unfiltered
    /// name and first route waypoint, plus the local index.
    pub players: Vec<(i32, Option<String>, [i32; 2])>,
    pub local_player: Option<i32>,
    /// System-message and minimap-flag requests made by component commands; the runtime moves them into `Engine`.
    pub system_messages: Vec<(i32, String)>,
    pub minimap_flag: Option<[i32; 2]>,
}

fn underflow() -> VmError {
    VmError::StackUnderflow { stack: "int" }
}
fn pop(ints: &mut Vec<i32>) -> VmResult<i32> {
    ints.pop().ok_or_else(underflow)
}
/// Removes the top `n` ints, oldest first (the order a command reads its
/// arguments).
fn take(ints: &mut Vec<i32>, n: usize) -> VmResult<Vec<i32>> {
    if ints.len() < n {
        return Err(underflow());
    }
    Ok(ints.split_off(ints.len() - n))
}
fn pop_obj(objs: &mut Vec<String>) -> VmResult<String> {
    objs.pop()
        .ok_or(VmError::StackUnderflow { stack: "object" })
}
/// A trapped command: the script fails with `reason`.
fn trap(command: &str, reason: impl Into<String>) -> VmError {
    VmError::TrapFailed {
        command: command.into(),
        reason: reason.into(),
    }
}

/// Case-insensitive equality over UTF-16 code units: units match directly,
/// by upper case, or by lower case of the upper-cased units.
fn equals_ignore_case(a: &str, b: &str) -> bool {
    let case = |u: u16| (crate::char_case::to_upper(u), crate::char_case::to_lower(u));
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    a.len() == b.len()
        && a.iter().zip(&b).all(|(&x, &y)| {
            if x == y {
                return true;
            }
            let (ux, uy) = (case(x).0, case(y).0);
            ux == uy || case(ux).1 == case(uy).1
        })
}

/// Lexicographic comparison over UTF-16 code units (difference of the first
/// mismatching units, else of the lengths).
fn compare_to(a: &str, b: &str) -> i32 {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    for (x, y) in a.iter().zip(&b) {
        if x != y {
            return i32::from(*x) - i32::from(*y);
        }
    }
    a.len() as i32 - b.len() as i32
}

/// The client's name/id quicksort, including its odd-index `compare < 1`
/// comparison.
fn sort_names(names: &mut [String], ids: &mut [i32], lo: i32, hi: i32) {
    if lo >= hi {
        return;
    }
    let mid = ((lo + hi) / 2) as usize;
    let (lo_u, hi_u) = (lo as usize, hi as usize);
    names.swap(mid, hi_u);
    ids.swap(mid, hi_u);
    let pivot = names[hi_u].clone();
    let pivot_id = ids[hi_u];
    let mut store = lo_u;
    for i in lo_u..hi_u {
        if compare_to(&names[i], &pivot) < (i as i32 & 1) {
            names.swap(i, store);
            ids.swap(i, store);
            store += 1;
        }
    }
    names[hi_u] = names[store].clone();
    names[store] = pivot;
    ids[hi_u] = ids[store];
    ids[store] = pivot_id;
    sort_names(names, ids, lo, store as i32 - 1);
    sort_names(names, ids, store as i32 + 1, hi);
}

/// Wear-slot order and the male / female identity-kit slot tables of the
/// local appearance editor.
const WEARPOS_ORDER: [usize; 8] = [8, 11, 4, 6, 9, 7, 10, 0];
const IDK_MALE: [i32; 8] = [0, 1, 2, 3, 4, 5, 6, 14];
const IDK_FEMALE: [i32; 8] = [7, 8, 9, 10, 11, 12, 13, 15];

impl State {
    /// Steps the scripted camera path, once per logic update while the camera
    /// state is 6. Maintains `spline_finished` / `spline_end_reached`.
    fn move_along(&mut self, legacy: &mut crate::ui_cam2::LegacyCamera) -> anyhow::Result<()> {
        let Some(m) = legacy.move_along.as_mut() else {
            return Ok(());
        };
        let step = (m
            .progress
            .wrapping_mul(m.speed_max.wrapping_sub(m.speed_min))
            >> 16)
            .wrapping_add(m.speed_min);
        m.progress = m.progress.wrapping_add(step);
        if m.progress >= 65535 {
            m.progress = 65535;
            self.spline_finished = !self.spline_end_reached;
            self.spline_end_reached = true;
        } else {
            self.spline_finished = false;
            self.spline_end_reached = false;
        }
        let t = m.progress as f32 / 65535.0;
        let sample = |spline: usize, keyframe: usize| -> anyhow::Result<[f32; 3]> {
            let s = self.cutscene_spline[spline]
                .as_ref()
                .context(Fault::MissingValue.message("cutscene spline"))?;
            let at = |row: usize, axis: usize| -> anyhow::Result<i32> {
                s.get(row)
                    .and_then(|r| r.get(axis))
                    .copied()
                    .context(Fault::IndexOutOfRange.message("cutscene spline"))
            };
            let mut out = [0.0; 3];
            for (axis, value) in out.iter_mut().enumerate() {
                let p0 = at(keyframe, axis)?;
                let a = p0.wrapping_mul(3);
                let b = at(keyframe + 1, axis)?.wrapping_mul(3);
                let p2 = at(keyframe + 2, axis)?;
                let c = p2
                    .wrapping_sub(at(keyframe + 3, axis)?.wrapping_sub(p2))
                    .wrapping_mul(3);
                let d1 = b.wrapping_sub(a);
                let d2 = a.wrapping_sub(b.wrapping_mul(2)).wrapping_add(c);
                let d3 = p2.wrapping_sub(p0).wrapping_add(b).wrapping_sub(c);
                *value = ((d3 as f32 * t + d2 as f32) * t + d1 as f32) * t + p0 as f32;
            }
            Ok(out)
        };
        let pos_row = m.pos_keyframe * 2;
        let eye = sample(m.pos_spline, pos_row)?;
        let look = sample(m.target_spline, m.target_keyframe * 2)?;
        let dx = look[0] - eye[0];
        let dy = -(look[1] - eye[1]);
        let dz = look[2] - eye[2];
        let horizontal = f64::from(dx * dx + dz * dz).sqrt();
        const UNITS: f64 = 2607.5945876176133;
        let pitch = ((f64::from(dy).atan2(horizontal) * UNITS) as i32) & 0x3FFF;
        let yaw = ((-f64::from(dx).atan2(f64::from(dz)) * UNITS) as i32) & 0x3FFF;
        let roll_row = |row: usize| -> anyhow::Result<i32> {
            self.cutscene_spline[m.pos_spline]
                .as_ref()
                .and_then(|s| s.get(row))
                .and_then(|r| r.get(3))
                .copied()
                .context(Fault::IndexOutOfRange.message("cutscene spline roll"))
        };
        let r0 = roll_row(pos_row)?;
        let r2 = roll_row(pos_row + 2)?;
        let roll = (m.progress.wrapping_mul(r2.wrapping_sub(r0)) >> 16).wrapping_add(r0);
        let base = [self.base[0] * 512, self.base[1] * 512];
        legacy.pose = crate::ui_cam2::LegacyPose {
            x: eye[0] as i32 - base[0],
            y: (eye[1] as i32).wrapping_neg(),
            z: eye[2] as i32 - base[1],
            pitch,
            yaw,
            roll,
        };
        legacy.move_along_view = Some(crate::ui_cam2::LegacyView {
            position: [
                eye[0] as i32 - base[0],
                (eye[1] as i32).wrapping_neg(),
                eye[2] as i32 - base[1],
            ],
            target: [
                look[0] as i32 - base[0],
                (look[1] as i32).wrapping_neg(),
                look[2] as i32 - base[1],
            ],
            pitch,
            yaw,
            roll,
        });
        Ok(())
    }
}

impl Engine {
    /// A packed coordinate minus the scene base, clamped to `[0, size]` per
    /// axis.
    fn scene_tile(&self, packed: i32) -> [i32; 2] {
        let host = &self.game_host;
        let x = ((packed >> 14) & 0x3FFF) - host.base[0];
        let z = (packed & 0x3FFF) - host.base[1];
        let clamp = |v: i32, size: i32| {
            if v < 0 {
                0
            } else if v >= size {
                size
            } else {
                v
            }
        };
        [clamp(x, host.size[0]), clamp(z, host.size[1])]
    }

    pub(crate) fn camera_force_angle(&mut self, pitch: i32, yaw: i32, roll: i32) {
        let pitch = pitch << 3;
        let yaw = yaw << 3;
        let roll = roll << 3;
        if self.camera.cam2.camera_state != 3 {
            // clampCamera on the orbit angles; the terrain
            // pitch floor is recomputed by the follow owner next frame.
            let p = (pitch as f32).clamp(1077.0, 2787.0);
            let mut y = yaw as f32;
            while y >= 16384.0 {
                y -= 16384.0;
            }
            while y < 0.0 {
                y += 16384.0;
            }
            self.game_host.orbit_pitch = p;
            self.game_host.orbit_yaw = y;
            self.game_host.orbit_force = Some((p, y));
            if self.camera.cam2.camera_state == 5 {
                // cameraPitch/Yaw/Roll of the cutscene camera.
                self.camera.cam2.legacy.force_angle(pitch >> 3, yaw >> 3);
                let pose = &mut self.camera.cam2.legacy.pose;
                pose.pitch = pitch;
                pose.yaw = yaw;
                pose.roll = roll;
            }
        } else if self.camera.cam2.position_mode == Some(crate::ui_cam2::MODE_ENTITY) {
            use crate::ui_cam2::{Quat, Vec3};
            let radians =
                |units: i32| (f64::from(units) * std::f64::consts::PI * 2.0 / 16384.0) as f32;
            let mut orientation =
                Quat::rotation(0.0, 1.0, 0.0, std::f32::consts::PI - radians(yaw));
            let mut pitch_axis = Vec3::new(1.0, 0.0, 0.0);
            pitch_axis.rotate(&orientation);
            pitch_axis.negate();
            let pitch_turn = Quat::rotation_axis(&pitch_axis, radians(pitch));
            orientation.multiply(&pitch_turn);
            if let Some(crate::ui_cam2::Position::Entity(p)) = self.camera.cam2.position.as_mut() {
                p.rotation = orientation;
            }
        }
        self.camera.cam2.changed = true;
    }

    /// `getHeightmapY(x, z, currentPlayerLevel)` over the camera's
    /// retained heightmap copy.
    fn legacy_height(&self, x: i32, z: i32) -> i32 {
        let level = self.game_host.level;
        self.camera
            .cam2
            .scene
            .heightmap
            .as_ref()
            .map_or(0, |h| h.heightmap_y(x, z, level))
    }

    pub(crate) fn camera_move_to(
        &mut self,
        x: i32,
        z: i32,
        height: i32,
        acceleration: i32,
        speed: i32,
        instant: bool,
    ) {
        self.camera.cam2.legacy.move_to(crate::ui_cam2::LegacyMove {
            x,
            z,
            source_height: height,
            acceleration,
            speed,
        });
        if self.camera.cam2.camera_state == 3 {
            self.camera.cam2.copy_to_legacy();
        }
        if instant && speed >= 100 {
            let px = x * 512 + 256;
            let pz = z * 512 + 256;
            let py = self.legacy_height(px, pz) - height;
            let pose = &mut self.camera.cam2.legacy.pose;
            pose.x = px;
            pose.z = pz;
            pose.y = py;
        }
        self.camera.cam2.camera_state = 5;
        self.scene.server_roof = [-1, -1];
    }

    pub(crate) fn camera_look_at(
        &mut self,
        x: i32,
        z: i32,
        height: i32,
        acceleration: i32,
        speed: i32,
    ) {
        self.camera.cam2.legacy.look_at(crate::ui_cam2::LegacyLook {
            x,
            z,
            height,
            acceleration,
            speed,
        });
        if self.camera.cam2.camera_state == 3 {
            self.camera.cam2.copy_to_legacy();
        }
        let level = self.game_host.level;
        let heightmap = self.camera.cam2.scene.heightmap.take();
        let height_fn =
            |x: i32, z: i32| heightmap.as_ref().map_or(0, |h| h.heightmap_y(x, z, level));
        self.camera.cam2.legacy.look_at_instant(&height_fn);
        self.camera.cam2.scene.heightmap = heightmap;
        self.camera.cam2.camera_state = 5;
        self.scene.server_roof = [-1, -1];
    }

    fn local_model(
        &mut self,
        command: &str,
    ) -> VmResult<Option<&mut crate::entities910::appearance::Model>> {
        if self.game_host.local_angle.is_none() {
            return Err(trap(
                command,
                Fault::MissingValue.message("local player entity"),
            ));
        }
        Ok(self.game_host.local_model.as_mut())
    }

    fn obj_category(&self, obj: i32) -> anyhow::Result<i32> {
        let objs = self
            .configs
            .objs
            .as_ref()
            .context("obj types not installed")?;
        Ok(match u32::try_from(obj).ok().and_then(|id| objs.get(id)) {
            Some(o) => o.category,
            None => crate::config::decode_obj(obj as u32, &[0])?.category,
        })
    }

    fn find_objs(&mut self, query: &str, tradeable: bool) -> anyhow::Result<i32> {
        let query = query.to_lowercase();
        self.game_host.obj_find_results = None;
        self.game_host.obj_find_index = 0;
        let objs = self
            .configs
            .objs
            .as_ref()
            .context("obj types not installed")?;
        let num = objs.iter().last().map_or(0, |(id, _)| id + 1);
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for id in 0..num {
            let default;
            let o = match objs.get(id) {
                Some(o) => o,
                None => {
                    default = crate::config::decode_obj(id, &[0])?;
                    &default
                }
            };
            let i = &o.inventory;
            if (!tradeable || i.stockmarket)
                && i.derived[0][1] == -1
                && i.derived[1][1] == -1
                && i.derived[2][1] == -1
                && i.dummy == 0
                && o.name.to_lowercase().contains(&query)
            {
                if ids.len() >= 250 {
                    return Ok(-1);
                }
                ids.push(id as i32);
                names.push(o.name.clone());
            }
        }
        let count = ids.len() as i32;
        sort_names(&mut names, &mut ids, 0, count - 1);
        self.game_host.obj_find_results = Some(ids);
        Ok(count)
    }

    fn affined_user(&self, index: i32) -> Option<&crate::ui_social::ClanChannelUser> {
        let channel = self.social.affined_channel.as_ref()?;
        usize::try_from(index)
            .ok()
            .and_then(|i| channel.users.get(i))
    }
}

/// Engine-level commands; `None` when `c.command` is not in this family.
pub(crate) fn dispatch(
    engine: &mut Engine,
    c: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    objs: &mut Vec<String>,
) -> Option<VmResult<Option<Value>>> {
    if !ENGINE_COMMANDS.contains(&c.command) {
        return None;
    }
    Some(run(engine, c.command, ints, objs))
}

fn run(
    engine: &mut Engine,
    command: &str,
    ints: &mut Vec<i32>,
    objs: &mut Vec<String>,
) -> VmResult<Option<Value>> {
    let int = |v: i32| Ok(Some(Value::Int(v)));
    let failed = |e: anyhow::Error| trap(command, format!("{e:#}"));
    match command {
        // shop_*: the shop service is disabled; every command is a fixed
        // stack effect.
        "shop_applypendingtransactions" | "shop_requestdata" => Ok(None),
        "shop_getcategorycount" | "shop_requestdatastatus" => int(0),
        "shop_getcategorydescription" => {
            pop(ints)?;
            Ok(Some(Value::Str(String::new())))
        }
        "shop_getcategoryid" | "shop_getindexforcategoryid" => {
            pop(ints)?;
            int(-1)
        }
        "shop_getindexforcategoryname" => {
            pop_obj(objs)?;
            int(-1)
        }
        "shop_getproductcount" => {
            pop(ints)?;
            int(0)
        }
        "shop_getproductdetails" => {
            take(ints, 2)?;
            objs.extend(std::iter::repeat_n(String::new(), 9));
            Ok(None)
        }
        "shop_isproductavailable" | "shop_isproductrecommended" => {
            take(ints, 2)?;
            int(0)
        }
        "shop_open" => {
            pop(ints)?;
            Ok(None)
        }
        "shop_opencategories" => {
            take(ints, 2)?;
            Ok(None)
        }
        // map_*: the asynchronous map-loading screen is compiled out;
        // loading is reported complete.
        "map_build_complete" => int(1),
        "map_loadedpercent" => int(100),
        "map_loadingscreen_isopen" => int(0),
        "map_loadingscreen_settriggerpercent" => {
            take(ints, 2)?;
            Ok(None)
        }
        "map_preload" => {
            pop(ints)?;
            Ok(None)
        }
        // map_isowner compares the session owner name.
        "map_isowner" => {
            let name = pop_obj(objs)?;
            let owner = engine.game_host.owner.as_deref();
            int(i32::from(
                owner.is_some_and(|o| equals_ignore_case(o, &name)),
            ))
        }
        // worldmap_3dview_*: the 3D world-map view is not part of this
        // client; the commands return fixed values.
        "worldmap_3dview_getloddistance" => int(-1),
        "worldmap_3dview_getscreenposition" => {
            take(ints, 2)?;
            ints.extend([-1, -1]);
            Ok(None)
        }
        "worldmap_3dview_gettextfont" => {
            pop(ints)?;
            int(-1)
        }
        "worldmap_3dview_settextfont" => {
            take(ints, 2)?;
            Ok(None)
        }
        "worldmap_getcategorypriority" => int(0),
        "worldmap_setcategorypriority" => Ok(None),
        // interface_getpickingradius and minimenu_close.
        "interface_getpickingradius" => int(0),
        "minimenu_close" => Ok(None),
        // mec_* over the map element types (an absent file decodes to the
        // defaults).
        "mec_text" | "mec_sprite" | "mec_textsize" | "mec_category" | "mec_param" => {
            let (id, param) = if command == "mec_param" {
                let a = take(ints, 2)?;
                (a[0], Some(a[1]))
            } else {
                (pop(ints)?, None)
            };
            let types = engine
                .configs
                .map_element_types
                .as_ref()
                .ok_or_else(|| trap(command, "map element types not installed"))?;
            let default;
            let t = match types.get(id) {
                Some(t) => t,
                None => {
                    default = crate::minimap::MapElement::decode(&[0]).map_err(failed)?;
                    &default
                }
            };
            match command {
                "mec_text" => Ok(Some(Value::Str(t.text.clone().unwrap_or_default()))),
                "mec_sprite" => int(t.sprite),
                "mec_textsize" => int(t.text_size),
                "mec_category" => int(t.category),
                _ => {
                    let param = param.unwrap();
                    let p = engine
                        .configs
                        .params
                        .get(&param)
                        .cloned()
                        .unwrap_or_default();
                    let string = p.kind == Some(36) || p.kind_legacy == Some(b's');
                    let value = t.params.iter().find(|(k, _)| *k == param).map(|(_, v)| v);
                    match (string, value) {
                        (true, Some(crate::config::ParamValue::Str(v))) => {
                            Ok(Some(Value::Str(v.clone())))
                        }
                        (true, None) => Ok(Some(Value::Str(p.default_string.unwrap_or_default()))),
                        (false, Some(crate::config::ParamValue::Int(v))) => int(*v),
                        (false, None) => int(p.default_int.unwrap_or(0)),
                        _ => Err(trap(
                            command,
                            Fault::WrongValueType.message("map element parameter"),
                        )),
                    }
                }
            }
        }
        // oc_find/oc_findnext/oc_findrestart.
        "oc_find" => {
            let query = pop_obj(objs)?;
            let tradeable = pop(ints)? == 1;
            let count = engine.find_objs(&query, tradeable).map_err(failed)?;
            int(count)
        }
        "oc_findnext" => {
            let host = &mut engine.game_host;
            let next = host
                .obj_find_results
                .as_ref()
                .and_then(|r| r.get(host.obj_find_index).copied());
            match next {
                Some(id) => {
                    host.obj_find_index += 1;
                    int(id & 0xFFFF)
                }
                None => int(-1),
            }
        }
        "oc_findrestart" => {
            engine.game_host.obj_find_index = 0;
            Ok(None)
        }
        // inv_stockbase reads the stock object/count tables.
        "inv_stockbase" => {
            let a = take(ints, 2)?;
            let invs = engine
                .game_host
                .inv_types
                .as_ref()
                .ok_or_else(|| trap(command, "inv types not installed"))?;
            let stock = u32::try_from(a[0])
                .ok()
                .and_then(|id| invs.get(id))
                .map_or(&[][..], |inv| inv.stock.as_slice());
            int(stock
                .iter()
                .find(|(obj, _)| *obj == a[1])
                .map_or(-1, |(_, count)| *count))
        }
        // inv_totalcat counts the objects of a category.
        "inv_totalcat" => {
            let a = take(ints, 2)?;
            let Some(inv) = engine.inv_cache.inventory(a[0], false).cloned() else {
                return int(0);
            };
            let mut total = 0i32;
            for (slot, &obj) in inv.obj_ids.iter().enumerate() {
                if obj >= 0 && engine.obj_category(obj).map_err(failed)? == a[1] {
                    total =
                        total.wrapping_add(engine.inv_cache.slot_count(a[0], slot as i32, false));
                }
            }
            int(total)
        }
        // affinedclansettings_*_fromchannel: clan settings updates sent for a
        // channel user.
        "affinedclansettings_addbanned_fromchannel" => {
            let index = pop(ints)?;
            let Some(user) = engine.affined_user(index) else {
                return Ok(None);
            };
            if user.rank != -1 {
                return Ok(None);
            }
            let name = encode_pjstr(&user.name)
                .ok_or_else(|| trap(command, "clan user name contains NUL"))?;
            let mut packet = vec![
                crate::proto::client::AFFINEDCLANSETTINGS_ADDBANNED_FROMCHANNEL,
                (name.len() + 2) as u8,
                (index >> 8) as u8,
                index as u8,
            ];
            packet.extend(name);
            engine.outgoing.extend(packet);
            Ok(None)
        }
        "affinedclansettings_setmuted_fromchannel" => {
            let a = take(ints, 2)?;
            let Some(user) = engine.affined_user(a[0]) else {
                return Ok(None);
            };
            let name = encode_pjstr(&user.name)
                .ok_or_else(|| trap(command, "clan user name contains NUL"))?;
            let mut packet = vec![
                crate::proto::client::AFFINEDCLANSETTINGS_SETMUTED_FROMCHANNEL,
                (name.len() + 3) as u8,
                (a[0] >> 8) as u8,
                a[0] as u8,
                u8::from(a[1] == 1),
            ];
            packet.extend(name);
            engine.outgoing.extend(packet);
            Ok(None)
        }
        // basecolour/baseidkit/basematerial/setgender/setobj edit the local
        // player's appearance model; gender reads it.
        "basecolour" | "basematerial" => {
            let a = take(ints, 2)?;
            if let Some(model) = engine.local_model(command)? {
                let slots = if command == "basecolour" {
                    &mut model.colours
                } else {
                    &mut model.textures
                };
                *usize::try_from(a[0])
                    .ok()
                    .and_then(|i| slots.get_mut(i))
                    .ok_or_else(|| {
                        trap(command, Fault::IndexOutOfRange.message("appearance slot"))
                    })? = a[1];
                model.update_hash();
                engine.game_host.model_dirty = true;
            }
            Ok(None)
        }
        "baseidkit" => {
            let a = take(ints, 2)?;
            if let Some(model) = engine.local_model(command)? {
                let index = IDK_MALE
                    .iter()
                    .position(|&p| p == a[0])
                    .or_else(|| IDK_FEMALE.iter().position(|&p| p == a[0]));
                if let Some(index) = index {
                    let slot = WEARPOS_ORDER[index];
                    *model.kits.get_mut(slot).ok_or_else(|| {
                        trap(
                            command,
                            Fault::IndexOutOfRange.message("appearance kit slot"),
                        )
                    })? = a[1] | i32::MIN;
                    model.update_hash();
                    engine.game_host.model_dirty = true;
                }
            }
            Ok(None)
        }
        "setgender" => {
            let female = pop(ints)? != 0;
            if let Some(model) = engine.local_model(command)? {
                model.female = female;
                model.update_hash();
                engine.game_host.model_dirty = true;
            }
            Ok(None)
        }
        "setobj" => {
            let a = take(ints, 2)?;
            if let Some(model) = engine.local_model(command)? {
                let slot = usize::try_from(a[0])
                    .ok()
                    .filter(|&s| s < model.kits.len())
                    .ok_or_else(|| {
                        trap(
                            command,
                            Fault::IndexOutOfRange.message("appearance kit slot"),
                        )
                    })?;
                if a[1] == -1 {
                    model.kits[slot] = 0;
                } else {
                    model.kits[slot] = a[1] | 0x4000_0000;
                    model.update_hash();
                }
                engine.game_host.model_dirty = true;
            }
            Ok(None)
        }
        "gender" => {
            let female = engine.local_model(command)?.is_some_and(|m| m.female);
            int(i32::from(female))
        }
        // get_selfyangle and facing_fine read the local player's facing.
        "get_selfyangle" | "facing_fine" => {
            let angle = engine
                .game_host
                .local_angle
                .ok_or_else(|| trap(command, Fault::MissingValue.message("local player entity")))?;
            int(if command == "facing_fine" {
                angle
            } else {
                angle >> 3
            })
        }
        // opcount.
        "opcount" => int(native910::vm::opcount()),
        // get_currentcursor.
        "get_currentcursor" => int(engine.game_host.current_cursor),
        // npc_find_active_minimenu_entry and
        // player_find_active_minimenu_entry.
        "npc_find_active_minimenu_entry" | "player_find_active_minimenu_entry" => {
            let wanted = if command.starts_with("npc") { 4 } else { 7 };
            match engine.game_host.minimenu_entity.clone() {
                Some((kind, entity)) if kind == wanted => {
                    engine.scene.active_entity = Some(entity);
                    int(1)
                }
                _ => int(0),
            }
        }
        // movescripted (move speed ids 0-2).
        "movescripted" => {
            let a = take(ints, 2)?;
            if a[1] == -1 {
                return Err(trap(
                    command,
                    Fault::InvalidState.message("coordinate level -1"),
                ));
            }
            if !(0..=2).contains(&a[0]) {
                return Err(trap(command, Fault::InvalidState.message("move speed")));
            }
            if engine.game_host.game_connection {
                let x = (a[1] >> 14) & 0x3FFF;
                let z = a[1] & 0x3FFF;
                engine.outgoing.extend([
                    crate::proto::client::MOVE_SCRIPTED,
                    (a[0] + 128) as u8,
                    (z >> 8) as u8,
                    (z + 128) as u8,
                    x as u8,
                    (x >> 8) as u8,
                ]);
            }
            Ok(None)
        }
        // Legacy camera commands.
        "cam_moveto" | "cam_lookat" => {
            let a = take(ints, 4)?;
            let x = ((a[0] >> 14) & 0x3FFF) - engine.game_host.base[0];
            let z = (a[0] & 0x3FFF) - engine.game_host.base[1];
            if command == "cam_moveto" {
                engine.camera_move_to(x, z, a[1] << 2, a[2], a[3], false);
            } else {
                engine.camera_look_at(x, z, a[1] << 2, a[2], a[3]);
            }
            engine.camera.cam2.changed = true;
            Ok(None)
        }
        "cam_movealong" => {
            let a = take(ints, 6)?;
            let spline_len = |spline: i32| -> VmResult<usize> {
                let spline = usize::try_from(spline).map_err(|_| {
                    trap(command, Fault::IndexOutOfRange.message("cutscene spline"))
                })?;
                engine.game_host.cutscene_spline[spline]
                    .as_ref()
                    .map(|s| s.len() >> 1)
                    .ok_or_else(|| trap(command, Fault::MissingValue.message("cutscene spline")))
            };
            if a[0] >= 2 {
                return Err(trap(command, Fault::InvalidState.message("spline index")));
            }
            if a[1] + 1 >= spline_len(a[0])? as i32 {
                return Err(trap(
                    command,
                    Fault::InvalidState.message("spline keyframe"),
                ));
            }
            if a[4] >= 2 {
                return Err(trap(
                    command,
                    Fault::InvalidState.message("target spline index"),
                ));
            }
            if a[5] + 1 >= spline_len(a[4])? as i32 {
                return Err(trap(
                    command,
                    Fault::InvalidState.message("target spline keyframe"),
                ));
            }
            let keyframe = |v: i32| {
                usize::try_from(v)
                    .map_err(|_| trap(command, Fault::IndexOutOfRange.message("cutscene spline")))
            };
            engine.camera.cam2.legacy.move_along = Some(crate::ui_cam2::LegacyMoveAlong {
                pos_spline: a[0] as usize,
                pos_keyframe: keyframe(a[1])?,
                target_spline: a[4] as usize,
                target_keyframe: keyframe(a[5])?,
                progress: 0,
                speed_min: a[2],
                speed_max: a[3],
            });
            engine.camera.cam2.camera_state = 6;
            engine.scene.server_roof = [-1, -1];
            Ok(None)
        }
        "cam_followcoord" => {
            let [x, z] = engine.scene_tile(pop(ints)?);
            engine.camera.cam2.legacy.follow_coord = Some([(x << 9) + 256, (z << 9) + 256]);
            engine.camera.cam2.camera_state = 4;
            engine.scene.server_roof = [-1, -1];
            engine.camera.cam2.changed = true;
            Ok(None)
        }
        "cam_removeroof" => {
            let packed = pop(ints)?;
            engine.scene.server_roof = if packed == -1 {
                [-1, -1]
            } else {
                let [x, z] = engine.scene_tile(packed);
                [(x << 9) + 256, (z << 9) + 256]
            };
            Ok(None)
        }
        "cam_forceangle" => {
            let a = take(ints, 2)?;
            engine.camera_force_angle(a[0], a[1], 0);
            Ok(None)
        }
        "cam_reset" => {
            let state = engine.camera.cam2.default_state;
            engine.camera.cam2.camera_reset(state);
            engine.scene.server_roof = [-1, -1];
            Ok(None)
        }
        "cam_smoothreset" => {
            engine.camera.cam2.camera_smooth_reset();
            engine.scene.server_roof = [-1, -1];
            Ok(None)
        }
        "cam_inc_x" | "cam_dec_x" | "cam_inc_y" | "cam_dec_y" => {
            let pitch = command.ends_with('x');
            let positive = command.starts_with("cam_inc");
            engine.game_host.orbit_inputs.push((pitch, positive));
            engine.camera.cam2.changed = true;
            Ok(None)
        }
        "cam_getangle_xa" => int(engine.game_host.orbit_pitch as i32 >> 3),
        "cam_getangle_ya" => int(if engine.camera.cam2.camera_state == 3 {
            (f64::from(engine.camera.cam2.yaw()) * 2607.5945876176133) as i32 >> 3
        } else {
            engine.game_host.orbit_yaw as i32 >> 3
        }),
        "cam_getfollowheight" => int(engine.game_host.follow_height - FOLLOW_HEIGHT_BASE),
        "cam_setfollowheight" => {
            let height = pop(ints)?.max(0);
            engine.game_host.follow_height = FOLLOW_HEIGHT_BASE + height;
            engine.game_host.follow_height_changed = true;
            Ok(None)
        }
        // Cutscene spline commands.
        "spline_new" => {
            let a = take(ints, 2)?;
            if (0..2).contains(&a[0]) {
                let rows = usize::try_from(a[1] << 1)
                    .map_err(|_| trap(command, Fault::NegativeSize.message("spline keyframes")))?;
                engine.game_host.cutscene_spline[a[0] as usize] = Some(vec![vec![0; 4]; rows]);
            }
            Ok(None)
        }
        "spline_addpoint" => {
            let a = take(ints, 7)?;
            let row = a[1] << 1;
            if (0..2).contains(&a[0]) {
                if let Some(spline) = engine.game_host.cutscene_spline[a[0] as usize].as_mut() {
                    if row >= 0 && (row as usize) < spline.len() {
                        let row = row as usize;
                        spline[row] = vec![
                            ((a[2] >> 14) & 0x3FFF) << 9,
                            a[3] << 2,
                            (a[2] & 0x3FFF) << 9,
                            a[6],
                        ];
                        // The next keyframe row is written without a bound check.
                        let next = spline.get_mut(row + 1).ok_or_else(|| {
                            trap(command, Fault::IndexOutOfRange.message("cutscene spline"))
                        })?;
                        *next = vec![
                            ((a[4] >> 14) & 0x3FFF) << 9,
                            a[5] << 2,
                            (a[4] & 0x3FFF) << 9,
                        ];
                    }
                }
            }
            Ok(None)
        }
        "spline_length" => {
            let index = pop(ints)?;
            let spline = usize::try_from(index)
                .ok()
                .filter(|&i| i < 2)
                .ok_or_else(|| trap(command, Fault::IndexOutOfRange.message("cutscene spline")))?;
            let len = engine.game_host.cutscene_spline[spline]
                .as_ref()
                .ok_or_else(|| trap(command, Fault::MissingValue.message("cutscene spline")))?
                .len();
            int((len >> 1) as i32)
        }
        _ => Err(absent(command)),
    }
}

/// Component commands; `None` when `c.command` is not in this family.
pub(crate) fn component(
    ctx: &mut Context<'_>,
    store: &mut Store,
    active: &mut [Active; 2],
    c: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
    longs: &mut Vec<i64>,
) -> Option<anyhow::Result<Option<Value>>> {
    if !COMPONENT_COMMANDS.contains(&c.command) {
        return None;
    }
    Some(run_component(ctx, store, active, c, ints, strs, longs))
}

fn apop(ints: &mut Vec<i32>) -> anyhow::Result<i32> {
    ints.pop().context("component stack underflow")
}
fn atake(ints: &mut Vec<i32>, n: usize) -> anyhow::Result<Vec<i32>> {
    anyhow::ensure!(ints.len() >= n, "component stack underflow");
    Ok(ints.split_off(ints.len() - n))
}

/// The component named by a packed id (`if_*`) or the active component
/// (`cc_*`).
fn target(
    store: &mut Store,
    active: &[Active; 2],
    c: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
) -> anyhow::Result<crate::ui_components::Ref> {
    if c.command.starts_with("if_") {
        let packed = apop(ints)?;
        store
            .get(packed, -1)?
            .context(Fault::MissingValue.message("component"))
    } else {
        active[c.secondary as usize]
            .component
            .clone()
            .context(Fault::MissingValue.message("active component"))
    }
}

/// The next customisation id (a process-wide counter).
fn next_customisation_id() -> i64 {
    thread_local! {
        static CUSTOMISATION_ID: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    }
    let id = CUSTOMISATION_ID.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    });
    (i64::from(id) << 32) | 0xFFFF_FFFF
}

fn run_component(
    ctx: &mut Context<'_>,
    store: &mut Store,
    active: &mut [Active; 2],
    c: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
    longs: &mut Vec<i64>,
) -> anyhow::Result<Option<Value>> {
    use crate::ui_interaction::Action;
    let command = c.command;
    // if_debug_button1..10 trigger the component's operation.
    if let Some(op) = command.strip_prefix("if_debug_button") {
        let op: i32 = op.parse()?;
        let a = atake(ints, 2)?;
        ctx.state.interaction.script_actions.push_back(Action::Op {
            op,
            parent: a[0],
            child: a[1],
            base: Some(vec![]),
        });
        return Ok(None);
    }
    match command {
        // if_debug_getcomcount/getcomname/getname/getservertriggers read the
        // interface table without loading it. getcomname/getservertriggers
        // index `components` with the whole packed id, unmasked.
        "if_debug_getcomcount" => {
            let id = apop(ints)?;
            let count = store
                .interfaces
                .get(&id)
                .map_or(0, |i| i.borrow().components.borrow().len() as i32);
            Ok(Some(Value::Int(count)))
        }
        "if_debug_getcomname" | "if_debug_getservertriggers" => {
            let packed = apop(ints)?;
            let Some(interface) = store.interfaces.get(&(packed >> 16)).cloned() else {
                return Ok(Some(if command == "if_debug_getcomname" {
                    Value::Str(String::new())
                } else {
                    Value::Int(0)
                }));
            };
            let component = usize::try_from(packed)
                .ok()
                .and_then(|i| interface.borrow().components.borrow().get(i).cloned())
                .context(Fault::IndexOutOfRange.message("interface components"))?
                .context(Fault::MissingValue.message("interface component"))?;
            let f = &component.borrow().f;
            Ok(Some(if command == "if_debug_getcomname" {
                Value::Str(
                    f.name
                        .as_deref()
                        .map(String::from_utf16_lossy)
                        .unwrap_or_default(),
                )
            } else {
                Value::Int(f.serverTriggers)
            }))
        }
        "if_debug_getname" => {
            let id = apop(ints)?;
            let Some(interface) = store.interfaces.get(&id).cloned() else {
                return Ok(Some(Value::Str(String::new())));
            };
            let first = interface
                .borrow()
                .components
                .borrow()
                .first()
                .cloned()
                .context(Fault::IndexOutOfRange.message("interface components"))?
                .context(Fault::MissingValue.message("first interface component"))?;
            let Some(name) = first.borrow().f.name.clone() else {
                return Ok(Some(Value::Str(String::new())));
            };
            let colon = name
                .iter()
                .position(|&u| u == u16::from(b':'))
                .context(Fault::IndexOutOfRange.message("component name has no separator"))?;
            Ok(Some(Value::Str(String::from_utf16_lossy(&name[..colon]))))
        }
        // if_debug_getopenifid.
        "if_debug_getopenifid" => {
            let mut n = apop(ints)?;
            let life = &ctx.state.life;
            if life.top != -1 {
                if n == 0 {
                    return Ok(Some(Value::Int(life.top)));
                }
                n -= 1;
            }
            let mut subs = life.subs.ordered();
            let mut sub = subs.next();
            while n > 0 {
                n -= 1;
                sub = subs.next();
            }
            let sub = sub.context(Fault::MissingValue.message("sub-interface"))?;
            Ok(Some(Value::Int(sub.borrow().id)))
        }
        // Character index at a pixel position.
        "if_getcharindexatpos" | "cc_getcharindexatpos" => {
            let com = target(store, active, c, ints)?;
            let metrics = ctx
                .state
                .component_metrics(&com)?
                .context(Fault::MissingValue.message("font metrics"))?;
            let y = apop(ints)?;
            let x = apop(ints)?;
            let f = com.borrow().f.clone();
            let index = char_index_at_pos(
                ctx,
                &metrics,
                f.text.as_deref(),
                f.width,
                f.textLineHeight,
                x,
                y,
            )?;
            Ok(Some(Value::Int(index)))
        }
        // Pixel position of a character index.
        "if_getcharposatindex" | "cc_getcharposatindex" => {
            let com = target(store, active, c, ints)?;
            let metrics = ctx
                .state
                .component_metrics(&com)?
                .context(Fault::MissingValue.message("font metrics"))?;
            let index = apop(ints)?;
            let f = com.borrow().f.clone();
            let [x, y] = char_pos_at_index(
                ctx,
                &metrics,
                f.text.as_deref(),
                f.width,
                f.textLineHeight,
                index,
            )?
            .context(Fault::MissingValue.message("character position"))?;
            ints.extend([x, y]);
            Ok(None)
        }
        // if_getgraphicdimensions /
        // cc_getgraphicdimensions (the padded sprite width/height).
        "if_getgraphicdimensions" | "cc_getgraphicdimensions" => {
            let com = target(store, active, c, ints)?;
            let size = ctx
                .state
                .component_graphic(&com, &mut crate::ui_sprites::Cpu)?
                .map_or([-1, -1], |mask| mask.size);
            ints.extend(size);
            Ok(None)
        }
        // Custom NPC body / head models on a component.
        "if_npc_setcustombodymodel"
        | "cc_npc_setcustombodymodel"
        | "if_npc_setcustombodymodel_transformed"
        | "cc_npc_setcustombodymodel_transformed"
        | "if_npc_setcustomheadmodel"
        | "cc_npc_setcustomheadmodel" => {
            let com = target(store, active, c, ints)?;
            let head = command.ends_with("headmodel");
            let (slot, model, scale, rotation, offset) = if command.ends_with("_transformed") {
                let a = atake(ints, 10)?;
                let scale = a[2] as f32 / a[3].max(1) as f32;
                (
                    a[0] - 1,
                    a[1],
                    scale,
                    [a[4], a[5], a[6]],
                    [a[7], a[8], a[9]],
                )
            } else {
                let model = apop(ints)?;
                (apop(ints)? - 1, model, 1.0, [0; 3], [0; 3])
            };
            let kind = com.borrow().f.modelkind;
            anyhow::ensure!(
                kind == if head { 2 } else { 6 },
                Fault::InvalidState.message(format_args!("model kind {kind}"))
            );
            anyhow::ensure!(
                (0..if head { 5 } else { 12 }).contains(&slot),
                Fault::InvalidState.message(format_args!("custom model slot {slot}"))
            );
            {
                let mut component = com.borrow_mut();
                if component.npc_customisation.is_none() {
                    let npc = npc_type(ctx, component.f.model)?;
                    component.npc_customisation =
                        Some(crate::ui_models::NpcCustomisation::new(&npc, !head)?);
                }
                let custom = component.npc_customisation.as_mut().unwrap();
                custom.cache_key_salt = next_customisation_id();
                if head {
                    custom.set_model(slot as usize, model);
                } else {
                    custom.set_model_transform(slot as usize, model, scale, rotation, offset);
                }
            }
            store.updated.push(com);
            Ok(None)
        }
        // cc_if_npc_setcustomrecol/retex.
        "if_npc_setcustomrecol"
        | "cc_npc_setcustomrecol"
        | "if_npc_setcustomretex"
        | "cc_npc_setcustomretex" => {
            let com = target(store, active, c, ints)?;
            let value = apop(ints)?;
            let slot = apop(ints)? - 1;
            let recol = command.ends_with("recol");
            let kind = com.borrow().f.modelkind;
            anyhow::ensure!(
                kind == 6 || kind == 2,
                Fault::InvalidState.message(format_args!("model kind {kind}"))
            );
            {
                let mut component = com.borrow_mut();
                let npc = npc_type(ctx, component.f.model)?;
                if component.npc_customisation.is_none() {
                    component.npc_customisation =
                        Some(crate::ui_models::NpcCustomisation::new(&npc, kind == 6)?);
                }
                let custom = component.npc_customisation.as_mut().unwrap();
                custom.cache_key_salt = next_customisation_id();
                let indices = if recol {
                    &npc.recolindices
                } else {
                    &npc.retexindices
                };
                let mut index = slot;
                if let Some(indices) = indices {
                    index = i32::from(
                        *usize::try_from(slot)
                            .ok()
                            .and_then(|s| indices.get(s))
                            .context(Fault::InvalidState.message("custom colour slot"))?,
                    );
                }
                let target = if recol {
                    custom.custom_recol_d.as_mut()
                } else {
                    custom.custom_retex_d.as_mut()
                };
                let target =
                    target.context(Fault::InvalidState.message("NPC has no recolour table"))?;
                *usize::try_from(index)
                    .ok()
                    .and_then(|i| target.get_mut(i))
                    .context(Fault::InvalidState.message("custom colour index"))? = value as i16;
            }
            store.updated.push(com);
            Ok(None)
        }
        // Resume a pause button (RESUME_PAUSEBUTTON p4_alt3/p2_alt2).
        "if_resume_pausebutton" | "cc_resume_pausebutton" => {
            let com = target(store, active, c, ints)?;
            let mask = ctx.state.layout.active_mask(&com);
            if crate::ui_minimenu::mask_pausebutton(mask)
                && ctx.state.life.pressed_continue.is_none()
            {
                let (parent, child) = {
                    let f = &com.borrow().f;
                    (f.parentlayer, f.id)
                };
                let mut out = vec![crate::proto::client::RESUME_PAUSEBUTTON];
                crate::ui_interaction::p4(&mut out, parent, 3);
                crate::ui_interaction::p2(&mut out, child, 2);
                ctx.state.interaction.outgoing.extend(out);
                ctx.state.life.pressed_continue = store.get(parent, child)?;
                if let Some(pressed) = &ctx.state.life.pressed_continue {
                    store.updated.push(pressed.clone());
                }
            }
            Ok(None)
        }
        // Friend, friend chat and player group links (group user kind ids).
        "if_setlinkfriend"
        | "cc_setlinkfriend"
        | "if_setlinkfriendchat"
        | "cc_setlinkfriendchat"
        | "if_setlinkplayergroup"
        | "cc_setlinkplayergroup" => {
            let named = command.starts_with("if_");
            let (com, members) = if command.ends_with("playergroup") {
                if named {
                    let a = atake(ints, 2)?;
                    let com = store
                        .get(a[1], -1)?
                        .context(Fault::MissingValue.message("component"))?;
                    (com, Some(a[0] == 1))
                } else {
                    let com = target(store, active, c, ints)?;
                    (com, Some(apop(ints)? == 1))
                }
            } else {
                (target(store, active, c, ints)?, None)
            };
            let index = apop(ints)?;
            let game = &ctx.state.game;
            let (kind, link) = match (command.ends_with("friend"), members) {
                (true, _) => {
                    let friend = usize::try_from(index)
                        .ok()
                        .filter(|_| game.friends_list_state == 2)
                        .and_then(|i| game.friends.get(i));
                    let Some(friend) = friend else {
                        return Ok(None);
                    };
                    (0, Some(friend.clone()))
                }
                (false, None) => (
                    5,
                    game.friend_chat
                        .as_ref()
                        .and_then(|users| usize::try_from(index).ok().and_then(|i| users.get(i)))
                        .cloned(),
                ),
                (false, Some(members)) => {
                    let (list, banned) = game
                        .player_group
                        .as_ref()
                        .context(Fault::MissingValue.message("player group"))?;
                    let names = if members { list } else { banned };
                    let name = usize::try_from(index)
                        .ok()
                        .and_then(|i| names.get(i))
                        .context(Fault::MissingValue.message("player group member"))?;
                    (if members { 1 } else { 2 }, Some(name.clone()))
                }
            };
            let mut component = com.borrow_mut();
            if let Some(link) = link {
                component.f.link = Some(link.encode_utf16().collect());
            }
            component.group_kind = Some(kind);
            Ok(None)
        }
        // Gamepad and pinch hooks are parsed and discarded; no component
        // field changes.
        "if_setongamepadaxis"
        | "cc_setongamepadaxis"
        | "if_setongamepadbutton"
        | "cc_setongamepadbutton"
        | "if_setongamepadbuttonheld"
        | "cc_setongamepadbuttonheld"
        | "if_setongamepadtrigger"
        | "cc_setongamepadtrigger"
        | "if_setonhorizontalpinch"
        | "cc_setonhorizontalpinch"
        | "if_setonverticalpinch"
        | "cc_setonverticalpinch" => {
            // The cc_ forms read the component reference without using it.
            if command.starts_with("if_") {
                let packed = apop(ints)?;
                store.get(packed, -1)?;
                anyhow::ensure!(packed >> 16 >= 0, "negative interface array index");
            }
            crate::ui_properties::parse_hook(ints, strs, longs)?;
            Ok(None)
        }
        // opplayer.
        "opplayer" => {
            let op = apop(ints)?;
            let name = strs.pop().context("component string stack underflow")?;
            let game = &ctx.state.game;
            let found = game.players.iter().find(|(index, player_name, _)| {
                Some(*index) != game.local_player
                    && player_name
                        .as_deref()
                        .is_some_and(|n| equals_ignore_case(n, &name))
            });
            if let Some(&(index, _, _)) = found {
                if let Some((opcode, payload)) =
                    crate::ui_player_options::build_opplayer_packet(43 + op, index as u16, false)
                        .filter(|_| (1..=10).contains(&op))
                {
                    ctx.state.interaction.outgoing.push(opcode);
                    ctx.state.interaction.outgoing.extend(payload);
                }
            } else {
                ctx.state.game.system_messages.push((
                    4,
                    format!("{}{name}", rs910_core::texts::Msg::UnableToFind.get()),
                ));
            }
            Ok(None)
        }
        // opplayert.
        "opplayert" => {
            let name = strs.pop().context("component string stack underflow")?;
            let target = ctx.state.interaction.target.clone();
            if !target.active || target.mask & 0x18 == 0 {
                return Ok(None);
            }
            let game = &ctx.state.game;
            let found = game.players.iter().find(|(index, player_name, _)| {
                player_name
                    .as_deref()
                    .is_some_and(|n| equals_ignore_case(n, &name))
                    && (Some(*index) == game.local_player && target.mask & 0x10 != 0
                        || target.mask & 0x8 != 0)
            });
            if let Some(&(index, _, route)) = found {
                let (opcode, payload) = crate::ui_player_options::build_opplayert(
                    15,
                    index as u16,
                    target.child,
                    target.object,
                    target.parent,
                    false,
                )
                .context("OPPLAYERT builder")?;
                ctx.state.interaction.outgoing.push(opcode);
                ctx.state.interaction.outgoing.extend(payload);
                ctx.state.game.minimap_flag = Some(route);
            } else {
                ctx.state.game.system_messages.push((
                    4,
                    format!("{}{name}", rs910_core::texts::Msg::UnableToFind.get()),
                ));
            }
            ctx.state
                .interaction
                .script_actions
                .push_back(Action::ClearTarget);
            Ok(None)
        }
        _ => anyhow::bail!("unrouted game component command {command}"),
    }
}

/// The NPC type for `id`: an absent file decodes to the defaults.
fn npc_type(ctx: &Context<'_>, id: i32) -> anyhow::Result<crate::config::Npc> {
    let npcs = ctx.state.npcs.as_ref().context("npc types not installed")?;
    Ok(match u32::try_from(id).ok().and_then(|id| npcs.get(id)) {
        Some(npc) => npc.clone(),
        None => crate::config::decode_npc(id as u32, &[0])?,
    })
}

const BR: [u16; 4] = [b'<' as u16, b'b' as u16, b'r' as u16, b'>' as u16];

/// Pixel position of a character index (text split without trimming).
fn char_pos_at_index(
    ctx: &Context<'_>,
    metrics: &crate::font_metrics::Metrics,
    text: Option<&[u16]>,
    width: i32,
    mut line_height: i32,
    mut index: i32,
) -> anyhow::Result<Option<[i32; 2]>> {
    let fonts = ctx
        .state
        .fonts
        .as_ref()
        .context("font provider not installed")?;
    let text = text.context(Fault::MissingValue.message("component text"))?;
    if index <= 0 {
        return Ok(Some([0, metrics.ascent + line_height]));
    }
    if index > text.len() as i32 {
        index = text.len() as i32;
    }
    if line_height == 0 {
        line_height = metrics.space_width;
    }
    let (lines, n) = fonts.lines(metrics, Some(text), width, false)?;
    let index = index as usize;
    let mut start = 0usize;
    for (mut line, l) in lines[..n].iter().enumerate() {
        let len = l.len();
        if start + len > index || n - 1 == line && index == text.len() {
            let mut sub = &text[start..index];
            if sub.ends_with(&BR) {
                sub = &[];
                line += 1;
            }
            let x = fonts.width(metrics, Some(sub))?;
            return Ok(Some([x, metrics.ascent + line_height * line as i32]));
        }
        start += len;
    }
    Ok(None)
}

/// Character index at a pixel position (text split without trimming).
fn char_index_at_pos(
    ctx: &Context<'_>,
    metrics: &crate::font_metrics::Metrics,
    text: Option<&[u16]>,
    width: i32,
    mut line_height: i32,
    x: i32,
    mut y: i32,
) -> anyhow::Result<i32> {
    let fonts = ctx
        .state
        .fonts
        .as_ref()
        .context("font provider not installed")?;
    let text = text.context(Fault::MissingValue.message("component text"))?;
    if line_height == 0 {
        line_height = metrics.space_width;
    }
    let (lines, n) = fonts.lines(metrics, Some(text), width, false)?;
    if n == 0 {
        return Ok(0);
    }
    if y < 0 {
        y = 0;
    }
    anyhow::ensure!(
        line_height != 0,
        Fault::DivisionByZero.message("line height")
    );
    let mut line = (y / line_height) as usize;
    if line >= n {
        line = n - 1;
    }
    let s = &lines[line];
    let mut i = 0usize;
    let mut previous = 0;
    let mut current = 0;
    while current < x && i < s.len() {
        previous = current;
        i += 1;
        current = fonts.width(metrics, Some(&s[..i]))?;
    }
    let mut index = i as i32;
    if i == s.len() && s.ends_with(&BR) {
        index -= 4;
    }
    if x - previous < current - x {
        index -= 1;
    }
    for l in &lines[..line] {
        index += l.len() as i32;
    }
    Ok(index)
}

impl Runtime {
    /// Per logic update, before the interface walk: mirror the scene inputs
    /// this family reads and step the scripted camera path.
    pub(crate) fn sync_host_game(&mut self, vars: &Variables<'_>) -> Result<(), anyhow::Error> {
        let host = &mut self.engine.game_host;
        host.base = [vars.scene.base[0] >> 9, vars.scene.base[1] >> 9];
        host.size = [vars.scene.map_width, vars.scene.map_width];
        let players = vars.scene.players;
        host.level = players.map_or(0, |p| p.current_level);
        self.engine.camera.cam2.legacy.loop_cycle = vars.cycle;
        let local = vars.scene.local_player.and_then(|l| {
            players?
                .players
                .get(l.index as usize)?
                .as_ref()
                .map(|p| (l.index, p))
        });
        host.local_angle = local.map(|(_, p)| p.angle);
        if !host.model_dirty {
            host.local_model = local.and_then(|(_, p)| p.appearance.model.clone());
        }
        let game = &mut self.state.game;
        game.local_player = local.map(|(index, _)| index);
        game.players = players.map_or_else(Vec::new, |ps| {
            ps.high_indices
                .iter()
                .filter_map(|&i| {
                    let p = ps.players.get(i)?.as_ref()?;
                    Some((i as i32, p.appearance.name.clone(), [p.x[0], p.z[0]]))
                })
                .collect()
        });
        // Only with a built scene.
        if vars.scene.terrain.is_some() && self.engine.camera.cam2.camera_state == 5 {
            let level = self.engine.game_host.level;
            let terrain = vars.scene.terrain;
            self.engine.camera.cam2.legacy.apply_cutscene(&|x, z| {
                crate::protocol910::terrain::height(terrain, x, z, level).unwrap_or(0)
            });
        } else if self.engine.camera.cam2.camera_state == 6 {
            let mut legacy = std::mem::take(&mut self.engine.camera.cam2.legacy);
            let result = self.engine.game_host.move_along(&mut legacy);
            self.engine.camera.cam2.legacy = legacy;
            result?;
        }
        self.state.life.cycles.spline_finished = self.engine.game_host.spline_finished;
        Ok(())
    }

    /// The UI-owned half of `logout`: the cutscene reset restores the saved
    /// viewport limits, and the camera is destroyed.
    pub(crate) fn logout_camera_and_cutscene(&mut self) {
        self.restore_cutscene_state();
        self.engine.camera.cutscene.scene_state = 3;
        self.engine.camera.cutscene.requests.clear();
        self.engine.camera.free_camera = None;
    }

    /// UI half of the cutscene reset: restore the viewport limits saved when
    /// the cutscene started and forget the saved song.
    fn restore_cutscene_state(&mut self) {
        if let Some([min_h, max_h, min_f, max_f]) = self.engine.camera.cutscene.saved.take() {
            let p = &mut self.state.viewport_profile;
            p.min_height = min_h;
            p.max_height = max_h;
            p.min_fov = min_f;
            p.max_fov = max_f;
        }
        self.engine.camera.cutscene.saved_song = -1;
    }

    /// Tests the cutscene cancel binding against the pending mouse event and
    /// key presses while the scene state is not 3.
    pub(crate) fn test_cutscene_cancel(&mut self, event: Option<crate::ui_defaults::MouseEvent>) {
        let cutscene = &self.engine.camera.cutscene;
        if cutscene.scene_state == 3 {
            self.engine.camera.cutscene.cancel = false;
            return;
        }
        let held = |k: i32| self.keyboard.held(k);
        // The binding test compares the key code and the modifier mask, not
        // the typed character.
        let presses: Vec<_> = self
            .key_presses
            .iter()
            .map(|e| (e.code, e.modifiers))
            .collect();
        let cancel = cutscene
            .cancel_binding
            .as_ref()
            .is_some_and(|b| b.test(event.as_ref(), &presses, &held));
        self.engine.camera.cutscene.cancel = cancel;
    }

    /// Runs a cutscene trigger script with optional subtitle locals.
    fn run_cutscene_trigger(
        &mut self,
        vars: &mut Variables<'_>,
        trigger: i32,
        primary: i32,
        locals: Option<(String, i32)>,
    ) -> Result<(), anyhow::Error> {
        let provider = super::Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let found = provider.get_trigger(trigger, primary, -1)?;
        if crate::game_debug_flags::flags().cutscene_trace {
            log::info!(
                "[cutscene] trigger {trigger} ({primary}, -1) {:?} locals {locals:?}",
                found.as_ref().map(|(id, _)| id)
            );
        }
        let Some((script_id, script)) = found else {
            return Ok(());
        };
        let mut runner = super::Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: super::Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        let (ints, strings) = match locals {
            Some((text, id)) => (vec![id], Some(vec![text.encode_utf16().collect()])),
            None => (vec![], None),
        };
        runner.run_compiled_with_locals(
            &mut self.store,
            &mut self.state,
            script_id,
            &script,
            crate::ui_hooks::INTERACTIVE_LIMIT,
            crate::ui_hook_host::TriggerLocals {
                ints: &ints,
                strings,
            },
        )?;
        self.diagnostics.record(runner.executions, runner.missing);
        Ok(())
    }

    /// Apply this update's cutscene side effects (`crate::cutscene::UiRequest`)
    /// in the order the cutscene actions performed them.
    pub(crate) fn apply_cutscene_requests(
        &mut self,
        vars: &mut Variables<'_>,
    ) -> Result<(), anyhow::Error> {
        use crate::cutscene::UiRequest;
        let sound = |command: &str, args: Vec<i32>| super::SoundRequest {
            command: command.into(),
            args,
        };
        for request in std::mem::take(&mut self.engine.camera.cutscene.requests) {
            match request {
                UiRequest::CloseMenu => {
                    // Close and reset the menu.
                    self.state.minimenu.close_popup();
                    self.state.menu.open = false;
                    self.state.minimenu.reset();
                }
                UiRequest::SaveState { viewport: [w, h] } => {
                    self.engine.camera.cutscene.saved = Some(self.state.set_cutscene_ratio(w, h));
                    self.engine.camera.cutscene.saved_song = self.audio.current_song();
                }
                UiRequest::RestoreState => self.restore_cutscene_state(),
                UiRequest::CameraMoveTo {
                    x,
                    z,
                    height,
                    acceleration,
                    speed,
                    instant,
                } => self
                    .engine
                    .camera_move_to(x, z, height, acceleration, speed, instant),
                UiRequest::CameraForceAngle { pitch, yaw, roll } => {
                    self.engine.camera_force_angle(pitch, yaw, roll)
                }
                UiRequest::CameraMoveAlong {
                    splines,
                    pos_keyframe,
                    target_keyframe,
                    min_speed,
                    max_speed,
                } => {
                    // Start the scripted camera path.
                    let [pos, target] = splines;
                    self.engine.game_host.cutscene_spline = [Some(pos), Some(target)];
                    self.engine.camera.cam2.legacy.move_along =
                        Some(crate::ui_cam2::LegacyMoveAlong {
                            pos_spline: 0,
                            pos_keyframe,
                            target_spline: 1,
                            target_keyframe,
                            progress: 0,
                            speed_min: min_speed,
                            speed_max: max_speed,
                        });
                    self.engine.camera.cam2.camera_state = 6;
                    let mut legacy = std::mem::take(&mut self.engine.camera.cam2.legacy);
                    let result = self.engine.game_host.move_along(&mut legacy);
                    self.engine.camera.cam2.legacy = legacy;
                    result?;
                }
                UiRequest::SoundCreate {
                    tag,
                    sound: id,
                    repeats,
                    volume,
                    rate,
                } => self.engine.effects.sounds.push(sound(
                    "cutscene_sound_create",
                    vec![tag as i32, id, repeats, volume, rate],
                )),
                UiRequest::SoundStart { tag } => self
                    .engine
                    .effects
                    .sounds
                    .push(sound("cutscene_sound_start", vec![tag as i32])),
                UiRequest::SoundCleanup { tag } => self
                    .engine
                    .effects
                    .sounds
                    .push(sound("cutscene_sound_cleanup", vec![tag as i32])),
                UiRequest::Jingle { id, volume } => self
                    .engine
                    .effects
                    .sounds
                    .push(sound("cutscene_jingle", vec![id, volume])),
                UiRequest::Song { id } => self
                    .engine
                    .effects
                    .sounds
                    .push(sound("cutscene_song", vec![id])),
                UiRequest::PreloadSong { id, volume } => self
                    .engine
                    .effects
                    .sounds
                    .push(sound("cutscene_song_preload", vec![id, volume])),
                UiRequest::RestoreSong => {
                    let song = self.engine.camera.cutscene.saved_song;
                    if song != -1 {
                        self.engine
                            .effects
                            .sounds
                            .push(sound("cutscene_song", vec![song]));
                    }
                }
                // Subtitle and end-of-cutscene requests.
                UiRequest::Subtitle {
                    cutscene,
                    text,
                    subtitle,
                } => self.run_cutscene_trigger(vars, 20, cutscene, Some((text, subtitle)))?,
                UiRequest::End { cutscene } => {
                    self.run_cutscene_trigger(vars, 28, cutscene, None)?
                }
                UiRequest::Finished { completed } => {
                    self.engine
                        .outgoing
                        .extend([crate::proto::client::CUTSCENE_FINISHED, u8::from(completed)]);
                }
            }
        }
        Ok(())
    }

    /// Social names used by `cc_if_setlink` (refreshed with the clan channel).
    pub(crate) fn sync_host_game_links(&mut self) {
        let social = &self.engine.social;
        let game = &mut self.state.game;
        game.friends_list_state = social.friends_list_state;
        game.friends = social
            .friends
            .iter()
            .map(|f| f.display_name.clone())
            .collect();
        game.friend_chat = (!social.friend_chat.users.is_empty()
            || social.friend_chat.owner_name.is_some())
        .then(|| {
            social
                .friend_chat
                .users
                .iter()
                .map(|u| u.name.clone())
                .collect()
        });
        game.player_group = social.player_group.as_ref().map(|g| {
            (
                g.members.iter().map(|m| m.display_name.clone()).collect(),
                g.banned.clone(),
            )
        });
    }

    /// After `update`: resolve the active entry's NPC/player so the
    /// `*_find_active_minimenu_entry` commands can install it as the script's
    /// active entity.
    pub(crate) fn sync_host_game_minimenu(&mut self, vars: &Variables<'_>) {
        let menu = &self.state.minimenu;
        let kind = menu.entity_type(menu.active);
        let binding = match (kind, menu.active) {
            (4, Some(e)) => Some(crate::server_prot::ActiveBinding::Npc {
                index: menu.entry(e).entity_id as i32,
            }),
            (7, Some(e)) => {
                let index = menu.entry(e).sub_id as i32;
                let high = vars
                    .scene
                    .players
                    .map_or(0, |p| p.high_indices.len() as i32);
                (index >= 0 && index <= high)
                    .then_some(crate::server_prot::ActiveBinding::Player { index })
            }
            _ => None,
        };
        self.engine.game_host.minimenu_entity =
            binding.and_then(|b| self.active_entity(&b, vars).map(|e| (kind, e)));
    }

    /// Move component-issued chat lines and minimap flags into their owners.
    pub(crate) fn drain_host_game(&mut self) {
        for (kind, message) in std::mem::take(&mut self.state.game.system_messages) {
            self.engine
                .messages
                .history
                .add_system_message(kind, message);
            self.engine.messages.changed = true;
        }
        if let Some(flag) = self.state.game.minimap_flag.take() {
            self.engine.minimap.flag = Some(flag);
        }
    }
}

/// The cutscene state `test_cutscene_cancel` reads.
pub fn sync_cutscene_input(engine: &mut Engine, game: &crate::game_runtime::Game) {
    engine.camera.cutscene.scene_state = game.cutscene.scene_state;
    engine.camera.cutscene.cancel_binding = game.cutscene.cancel_binding.clone();
}

/// Application hook before the retained VM tick: mirror the live follow
/// camera and the window cursor into the script-visible owners.
pub fn before_tick(engine: &mut Engine, game: &mut crate::game_runtime::Game, cursor: i32) {
    // Cutscene statics the interface walk and draw read this update, plus
    // the requests its actions queued (crate::cutscene::UiRequest).
    engine.camera.cutscene.scene_state = game.cutscene.scene_state;
    engine.camera.cutscene.client_id = game.cutscene.client_id;
    engine.camera.cutscene.fade = game.cutscene.fade.ui();
    engine.camera.cutscene.cancel_binding = game.cutscene.cancel_binding.clone();
    engine
        .camera
        .cutscene
        .requests
        .extend(game.cutscene.take_requests());
    let host = &mut engine.game_host;
    host.orbit_pitch = game.camera.pitch;
    host.orbit_yaw = game.camera.yaw;
    if !host.follow_height_changed {
        host.follow_height = game.camera.height;
    }
    host.current_cursor = cursor;
    host.game_connection = true;
}

/// Application hook after the retained VM tick: apply follow-camera writes
/// (orbit nudges, forced angle, follow height) and write the edited local
/// appearance model back to the player entity.
pub fn after_tick(engine: &mut Engine, game: &mut crate::game_runtime::Game) {
    // A tick whose input was not split ahead of the update (no game
    // update ran) tests the binding inside the tick; apply it next update.
    if std::mem::take(&mut engine.camera.cutscene.cancel) {
        game.cutscene.cancel_requested = true;
    }
    let host = &mut engine.game_host;
    if let Some((pitch, yaw)) = host.orbit_force.take() {
        game.camera.pitch = pitch;
        game.camera.yaw = yaw;
        game.camera.changed = true;
    }
    for (pitch, positive) in std::mem::take(&mut host.orbit_inputs) {
        game.camera.input(pitch, positive);
    }
    if std::mem::take(&mut host.follow_height_changed) {
        game.camera.height = host.follow_height;
    }
    if std::mem::take(&mut host.model_dirty) {
        let local = game.runtime.map.local;
        if let Some(player) = game.runtime.feed.state.players.players[local].as_mut() {
            player.appearance.model = host.local_model.clone();
        }
    }
}

#[cfg(test)]
#[path = "ui_host_game/tests.rs"]
mod tests;
