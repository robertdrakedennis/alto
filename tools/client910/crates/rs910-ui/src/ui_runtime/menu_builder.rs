//! The retained `MiniMenu` builder and its click path: `addSceneOptions`
//! and the `add*Entries` builders, the menu open/click/
//! dispatch path (`doAction`) and the menu-state packets.
use super::Cross;
use super::Engine;
use super::PlayerOp;
use super::Runtime;
use crate::ui_hook_host::Domains;
use crate::ui_hook_host::Runner;
use crate::ui_scripts::Provider;
use crate::ui_vars::Variables;
use anyhow::Result;
use rs910_core::fault::Fault;
use std::collections::BTreeMap;

/// The local player's var domain a `multiloc`/`multinpc` selection reads
/// (the local player game state through the variable type provider); a `None` half
/// reads as a null type.
#[derive(Clone, Copy)]
pub(super) struct LocalVars<'a> {
    pub(super) varps: Option<&'a crate::entities910::varps::Varps>,
    pub(super) varbits: Option<&'a crate::entity_runtime::bits_pack::Inputs>,
}

impl LocalVars<'_> {
    /// `getVarBitType/getVarType` then the
    /// `localPlayerGameState` value; `None` is a null type (index -1).
    pub(super) fn get(self, bit: bool, id: i32) -> Option<i32> {
        let Self { varps, varbits } = self;
        if bit {
            let definition = varbits?.get(id, false).ok()?;
            varps?.get_bit(&definition).ok()
        } else {
            varps?.get(id).ok()
        }
    }
}

/// `op` after `defaultops`:
/// the RuneScape factory fills slot 5 with `EXAMINE` and no
/// opcode writes that slot (opcodes 30-34/150-154 address 0-4). `ops` is
/// [`crate::config::Npc::ops_for`] under the factory's `allowMembers`.
pub(crate) fn npc_type_ops(ops: [Option<&str>; 5]) -> [Option<&str>; 6] {
    let [a, b, c, d, e] = ops;
    [a, b, c, d, e, Some(rs910_core::texts::Msg::Examine.get())]
}

/// `addNPCEntries`: a non-zero `vislevel` appends the
/// combat-level colour tag and ` (level: N)` in the client's language.
pub(crate) fn npc_menu_name(name: String, vislevel: i32, local_combat: i32) -> String {
    if vislevel == 0 {
        return name;
    }
    format!(
        "{}{} ({}{})",
        name,
        crate::ui_player_options::combat_colour(vislevel, local_combat),
        rs910_core::texts::Msg::Level.get(),
        vislevel
    )
}

pub(super) const NPC_OP_CODES: [i32; 6] = [9, 10, 11, 12, 13, 1003];

/// The NPC side of `addNPCEntries`' operation passes: the type's
/// ops after `defaultops`, `reprioritiseAttackOp`, cursors and
/// `cursorattack`, and the live NPC's op mask and vislevel.
#[derive(Clone, Copy)]
pub(super) struct NpcOps<'a> {
    pub(super) ops: &'a [Option<&'a str>; 6],
    pub(super) op_mask: i32,
    pub(super) reprioritise: i8,
    pub(super) vislevel: i32,
    pub(super) cursors: &'a [i32; 6],
    pub(super) cursor_attack: i32,
}

/// The local-player side: `npcAttackPriority`, the local combat
/// level and `defaultMenuCursor`.
#[derive(Clone, Copy)]
pub(super) struct AttackMenu {
    pub(super) priority: crate::ui_player_options::AttackPriority,
    pub(super) local_combat: i32,
    pub(super) default_menu_cursor: i32,
}

/// `addNPCEntries` operation passes. Returns
/// `(op index, menu action, cursor)` in the original client insertion order. The first pass
/// walks slots 5..0, skipping masked ops; attack/examine are hidden or
/// delayed by `npcAttackPriority`, and the delayed pass (0..5) adds the
/// `+2000` right-click offset for always-right or higher-level-right.
/// Cursors start at `defaultMenuCursor`, take `getCursor(slot)` when
/// set and `cursorattack` for Attack (unconditionally,).
pub(super) fn npc_menu_operation_slots(
    npc: NpcOps<'_>,
    menu: AttackMenu,
) -> Vec<(usize, i32, i32)> {
    use crate::ui_player_options::AttackPriority;
    let NpcOps {
        ops,
        op_mask,
        reprioritise,
        vislevel,
        cursors,
        cursor_attack,
    } = npc;
    let AttackMenu {
        priority,
        local_combat,
        default_menu_cursor,
    } = menu;
    let mut out = Vec::new();
    let mut delayed = false;
    for op_index in (0..ops.len()).rev() {
        let Some(op) = ops[op_index] else {
            continue;
        };
        if op_mask & (1 << op_index) != 0 {
            continue;
        }
        let mut cursor = default_menu_cursor;
        if cursors[op_index] != -1 {
            cursor = cursors[op_index];
        }
        let is_attack = op.eq_ignore_ascii_case(rs910_core::texts::Msg::Attack.get());
        let is_examine = op.eq_ignore_ascii_case(rs910_core::texts::Msg::Examine.get());
        if is_attack || is_examine {
            if delayed {
                continue;
            }
            match priority {
                AttackPriority::Hidden if is_attack => continue,
                AttackPriority::Default | AttackPriority::HigherLevelRight if reprioritise == 1 => {
                    delayed = true;
                    continue;
                }
                AttackPriority::AlwaysRight => {
                    delayed = true;
                    continue;
                }
                _ => {}
            }
            if is_attack {
                cursor = cursor_attack;
            }
        }
        out.push((op_index, NPC_OP_CODES[op_index], cursor));
    }
    if delayed {
        for op_index in 0..ops.len() {
            let Some(op) = ops[op_index] else {
                continue;
            };
            if op_mask & (1 << op_index) != 0 {
                continue;
            }
            let is_attack = op.eq_ignore_ascii_case(rs910_core::texts::Msg::Attack.get());
            let is_examine = op.eq_ignore_ascii_case(rs910_core::texts::Msg::Examine.get());
            if !is_attack && !is_examine {
                continue;
            }
            let mut cursor = default_menu_cursor;
            if cursors[op_index] != -1 {
                cursor = cursors[op_index];
            }
            let right = matches!(priority, AttackPriority::AlwaysRight)
                || matches!(priority, AttackPriority::HigherLevelRight) && vislevel > local_combat;
            let action = NPC_OP_CODES[op_index] + if right { 2000 } else { 0 };
            if is_attack {
                cursor = cursor_attack;
            }
            out.push((op_index, action, cursor));
        }
    }
    out
}

impl Engine {
    /// Server menu state (`SET_MOVEACTION`, `SHOW_FACE_HERE`, `SET_PLAYER_OP`,
    /// `REDUCE_PLAYER_ATTACK_PRIORITY`). Returns
    /// true when the event is owned here; `Runtime::packet` returns early on
    /// true, before the canvas gate. Only `SHOW_FACE_HERE` bumps the
    /// verification stamp (the verify id is incremented per packet even when
    /// the value is unchanged); the others store silently like the original client.
    pub fn apply_menu_state(
        &mut self,
        event: &crate::server_prot::UiEvent,
        life: &mut crate::ui_lifecycle::Life,
    ) -> bool {
        match event {
            crate::server_prot::UiEvent::SetMoveAction { text, action } => {
                self.menu.walk_here_text = text.clone();
                self.menu.default_walk_action = *action;
                true
            }
            crate::server_prot::UiEvent::ShowFaceHere { enabled } => {
                self.menu.show_face_here = *enabled;
                life.verify = life.verify.wrapping_add(1);
                life.verify_changed = true;
                true
            }
            crate::server_prot::UiEvent::SetPlayerOp {
                slot,
                cursor,
                name,
                deprioritised,
            } => {
                if (1..=8).contains(slot) {
                    self.menu.player_ops[usize::from(*slot - 1)] = Some(PlayerOp {
                        name: name.clone(),
                        cursor: *cursor,
                        deprioritised: *deprioritised,
                    });
                }
                true
            }
            crate::server_prot::UiEvent::PlayerAttackPriority { value } => {
                use crate::ui_player_options::AttackPriority::*;
                // The attack priority id, with the original client's fallback.
                self.menu.player_attack_priority = match value {
                    0 => Default,
                    2 => AlwaysRight,
                    3 => Hidden,
                    _ => HigherLevelRight,
                };
                true
            }
            crate::server_prot::UiEvent::NpcAttackPriority { value } => {
                use crate::ui_player_options::AttackPriority::*;
                self.menu.npc_attack_priority = match value {
                    0 => Default,
                    2 => AlwaysRight,
                    3 => Hidden,
                    _ => HigherLevelRight,
                };
                true
            }
            _ => false,
        }
    }
    pub(super) fn minimenu_colour(&self, obj: i32) -> anyhow::Result<i32> {
        let objs = self
            .configs
            .objs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("obj types not installed"))?;
        // An absent file decodes to a default object type.
        let default = crate::config::Obj {
            inventory: Default::default(),
            id: obj as u32,
            name: "null".into(),
            models: vec![],
            head_models: [[-1; 2]; 2],
            recol_s: vec![],
            recol_d: vec![],
            retex_s: vec![],
            retex_d: vec![],
            ops: Default::default(),
            iops: Default::default(),
            members: false,
            minimenu_colour: None,
            params: Vec::new(),
            category: -1,
            scattered_drop: false,
        };
        let o = u32::try_from(obj)
            .ok()
            .and_then(|id| objs.get(id))
            .unwrap_or(&default);
        if let Some(colour) = o.minimenu_colour {
            return Ok(colour);
        }
        let defaults = self
            .configs
            .minimenu
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("minimenu defaults not installed"))?;
        Ok(if o.members {
            defaults.members_colour
        } else {
            defaults.free_colour
        })
    }
}

