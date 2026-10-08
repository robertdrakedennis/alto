//! The top-level interface update, the interface and layer loops, and the
//! three hook queues the game update drains in order (timer, mouse-stop,
//! main). This is the per-logic-cycle
//! walk that turns mouse state, timers and transmit counters into queued
//! hook requests on the retained component graph.
//!
//! Not yet owned here (each fails or skips loudly):
//! The scene viewport player/entity hit-box pass apart from
//! active player/NPC subinterface processing and the object icon prewarm.
//! Active-entity sub-interface
//! validity is preflighted by the retained runtime before this walk; the
//! `IF_PROCESS_ACTIVE_*` hooks and global player/NPC process triggers are
//! dispatched by the retained runtime before this walk.
use crate::{
    ui_components::{field_snapshot, Array, Ref, Store},
    ui_draw::Frame,
    ui_hooks::{Executor, Request, INTERACTIVE_LIMIT},
    ui_properties::State,
};
use anyhow::{Context, Result};
use std::{collections::VecDeque, rc::Rc};
field_snapshot!(
    /// The scalars `loopLayer` reads for one component, taken when the walk
    /// reaches it (hooks, key ops and `held`/`hovered` writes later in the
    /// same iteration do not change them).
    LayerFields {
        angle2d: i32,
        clickmask: bool,
        clientcode: i32,
        colour: i32,
        fontmono: bool,
        graphicshadow: i32,
        hasKeybinds: bool,
        hashook: bool,
        height: i32,
        held: bool,
        id: i32,
        invobject: i32,
        layer: i32,
        mouseovercursor: i32,
        noclickthrough: bool,
        parentlayer: i32,
        r#type: i32,
        scrollx: i32,
        scrolly: i32,
        skyboxId: i32,
        textHAlign: i32,
        textVAlign: i32,
        textfont: i32,
        tiling: bool,
        trans: i32,
        width: i32,
        x: i32,
        y: i32,
    }
);

pub const SCENE_VIEWPORT: i32 = 1337;
pub const MINIMAP: i32 = 1338;
pub const WORLD_MAP: i32 = 1400;
pub const WORLD_MAP_OVERVIEW: i32 = 1401;
pub const SCENE_VIEWPORT_NO_PROJECTILES: i32 = 1403;
pub const HOVER_TEXT: i32 = 1406;

/// The camera/local-player state needed by the minimap click transform. The
/// app renderer owns the minimap pixels; the retained UI walk only needs this
/// scene-space input to build the ordinary menu entry the minimenu runs.
#[derive(Clone, Debug, Default)]
pub struct MinimapInput {
    pub player_fine: [i32; 2],
    pub player_size: i32,
    pub camera_state: i32,
    pub camera_yaw: i32,
    pub orbit_yaw: i32,
    pub anticheat_angle: i32,
    pub zoom: i32,
    pub toggle: i32,
    pub walk_text: String,
    pub walk_cursor: i32,
}

/// One logic cycle's input as the client samples it: the mouse snapshot, the
/// first queued mouse event (button action 0 = left press), whether the left
/// button is held, the wheel rotation and the list of keyboard events (key
/// code, char).
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub single_mouse_button: i32,
    pub console_open: bool,
    pub mouse: [i32; 2],
    pub click: Option<[i32; 2]>,
    /// The first queued mouse event with its button action and click count
    /// (the minimenu tests its bindings against it).
    pub event: Option<crate::ui_defaults::MouseEvent>,
    pub left_held: bool,
    pub middle_held: bool,
    pub right_held: bool,
    pub wheel: i32,
    pub keys: Vec<(i32, i32)>,
    pub held_keys: Vec<bool>,
    pub cycle: i32,
    /// The scene options resolved for this cycle's mouse and camera
    /// (`ui_scene_options.rs`); added when the loop reaches the scene
    /// viewport under the mouse.
    pub scene_options: Vec<crate::ui_scene_options::SceneOption>,
    /// The minimap input state, installed by the retained camera owner.
    pub minimap: Option<MinimapInput>,
}

/// The world map state the component walk drives and what it leaves for the
/// cycle's end.
pub struct WorldMapInteraction {
    pub map: Rc<std::cell::RefCell<crate::world_map_client::ClientWorldMap>>,
    /// The map component was walked this cycle.
    pub seen: bool,
    /// `minimapClicked` with its packed source coordinate.
    pub click: Option<i32>,
    /// Show the minimenu at the press position on a plain release.
    pub show_menu: Option<[i32; 2]>,
}

impl WorldMapInteraction {
    pub fn new(map: Rc<std::cell::RefCell<crate::world_map_client::ClientWorldMap>>) -> Self {
        Self {
            map,
            seen: false,
            click: None,
            show_menu: None,
        }
    }
}

