//! The engine's world-map owner calls: cache install, the per-cycle
//! `ClientWorldMap` update, the `worldmap_*` commands and the world-map
//! interaction at the end of a UI cycle.
use super::Engine;
use super::Runtime;
use crate::cache::Pack;
use anyhow::Result;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;
use rs910_core::fault::Fault;

impl Engine {
    /// Loads the world map: the
    /// area metadata, defaults and config lists of the world map, plus the
    /// map element types `mec_param` reads.
    pub fn load_world_map(&mut self, pack: &Pack) {
        match crate::minimap::MapElementStore::load(pack) {
            Ok(elements) => self.configs.map_element_types = Some(elements),
            Err(error) => log::warn!("[client910] world-map element types unavailable: {error:#}"),
        }
        let mut world_map = self.world_map.borrow_mut();
        if world_map.map.locs.is_none() {
            world_map.map.locs = self.configs.locs.clone();
        }
        world_map.install(pack);
    }
    /// The local player's `[level, x, z]` absolute tile
    /// (`localPlayerEntity`).
    pub(super) fn world_map_player(&self) -> Option<[i32; 3]> {
        self.camera
            .cam2
            .scene
            .local_player
            .map(|p| [p.level, p.coord[0] >> 9, p.coord[2] >> 9])
    }
    /// Every logic cycle: the map update, then one loading step.
    pub fn update_world_map(&mut self, fonts: Option<&crate::ui_fonts::Fonts>) {
        let player = self.world_map_player();
        let mut world_map = self.world_map.borrow_mut();
        world_map.members = self.account.logged_in_members;
        world_map.varbits = self.configs.inv_varbits.clone();
        if let Some([_, x, z]) = player {
            world_map.follow_player([x, z], false);
        }
        let ready =
            |id: i32| fonts.is_some_and(|f| f.get_font(id, true, true).ok().flatten().is_some());
        world_map.update_loading(player, &ready);
    }
    /// The `worldmap_*` commands over
    /// [`crate::world_map_client::ClientWorldMap`].
    /// `None` when `command` is not one of them.
    pub(super) fn world_map_command(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
    ) -> VmResult<Option<Option<Value>>> {
        if !command.starts_with("worldmap_")
            || command.starts_with("worldmap_3dview")
            || matches!(
                command,
                "worldmap_getcategorypriority" | "worldmap_setcategorypriority"
            )
        {
            return Ok(None);
        }
        let pop = |ints: &mut Vec<i32>| ints.pop().ok_or(VmError::StackUnderflow { stack: "int" });
        let pop2 = |ints: &mut Vec<i32>| {
            if ints.len() < 2 {
                return Err(VmError::StackUnderflow { stack: "int" });
            }
            let b = ints.pop().unwrap();
            let a = ints.pop().unwrap();
            Ok([a, b])
        };
        let int = |v: i32| Ok(Some(Some(Value::Int(v))));
        let fail = |reason: &str| {
            Err(VmError::TrapFailed {
                command: command.into(),
                reason: reason.into(),
            })
        };
        let player = self.world_map_player();
        let mut wm = self.world_map.borrow_mut();
        let coord = |packed: i32| (packed >> 14 & 0x3FFF, packed & 0x3FFF);
        match command {
            "worldmap_setzoom" => wm.set_zoom(pop(ints)?),
            "worldmap_getzoom" => return int(wm.get_zoom()),
            "worldmap_setmap" => {
                let id = pop(ints)?;
                wm.set_map(id, -1, -1, false);
            }
            "worldmap_getmap" => {
                let (x, z) = coord(pop(ints)?);
                let id = wm
                    .map
                    .map_at(x, z)
                    .map_or(-1, |i| wm.map.areas[i].id as i32);
                return int(id);
            }
            "worldmap_getmapname" => {
                let id = pop(ints)?;
                let name = wm
                    .map
                    .by_id(id)
                    .map(|i| wm.map.areas[i].map_name.clone())
                    .unwrap_or_default();
                objs.push(name);
            }
            "worldmap_getsize" => ints.extend([wm.window[2], wm.window[3]]),
            "worldmap_getdisplayposition" => ints.extend(wm.display_position()),
            "worldmap_getconfigorigin" => {
                let id = pop(ints)?;
                return int(wm
                    .map
                    .by_id(id)
                    .map_or(0, |i| wm.map.areas[i].config_origin));
            }
            "worldmap_getconfigsize" => {
                let id = pop(ints)?;
                let [x0, x1, z0, z1] = wm
                    .map
                    .by_id(id)
                    .map_or([0; 4], |i| wm.map.areas[i].config_bounds);
                ints.extend([x1 - x0, z1 - z0]);
            }
            "worldmap_getconfigbounds" => {
                let id = pop(ints)?;
                // The config bounds (x0, x1, z0, z1).
                let [x0, x1, z0, z1] = wm
                    .map
                    .by_id(id)
                    .map_or([0; 4], |i| wm.map.areas[i].config_bounds);
                ints.extend([x0, z0, x1, z1]);
            }
            "worldmap_listelement_start" | "worldmap_listelement_next" => {
                match wm.list_element(command == "worldmap_listelement_start") {
                    Some((id, packed)) => ints.extend([id, packed]),
                    None => ints.extend([-1, -1]),
                }
            }
            "worldmap_jumptosourcecoord" | "worldmap_jumptosourcecoord_instant" => {
                let packed = pop(ints)?;
                let display = wm.map.metadata().and_then(|m| {
                    crate::world_map::source_to_display(
                        m,
                        packed >> 28 & 0x3,
                        packed >> 14 & 0x3FFF,
                        packed & 0x3FFF,
                    )
                });
                if let Some([x, z]) = display {
                    if command == "worldmap_jumptosourcecoord" {
                        wm.jump_to(x, z);
                    } else {
                        wm.jump_to_instant(x, z);
                    }
                }
            }
            "worldmap_coordinmap" => {
                let [packed, id] = pop2(ints)?;
                let (x, z) = coord(packed);
                let found = wm
                    .map
                    .areas
                    .iter()
                    .any(|a| a.active && a.contains(x, z) && a.id as i32 == id);
                return int(i32::from(found));
            }
            "worldmap_getconfigzoom" => {
                let id = pop(ints)?;
                return int(wm.map.by_id(id).map_or(-1, |i| wm.map.areas[i].config_zoom));
            }
            "worldmap_isloaded" => return int(i32::from(wm.loading == 100)),
            "worldmap_jumptodisplaycoord" => {
                let (x, z) = coord(pop(ints)?);
                wm.jump_to(x, z);
            }
            "worldmap_jumptodisplaycoord_instant" => {
                let (x, z) = coord(pop(ints)?);
                wm.jump_to_instant(x, z);
            }
            "worldmap_getsourceposition" => {
                let [x, z] = wm.display_position();
                let source = wm
                    .map
                    .metadata()
                    .map(|m| crate::world_map::display_to_source(m, x, z));
                match source {
                    None | Some(None) => ints.extend([-1, -1]),
                    Some(Some([_, sx, sz])) => ints.extend([sx, sz]),
                }
            }
            "worldmap_setmap_coord" | "worldmap_setmap_coord_override" => {
                let [id, packed] = pop2(ints)?;
                let (x, z) = coord(packed);
                wm.set_map(id, x, z, command == "worldmap_setmap_coord_override");
            }
            "worldmap_getdisplaycoord" => {
                let packed = pop(ints)?;
                let display = wm.map.metadata().and_then(|m| {
                    crate::world_map::source_to_display(
                        m,
                        packed >> 28 & 0x3,
                        packed >> 14 & 0x3FFF,
                        packed & 0x3FFF,
                    )
                });
                ints.extend(display.unwrap_or([-1, -1]));
            }
            "worldmap_getsourcecoord" => {
                let (x, z) = coord(pop(ints)?);
                let source = wm
                    .map
                    .metadata()
                    .and_then(|m| crate::world_map::display_to_source(m, x, z));
                ints.extend(source.map_or([-1, -1], |[_, sx, sz]| [sx, sz]));
            }
            "worldmap_flashelement" => {
                let id = pop(ints)?;
                wm.flash_element(id);
            }
            "worldmap_flashelementcategory" => {
                let id = pop(ints)?;
                wm.flash_category(id);
            }
            "worldmap_disableelements" => wm.disable_elements = pop(ints)? == 1,
            "worldmap_getdisableelements" => return int(i32::from(wm.disable_elements)),
            "worldmap_disableelementcategory" | "worldmap_disableelement" => {
                let [id, flag] = pop2(ints)?;
                let set = if command == "worldmap_disableelement" {
                    &mut wm.disabled_elements
                } else {
                    &mut wm.disabled_categories
                };
                if flag == 1 {
                    set.insert(id);
                } else {
                    set.remove(&id);
                }
            }
            "worldmap_getdisableelementcategory" => {
                let id = pop(ints)?;
                return int(i32::from(wm.disabled_categories.contains(&id)));
            }
            "worldmap_getdisableelement" => {
                let id = pop(ints)?;
                return int(i32::from(wm.disabled_elements.contains(&id)));
            }
            "worldmap_getcurrentmap" => return int(wm.map.metadata().map_or(-1, |m| m.id as i32)),
            "worldmap_findnearestelement" => {
                let [id, packed] = pop2(ints)?;
                let (x, z) = coord(packed);
                let nearest = wm.nearest_element(id, x, z);
                return int(if nearest < 0 { -1 } else { nearest });
            }
            "worldmap_closemap" => {
                // Closing the map reads the local player's coordinate.
                let Some([_, x, z]) = player else {
                    return fail(&Fault::MissingValue.message("local player entity"));
                };
                wm.close_map([x, z]);
            }
            "worldmap_disabletextsize" => {
                let [size, flag] = pop2(ints)?;
                if !(0..3).contains(&size) {
                    return fail(&Fault::InvalidState.message("world-map text size"));
                }
                wm.hide_text[size as usize] = flag == 1;
            }
            "worldmap_getdisabletextsize" => {
                let size = pop(ints)?;
                if !(0..3).contains(&size) {
                    return fail(&Fault::InvalidState.message("world-map text size"));
                }
                return int(i32::from(wm.hide_text[size as usize]));
            }
            "worldmap_disabletype" => {
                let [kind, flag] = pop2(ints)?;
                if !wm.set_disable_type(kind, flag == 1) {
                    return fail(&Fault::InvalidState.message("world-map disable type"));
                }
            }
            "worldmap_getdisabletype" => {
                let kind = pop(ints)?;
                let value = wm.get_disable_type(kind);
                if value < 0 {
                    return fail(&Fault::InvalidState.message("world-map disable type"));
                }
                return int(value);
            }
            "worldmap_setflashloops" => {
                let v = pop(ints)?;
                wm.set_flash_loops(v);
            }
            "worldmap_setflashloops_default" => {
                wm.set_flash_loops(crate::world_map_client::FLASH_LOOPS)
            }
            "worldmap_setflashtics" => {
                let v = pop(ints)?;
                wm.set_flash_tics(v);
            }
            "worldmap_setflashtics_default" => {
                wm.set_flash_tics(crate::world_map_client::FLASH_TICS)
            }
            "worldmap_perpetualflash" => wm.perpetual_flash = pop(ints)? == 1,
            "worldmap_stopcurrentflashes" => {
                wm.flash_elements.clear();
                wm.flash_categories.clear();
            }
            _ => return Ok(None),
        }
        Ok(Some(None))
    }
}