impl Runtime {
    /// The view and projection the scene pick unprojects through, in scene-local
    /// fine units. Every camera mode picks: they are the matrices of the last
    /// scene draw, whichever camera produced it (orbit, follow-coordinate,
    /// smooth reset, cutscene pose, move-along, scripted or free camera). A
    /// blacked-out frame leaves the previous draw's matrices in place. Nothing
    /// picks before the first scene draw. A session that never draws (no
    /// renderer) derives them from the scripted camera.
    fn pick_matrices(&self) -> Option<([f32; 16], [f32; 16])> {
        if let Some(drawn) = &self.engine.scene.drawn_view {
            return Some((drawn.view, drawn.projection));
        }
        let cam2 = &self.engine.camera.cam2;
        if cam2.camera_state != 3 {
            return None;
        }
        let frame = cam2.frame()?;
        let mut rel = frame.clone();
        // The view works in scene-local fine units, so the scene base is subtracted.
        rel.rebase([cam2.scene.base[0], 0, cam2.scene.base[1]]);
        Some((rel.view_matrix([0, 0, 0]).to_entries(), frame.projection()))
    }
    /// The scene options for this cycle: the walk tile under the mouse
    /// through the last scene draw's matrices and viewport.
    /// The same ground option used by native picking, without a screen ray.
    pub fn walk_scene_option(&self, tile: [i32; 2]) -> crate::ui_scene_options::SceneOption {
        crate::ui_scene_options::SceneOption {
            detail: None,
            op: self.engine.menu.walk_here_text.clone(),
            target: Some(String::new()),
            cursor: self.engine.menu.default_walk_action,
            action: crate::ui_scene_options::WALK_ACTION,
            obj_id: -1,
            entity_id: 0,
            tile,
            enabled: true,
            has_arrow: false,
            sub_id: (i64::from(tile[0]) << 32) | i64::from(tile[1]),
            force_submenu: true,
        }
    }

    /// Installed-instance operations use the ordinary cache/multiloc builder.
    pub fn loc_scene_options(
        &mut self,
        target: crate::ui_scene_options::LocTarget,
        same_level: bool,
        varps: Option<&crate::entities910::varps::Varps>,
        varbits: Option<&crate::entity_runtime::bits_pack::Inputs>,
    ) -> Vec<crate::ui_scene_options::SceneOption> {
        let mut options = Vec::new();
        self.add_loc_target(
            target,
            same_level,
            LocalVars { varps, varbits },
            &mut options,
        );
        options
    }