/// A transmit counter with its 64-entry ring (`varpTransmitNum` /
/// `varpTransmitted` and siblings).
#[derive(Clone, Copy, Debug)]
pub struct Counter {
    pub num: i32,
    pub ids: [i32; 64],
}
impl Default for Counter {
    fn default() -> Self {
        Self {
            num: 0,
            ids: [0; 64],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Transmits {
    pub varp: Counter,
    pub varc: Counter,
    pub varcstr: Counter,
    pub inv: Counter,
    pub stat: Counter,
    pub varclan: Counter,
}

/// The redraw cycle and the per-transmit stamps of the last redraw that the
/// redraw-cycle triggers compare against; `splineFinished` for oncamfinished.
#[derive(Clone, Debug, Default)]
pub struct Cycles {
    pub redraw: i32,
    pub chat: i32,
    pub friend: i32,
    pub clan: i32,
    pub clan_settings: i32,
    pub clan_channel: i32,
    pub stock: i32,
    pub misc: i32,
    pub player_group: i32,
    pub player_group_varp: i32,
    pub camera_update: i32,
    pub spline_finished: bool,
}

impl Cycles {
    /// Resets the per-transmit stamps; the redraw cycle itself is not reset.
    pub fn reset_transmit_nums(&mut self) {
        self.chat = 0;
        self.friend = 0;
        self.clan = 0;
        self.clan_settings = 0;
        self.clan_channel = 0;
        self.stock = 0;
        self.misc = 0;
        self.player_group = 0;
        self.player_group_varp = 0;
    }
}

struct Walk<'a, 'b, 'c> {
    store: &'a mut Store,
    state: &'a mut State,
    frame: &'a mut Frame,
    input: &'a Input,
    transmits: &'a Transmits,
    /// Whether the interface being looped is transient.
    transient: bool,
    executor: Option<&'b mut dyn Executor>,
    world_map: Option<&'c mut WorldMapInteraction>,
}

fn is(c: &Ref, other: &Option<Ref>) -> bool {
    other.as_ref().is_some_and(|o| Rc::ptr_eq(o, c))
}

impl Walk<'_, '_, '_> {
    fn request(&self, c: &Ref, hook: &str) -> Option<Request> {
        let args = c.borrow().hooks.get(hook).cloned()?;
        let mut request = Request::component(c, args);
        if hook == crate::ui_properties::RETAINED_PLAYER_HOOK {
            request.variable_event_tokens = c.borrow().retained_player_transmit;
        }
        Some(request)
    }
    fn mouse_request(&self, c: &Ref, hook: &str, mouse: [i32; 2]) -> Option<Request> {
        let mut r = self.request(c, hook)?;
        r.is_mouse = true;
        r.mouse = mouse;
        Some(r)
    }
    /// Whether a point is inside the component, or
    /// inside its clickmask sprite when the component asks for pixel hits.
    fn hits(&mut self, c: &Ref, masked: bool, at: [i32; 2], origin: [i32; 2]) -> Result<bool> {
        if !masked {
            return Ok(true);
        }
        let (width, height) = {
            let f = &c.borrow().f;
            (f.width, f.height)
        };
        let Some(mask) = self
            .state
            .component_graphic(c, &mut crate::ui_sprites::Cpu)?
        else {
            return Ok(true);
        };
        if width != mask.size[0] || height != mask.size[1] {
            return Ok(true);
        }
        let dx = at[0].wrapping_sub(origin[0]);
        let dy = at[1].wrapping_sub(origin[1]);
        let Some(row) = usize::try_from(dy)
            .ok()
            .filter(|&row| row < mask.starts.len())
        else {
            return Ok(false);
        };
        let start = mask.starts[row];
        Ok(dx >= start && dx <= mask.lengths[row].wrapping_add(start))
    }
    /// One transmit family against the component's stamp.
    fn transmit(
        &mut self,
        c: &Ref,
        hook: &str,
        list: Option<&str>,
        counter: &Counter,
        last: fn(&crate::ui_component_fields::Fields) -> i32,
        stamp: fn(&mut crate::ui_component_fields::Fields, i32),
    ) -> Result<()> {
        let last_seen = last(&c.borrow().f);
        if !c.borrow().hooks.contains_key(hook) {
            return Ok(());
        }
        let list = list.and_then(|name| c.borrow().transmits.get(name).cloned());
        let retained = hook == crate::ui_properties::RETAINED_PLAYER_HOOK
            && c.borrow().retained_player_transmit.is_some();
        let fire = if retained {
            let Some(fire) =
                crate::ui_properties::retained_player_event(counter, last_seen, list.as_deref())
            else {
                return Ok(());
            };
            fire
        } else {
            if counter.num <= last_seen {
                return Ok(());
            }
            match list {
                None => true,
                Some(_) if counter.num.wrapping_sub(last_seen) > 64 => true,
                Some(ids) => {
                    (last_seen..counter.num).any(|n| ids.contains(&counter.ids[(n & 63) as usize]))
                }
            }
        };
        if fire {
            if let Some(r) = self.request(c, hook) {
                self.state.layout.hooks.push_back(r);
            }
        }
        stamp(&mut c.borrow_mut().f, counter.num);
        Ok(())
    }
    /// Loops one interface's layers.
    fn interface(
        &mut self,
        sub: Option<&crate::ui_lifecycle::SubRef>,
        id: i32,
        clip: [i32; 4],
        offset: [i32; 2],
    ) -> Result<()> {
        if id == -1 || !self.store.open(id, None)? {
            return Ok(());
        }
        // Active bindings were validated against the live scene snapshot by
        // `Runtime::tick` before this walk. Valid active subs use the same
        // interface traversal as plain subs; invalid ones have already been
        // purged with the normal lifecycle owner.
        let _ = sub;
        let i = self
            .store
            .interfaces
            .get(&id)
            .context("interface missing after open")?
            .borrow();
        let array = i.sorted.as_ref().unwrap_or(&i.components).clone();
        self.transient = i.transient;
        drop(i);
        self.layer(&array, -1, clip, offset)
    }
    /// Loops one layer of components.
    fn layer(&mut self, array: &Array, layer: i32, clip: [i32; 4], offset: [i32; 2]) -> Result<()> {
        let [mx, my] = self.input.mouse;
        let count = array.borrow().len();
        for index in 0..count {
            // Other layers' components are skipped before their snapshot is
            // taken (same read, no reference-count traffic; programme Phase 6).
            let c = {
                let array = array.borrow();
                let Some(c) = array.get(index).context("component array index")? else {
                    continue;
                };
                if c.borrow().f.layer != layer {
                    continue;
                }
                c.clone()
            };
            let f = LayerFields::of(&c.borrow().f);
            if f.layer != layer {
                continue;
            }
            let x = f.x.wrapping_add(offset[0]);
            let y = f.y.wrapping_add(offset[1]);
            let bounds = if f.r#type == 2 {
                clip
            } else {
                let extra = (f.r#type == 9) as i32;
                [
                    x.max(clip[0]),
                    y.max(clip[1]),
                    x.wrapping_add(f.width).wrapping_add(extra).min(clip[2]),
                    y.wrapping_add(f.height).wrapping_add(extra).min(clip[3]),
                ]
            };
            let [l, t, r, b] = bounds;
            let interactive = f.r#type == 0
                || f.hashook
                || self.state.layout.active_mask(&c) != 0
                || is(&c, &self.state.interaction.drag.layer)
                || matches!(
                    f.clientcode,
                    MINIMAP | HOVER_TEXT | SCENE_VIEWPORT | SCENE_VIEWPORT_NO_PROJECTILES
                );
            if !interactive {
                // Gap: the object icon prewarm needs the icon service.
                continue;
            }
            if self.frame.hidden(self.state, &c) {
                continue;
            }
            // Sample the dragged component's origin.
            if is(&c, &self.state.interaction.drag.component)
                && crate::ui_interaction::draggable(self.state, &c)
            {
                self.state.interaction.drag_ready = true;
                self.state.interaction.drag_origin = [x, y];
            }
            if f.hasKeybinds || (l < r && t < b) {
                let inside = mx >= l && my >= t && mx < r && my < b;
                if f.noclickthrough && inside {
                    // A click-blocking layer under the mouse cancels
                    // pending mouse hooks and hover state.
                    let mut kept = VecDeque::new();
                    for req in std::mem::take(&mut self.state.layout.hooks) {
                        if req.is_mouse {
                            if let Some(target) = &req.component {
                                target.borrow_mut().f.hovered = false;
                            }
                        } else {
                            kept.push_back(req);
                        }
                    }
                    self.state.layout.hooks = kept;
                    if self.state.interaction.drag_cycle == 0 {
                        crate::ui_interaction::cancel_drag(self.state);
                    }
                    // Clear the world-map click state and the minimap click.
                    if let Some(world_map) = self.world_map.as_deref_mut() {
                        let mut map = world_map.map.borrow_mut();
                        map.click_state = 0;
                        map.mouse_over = false;
                        world_map.click = None;
                    }
                    // An open minimenu is left alone; otherwise it resets.
                    if !self.state.minimenu.open {
                        self.state.minimenu.reset();
                    }
                }
                let masked = f.clickmask
                    && f.r#type == 5
                    && f.trans == 0
                    && f.skyboxId < 0
                    && f.invobject == -1
                    && !f.tiling
                    && f.angle2d == 0;
                let mut hovered = inside && self.hits(&c, masked, [mx, my], [x, y])?;
                // Cursor precedence applies only when no target is active;
                // otherwise the target cursor is preserved.
                if hovered && !self.state.interaction.target.active {
                    if f.mouseovercursor >= 0 {
                        self.state.minimenu.default_cursor = f.mouseovercursor;
                    } else if f.noclickthrough {
                        self.state.minimenu.default_cursor = -1;
                    }
                }
                // The hovered component's own option (action 58).
                if !self.state.minimenu.open && hovered && !self.transient {
                    let mask = self.state.layout.active_mask(&c);
                    let target = &self.state.interaction.target;
                    if target.active
                        && crate::ui_minimenu::mask_targetable(mask)
                        && target.mask & 32 != 0
                    {
                        // A missing param type (param -1 or unknown) always
                        // passes; else the component's param must differ from
                        // the param type's default.
                        let gate = if target.param == -1 {
                            true
                        } else if let Some(p) = self.state.params.get(&target.param) {
                            let value = c
                                .borrow()
                                .params
                                .as_ref()
                                .and_then(|v| {
                                    v.iter().find(|(id, _)| *id == i64::from(target.param))
                                })
                                .and_then(|(_, v)| {
                                    if let crate::ui_components::Arg::Int(v) = v {
                                        Some(*v)
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(p.integer);
                            value != p.integer
                        } else {
                            true
                        };
                        if gate {
                            self.state.minimenu.add_option(crate::ui_minimenu::Entry {
                                op: target.verb.clone(),
                                target: Some(format!(
                                    "{} -> {}",
                                    target.name,
                                    // Nothing since the walk reached `c` writes
                                    // `opbase` (only `hovered` and drag state).
                                    c.borrow()
                                        .f
                                        .opbase
                                        .as_ref()
                                        .map(|s| String::from_utf16_lossy(s))
                                        .unwrap_or_else(|| "null".into())
                                )),
                                cursor: target.cursor,
                                action: 58,
                                obj_id: f.invobject,
                                entity_id: 0,
                                tile_x: f.id,
                                tile_z: f.parentlayer,
                                enabled: true,
                                has_arrow: false,
                                sub_id: i64::from(f.id.wrapping_shl(32) | f.parentlayer),
                                force_submenu: false,
                                detail: None,
                            });
                        }
                    }
                    self.state.minimenu.add_component_options(&c, mask);
                }
                let mut held = self.input.left_held && hovered;
                let mut clicked = false;
                let click = self.input.click;
                if let Some(at) = click {
                    if at[0] >= l && at[1] >= t && at[0] < r && at[1] < b {
                        clicked = self.hits(&c, masked, at, [x, y])?;
                    }
                }
                // Synchronous ops run before drag pickup.
                if f.hasKeybinds && !self.input.console_open {
                    // The key tables as of this point; ops dispatched below may
                    // rewrite the live component.
                    let (keys, key_chars, key_mods) = {
                        let c = c.borrow();
                        (c.keys.clone(), c.key_chars.clone(), c.key_mods.clone())
                    };
                    if let Some(keys) = keys.as_ref() {
                        for index in 0..keys.len() {
                            let chars = key_chars
                                .as_ref()
                                .and_then(|v| v.get(index))
                                .copied()
                                .unwrap_or(0);
                            let next = c
                                .borrow()
                                .key_next_fire
                                .as_ref()
                                .and_then(|v| v.get(index))
                                .copied()
                                .unwrap_or(0);
                            let keyheld = |k: i32| {
                                self.input
                                    .held_keys
                                    .get(k as usize)
                                    .copied()
                                    .unwrap_or(false)
                            };
                            let mut down =
                                chars > 0 && self.input.keys.iter().any(|(_, ch)| *ch == chars);
                            let mut fire = down && next <= self.input.cycle;
                            if !down {
                                if let Some(ks) = &keys[index] {
                                    for (j, &key) in ks.iter().enumerate() {
                                        if keyheld(i32::from(key)) {
                                            down = true;
                                            if next > self.input.cycle {
                                                break;
                                            }
                                            let mods = key_mods
                                                .as_ref()
                                                .and_then(|v| v.get(index))
                                                .and_then(Option::as_ref)
                                                .and_then(|v| v.get(j))
                                                .copied()
                                                .unwrap_or(0);
                                            if mods == 0
                                                || ((mods & 8 == 0
                                                    || (!keyheld(86)
                                                        && !keyheld(82)
                                                        && !keyheld(81)))
                                                    && (mods & 2 == 0 || keyheld(86))
                                                    && (mods & 1 == 0 || keyheld(82))
                                                    && (mods & 4 == 0 || keyheld(81)))
                                            {
                                                fire = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if fire {
                                let action = if index < 10 {
                                    crate::ui_interaction::Action::Op {
                                        op: index as i32 + 1,
                                        parent: f.parentlayer,
                                        child: f.id,
                                        base: Some(vec![]),
                                    }
                                } else {
                                    crate::ui_interaction::Action::Select {
                                        parent: f.parentlayer,
                                        child: f.id,
                                    }
                                };
                                if let Some(executor) = self.executor.as_deref_mut() {
                                    crate::ui_interaction::dispatch(
                                        self.store, self.state, executor, action,
                                    )?;
                                } else {
                                    anyhow::bail!(
                                        "key operation requires synchronous hook executor"
                                    );
                                }
                                let mut c = c.borrow_mut();
                                let delay = c
                                    .key_delays
                                    .as_ref()
                                    .and_then(|v| v.get(index))
                                    .copied()
                                    .unwrap_or(0);
                                let rate =
                                    c.key_rates.get_or_insert_with(|| vec![0; keys.len()])[index];
                                let next = &mut c
                                    .key_next_fire
                                    .get_or_insert_with(|| vec![0; keys.len()])[index];
                                *next = if delay == 0 {
                                    i32::MAX
                                } else {
                                    self.input
                                        .cycle
                                        .wrapping_add(delay)
                                        .wrapping_add(if *next == 0 { rate } else { 0 })
                                };
                            }
                            if !down {
                                if let Some(next) = c.borrow_mut().key_next_fire.as_mut() {
                                    next[index] = 0;
                                }
                            }
                        }
                    }
                }
                // Runs before the ordinary mouse hooks.
                if clicked {
                    let at = click.unwrap();
                    crate::ui_interaction::pickup(
                        self.store,
                        self.state,
                        Some(c.clone()),
                        [at[0] - x, at[1] - y],
                    )?;
                }
                if self.state.interaction.drag.component.is_some()
                    && !is(&c, &self.state.interaction.drag.component)
                    && hovered
                {
                    if f.noclickthrough {
                        self.state.interaction.drop_target = None;
                    }
                    if self.state.layout.active_mask(&c) >> 21 & 1 != 0 {
                        self.state.interaction.drop_target = Some(c.clone());
                    }
                }
                if is(&c, &self.state.interaction.drag.layer) {
                    self.state.interaction.drag.parent_ready = true;
                    self.state.interaction.drag.bounds = [x, y, f.width, f.height];
                }
                if f.hashook || f.clientcode != 0 {
                    if hovered && self.input.wheel != 0 {
                        if let Some(mut req) = self.request(&c, "onscrollwheel") {
                            req.is_mouse = true;
                            req.mouse = [0, self.input.wheel];
                            self.state.layout.hooks.push_back(req);
                        }
                    }
                    if self.state.interaction.drag.component.is_some() {
                        clicked = false;
                        held = false;
                    } else if self.state.minimenu.open {
                        // The minimenu is open (the world-map click state is only
                        // raised by the world map owner).
                        clicked = false;
                        held = false;
                        hovered = false;
                    }
                    if f.clientcode != 0 {
                        if matches!(f.clientcode, SCENE_VIEWPORT | SCENE_VIEWPORT_NO_PROJECTILES) {
                            // Record the viewport and the skybox draw state.
                            self.state.layout.viewport = Some(c.clone());
                            if f.clientcode == SCENE_VIEWPORT {
                                // The scene options under the mouse (the menu-draw
                                // branch only restores draw state).
                                if !self.state.minimenu.open
                                    && mx >= l
                                    && my >= t
                                    && mx < r
                                    && my < b
                                {
                                    for o in &self.input.scene_options {
                                        let count = self.state.minimenu.option_count;
                                        self.state.minimenu.add_option(crate::ui_minimenu::Entry {
                                            op: o.op.clone(),
                                            target: o.target.clone(),
                                            cursor: o.cursor,
                                            action: o.action,
                                            obj_id: o.obj_id,
                                            entity_id: o.entity_id,
                                            tile_x: o.tile[0],
                                            tile_z: o.tile[1],
                                            enabled: o.enabled,
                                            has_arrow: o.has_arrow,
                                            sub_id: o.sub_id,
                                            force_submenu: o.force_submenu,
                                            detail: None,
                                        });
                                        if self.state.minimenu.option_count > count {
                                            self.state.minimenu.set_last_detail(o.detail.clone());
                                        }
                                        if self.state.minimenu.option_count > count {
                                            self.state.minimenu.set_last_quest_text(
                                                self.state
                                                    .scene_quest_text
                                                    .get(&(o.action, o.entity_id))
                                                    .cloned(),
                                            );
                                        }
                                    }
                                }
                                // Cover-marker rectangles are published by
                                // app::scene_cover_marker_overlays and consumed above by
                                // Runtime::scene_options to route the marker click to its NPC.
                            }
                            // Entity hit boxes remain a scene
                            // owner gap; process triggers run from Runtime::tick.
                            continue;
                        }
                        if matches!(
                            f.clientcode,
                            MINIMAP | WORLD_MAP | WORLD_MAP_OVERVIEW | HOVER_TEXT
                        ) {
                            if f.clientcode == MINIMAP {
                                if let Some(minimap) = self.input.minimap.as_ref() {
                                    // Minimap options are suppressed while the
                                    // map is hidden or a popup is already open.
                                    if (minimap.toggle == 0 || minimap.toggle == 3)
                                        && !self.state.minimenu.open
                                        && hovered
                                    {
                                        let tile = minimap_tile(
                                            minimap,
                                            [x, y, f.width, f.height],
                                            [mx, my],
                                        );
                                        let target = &self.state.interaction.target;
                                        if target.active && target.mask & 0x40 != 0 {
                                            self.state.minimenu.add_option(
                                                crate::ui_minimenu::Entry {
                                                    op: target.verb.clone(),
                                                    target: Some(format!("{} ->", target.name)),
                                                    cursor: target.cursor,
                                                    action: 59,
                                                    obj_id: f.invobject,
                                                    entity_id: 1,
                                                    tile_x: tile[0],
                                                    tile_z: tile[1],
                                                    enabled: true,
                                                    has_arrow: false,
                                                    sub_id: i64::from(
                                                        f.id.wrapping_shl(32) | f.parentlayer,
                                                    ),
                                                    force_submenu: true,
                                                    detail: None,
                                                },
                                            );
                                        } else {
                                            self.state.minimenu.add_option(
                                                crate::ui_minimenu::Entry {
                                                    op: minimap.walk_text.clone(),
                                                    target: Some(String::new()),
                                                    cursor: minimap.walk_cursor,
                                                    action: 23,
                                                    obj_id: -1,
                                                    entity_id: 1,
                                                    tile_x: tile[0],
                                                    tile_z: tile[1],
                                                    enabled: true,
                                                    has_arrow: false,
                                                    sub_id: 0,
                                                    force_submenu: true,
                                                    detail: None,
                                                },
                                            );
                                        }
                                    }
                                }
                            } else if f.clientcode == HOVER_TEXT {
                                // Retain the component's text style
                                // and origin for drawHoverText after the tree
                                // has finished painting.
                                self.state.hover_text = Some(crate::ui_properties::HoverText {
                                    origin: [x, y],
                                    size: [f.width, f.height],
                                    colour: f.colour,
                                    shadow: f.graphicshadow,
                                    halign: f.textHAlign,
                                    valign: f.textVAlign,
                                    font: f.textfont,
                                    font_mono: f.fontmono,
                                });
                            } else if f.clientcode == WORLD_MAP {
                                if let Some(world_map) = self.world_map.as_deref_mut() {
                                    // Hovered, held, and the queued press.
                                    world_map.seen = true;
                                    let press = clicked.then_some(click.unwrap_or([mx, my]));
                                    let events = world_map.map.borrow_mut().component_input(
                                        [x, y, f.width, f.height],
                                        [mx, my],
                                        hovered,
                                        held,
                                        press,
                                        self.state.minimenu.option_count > 0,
                                    );
                                    if events.click.is_some() {
                                        world_map.click = events.click;
                                    }
                                    if events.show_menu.is_some() {
                                        world_map.show_menu = events.show_menu;
                                    }
                                }
                            } else if f.clientcode == WORLD_MAP_OVERVIEW {
                                // The overview map reacts while held.
                                if let Some(world_map) = self.world_map.as_deref_mut() {
                                    if held {
                                        world_map.map.borrow_mut().overview_click(
                                            mx.wrapping_sub(x),
                                            my.wrapping_sub(y),
                                            f.width,
                                            f.height,
                                        );
                                    }
                                }
                            }
                            // The world map and minimap own their input before
                            // ordinary hooks.
                            continue;
                        }
                    }
                    let click_at = click
                        .map(|at| [at[0].wrapping_sub(x), at[1].wrapping_sub(y)])
                        .unwrap_or([0, 0]);
                    let mouse_at = [mx.wrapping_sub(x), my.wrapping_sub(y)];
                    if !f.held && clicked {
                        c.borrow_mut().f.held = true;
                        if let Some(r) = self.mouse_request(&c, "onclick", click_at) {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                    let is_held = c.borrow().f.held;
                    if is_held && held {
                        if let Some(r) = self.mouse_request(&c, "onclickrepeat", mouse_at) {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                    if is_held && !held {
                        c.borrow_mut().f.held = false;
                        if let Some(r) = self.mouse_request(&c, "onrelease", mouse_at) {
                            self.state.layout.hooks_mouse_stop.push_back(r);
                        }
                    }
                    if held {
                        if let Some(r) = self.mouse_request(&c, "onhold", mouse_at) {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                    let was_hovered = c.borrow().f.hovered;
                    if !was_hovered && hovered {
                        c.borrow_mut().f.hovered = true;
                        if let Some(r) = self.mouse_request(&c, "onmouseover", mouse_at) {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                    let is_hovered = c.borrow().f.hovered;
                    if is_hovered && hovered {
                        if let Some(r) = self.mouse_request(&c, "onmouserepeat", mouse_at) {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                    if is_hovered && !hovered {
                        c.borrow_mut().f.hovered = false;
                        if let Some(r) = self.mouse_request(&c, "onmouseleave", mouse_at) {
                            self.state.layout.hooks_mouse_stop.push_back(r);
                        }
                    }
                    if let Some(r) = self.request(&c, "ontimer") {
                        self.state.layout.hooks_timer.push_back(r);
                    }
                    use crate::ui_component_fields::Fields as F;
                    let tx = self.transmits.clone();
                    self.transmit(
                        &c,
                        "onvarctransmit",
                        Some("onvarctransmitlist"),
                        &tx.varc,
                        |f: &F| f.lastVarcTransmit,
                        |f: &mut F, v| f.lastVarcTransmit = v,
                    )?;
                    self.transmit(
                        &c,
                        "onvarcstrtransmit",
                        Some("onvarcstrtransmitlist"),
                        &tx.varcstr,
                        |f: &F| f.lastVarcstrTransmit,
                        |f: &mut F, v| f.lastVarcstrTransmit = v,
                    )?;
                    self.transmit(
                        &c,
                        "onvartransmit",
                        Some("onvartransmitlist"),
                        &tx.varp,
                        |f: &F| f.lastVarTransmit,
                        |f: &mut F, v| f.lastVarTransmit = v,
                    )?;
                    self.transmit(
                        &c,
                        "oninvtransmit",
                        Some("oninvtransmitlist"),
                        &tx.inv,
                        |f: &F| f.lastInvTransmit,
                        |f: &mut F, v| f.lastInvTransmit = v,
                    )?;
                    self.transmit(
                        &c,
                        "onstattransmit",
                        Some("onstattransmitlist"),
                        &tx.stat,
                        |f: &F| f.lastStatTransmit,
                        |f: &mut F, v| f.lastStatTransmit = v,
                    )?;
                    self.transmit(
                        &c,
                        "onvarclantransmit",
                        None,
                        &tx.varclan,
                        |f: &F| f.lastVarclanTransmit,
                        |f: &mut F, v| f.lastVarclanTransmit = v,
                    )?;
                    // Redraw-cycle stamped triggers.
                    let cycles = self.state.life.cycles.clone();
                    let last_redraw = c.borrow().f.lastRedrawCycle;
                    for (stamp, hook) in [
                        (cycles.chat, "onchattransmit"),
                        (cycles.friend, "onfriendtransmit"),
                        (cycles.clan, "onclantransmit"),
                        (cycles.clan_settings, "onclansettingstransmit"),
                        (cycles.clan_channel, "onclanchanneltransmit"),
                        (cycles.stock, "onstocktransmit"),
                        (cycles.misc, "onmisctransmit"),
                        (cycles.player_group, "onplayergrouptransmit"),
                        (cycles.player_group_varp, "onplayergroupvarptransmit"),
                        (cycles.camera_update, "oncameraupdatetransmit"),
                    ] {
                        if stamp > last_redraw {
                            if let Some(r) = self.request(&c, hook) {
                                self.state.layout.hooks.push_back(r);
                            }
                        }
                    }
                    c.borrow_mut().f.lastRedrawCycle = cycles.redraw;
                    if c.borrow().hooks.contains_key("onkey") {
                        for &(key, keychar) in &self.input.keys {
                            if let Some(mut r) = self.request(&c, "onkey") {
                                r.key = key;
                                r.keychar = keychar;
                                self.state.layout.hooks.push_back(r);
                            }
                        }
                    }
                    if cycles.spline_finished {
                        if let Some(r) = self.request(&c, "oncamfinished") {
                            self.state.layout.hooks.push_back(r);
                        }
                    }
                }
                // Gaps: the skybox component update (environment owner) and
                // the object icon prewarm.
                if f.r#type == 0 {
                    let inner = [x.wrapping_sub(f.scrollx), y.wrapping_sub(f.scrolly)];
                    let runtime = c.borrow().has_runtime_parent();
                    if !runtime {
                        self.layer(array, f.parentlayer, bounds, inner)?;
                    }
                    let sorted = c.borrow().child_drawing_order();
                    if let Some(sorted) = sorted {
                        self.layer(&sorted, f.parentlayer, bounds, inner)?;
                    }
                    let sub = self.state.life.subs.get(f.parentlayer);
                    if let Some(sub) = sub.filter(|_| !runtime) {
                        let id = sub.borrow().id;
                        self.interface(Some(&sub), id, bounds, inner)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// The top-level interface update (world-map follow-up excluded).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the top-level interface update; exercised by tests only"
    )
)]
pub fn update_top_level(
    store: &mut Store,
    state: &mut State,
    frame: &mut Frame,
    input: &Input,
    transmits: &Transmits,
) -> Result<()> {
    update_top_level_with(store, state, frame, input, transmits, None)
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the top-level interface update; exercised by tests only"
    )
)]
pub fn update_top_level_with(
    store: &mut Store,
    state: &mut State,
    frame: &mut Frame,
    input: &Input,
    transmits: &Transmits,
    executor: Option<&mut dyn Executor>,
) -> Result<()> {
    update_top_level_with_world_map(store, state, frame, input, transmits, executor, None)
}

pub fn update_top_level_with_world_map(
    store: &mut Store,
    state: &mut State,
    frame: &mut Frame,
    input: &Input,
    transmits: &Transmits,
    executor: Option<&mut dyn Executor>,
    world_map: Option<&mut WorldMapInteraction>,
) -> Result<()> {
    if state.life.top == -1 {
        return Ok(());
    }
    let mut input = input.clone();
    if let Some(at) = input.click {
        // The queued event's position replaces the sampled cursor.
        input.mouse = at;
    }
    if state.interaction.drag.component.is_some()
        && state.interaction.drag.layer.is_some()
        && Rc::ptr_eq(
            state.interaction.drag.layer.as_ref().unwrap(),
            state
                .interaction
                .drag
                .default_layer
                .as_ref()
                .unwrap_or(state.interaction.drag.layer.as_ref().unwrap()),
        )
    {
        state.interaction.drag.parent_ready = true;
        state.interaction.drag.bounds = [0, 0, state.layout.canvas[0], state.layout.canvas[1]];
    }
    let [w, h] = state.layout.canvas;
    let top = state.life.top;
    // Preserve the active target cursor.
    if !state.interaction.target.active {
        state.minimenu.default_cursor = -1;
    }
    let mut walk = Walk {
        store,
        state,
        frame,
        input: &input,
        transmits,
        transient: false,
        executor,
        world_map,
    };
    walk.interface(None, top, [0, 0, w, h], [0, 0])
}

/// The inverse rotation from a component-local
/// cursor position to a scene tile. The returned tile is scene-local, which
/// is the coordinate convention used by the existing movement packet builder.
fn minimap_tile(input: &MinimapInput, component: [i32; 4], mouse: [i32; 2]) -> [i32; 2] {
    let mut x = mouse[0].wrapping_sub(component[0]) - component[2] / 2;
    let mut z = mouse[1].wrapping_sub(component[1]) - component[3] / 2;
    let yaw = match input.camera_state {
        4 => input.orbit_yaw,
        3 => input.camera_yaw,
        _ => input.anticheat_angle.wrapping_add(input.orbit_yaw),
    } & 0x3fff;
    let mut sin = crate::trig::sin(yaw);
    let mut cos = crate::trig::cos(yaw);
    if input.camera_state != 4 {
        sin = (input.zoom.wrapping_add(256) * sin) >> 8;
        cos = (input.zoom.wrapping_add(256) * cos) >> 8;
    }
    let rotated_x = (x * cos + z * sin) >> 14;
    let rotated_z = (z * cos - x * sin) >> 14;
    x = ((input.player_fine[0] - (input.player_size - 1) * 256) >> 9).wrapping_add(rotated_x >> 2);
    z = ((input.player_fine[1] - (input.player_size - 1) * 256) >> 9).wrapping_sub(rotated_z >> 2);
    [x, z]
}

/// The state an active-entity subinterface walk runs over, with the scene
/// viewport's clip and origin.
pub struct ActiveWalk<'a> {
    pub store: &'a mut Store,
    pub state: &'a mut State,
    pub frame: &'a mut Frame,
    pub input: &'a Input,
    pub transmits: &'a Transmits,
    pub clip: [i32; 4],
    pub origin: [i32; 2],
}

/// Walks an active player/NPC subinterface with the scene viewport's clip
/// and origin. The runtime has already validated the binding and resolved
/// its interface; this entry point reuses the ordinary component/hook walk
/// with the same executor and game domains instead of treating the
/// subinterface as a render-only attachment.
pub fn process_active_interface(
    active: ActiveWalk<'_>,
    sub: &crate::ui_lifecycle::SubRef,
    executor: &mut dyn Executor,
) -> Result<()> {
    let id = sub.borrow().id;
    let mut walk = Walk {
        store: active.store,
        state: active.state,
        frame: active.frame,
        input: active.input,
        transmits: active.transmits,
        transient: false,
        executor: Some(executor),
        world_map: None,
    };
    walk.interface(Some(sub), id, active.clip, active.origin)
}

/// The hook queues of the game update: timer queue, then mouse-stop, then the main queue,
/// each entry skipped when its component is no longer attached.
pub fn drain(store: &mut Store, state: &mut State, e: &mut impl Executor) -> Result<()> {
    for which in 0..2 {
        loop {
            let next = if which == 0 {
                state.layout.hooks_timer.pop_front()
            } else {
                state.layout.hooks_mouse_stop.pop_front()
            };
            let Some(r) = next else { break };
            let c = r
                .component
                .as_ref()
                .context("queued hook has no component")?;
            if !crate::ui_hooks::attached(store, c)? {
                continue;
            }
            e.run(store, state, r, INTERACTIVE_LIMIT)?;
        }
    }
    crate::ui_hooks::drain_main(store, state, e)
}