impl Runtime {
    /// The world-map tail of a logic cycle: the map update when the map
    /// component was walked, the `CLICKWORLDMAP` of a map press, clearing the
    /// click state when there is no component, and a release's menu.
    pub(super) fn finish_world_map_cycle(
        &mut self,
        interaction: crate::ui_loop::WorldMapInteraction,
        mouse: [i32; 2],
    ) -> Result<()> {
        if interaction.seen {
            let events = self.engine.world_map.borrow_mut().update(mouse);
            if crate::ui_debug_flags::flags().ui_trace_input {
                let wm = self.engine.world_map.borrow();
                log::info!(
                    "[ui-input] world map mouse={mouse:?} over={} click_state={} containers={} nearest={:?} options={} triggers={:?}",
                    wm.mouse_over,
                    wm.click_state,
                    wm.containers.as_ref().map_or(0, Vec::len),
                    wm.containers.as_ref().and_then(|c| c
                        .iter()
                        .filter(|c| c.sprite[1] > c.sprite[0])
                        .min_by_key(|c| (c.sprite[0] + c.sprite[1] - 2 * mouse[0]).abs() + (c.sprite[2] + c.sprite[3] - 2 * mouse[1]).abs())
                        .map(|c| c.sprite)),
                    events.options.len(),
                    events.triggers
                );
            }
            if !self.state.minimenu.open {
                for o in events.options {
                    self.state.minimenu.add_option(crate::ui_minimenu::Entry {
                        op: o.op,
                        target: o.target,
                        cursor: -1,
                        action: o.action,
                        obj_id: -1,
                        entity_id: i64::from(o.element),
                        tile_x: o.category,
                        tile_z: 0,
                        enabled: true,
                        has_arrow: false,
                        sub_id: i64::from(o.element),
                        force_submenu: false,
                        detail: None,
                    });
                }
            }
            for t in events.triggers {
                self.state.interaction.actions.push_back(
                    crate::ui_interaction::Action::MapElementTrigger {
                        trigger: t.trigger,
                        element: t.element,
                        category: t.category,
                        mouse: Some(mouse),
                    },
                );
            }
        } else {
            self.engine.world_map.borrow_mut().click_state = 0;
        }
        if let Some(packed) = interaction
            .click
            .filter(|_| self.engine.account.staff_mod_level > 0 && self.keyboard.held(82))
        {
            // Staff Ctrl-press `jtele`s instead of
            // pressing the map (`continue` before `clickState = 1`).
            self.engine.world_map.borrow_mut().click_state = 0;
            let command = format!(
                "tele {},{},{},{},{}",
                packed >> 28 & 0x3,
                (packed >> 14 & 0x3FFF) >> 6,
                (packed & 0x3FFF) >> 6,
                packed >> 14 & 0x3F,
                packed & 0x3F
            );
            match crate::client_command::remote(&command, true, false) {
                Ok(bytes) => self.engine.outgoing.extend(bytes),
                Err(error) => Self::gap_once(
                    &mut self.engine.menu.gaps,
                    &format!("jtele command encoding: {error}"),
                ),
            }
        } else if let Some(packed) = interaction.click {
            // `CLICKWORLDMAP` (18, size 4): `p4_alt3`.
            self.engine.outgoing.extend([
                crate::proto::client::CLICKWORLDMAP,
                (packed >> 16) as u8,
                (packed >> 24) as u8,
                packed as u8,
                (packed >> 8) as u8,
            ]);
        }
        if let Some([x, y]) = interaction.show_menu {
            if self.state.minimenu.pending_open {
                self.state.minimenu.pending_open = false;
                self.open_menu([x, y])?;
            } else if let Some(entry) = self.state.minimenu.pending.take() {
                self.use_menu_option(&entry, x, y, false);
            }
        }
        Ok(())
    }
}