    /// Decoded ground stacks use the ordinary cache, target and stack-menu builder.
    pub fn object_scene_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        level: i32,
        tile: [i32; 2],
    ) -> Vec<crate::ui_scene_options::SceneOption> {
        let mut options = Vec::new();
        self.add_object_options(input, level, tile, &mut options);
        options
    }

    /// Live NPC operations use the ordinary mask, attack-priority and cache builder.
    pub fn npc_scene_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        index: usize,
        other_level: bool,
        varps: Option<&crate::entities910::varps::Varps>,
        varbits: Option<&crate::entity_runtime::bits_pack::Inputs>,
    ) -> Vec<crate::ui_scene_options::SceneOption> {
        let mut options = Vec::new();
        self.add_npc_options(
            input,
            index,
            other_level,
            LocalVars { varps, varbits },
            &mut options,
        );
        options
    }

    pub(super) fn scene_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        local_vars: LocalVars<'_>,
    ) -> Vec<crate::ui_scene_options::SceneOption> {
        use crate::ui_scene_options::SceneOption;
        let mouse = self.input.click.unwrap_or(self.input.mouse);
        let Some((viewport, _)) = self.state.viewport else {
            return Vec::new();
        };
        let Some((view, proj)) = self.pick_matrices() else {
            return Vec::new();
        };
        if mouse[0] < 0 || mouse[1] < 0 {
            return Vec::new();
        }
        // No scene yet.
        let Some(h) = self.engine.camera.cam2.scene.heightmap.as_ref() else {
            return Vec::new();
        };
        let Some(player) = self.engine.camera.cam2.scene.local_player.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(marker) = self
            .engine
            .scene
            .cover_markers
            .iter()
            .find(|marker| {
                mouse[0] >= marker.rect[0]
                    && mouse[0] < marker.rect[2]
                    && mouse[1] >= marker.rect[1]
                    && mouse[1] < marker.rect[3]
            })
            .cloned()
        {
            let Some(npc) = input.npcs.and_then(|npcs| npcs.entities.get(&marker.npc)) else {
                return out;
            };
            let other_level = input
                .local_player
                .is_some_and(|player| player.level != npc.path.level);
            self.add_npc_options(input, marker.npc, other_level, local_vars, &mut out);
            return out;
        }
        if let Some(tile) = crate::ui_scene_options::pick_walk_tile(
            &view,
            &proj,
            viewport,
            mouse,
            h,
            player.level,
            self.engine.camera.cam2.scene.local_size,
        ) {
            let sub = (i64::from(tile[0]) << 32) | i64::from(tile[1]);
            // Target mode with the coordinate bit keeps the world pick and
            // adds APCOORDT. The original client clears a stale target when its component no longer resolves;
            // the retained target owner already enforces that lifecycle.
            if self.state.interaction.target.active
                && self.state.interaction.target.mask & 0x40 != 0
            {
                let target = &self.state.interaction.target;
                out.push(SceneOption {
                    detail: None,
                    op: target.verb.clone(),
                    target: Some(" -> ".into()),
                    cursor: target.cursor,
                    action: 59,
                    obj_id: -1,
                    entity_id: 0,
                    tile,
                    enabled: true,
                    has_arrow: false,
                    sub_id: sub,
                    force_submenu: true,
                });
            } else if !self.state.interaction.target.active {
                if self.engine.menu.show_face_here {
                    out.push(SceneOption {
                        detail: None,
                        op: rs910_core::texts::Msg::FaceHere.get().into(),
                        target: Some(String::new()),
                        cursor: -1,
                        action: 60,
                        obj_id: -1,
                        entity_id: 0,
                        tile,
                        enabled: true,
                        has_arrow: false,
                        sub_id: sub,
                        force_submenu: true,
                    });
                }
                out.push(self.walk_scene_option(tile));
            }
        }
        // The pickable pass runs whether or not the ground ray hit.
        self.add_pickable_options(input, viewport, mouse, local_vars, &mut out);
        out
    }
    /// `getEntryQuests/getQuestIconTags` for scene-owned
    /// entries. Quest config is already installed for CS2; retain the rendered
    /// `<sprite=...>` suffix by action/entity key so the real minimenu hook
    /// owner can return it for NPC and loc entries as well as objects.
    pub(super) fn install_scene_quest_text(&mut self, vars: &mut Variables<'_>) -> Result<()> {
        self.state.scene_quest_text.clear();
        let Some(quests) = vars.state.quests.as_ref() else {
            return Ok(());
        };
        for option in &self.input.scene_options {
            let ids: Option<Vec<i32>> = if crate::ui_minimenu::is_obj_action(option.action) {
                self.engine.configs.objs.as_ref().and_then(|store| {
                    u32::try_from(option.entity_id)
                        .ok()
                        .and_then(|id| store.get(id))
                        // postDecode: a members object
                        // without allowMembers has `quests = null`.
                        .map(|object| {
                            if object.members && !store.allow_members.get() {
                                Vec::new()
                            } else {
                                object.inventory.quests.clone()
                            }
                        })
                })
            } else if crate::ui_minimenu::is_loc_action(option.action) {
                let id = ((option.entity_id >> 32) & 0x7fff_ffff) as u32;
                self.engine.configs.locs.as_ref().and_then(|store| {
                    self.resolve_multiloc(
                        store,
                        id,
                        LocalVars {
                            varps: vars.player.as_deref(),
                            varbits: Some(vars.definitions),
                        },
                    )
                    .map(|loc| loc.quests_for(store.allow_members.get()).to_vec())
                })
            } else if crate::ui_minimenu::is_npc_action(option.action) {
                usize::try_from(option.entity_id)
                    .ok()
                    .and_then(|index| vars.scene.npcs.and_then(|npcs| npcs.entities.get(&index)))
                    .and_then(|npc| {
                        let store = self.engine.configs.npcs.as_ref()?;
                        let base = u32::try_from(npc.type_id)
                            .ok()
                            .and_then(|id| store.get(id))?;
                        self.resolve_multinpc(
                            store,
                            base,
                            LocalVars {
                                varps: vars.player.as_deref(),
                                varbits: Some(vars.definitions),
                            },
                        )
                        .map(|definition| definition.quests.clone())
                    })
            } else {
                None
            };
            let Some(ids) = ids else {
                continue;
            };
            let mut tags = String::new();
            for quest_id in ids {
                let quest = quests.quest(quest_id)?;
                if quest.icon_sprite != -1 {
                    tags.push_str(&format!(" <sprite={}>", quest.icon_sprite));
                }
            }
            self.state
                .scene_quest_text
                .insert((option.action, option.entity_id), tags);
        }
        Ok(())
    }
    /// `getQuestIconTags(objType(obj).quests)`: one
    /// ` <sprite=N>` per quest with an icon. An unresolvable object or absent
    /// quest owner yields no tags, as for scene entries.
    pub(super) fn object_quest_tags(&self, vars: &Variables<'_>, obj: i32) -> Result<String> {
        let mut tags = String::new();
        let Some(quests) = vars.state.quests.as_ref() else {
            return Ok(tags);
        };
        let ids = self.engine.configs.objs.as_ref().and_then(|store| {
            u32::try_from(obj)
                .ok()
                .and_then(|id| store.get(id))
                .map(|object| {
                    // postDecode: members objects without
                    // allowMembers have `quests = null`.
                    if object.members && !store.allow_members.get() {
                        Vec::new()
                    } else {
                        object.inventory.quests.clone()
                    }
                })
        });
        for quest_id in ids.unwrap_or_default() {
            let quest = quests.quest(quest_id)?;
            if quest.icon_sprite != -1 {
                tags.push_str(&format!(" <sprite={}>", quest.icon_sprite));
            }
        }
        Ok(tags)
    }
    /// `addSceneOptions` for ground stacks. The decoded
    /// zone stack is already sorted by the original client's `sortObjStacks`; iterate its
    /// tail and retain each menu entry's stack index for submenu grouping.
    pub(super) fn add_object_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        level: i32,
        tile: [i32; 2],
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        use crate::ui_scene_options::SceneOption;
        let Some(objects) = input.objects else {
            return;
        };
        // Target and operation rows need the local player's level.
        let same_level = input.local_player.is_some_and(|p| p.level == level);
        let x = (input.base[0] >> 9).wrapping_add(tile[0]);
        let z = (input.base[1] >> 9).wrapping_add(tile[1]);
        let key = ((level & 3) as i64) << 28 | ((z & 0x3fff) as i64) << 14 | (x & 0x3fff) as i64;
        let Some(stack) = objects.stacks.get(&key) else {
            return;
        };
        let Some(defs) = self.engine.configs.objs.as_ref() else {
            Self::gap_once(
                &mut self.engine.menu.gaps,
                "ground-object menu without object configs",
            );
            return;
        };
        let target = &self.state.interaction.target;
        let params = &self.engine.configs.params;
        let autodisable = |key: i32| params.get(&key).is_none_or(|d| d.autodisable);
        for (stack_index, ground) in stack.iter().enumerate().rev() {
            let Some(obj) = defs.get(u32::try_from(ground.id).ok().unwrap_or(u32::MAX)) else {
                continue;
            };
            // list applies the postDecode members gate
            // default ops, pruned params.
            let obj = obj.members_gated(defs.allow_members.get(), &autodisable);
            let colour = match self.engine.minimenu_colour(ground.id) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let coloured_name = format!("<col={:x}>{}", colour as u32, obj.name);
            if target.active
                && same_level
                && target.mask & 1 != 0
                && self.target_param_matches(target.param, &obj.params)
            {
                out.push(SceneOption {
                    detail: None,
                    op: target.verb.clone(),
                    target: Some(format!("{} -> {}", target.name, coloured_name)),
                    cursor: target.cursor,
                    action: 17,
                    obj_id: -1,
                    entity_id: ground.id as i64,
                    tile,
                    enabled: true,
                    has_arrow: false,
                    sub_id: stack_index as i64,
                    force_submenu: false,
                });
            }
            if !same_level {
                continue;
            }
            // The operations follow the target row without an else; the slot-5
            // op is the object's default Examine
            let ops: [Option<&str>; 6] = [
                obj.ops[0].as_deref(),
                obj.ops[1].as_deref(),
                obj.ops[2].as_deref(),
                obj.ops[3].as_deref(),
                obj.ops[4].as_deref(),
                Some(rs910_core::texts::Msg::Examine.get()),
            ];
            for index in (0..ops.len()).rev() {
                let Some(op) = ops[index] else {
                    continue;
                };
                let action = [18, 19, 20, 21, 22, 1004][index];
                // getCursor: opcodes 142-146 fill slots 0-4 only.
                let type_cursor = obj.inventory.cursor.get(index).copied().unwrap_or(-1);
                let cursor = if type_cursor != -1 {
                    type_cursor
                } else {
                    self.engine.menu.default_cursors[1]
                };
                out.push(SceneOption {
                    detail: None,
                    op: op.to_owned(),
                    target: Some(coloured_name.clone()),
                    cursor,
                    action,
                    obj_id: -1,
                    entity_id: ground.id as i64,
                    tile,
                    enabled: true,
                    has_arrow: false,
                    sub_id: stack_index as i64,
                    force_submenu: false,
                });
            }
        }
    }
    /// `addSceneOptions` location pass for one picked
    /// `Location`: `getId/getShape/getAngle` of the drawn entity and its
    /// scene tile, resolved through the live `multiloc` selection.
    pub(super) fn add_loc_entry(
        &mut self,
        pick: &crate::player_picking::LocPick,
        same_level: bool,
        local_vars: LocalVars<'_>,
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        self.add_loc_target(
            crate::ui_scene_options::LocTarget {
                id: pick.id,
                shape: pick.shape,
                angle: pick.angle,
                tile: pick.tile,
            },
            same_level,
            local_vars,
            out,
        );
    }

    fn add_loc_target(
        &mut self,
        target: crate::ui_scene_options::LocTarget,
        same_level: bool,
        local_vars: LocalVars<'_>,
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        let crate::ui_scene_options::LocTarget {
            id,
            shape,
            angle,
            tile,
        } = target;
        use crate::ui_scene_options::{encode_loc_id, SceneOption};
        let Some(locs) = self.engine.configs.locs.as_ref() else {
            Self::gap_once(
                &mut self.engine.menu.gaps,
                "location menu without loc configs",
            );
            return;
        };
        let target = &self.state.interaction.target;
        let Some(placed) = u32::try_from(id).ok().and_then(|id| locs.get(id)) else {
            return;
        };
        let Some(loc) = self.resolve_multiloc(locs, placed.id, local_vars) else {
            return;
        };
        // encodeLocId reads the placed type too.
        let entity_id = encode_loc_id(
            id,
            tile[0],
            tile[1],
            shape,
            angle,
            // active under allowMembers.
            placed.active_for(locs.allow_members.get()) == 0,
            placed.raiseobject == 1,
        );
        //  `colTag`.
        let coloured_name = format!("<col=ffff>{}", loc.name);
        if target.active
            && same_level
            && target.mask & 4 != 0
            && self.target_param_matches(target.param, &loc.params)
        {
            out.push(SceneOption {
                detail: None,
                op: target.verb.clone(),
                target: Some(format!("{} -> {}", target.name, coloured_name)),
                cursor: target.cursor,
                action: 2,
                obj_id: -1,
                entity_id,
                tile,
                enabled: true,
                has_arrow: false,
                sub_id: entity_id,
                force_submenu: false,
            });
        }
        if !same_level {
            return;
        }
        // Operations follow the target row without an else; slot 5 is the
        // loc's default Examine op under allow-members.
        let [o0, o1, o2, o3, o4] = loc.ops_for(locs.allow_members.get());
        let ops: [Option<&str>; 6] = [
            o0,
            o1,
            o2,
            o3,
            o4,
            Some(rs910_core::texts::Msg::Examine.get()),
        ];
        for index in (0..ops.len()).rev() {
            let Some(op) = ops[index] else {
                continue;
            };
            let action = [3, 4, 5, 6, 1001, 1002][index];
            let cursor = loc.cursor[index];
            out.push(SceneOption {
                detail: None,
                op: op.to_owned(),
                target: Some(coloured_name.clone()),
                cursor: if cursor != -1 {
                    cursor
                } else {
                    self.engine.menu.default_cursors[1]
                },
                action,
                obj_id: -1,
                entity_id,
                tile,
                enabled: true,
                has_arrow: false,
                sub_id: entity_id,
                force_submenu: false,
            });
        }
    }

    /// Selects the active multi-loc
    /// definition from the live player varp/varbit domain before menus read
    /// names, operations, active flags or raise-object behavior.
    pub(super) fn resolve_multiloc<'a>(
        &self,
        locs: &'a crate::config::LocStore,
        id: u32,
        local_vars: LocalVars<'_>,
    ) -> Option<&'a crate::config::Loc> {
        let base = locs.get(id)?;
        if !base.has_multiloc || base.multiloc.is_empty() {
            return Some(base);
        }
        // addSceneOptions: a null selection drops the
        // loc from the menu. `LocStore` decodes every archived loc, so a
        // selected id is only absent for an inconsistent cache.
        let selected = base.multi_loc(&|bit, var| local_vars.get(bit, var))?;
        locs.get(selected)
    }
    /// Selects the active multi-NPC definition from the same live
    /// varp/varbit domains used by locs. The NPC entry pass performs one
    /// selection (no recursion) and returns on null.
    pub(super) fn resolve_multinpc<'a>(
        &self,
        store: &'a crate::config::NpcStore,
        base: &'a crate::config::Npc,
        local_vars: LocalVars<'_>,
    ) -> Option<&'a crate::config::Npc> {
        if base.multinpc.is_empty() {
            return Some(base);
        }
        let selected = base.multi_npc(&|bit, var| local_vars.get(bit, var))?;
        store.get(selected)
    }
    /// `ParamConfig varX == null || getParam(targetParam, defaultint) !=
    /// defaultint`. Unknown param definitions pass through, while a known
    /// definition compares its integer default against the retained config
    /// value. String values have no integer match and therefore use default.
    pub(super) fn target_param_matches(
        &self,
        target_param: i32,
        params: &[(i32, crate::config::ParamValue)],
    ) -> bool {
        if target_param == -1 {
            return true;
        }
        let Some(param_type) = self.engine.configs.params.get(&target_param) else {
            return true;
        };
        let default = param_type.default_int.unwrap_or(0);
        let value = params
            .iter()
            .find(|(id, _)| *id == target_param)
            .and_then(|(_, value)| match value {
                crate::config::ParamValue::Int(value) => Some(*value),
                crate::config::ParamValue::Str(_) => None,
            })
            .unwrap_or(default);
        value != default
    }
    /// `addSceneOptions`: walk `Scene.pickableEntities`
    /// from the last draw in list order, hit-testing each with
    /// `hitTest` and dispatching by entity kind. A centred
    /// player or NPC first adds the deferred NPCs, then the deferred players,
    /// whose footprint it covers; `lastMenuCycle` keeps each actor
    /// to one set of entries per cycle.
    pub(super) fn add_pickable_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        viewport: [i32; 4],
        mouse: [i32; 2],
        local_vars: LocalVars<'_>,
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        use crate::player_picking::PickRef;
        let Some(frame) = self
            .engine
            .scene
            .player_picks
            .as_ref()
            .filter(|frame| frame.matches(input, viewport))
            .cloned()
        else {
            return;
        };
        let Some(local_level) = input.local_player.map(|player| player.level) else {
            return;
        };
        // RuneScape clears `ignoreLevelWhenPicking`.
        let ignore_level = crate::applet_params::get()
            .mode_game()
            .ok()
            .flatten()
            .is_none_or(|game| game != "runescape");
        let players = input.players;
        let npcs = input.npcs;
        let mut seen_players = std::collections::HashSet::new();
        let mut seen_npcs = std::collections::HashSet::new();
        let mut seen_objs = std::collections::HashSet::new();
        // (level, fine x, fine z, size, sceneAddDeferred) of a live actor:
        // the deferred flag (`npc_scene_flags` for NPCs)
        // leaves a deferred actor out of the scene, so only this covering
        // pass offers it.
        let player_at = |id: usize| {
            players
                .and_then(|p| p.players.get(id).and_then(Option::as_ref))
                .map(|p| {
                    (
                        p.level,
                        p.fine_x as i32,
                        p.fine_z as i32,
                        p.size,
                        p.actor.scene.deferred,
                    )
                })
        };
        let npc_store = self.engine.configs.npcs.clone();
        let npc_at = |slot: usize| {
            npcs.and_then(|n| n.entities.get(&slot)).map(|n| {
                let size = npc_store
                    .as_ref()
                    .and_then(|store| u32::try_from(n.type_id).ok().and_then(|id| store.get(id)))
                    .map_or(n.path.size, |t| i32::from(t.size));
                (
                    n.path.level,
                    n.path.fine_x as i32,
                    n.path.fine_z as i32,
                    size,
                    n.path.actor.scene.deferred,
                )
            })
        };
        let centred = |x: i32, z: i32, size: i32| {
            let centre = if size & 1 == 0 { 0 } else { 256 };
            x & 0x1ff == centre && z & 0x1ff == centre
        };
        // /: `x - (size - 1 << 8) >= origin` and the size fits.
        let covered = |origin: [i32; 2], outer: i32, x: i32, z: i32, size: i32| {
            let x = x - ((size - 1) << 8);
            let z = z - ((size - 1) << 8);
            x >= origin[0]
                && size <= outer - ((x - origin[0]) >> 9)
                && z >= origin[1]
                && size <= outer - ((z - origin[1]) >> 9)
        };
        for &entry in &frame.order {
            enum Hit {
                Actor {
                    npc: Option<usize>,
                    player: Option<usize>,
                },
                Obj([i32; 2]),
                Loc(crate::player_picking::LocPick),
            }
            let (level, hit) = match entry {
                PickRef::Player(k) => {
                    let Some(pick) = frame.picks.get(k) else {
                        continue;
                    };
                    let id = pick.id.pid as usize;
                    if players.and_then(|p| p.generations.get(id)) != Some(&pick.id.generation) {
                        continue;
                    }
                    let Some((level, ..)) = player_at(id) else {
                        continue;
                    };
                    if !ignore_level && level != local_level {
                        continue;
                    }
                    if !crate::scene_player_pick::pick_one(pick, frame.screen, mouse, [0, 0]) {
                        continue;
                    }
                    (
                        level,
                        Hit::Actor {
                            npc: None,
                            player: Some(id),
                        },
                    )
                }
                PickRef::Npc(k) => {
                    let Some(pick) = frame.npc_picks.get(k) else {
                        continue;
                    };
                    let slot = pick.id.pid as usize;
                    let Some((level, ..)) = npc_at(slot) else {
                        continue;
                    };
                    if !ignore_level && level != local_level {
                        continue;
                    }
                    if !crate::scene_player_pick::pick_one(pick, frame.screen, mouse, [0, 0]) {
                        continue;
                    }
                    (
                        level,
                        Hit::Actor {
                            npc: Some(slot),
                            player: None,
                        },
                    )
                }
                PickRef::Obj(k) => {
                    let Some(obj) = frame.obj_picks.get(k) else {
                        continue;
                    };
                    if !ignore_level && obj.level != local_level {
                        continue;
                    }
                    // pick: primary, secondary, tertiary.
                    if !obj.picks.iter().any(|(_, pick)| {
                        crate::scene_player_pick::pick_one(pick, frame.screen, mouse, [0, 0])
                    }) {
                        continue;
                    }
                    (obj.level, Hit::Obj(obj.tile))
                }
                PickRef::Loc(k) => {
                    let Some(pick) = frame.loc_picks.get(k) else {
                        continue;
                    };
                    if !ignore_level && pick.level != local_level {
                        continue;
                    }
                    if !pick.hit {
                        continue;
                    }
                    (pick.level, Hit::Loc(pick.clone()))
                }
            };
            let other_level = level != local_level;
            match hit {
                Hit::Actor { npc, player } => {
                    let own = match (npc, player) {
                        (Some(slot), _) => npc_at(slot),
                        (_, Some(id)) => player_at(id),
                        _ => None,
                    };
                    // An NPC whose type is null adds nothing.
                    if let Some(slot) = npc {
                        let known = npcs
                            .and_then(|n| n.entities.get(&slot))
                            .and_then(|n| u32::try_from(n.type_id).ok())
                            .is_some_and(|id| {
                                npc_store.as_ref().is_some_and(|s| s.get(id).is_some())
                            });
                        if !known {
                            continue;
                        }
                    }
                    if let Some((_, x, z, size, _)) =
                        own.filter(|&(_, x, z, size, _)| centred(x, z, size))
                    {
                        let origin = [x - ((size - 1) << 8), z - ((size - 1) << 8)];
                        if let Some(list) = npcs {
                            for &other in &list.slots {
                                if Some(other) == npc || seen_npcs.contains(&other) {
                                    continue;
                                }
                                let Some((_, ox, oz, osize, deferred)) = npc_at(other) else {
                                    continue;
                                };
                                if deferred && covered(origin, size, ox, oz, osize) {
                                    self.add_npc_options(
                                        input,
                                        other,
                                        other_level,
                                        local_vars,
                                        out,
                                    );
                                    seen_npcs.insert(other);
                                }
                            }
                        }
                        if let Some(list) = players {
                            for &other in &list.high_indices {
                                if Some(other) == player || seen_players.contains(&other) {
                                    continue;
                                }
                                let Some((_, ox, oz, osize, deferred)) = player_at(other) else {
                                    continue;
                                };
                                if deferred && covered(origin, size, ox, oz, osize) {
                                    self.add_player_options(input, &[other], out);
                                    seen_players.insert(other);
                                }
                            }
                        }
                    }
                    if let Some(slot) = npc {
                        if seen_npcs.insert(slot) {
                            self.add_npc_options(input, slot, other_level, local_vars, out);
                        }
                    }
                    if let Some(id) = player {
                        if seen_players.insert(id) {
                            self.add_player_options(input, &[id], out);
                        }
                    }
                }
                Hit::Obj(tile) => {
                    // One object stack per tile; its ranks share the stack.
                    if seen_objs.insert((level, tile)) {
                        self.add_object_options(input, level, tile, out);
                    }
                }
                Hit::Loc(pick) => self.add_loc_entry(&pick, !other_level, local_vars, out),
            }
        }
    }
    /// for one live NPC slot; `other_level` is the original client's
    /// `localPlayerEntity.level != entity.level` argument.
    pub(super) fn add_npc_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        index: usize,
        other_level: bool,
        local_vars: LocalVars<'_>,
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        use crate::ui_scene_options::SceneOption;
        let Some(npcs) = input.npcs else {
            return;
        };
        let Some(store) = self.engine.configs.npcs.as_ref() else {
            Self::gap_once(&mut self.engine.menu.gaps, "NPC menu without NPC configs");
            return;
        };
        let local_combat = input
            .local_player
            .and_then(|player| {
                input.players.and_then(|players| {
                    players
                        .players
                        .get(player.index as usize)
                        .and_then(Option::as_ref)
                })
            })
            .map_or(0, |player| player.appearance.combat);
        let target = &self.state.interaction.target;
        let Some(npc) = npcs.entities.get(&index) else {
            return;
        };
        let Some(base) = store.get(u32::try_from(npc.type_id).ok().unwrap_or(u32::MAX)) else {
            return;
        };
        let Some(def) = self.resolve_multinpc(store, base, local_vars) else {
            return;
        };
        if !def.active {
            return;
        }
        if self.state.minimenu.option_count as usize + out.len() >= 407 {
            return;
        }
        // The entity name, replaced by the selected
        // multinpc type's name.
        let mut name = if base.multinpc.is_empty() {
            if npc.name.is_empty() {
                def.name.clone()
            } else {
                npc.name.clone()
            }
        } else {
            def.name.clone()
        };
        name = npc_menu_name(name, npc.vislevel, local_combat);
        let coloured = format!("<col=ffff00>{name}");
        if target.active
            && !other_level
            && target.mask & 2 != 0
            && self.target_param_matches(target.param, &def.params)
        {
            out.push(SceneOption {
                detail: None,
                op: target.verb.clone(),
                target: Some(format!("{} -> {}", target.name, coloured)),
                cursor: target.cursor,
                action: 8,
                obj_id: -1,
                entity_id: index as i64,
                tile: [0, 0],
                enabled: true,
                has_arrow: false,
                sub_id: index as i64,
                force_submenu: false,
            });
        }
        if other_level {
            return;
        }
        // op under allowMembers.
        let ops = npc_type_ops(def.ops_for(store.allow_members.get()));
        for (op_index, action, cursor) in npc_menu_operation_slots(
            NpcOps {
                ops: &ops,
                op_mask: npc.op_mask,
                reprioritise: def.reprioritise_attack_op,
                vislevel: npc.vislevel,
                cursors: &def.cursor,
                cursor_attack: def.cursorattack,
            },
            AttackMenu {
                priority: self.engine.menu.npc_attack_priority,
                local_combat,
                default_menu_cursor: self.engine.menu.default_cursors[1],
            },
        ) {
            out.push(SceneOption {
                detail: None,
                op: ops[op_index].expect("NPC operation slot").to_owned(),
                target: Some(coloured.clone()),
                cursor,
                action,
                obj_id: -1,
                entity_id: index as i64,
                tile: [0, 0],
                enabled: true,
                has_arrow: false,
                sub_id: index as i64,
                force_submenu: false,
            });
        }
    }
    /// `addPlayerEntries` for each id in order.
    pub(super) fn add_player_options(
        &mut self,
        input: &crate::ui_cam2::SceneInput<'_>,
        ids: &[usize],
        out: &mut Vec<crate::ui_scene_options::SceneOption>,
    ) {
        let Some(players) = input.players else {
            return;
        };
        let Some(local_id) = input.local_player.map(|p| p.index as usize) else {
            return;
        };
        let Some(local) = players.players[local_id].as_ref() else {
            return;
        };
        let names: [Option<&str>; 8] = std::array::from_fn(|i| {
            self.engine.menu.player_ops[i]
                .as_ref()
                .and_then(|p| p.name.as_deref())
        });
        let deprioritised = std::array::from_fn(|i| {
            self.engine.menu.player_ops[i]
                .as_ref()
                .is_some_and(|p| p.deprioritised)
        });
        let cursors = std::array::from_fn(|i| {
            self.engine.menu.player_ops[i]
                .as_ref()
                .map_or(-1, |p| p.cursor)
        });
        let target = &self.state.interaction.target;
        for &id in ids {
            let Some(p) = players.players.get(id).and_then(Option::as_ref) else {
                continue;
            };
            if p.appearance.visibility == Some(1) {
                continue;
            }
            let Some(player_name) = p.appearance.name.as_deref() else {
                continue;
            };
            // addPlayerEntries replaces the displayed
            // player name when the rendered model is a fake NPC. The menu
            // remains a player entry (actions 15/44-53 still send the player
            // index), so reuse the ordinary player-option owner instead of
            // dropping the hit as an unsupported NPC menu.
            let transformed = p
                .appearance
                .model
                .as_ref()
                .and_then(|model| (model.npc >= 0).then_some(model.npc))
                .and_then(|npc_id| {
                    self.engine
                        .configs
                        .npcs
                        .as_ref()
                        .and_then(|store| store.get(npc_id as u32))
                        .filter(|npc| npc.transmogfakenpc)
                });
            let (name, skill_level) = if let Some(npc) = transformed {
                // The NPC name plus, for a non-zero vislevel,
                // the combat colour and ` (level: N)`.
                let level = if npc.vislevel != 0 {
                    format!(
                        "{} ({}{})",
                        crate::ui_player_options::combat_colour(
                            npc.vislevel,
                            local.appearance.combat
                        ),
                        rs910_core::texts::Msg::Level.get(),
                        npc.vislevel
                    )
                } else {
                    String::new()
                };
                (format!("{}{}", npc.name, level), -1)
            } else {
                (
                    p.appearance.title.as_ref().map_or_else(
                        || player_name.to_owned(),
                        |t| t.replace("<name>", player_name),
                    ),
                    p.appearance.skill,
                )
            };
            let options = crate::ui_player_options::PlayerOptionInput {
                option_count: self.state.minimenu.option_count as usize + out.len(),
                same_plane: p.level == local.level,
                local: id == local_id,
                index: id as i32,
                name: &name,
                transformed: transformed.is_some(),
                combat_level: p.appearance.combat,
                max_combat_level: p.appearance.max_combat,
                skill_level,
                local_combat_level: local.appearance.combat,
                local_wilderness_level: local.appearance.wilderness,
                wilderness_level: p.appearance.wilderness,
                local_team: local.appearance.team,
                team: p.appearance.team,
                suppress_partner_highlight: p.suppress_partner,
                options: &names,
                deprioritised,
                cursors,
                target_mode: target.active,
                target_mask: target.mask,
                target_verb: &target.verb,
                target_name: &target.name,
                target_cursor: target.cursor,
                attack_priority: self.engine.menu.player_attack_priority,
                default_cursor: self.engine.menu.default_cursors[1],
            };
            if options.option_count >= 407 {
                continue;
            }
            // Same-level, non-fake-NPC players label the first
            // `Walk here` row.
            if id != local_id && options.same_plane && transformed.is_none() {
                if let Some(walk) = out.iter_mut().find(|o| o.action == 23) {
                    walk.detail = Some(crate::ui_player_options::player_label(&options));
                }
            }
            for e in crate::ui_player_options::build_player_options(&options) {
                out.push(crate::ui_scene_options::SceneOption {
                    op: e.op,
                    target: e.target,
                    cursor: e.cursor,
                    action: e.action,
                    obj_id: e.obj_id,
                    entity_id: e.entity_id,
                    tile: [e.tile_x, e.tile_z],
                    enabled: e.enabled,
                    has_arrow: e.has_arrow,
                    sub_id: e.sub_id,
                    force_submenu: e.force_submenu,
                    detail: e.detail,
                });
            }
        }
    }
    pub(super) fn gap_once(gaps: &mut BTreeMap<String, usize>, what: &str) {
        let n = gaps.entry(what.to_string()).or_default();
        if *n == 0 {
            log::info!("[client910] minimenu: {what} has no owner");
        }
        *n += 1;
    }
    pub(super) fn run_actions(&mut self, vars: &mut Variables<'_>) -> Result<()> {
        self.sync_active_clan_channel();
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        while let Some(action) = self.state.interaction.actions.pop_front() {
            match action {
                crate::ui_interaction::Action::MapElementTrigger {
                    trigger,
                    element,
                    category,
                    mouse,
                } => {
                    if let Some((script_id, script)) =
                        provider.get_trigger(trigger, element, category)?
                    {
                        let mut locals = vec![element];
                        if let Some([x, y]) = mouse {
                            locals.extend([x, y]);
                        }
                        runner.run_compiled_with_ints(
                            &mut self.store,
                            &mut self.state,
                            script_id,
                            &script,
                            crate::ui_hooks::INTERACTIVE_LIMIT,
                            &locals,
                        )?;
                    }
                }
                action => crate::ui_interaction::dispatch(
                    &mut self.store,
                    &mut self.state,
                    &mut runner,
                    action,
                )?,
            }
        }
        self.diagnostics.record(runner.executions, runner.missing);
        Ok(())
    }
    pub fn cursor_id(&self) -> i32 {
        let menu = self.state.minimenu.hover_cursor(
            self.engine.platform.mouse,
            self.state.interaction.drag.component.is_some(),
            self.engine
                .configs
                .minimenu
                .as_ref()
                .is_some_and(|d| d.hover_cursor_override),
        );
        crate::ui_cursor::select(
            menu,
            self.state.minimenu.default_cursor,
            self.engine.menu.default_cursors[0],
        )
    }
    pub(super) fn menu_font(&self) -> Result<std::rc::Rc<crate::ui_fonts::Font>> {
        crate::ui_menu_render::font(&self.state)
    }
    /// `submenuArrowSprite.getWidth()` for `getEntryWidth`
    ///  (getWidth: the unpadded image width).
    pub(super) fn submenu_arrow_width(&self) -> Result<i32> {
        if !self.state.minimenu.has_arrow_entry() {
            return Ok(0);
        }
        self.state
            .menu
            .submenu_arrow
            .as_ref()
            .map(|sprite| sprite.size[0])
            .ok_or_else(|| anyhow::anyhow!(Fault::MissingValue.message("submenu arrow sprite")))
    }
    pub(super) fn open_menu(&mut self, at: [i32; 2]) -> Result<()> {
        let font = self.menu_font()?;
        let arrow_width = self.submenu_arrow_width()?;
        let fonts = self
            .state
            .fonts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("menu fonts not installed"))?;
        let mut failure = None;
        // FontMetrics.stringWidth(text, fontSprites) with the
        // font icon provider (`<img=>`/`<sprite=>` widths).
        let width = |s: &str| {
            let units: Vec<u16> = s.encode_utf16().collect();
            fonts
                .width(&font.metrics, Some(&units))
                .unwrap_or_else(|e| {
                    failure.get_or_insert(e);
                    0
                })
        };
        self.state.minimenu.open_at(
            at,
            self.state.layout.canvas,
            crate::ui_minimenu::PopupLook {
                ascent: font.metrics.ascent,
                descent: font.metrics.descent,
                custom: self.state.menu.custom,
            },
            self.engine.menu.show_single_option_menu,
            arrow_width,
            width,
        );
        if let Some(error) = failure {
            return Err(error);
        }
        self.state.minimenu.popup.alpha_noise = (self.engine.next_double() * 24.0) as i32;
        self.state.menu.open = self.state.minimenu.open;
        self.state.menu.bounds = self.state.minimenu.popup.bounds;
        Ok(())
    }
    /// `jtele` sends the developer-console `tele` command
    /// through the ordinary CLIENT_CHEAT writer. Staff shift-clicks use the
    /// same route as the original client; no movement packet or local-only teleport is made.
    pub(super) fn queue_jtele(&mut self, tile: [i32; 2]) {
        let Some(local) = self.engine.camera.cam2.scene.local_player.as_ref() else {
            Self::gap_once(&mut self.engine.menu.gaps, "jtele without local player");
            return;
        };
        let base = [
            self.engine.camera.cam2.scene.base[0] >> 9,
            self.engine.camera.cam2.scene.base[1] >> 9,
        ];
        let x = base[0].wrapping_add(tile[0]);
        let z = base[1].wrapping_add(tile[1]);
        let command = format!(
            "tele {},{},{},{},{}",
            local.level,
            x >> 6,
            z >> 6,
            x & 0x3f,
            z & 0x3f
        );
        match crate::client_command::remote(&command, true, false) {
            Ok(packet) => self.engine.outgoing.extend(packet),
            Err(error) => Self::gap_once(
                &mut self.engine.menu.gaps,
                &format!("jtele command encoding: {error}"),
            ),
        }
    }
    /// update. Pending clicks survive the drag threshold.
    pub(super) fn menu_click(
        &mut self,
        event: Option<crate::ui_defaults::MouseEvent>,
    ) -> Result<()> {
        let [x, y] = event.map_or(self.engine.platform.mouse, |e| e.pos);
        let Some(defaults) = self.engine.configs.minimenu.as_ref() else {
            return Ok(());
        };
        let held = |k: i32| self.keyboard.held(k);
        // The binding test compares the modifier mask.
        let presses: Vec<_> = self
            .key_presses
            .iter()
            .map(|e| (e.code, e.modifiers))
            .collect();
        let test = |b: &Option<crate::ui_defaults::Binding>| {
            b.as_ref()
                .is_some_and(|b| b.test(event.as_ref(), &presses, &held))
        };
        let (primary, secondary, mut menu, select) = (
            test(&defaults.primary),
            test(&defaults.secondary),
            test(&defaults.menu),
            test(&defaults.select),
        );
        if self.state.minimenu.open {
            let font = self.menu_font()?;
            let arrow_width = self.submenu_arrow_width()?;
            let fonts = self
                .state
                .fonts
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("menu fonts not installed"))?;
            let mut failure = None;
            let width = |s: &str| {
                let units: Vec<u16> = s.encode_utf16().collect();
                fonts
                    .width(&font.metrics, Some(&units))
                    .unwrap_or_else(|e| {
                        failure.get_or_insert(e);
                        0
                    })
            };
            let entry = self.state.minimenu.popup_input(
                [x, y],
                select,
                self.state.layout.canvas,
                arrow_width,
                width,
            );
            if let Some(error) = failure {
                return Err(error);
            }
            if let Some(entry) = entry {
                // The original client passes `fromMenu = true` for both the main popup and
                // an expanded submenu.  Object-operation packets preserve
                // that bit in their final flags byte.
                self.use_menu_option(&entry, x, y, true);
            }
            self.state.menu.open = self.state.minimenu.open;
            return Ok(());
        }
        let m = &self.state.minimenu;
        let last_op = m.last_option.is_some_and(|e| {
            let a = m.entry(e).action;
            (if a >= 2000 { a - 2000 } else { a }) == 1007
        });
        if (primary || secondary)
            && (last_op || (self.input.single_mouse_button == 1 && m.option_count > 2))
        {
            menu = true;
        }
        // `draggedComponent != null || clickState > 0`
        // only the world map raises clickState.
        let dragging = self.state.interaction.drag.component.is_some()
            || self.engine.world_map.borrow().click_state > 0;
        if (menu || primary || secondary) && crate::ui_debug_flags::flags().ui_trace_input {
            {
                // Diagnostic: the original allEntries order (bottom row first).
                for &id in &m.entries {
                    let e = m.entry(id);
                    log::info!(
                        "[ui-input] menu at={:?} action={} cursor={} entity={} tile={},{} label={:?}",
                        [x, y],
                        e.action,
                        e.cursor,
                        e.entity_id,
                        e.tile_x,
                        e.tile_z,
                        e.label()
                    );
                }
            }
        }
        if menu && m.option_count > 0 {
            if dragging {
                self.state.minimenu.pending_open = true;
            } else {
                self.open_menu([x, y])?;
            }
        } else if secondary {
            if let Some(e) = m.secondary {
                let entry = m.entry(e).clone();
                self.use_menu_option(&entry, x, y, false);
            }
        } else if primary {
            if let Some(e) = m.active {
                let entry = m.entry(e).clone();
                if dragging {
                    self.state.minimenu.pending = Some(entry);
                } else {
                    self.use_menu_option(&entry, x, y, false);
                }
            } else if self.state.interaction.target.active {
                self.state
                    .interaction
                    .actions
                    .push_back(crate::ui_interaction::Action::ClearTarget);
            }
        }
        if !dragging {
            self.state.minimenu.pending = None;
            self.state.minimenu.pending_open = false;
        }
        Ok(())
    }
    /// `useMenuOption(entry, x, y, fromMenu)`: every
    /// original action branch (world map 1008-1012, interface 25/30/57/58/1007,
    /// OPPLAYER 44-53, OPPLAYERT 15/16, OPLOC 3-6/1001/1002, OPLOCT 2,
    /// OPNPC 9-13/1003, OPNPCT 8, OPOBJ 18-22/1004, OPOBJT 17, APCOORDT 59,
    /// walk 23 and face 60) plus the shared target-clearing tail.
    pub fn use_menu_option(
        &mut self,
        entry: &crate::ui_minimenu::Entry,
        x: i32,
        y: i32,
        from_menu: bool,
    ) {
        let queued = self.engine.outgoing.len();
        self.dispatch_menu_option(entry, x, y, from_menu);
        if crate::ui_debug_flags::flags().ui_trace_input {
            // Diagnostic: the client-protocol bytes this option queued for
            // the session writer (opcode first, original payload order).
            log::info!(
                "[ui-input] queued action={} bytes={:?}",
                entry.action,
                &self.engine.outgoing[queued.min(self.engine.outgoing.len())..]
            );
        }
    }
    pub(super) fn dispatch_menu_option(
        &mut self,
        entry: &crate::ui_minimenu::Entry,
        x: i32,
        y: i32,
        from_menu: bool,
    ) {
        if crate::ui_debug_flags::flags().ui_trace_input {
            log::info!(
                "[ui-input] action={} op={} component={}:{} child={} label={}",
                entry.action,
                entry.entity_id,
                entry.tile_z >> 16,
                entry.tile_z & 65535,
                entry.tile_x,
                entry.label()
            );
        }
        let mut action = entry.action;
        if action >= 2000 {
            action -= 2000;
        }
        use crate::ui_interaction::Action;
        match action {
            1008..=1012 => {
                // The map-element action forwards the
                // element id and category to OPWORLDMAPELEMENT1..5. The
                // trigger executes through the normal action queue after the
                // menu click, before the next client tick flushes state.
                self.state
                    .interaction
                    .actions
                    .push_back(Action::MapElementTrigger {
                        trigger: action - 998,
                        element: entry.entity_id as i32,
                        category: entry.tile_x,
                        mouse: None,
                    });
            }
            44..=53 => {
                if let Some(tile) = self
                    .engine
                    .scene
                    .player_routes
                    .get(&(entry.entity_id as usize))
                    .copied()
                {
                    let ctrl = self.engine.configs.minimenu.as_ref().is_some_and(|d| {
                        crate::ui_defaults::key_binding_held(&d.ctrlrunning, |k| {
                            self.keyboard.held(k)
                        })
                    });
                    if let Some((opcode, payload)) = crate::ui_player_options::build_opplayer_packet(
                        action,
                        entry.entity_id as u16,
                        ctrl,
                    ) {
                        self.engine.outgoing.push(opcode);
                        self.engine.outgoing.extend(payload);
                        self.engine.menu.cross = Cross {
                            x,
                            y,
                            mode: 2,
                            cycle: 0,
                        };
                        self.engine.minimap.flag = Some(tile);
                    }
                }
            }
            15 | 16 => {
                //  (16, self) and (15). The original client sends the
                // retained `activeComponent*` fields without re-checking
                // target mode; `clearTargetMode` only resets the invobject.
                let target = self.state.interaction.target.clone();
                let ctrl = self.ctrl_held();
                let player = if action == 16 {
                    self.engine
                        .camera
                        .cam2
                        .scene
                        .local_player
                        .map(|player| player.index as usize)
                } else {
                    usize::try_from(entry.entity_id)
                        .ok()
                        .filter(|index| self.engine.scene.player_routes.contains_key(index))
                };
                if let Some(player) = player {
                    if let Some((opcode, payload)) = crate::ui_player_options::build_opplayert(
                        action,
                        player as u16,
                        target.child,
                        target.object,
                        target.parent,
                        ctrl,
                    ) {
                        self.engine.menu.cross = Cross {
                            x,
                            y,
                            mode: 2,
                            cycle: 0,
                        };
                        self.engine.outgoing.push(opcode);
                        self.engine.outgoing.extend(payload);
                        if action == 15 {
                            self.engine.minimap.flag =
                                self.engine.scene.player_routes.get(&player).copied();
                        }
                    }
                }
            }
            57 | 1007 => self.state.interaction.actions.push_back(Action::Op {
                op: entry.entity_id as i32,
                parent: entry.tile_z,
                child: entry.tile_x,
                base: entry.target.as_ref().map(|s| s.encode_utf16().collect()),
            }),
            25 => {
                self.state.interaction.actions.push_back(Action::Select {
                    parent: entry.tile_z,
                    child: entry.tile_x,
                });
                return;
            }
            58 => self.state.interaction.actions.push_back(Action::Target {
                parent: entry.tile_z,
                child: entry.tile_x,
            }),
            30 => self.state.interaction.actions.push_back(Action::Pause {
                parent: entry.tile_z,
                child: entry.tile_x,
            }),
            23 => {
                let defaults = self.engine.configs.minimenu.as_ref();
                let shift = defaults.is_some_and(|d| {
                    crate::ui_defaults::key_binding_held(&d.shiftteleport, |k| {
                        self.keyboard.held(k)
                    })
                });
                let ctrl = defaults.is_some_and(|d| {
                    crate::ui_defaults::key_binding_held(&d.ctrlrunning, |k| self.keyboard.held(k))
                });
                let kind = entry.entity_id as i32;
                if self.engine.account.staff_mod_level > 0 && shift {
                    self.queue_jtele([entry.tile_x, entry.tile_z]);
                } else if kind == 1 {
                    // A minimap click: the move message selects
                    // `MOVE_MINIMAPCLICK`, then appends the
                    // anticheat tail. No cross update for this kind (the
                    //  cross branch is game-kind only); the shared
                    // tail below still clears an active target.
                    let base = [
                        self.engine.camera.cam2.scene.base[0] >> 9,
                        self.engine.camera.cam2.scene.base[1] >> 9,
                    ];
                    let tile = [entry.tile_x, entry.tile_z];
                    let Some(local) = self.engine.camera.cam2.scene.local_player.as_ref() else {
                        Self::gap_once(
                            &mut self.engine.menu.gaps,
                            "MOVE_MINIMAPCLICK without local player",
                        );
                        self.finish_menu_option();
                        return;
                    };
                    // `Trackable.coord` is absolute (`trans + base * 512`,
                    // the original client sends `trans`.
                    let player = [
                        local.coord[0] - self.engine.camera.cam2.scene.base[0],
                        local.coord[2] - self.engine.camera.cam2.scene.base[1],
                    ];
                    self.engine
                        .outgoing
                        .extend(crate::ui_scene_options::move_minimap_click(
                            base,
                            tile,
                            ctrl,
                            self.engine.minimap.orbit_yaw & 0x3FFF,
                            self.engine.minimap.angle,
                            self.engine.minimap.zoom,
                            player,
                        ));
                    self.engine.minimap.flag = Some(tile);
                } else {
                    let base = [
                        self.engine.camera.cam2.scene.base[0] >> 9,
                        self.engine.camera.cam2.scene.base[1] >> 9,
                    ];
                    let tile = [entry.tile_x, entry.tile_z];
                    let bytes = crate::ui_scene_options::move_game_click(base, tile, ctrl);
                    // createMoveMessage then.
                    self.engine.minimap.flag = Some(tile);
                    self.engine.menu.cross = Cross {
                        x,
                        y,
                        mode: 1,
                        cycle: 0,
                    };
                    self.engine.outgoing.extend(bytes);
                }
            }
            60 => {
                let defaults = self.engine.configs.minimenu.as_ref();
                let shift = defaults.is_some_and(|d| {
                    crate::ui_defaults::key_binding_held(&d.shiftteleport, |k| {
                        self.keyboard.held(k)
                    })
                });
                if self.engine.account.staff_mod_level > 0 && shift {
                    self.queue_jtele([entry.tile_x, entry.tile_z]);
                } else {
                    let base = self.scene_base_tile();
                    self.engine.menu.cross = Cross {
                        x,
                        y,
                        mode: 1,
                        cycle: 0,
                    };
                    self.engine
                        .outgoing
                        .extend(crate::ui_scene_options::face_square(
                            base,
                            [entry.tile_x, entry.tile_z],
                        ));
                }
            }
            1006 => {}
            3..=6 | 18..=22 | 1001 | 1002 | 1004 => {
                // OPLOC1-6 and OPOBJ1-6.
                let ctrl = self.ctrl_held();
                let base = self.scene_base_tile();
                let tile = [entry.tile_x, entry.tile_z];
                let packet = match action {
                    3..=6 | 1001 | 1002 => crate::ui_scene_options::build_oploc(
                        action,
                        entry.entity_id,
                        base,
                        tile,
                        ctrl,
                    )
                    .map(|(opcode, payload)| (opcode, payload.to_vec())),
                    _ => crate::ui_scene_options::build_opobj(
                        action,
                        if entry.obj_id >= 0 {
                            entry.obj_id
                        } else {
                            entry.entity_id as i32
                        },
                        base,
                        tile,
                        from_menu,
                        ctrl,
                    )
                    .map(|(opcode, payload)| (opcode, payload.to_vec())),
                };
                if let Some((opcode, payload)) = packet {
                    self.engine.menu.cross = Cross {
                        x,
                        y,
                        mode: 2,
                        cycle: 0,
                    };
                    self.engine.outgoing.push(opcode);
                    self.engine.outgoing.extend(payload);
                    self.engine.minimap.flag = Some(tile);
                }
            }
            9..=13 | 1003 | 8 => {
                // OPNPC1-6 and OPNPCT: only an NPC
                // still in `npcs`; the flag is its first waypoint.
                let index = entry.entity_id as i32;
                let Some(route) = usize::try_from(index)
                    .ok()
                    .and_then(|index| self.engine.scene.npc_routes.get(&index).copied())
                else {
                    self.finish_menu_option();
                    return;
                };
                let ctrl = self.ctrl_held();
                let packet = if action == 8 {
                    let target = &self.state.interaction.target;
                    crate::ui_scene_options::build_opnpct(
                        action,
                        index as u16,
                        crate::ui_scene_options::ActiveTarget {
                            parentlayer: target.parent,
                            invobject: target.object,
                            id: target.child,
                        },
                        ctrl,
                    )
                    .map(|(opcode, payload)| (opcode, payload.to_vec()))
                } else {
                    crate::ui_scene_options::build_opnpc(action, index as u16, ctrl)
                        .map(|(opcode, payload)| (opcode, payload.to_vec()))
                };
                if let Some((opcode, payload)) = packet {
                    self.engine.menu.cross = Cross {
                        x,
                        y,
                        mode: 2,
                        cycle: 0,
                    };
                    self.engine.outgoing.push(opcode);
                    self.engine.outgoing.extend(payload);
                    self.engine.minimap.flag = Some(route);
                }
            }
            2 | 17 | 59 => {
                // OPLOCT, OPOBJT and APCOORDT
                //  read the retained `activeComponent*` fields.
                let target = self.state.interaction.target.clone();
                let active = crate::ui_scene_options::ActiveTarget {
                    parentlayer: target.parent,
                    invobject: target.object,
                    id: target.child,
                };
                let ctrl = self.ctrl_held();
                let base = self.scene_base_tile();
                let tile = [entry.tile_x, entry.tile_z];
                let packet = match action {
                    2 => crate::ui_scene_options::build_oploct(
                        action,
                        entry.entity_id,
                        base,
                        tile,
                        active,
                        ctrl,
                    )
                    .map(|(opcode, payload)| (opcode, payload.to_vec())),
                    17 => crate::ui_scene_options::build_opobjt(
                        action,
                        if entry.obj_id >= 0 {
                            entry.obj_id
                        } else {
                            entry.entity_id as i32
                        },
                        base,
                        tile,
                        active,
                        ctrl,
                    )
                    .map(|(opcode, payload)| (opcode, payload.to_vec())),
                    _ => crate::ui_scene_options::build_apcoordt(action, base, tile, active)
                        .map(|(opcode, payload)| (opcode, payload.to_vec())),
                };
                if let Some((opcode, payload)) = packet {
                    // APCOORDT uses the walk cross.
                    self.engine.menu.cross = Cross {
                        x,
                        y,
                        mode: if action == 59 { 1 } else { 2 },
                        cycle: 0,
                    };
                    self.engine.outgoing.push(opcode);
                    self.engine.outgoing.extend(payload);
                    self.engine.minimap.flag = Some(tile);
                }
            }
            // No other action id has a branch in the original client's useMenuOption: the
            // Cancel row and the disabled other-level player row
            // (action -1,) fall through to the shared tail.
            _ => {}
        }
        self.finish_menu_option();
    }
    /// `useMenuOption` shared tail: clear target mode.
    /// The `selectedArea` redraw is subsumed by the GPU full redraw
    /// (`mainredraw`).
    pub(super) fn finish_menu_option(&mut self) {
        if self.state.interaction.target.active {
            self.state
                .interaction
                .actions
                .push_back(crate::ui_interaction::Action::ClearTarget);
        }
    }
    /// `isCtrlKeyHeld` over `miniMenuDefaults.ctrlrunning`.
    pub(super) fn ctrl_held(&self) -> bool {
        self.engine.configs.minimenu.as_ref().is_some_and(|d| {
            crate::ui_defaults::key_binding_held(&d.ctrlrunning, |k| self.keyboard.held(k))
        })
    }
    /// `world.getBase()` in tiles.
    pub(super) fn scene_base_tile(&self) -> [i32; 2] {
        [
            self.engine.camera.cam2.scene.base[0] >> 9,
            self.engine.camera.cam2.scene.base[1] >> 9,
        ]
    }
}
