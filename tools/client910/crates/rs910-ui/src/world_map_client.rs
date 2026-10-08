//! The client-side world map: the loading steps, the main and overview
//! draws, element drawing and hit boxes, the per-cycle update (zoom and jump
//! animation, flashes, element hover and menu options), the map component's
//! input and the script-facing state.
//!
//! The area renderer state lives in [`crate::world_map::WorldMap`].
use crate::{
    minimap::{MapElement, MapElementStore},
    world_map::{self, Canvas, Element, StaticElements, View, WorldMap},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

/// The `worldmap_disabletype` slots.
pub const DISABLE_ALL: i32 = 0;
pub const DISABLE_LOCS: i32 = 1; // not a content id
pub const DISABLE_ICONS: i32 = 2;
/// The jump animation's divisor cap.
const JUMP_STEPS: i32 = 8;
/// The default flash loops and tics.
pub const FLASH_LOOPS: i32 = 3;
pub const FLASH_TICS: i32 = 50;

/// The flash state of one element or category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flash {
    /// Loops left.
    pub loops: i32,
    /// Tics left in this loop.
    pub ticks: i32,
}

/// The drawn hit boxes of one element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Container {
    /// Index of the element in the area's element list.
    pub element: usize,
    /// The sprite box `[x0, x1, y0, y1]`.
    pub sprite: [i32; 4],
    /// The label box `[x0, x1, y0, y1]`.
    pub text: [i32; 4],
}
impl Container {
    /// Whether `(x, y)` is inside the sprite or label box (inclusive edges).
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let [x0, x1, y0, y1] = self.sprite;
        let [tx0, tx1, ty0, ty1] = self.text;
        x >= x0 && x <= x1 && y >= y0 && y <= y1 || x >= tx0 && x <= tx1 && y >= ty0 && y <= ty1
    }
}

/// A menu option the per-cycle update adds for a hovered element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementOption {
    pub op: String,
    pub target: Option<String>,
    pub action: i32,
    pub element: i32,
    pub category: i32,
}

/// A request to run a map element's triggered script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementTrigger {
    /// The trigger type id: 15 mouseover, 16 mouseleave, 17 mouserepeat.
    pub trigger: i32,
    pub element: i32,
    pub category: i32,
}

/// What the per-cycle update asks its caller to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateEvents {
    pub options: Vec<ElementOption>,
    pub triggers: Vec<ElementTrigger>,
}

/// What the `WORLD_MAP` component input asks its caller to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputEvents {
    /// The clicked source tile packed as `level << 28 | x << 14 | z`, for the
    /// cycle-end `CLICKWORLDMAP` event.
    pub click: Option<i32>,
    /// Show the menu at the press point on release.
    pub show_menu: Option<[i32; 2]>,
    /// The press became a drag.
    pub dragged: bool,
}

/// The random choices of one area load: the view-centre offset around the
/// player and the colour jitter steps. Tests inject fixed values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadRoll {
    pub centre: [i32; 2],
    pub hue: i32,
    pub lightness: i32,
}
impl LoadRoll {
    /// Centre offset of -5..=4 tiles, hue step of -2..=2, lightness step of
    /// -2..=2.
    pub fn random() -> Self {
        let pick = |range: f64, low: i32| (crate::ui_cam2::random_unit() * range) as i32 - low;
        Self {
            centre: [pick(10.0, 5), pick(10.0, 5)],
            hue: pick(5.0, 2),
            lightness: pick(5.0, 2),
        }
    }
}

/// The client-side world-map state.
pub struct ClientWorldMap {
    pub map: WorldMap,
    /// Loading progress (0..100).
    pub loading: i32,
    /// The view centre relative to the origin.
    pub position: [i32; 2],
    /// The animated jump target (-1 none).
    pub jump: [i32; 2],
    /// The visible tile window `[x, z, width, height]` of the last main draw.
    pub window: [i32; 4],
    /// The source coordinate and override flag requested by the last map
    /// switch.
    pub requested: [i32; 2],
    pub override_player: bool,
    /// The mouse is over the map component this cycle.
    pub mouse_over: bool,
    /// The flashing elements and categories, the loops and tics a new flash
    /// starts with, and whether flashes repeat forever.
    pub flash_elements: BTreeMap<i32, Flash>,
    pub flash_categories: BTreeMap<i32, Flash>,
    pub flash_loops: i32,
    pub flash_tics: i32,
    pub perpetual_flash: bool,
    /// The drawn hit boxes (`None` until loading step 10).
    pub containers: Option<Vec<Container>>,
    /// Whether element drawing is disabled, and the disabled element ids and
    /// categories.
    pub disable_elements: bool,
    pub disabled_elements: BTreeSet<i32>,
    pub disabled_categories: BTreeSet<i32>,
    /// Hide labels of text size 0/1/2.
    pub hide_text: [bool; 3],
    /// The `worldmap_disabletype` flags (all, locs, icons).
    pub disable_types: [bool; 3],
    /// The `worldmap_listelement_*` cursor.
    list_cursor: usize,
    /// The map last chosen for the player.
    player_map: Option<usize>,
    /// The overview sprite's size once drawn.
    pub overview_drawn: Option<[i32; 2]>,
    /// Label fonts loaded (`loading >= 70`).
    fonts_ready: [[bool; 5]; 3],
    /// The click state, press point, drag base and drag flag of the map
    /// component.
    pub click_state: i32,
    pub press: [i32; 2],
    pub drag_base: [i32; 2],
    pub dragged: bool,
    /// The map element types.
    pub element_types: MapElementStore,
    /// The random choices the next area load uses (`None`: roll them).
    pub roll: Option<LoadRoll>,
    /// Whether the player is logged in as a member.
    pub members: bool,
    /// The local player's varps and the varbit definitions, for
    /// `variableTest`/`getMultiME`/`getMultiLoc`.
    pub varps: Vec<i32>,
    pub varbits: Option<std::sync::Arc<crate::scenery_varbits::Inputs>>,
}

impl Default for ClientWorldMap {
    fn default() -> Self {
        Self {
            map: WorldMap::default(),
            loading: 0,
            position: [0, 0],
            jump: [-1, -1],
            window: [0; 4],
            requested: [-1, -1],
            override_player: false,
            mouse_over: false,
            flash_elements: BTreeMap::new(),
            flash_categories: BTreeMap::new(),
            flash_loops: FLASH_LOOPS,
            flash_tics: FLASH_TICS,
            perpetual_flash: false,
            containers: None,
            disable_elements: false,
            disabled_elements: BTreeSet::new(),
            disabled_categories: BTreeSet::new(),
            hide_text: [false; 3],
            disable_types: [false; 3],
            list_cursor: 0,
            player_map: None,
            overview_drawn: None,
            fonts_ready: [[false; 5]; 3],
            click_state: 0,
            press: [0, 0],
            drag_base: [0, 0],
            dragged: false,
            element_types: MapElementStore::default(),
            roll: None,
            members: false,
            varps: Vec::new(),
            varbits: None,
        }
    }
}

/// The zoom table: the config zoom percentage to the zoom factor.
fn zoom_for(config: i32) -> Option<f32> {
    Some(match config {
        25 => 2.0,
        37 => 3.0,
        50 => 4.0,
        75 => 6.0,
        100 => 8.0,
        200 => 16.0,
        _ => return None,
    })
}

impl ClientWorldMap {
    /// Loads the world-map config at the loading stage that owns it.
    pub fn install(&mut self, pack: &crate::cache::Pack) {
        self.map.install(pack);
        match MapElementStore::load(pack) {
            Ok(types) => self.element_types = types,
            Err(error) => log::warn!("[client910] world-map element types unavailable: {error:#}"),
        }
    }

    /// A varp (`is_varbit == false`) or varbit value of the local player.
    pub fn read_var(&self, is_varbit: bool, id: i32) -> i32 {
        read_var(&self.varps, self.varbits.as_deref(), is_varbit, id)
    }

    /// The resolved element type to draw, or `None` when it is hidden.
    pub fn visible_type(&self, id: i32) -> Option<MapElement> {
        let mut t = self.element_types.get(id)?.clone();
        let mut resolved = id;
        let read = |b: bool, v: i32| Ok(self.read_var(b, v));
        if !t.multime.is_empty() {
            resolved = t.multi(&read).ok()??;
            t = self.element_types.get(resolved)?.clone();
        }
        if !t.drawn_on_world_map || !t.variable_test(&read).unwrap_or(false) {
            return None;
        }
        // The disabled-elements check uses the resolved type's own id.
        if self.disabled_elements.contains(&resolved)
            || self.disabled_categories.contains(&t.category)
        {
            return None;
        }
        if t.text.is_some() {
            let size = t.text_size;
            if (0..3).contains(&size) && self.hide_text[size as usize] {
                return None;
            }
        }
        Some(t)
    }

    /// Clamps the view centre into the area (resetting any jump that hit an
    /// edge).
    pub fn clamp(&mut self) {
        let [w, h] = self.map.area.as_ref().map_or([0, 0], |a| a.size);
        if self.position[0] < 0 {
            self.position[0] = 0;
            self.jump = [-1, -1];
        }
        if self.position[0] > w {
            self.position[0] = w;
            self.jump = [-1, -1];
        }
        if self.position[1] < 0 {
            self.position[1] = 0;
            self.jump = [-1, -1];
        }
        if self.position[1] > h {
            self.position[1] = h;
            self.jump = [-1, -1];
        }
    }

    /// Resets the loading state and drops the loaded area.
    pub fn reset(&mut self) {
        self.map.scenes.reset();
        self.containers = None;
        self.loading = 0;
        self.map.decoded = false;
        self.map.release();
        self.map.static_elements = None;
        self.map.main_cache.reset();
        self.map.overview_cache.reset();
        self.overview_drawn = None;
        self.jump = [-1, -1];
    }

    /// Resets everything on logout.
    pub fn logout(&mut self) {
        self.reset();
        self.fonts_ready = [[false; 5]; 3];
        self.map.current = None;
        self.player_map = None;
        self.disabled_elements.clear();
        self.disabled_categories.clear();
    }

    /// Reloads the same map; a toolkit change calls it.
    pub fn reload(&mut self) {
        let id = self.map.metadata().map_or(-1, |m| m.id as i32);
        self.reset();
        self.fonts_ready = [[false; 5]; 3];
        self.map.current = None;
        if id != -1 {
            self.set_map(id, -1, -1, false);
        }
    }

    /// Picks the map for the player's absolute tile.
    pub fn follow_player(&mut self, player: [i32; 2], force: bool) {
        let chosen = self
            .map
            .map_at(player[0], player[1])
            .or_else(|| self.map.by_id(self.map.defaults.default_map));
        if self.player_map == chosen && !force {
            return;
        }
        self.player_map = chosen;
        if self.map.select_area(chosen) {
            self.map.incremental = true;
            self.reset();
        }
    }

    /// Closes the map, returning to the player's own map.
    pub fn close_map(&mut self, player: [i32; 2]) {
        self.follow_player(player, true);
    }

    /// Switches to map `id`, optionally centred on source coordinate `(x, z)`.
    pub fn set_map(&mut self, id: i32, x: i32, z: i32, override_player: bool) {
        let before = self.map.current;
        self.map.select_id(id);
        self.map.incremental = false;
        if self.map.current != before {
            self.reset();
        }
        self.requested = [x, z];
        self.override_player = override_player;
    }

    /// One loading step. `player` is `[level, x, z]` (absolute tiles);
    /// `font_ready` reports whether a label font of the defaults table is
    /// loaded.
    pub fn update_loading(&mut self, player: Option<[i32; 3]>, font_ready: &dyn Fn(i32) -> bool) {
        if self.loading == 100 {
            return;
        }
        let Some(meta) = self.map.metadata().cloned() else {
            return;
        };
        if self.loading < 10 {
            // The cache is local: `isGroupReady` holds at once.
            self.loading = 10;
        }
        if self.loading == 10 {
            let origin = [
                meta.config_bounds[0] >> 6 << 6,
                meta.config_bounds[2] >> 6 << 6,
            ];
            let size = [
                (meta.config_bounds[1] >> 6 << 6) - origin[0] + 64,
                (meta.config_bounds[3] >> 6 << 6) - origin[1] + 64,
            ];
            let mut player_tile = [-1, -1];
            if let Some([level, x, z]) = player {
                if let Some([dx, dz]) = world_map::source_to_display(&meta, level, x, z) {
                    player_tile = [dx - origin[0], dz - origin[1]];
                }
            }
            let [px, pz] = player_tile;
            let roll = self.roll.take().unwrap_or_else(LoadRoll::random);
            if !self.override_player && px >= 0 && px < size[0] && pz >= 0 && pz < size[1] {
                self.position = [px + roll.centre[0], pz + roll.centre[1]];
            } else if self.requested[0] == -1 || self.requested[1] == -1 {
                let o = meta.config_origin;
                if let Some([x, z]) =
                    world_map::source_to_display_any_level(&meta, o >> 14 & 0x3FFF, o & 0x3FFF)
                {
                    self.position = [x - origin[0], z - origin[1]];
                }
            } else {
                if let Some([x, z]) = world_map::source_to_display_any_level(
                    &meta,
                    self.requested[0],
                    self.requested[1],
                ) {
                    self.position = [x - origin[0], z - origin[1]];
                }
                self.requested = [-1, -1];
                self.override_player = false;
            }
            let zoom = zoom_for(meta.config_zoom).unwrap_or(8.0);
            self.map.zoom = zoom;
            self.map.target_zoom = zoom;
            self.map.shape_size = self.map.target_zoom as i32 >> 1;
            self.map.shapes = world_map::tile_shapes(self.map.shape_size);
            self.map.allocate(origin, size);
            self.map.scenes.reset();
            self.clamp();
            self.containers = Some(Vec::new());
            self.map.hue_jitter = (self.map.hue_jitter + roll.hue).clamp(-8, 8);
            self.map.lightness_jitter = (self.map.lightness_jitter + roll.lightness).clamp(-16, 16);
            self.map.build_overlay_colours(
                self.map.hue_jitter >> 2 << 10,
                self.map.lightness_jitter >> 1,
            );
            self.loading = 20;
        } else if self.loading == 20 {
            let varps = self.varps.clone();
            let varbits = self.varbits.clone();
            let read = move |b: bool, v: i32| read_var(&varps, varbits.as_deref(), b, v);
            let incremental = self.map.incremental;
            match self.map.decode(incremental, &read) {
                Ok(true) => self.loading = 60,
                Ok(false) if !incremental => self.loading = 60,
                Ok(false) => {}
                Err(error) => {
                    log::warn!(
                        "[client910] world-map area {:?} undecodable: {error:#}",
                        meta.name
                    );
                    self.loading = 60;
                }
            }
        } else if self.loading == 60 {
            let pack = self.map.pack.clone();
            self.map.static_elements = Some(
                pack.and_then(|pack| {
                    StaticElements::load(&pack, &meta.name, self.members)
                        .map_err(|error| {
                            log::warn!(
                                "[client910] world-map elements of {:?}: {error:#}",
                                meta.name
                            )
                        })
                        .ok()
                })
                .unwrap_or_default(),
            );
            self.map.list_static_elements();
            self.loading = 70;
        } else if self.loading >= 70 {
            // Each font that loads adds 3; a missing one waits.
            for (row, fonts) in self.map.defaults.fonts.iter().enumerate() {
                for (column, &font) in fonts.iter().enumerate() {
                    if !self.fonts_ready[row][column] {
                        if !font_ready(font) {
                            return;
                        }
                        self.fonts_ready[row][column] = true;
                        self.loading += 3;
                    }
                }
            }
            self.loading = 100;
        }
    }

    /// Sets the target zoom from a config percentage. The jump target's z is
    /// cleared even when the zoom is unknown.
    pub fn set_zoom(&mut self, zoom: i32) {
        if let Some(z) = zoom_for(zoom) {
            self.map.target_zoom = z;
        }
        self.jump[1] = -1;
    }
    /// The target zoom as a config percentage.
    pub fn get_zoom(&self) -> i32 {
        match self.map.target_zoom {
            2.0 => 25,
            3.0 => 37,
            4.0 => 50,
            6.0 => 75,
            8.0 => 100,
            _ => 200,
        }
    }
    fn origin(&self) -> [i32; 2] {
        self.map.area.as_ref().map_or([0, 0], |a| a.origin)
    }
    /// The `worldmap_getdisplayposition` value.
    pub fn display_position(&self) -> [i32; 2] {
        let o = self.origin();
        [self.position[0] + o[0], self.position[1] + o[1]]
    }
    /// Animates the view centre to display tile `(x, z)`.
    pub fn jump_to(&mut self, x: i32, z: i32) {
        let o = self.origin();
        self.jump = [x - o[0], z - o[1]];
    }
    /// Moves the view centre to display tile `(x, z)` at once.
    pub fn jump_to_instant(&mut self, x: i32, z: i32) {
        let o = self.origin();
        self.position = [x - o[0], z - o[1]];
        self.jump = [-1, -1];
        self.clamp();
    }
    /// Sets one axis of the view centre.
    pub fn set_position_x(&mut self, x: i32) {
        self.position[0] = x;
        self.jump = [-1, -1];
        self.clamp();
    }
    pub fn set_position_z(&mut self, z: i32) {
        self.position[1] = z;
        // The jump target's z is cleared and its x is left as is.
        self.jump[1] = -1;
        self.clamp();
    }
    /// Starts a flash on an element / a category.
    pub fn flash_element(&mut self, id: i32) {
        self.flash_elements.insert(
            id,
            Flash {
                loops: self.flash_loops,
                ticks: self.flash_tics,
            },
        );
    }
    pub fn flash_category(&mut self, category: i32) {
        self.flash_categories.insert(
            category,
            Flash {
                loops: self.flash_loops,
                ticks: self.flash_tics,
            },
        );
    }
    /// Sets the loops / tics new flashes start with (below 1 restores the
    /// default).
    pub fn set_flash_loops(&mut self, loops: i32) {
        self.flash_loops = if loops < 1 { FLASH_LOOPS } else { loops };
    }
    pub fn set_flash_tics(&mut self, tics: i32) {
        self.flash_tics = if tics < 1 { FLASH_TICS } else { tics };
    }
    fn flash_of(&self, id: i32, category: i32) -> Option<Flash> {
        self.flash_elements
            .get(&id)
            .or_else(|| self.flash_categories.get(&category))
            .copied()
    }
    /// The pulse alpha of a flash.
    fn flash_alpha(&self, flash: Flash) -> i32 {
        let t = self.flash_tics;
        if flash.ticks > t / 2 {
            (t * 255 - flash.ticks * 255) / t
        } else {
            flash.ticks * 255 / t
        }
    }
    /// Sets / gets a `worldmap_disabletype` flag (false / -1 for an unknown
    /// kind).
    pub fn set_disable_type(&mut self, kind: i32, value: bool) -> bool {
        match kind {
            DISABLE_ALL | DISABLE_LOCS | DISABLE_ICONS => {
                self.disable_types[kind as usize] = value;
                true
            }
            _ => false,
        }
    }
    pub fn get_disable_type(&self, kind: i32) -> i32 {
        match kind {
            DISABLE_ALL | DISABLE_LOCS | DISABLE_ICONS => {
                i32::from(self.disable_types[kind as usize])
            }
            _ => -1,
        }
    }
    /// The packed tile of the nearest element `id` to display tile `(x, z)`
    /// (-2 when none or still loading).
    pub fn nearest_element(&self, id: i32, x: i32, z: i32) -> i32 {
        if self.loading < 100 {
            return -2;
        }
        let Some(area) = self.map.area.as_ref() else {
            return -2;
        };
        let (tx, tz) = (x - area.origin[0], z - area.origin[1]);
        let mut best = -2;
        let mut distance = i32::MAX;
        for e in &area.elements {
            if e.id != id {
                continue;
            }
            let packed = (area.origin[0] + e.x) << 14 | (area.origin[1] + e.z);
            let d = (tx - e.x)
                .wrapping_mul(tx - e.x)
                .wrapping_add((tz - e.z).wrapping_mul(tz - e.z));
            if best < 0 || d < distance {
                best = packed;
                distance = d;
            }
        }
        best
    }
    fn listed(&self, e: &Element) -> bool {
        let read = |b: bool, v: i32| Ok(self.read_var(b, v));
        self.element_types
            .get(e.id)
            .is_some_and(|t| t.show_on_world_map && t.variable_test(&read).unwrap_or(false))
    }
    /// The next listed element as `(id, level << 28 | x << 14 | z)`;
    /// `start` rewinds the cursor.
    pub fn list_element(&mut self, start: bool) -> Option<(i32, i32)> {
        let area = self.map.area.as_ref()?;
        if start {
            self.list_cursor = 0;
        }
        while self.list_cursor < area.elements.len() {
            let e = &area.elements[self.list_cursor];
            self.list_cursor += 1;
            if self.listed(e) {
                let packed = e.level << 28 | (e.x + area.origin[0]) << 14 | (e.z + area.origin[1]);
                return Some((e.id, packed));
            }
        }
        None
    }

    /// Draws the main map into `[x, y, w, h]`.
    pub fn draw(&mut self, canvas: &mut dyn Canvas, [x, y, w, h]: [i32; 4]) {
        if self.loading < 100 {
            let bar = 20;
            let cx = w / 2 + x;
            let cy = h / 2 + y - 18 - bar;
            // The loading bar colours at the default index 0.
            canvas.fill([x, y, w, h], -16_777_216);
            canvas.outline([cx - 152, cy, 304, 34], 9_179_409 | 0xFF00_0000u32 as i32);
            canvas.fill(
                [cx - 150, cy + 2, self.loading * 3, 30],
                9_179_409 | 0xFF00_0000u32 as i32,
            );
            canvas.text_centre(
                rs910_core::texts::Msg::LoadingEllipsis.get(),
                [cx, bar + cy],
                16_777_215 | 0xFF00_0000u32 as i32,
            );
            return;
        }
        let zoom = self.map.zoom;
        let left = self.position[0] - (w as f32 / zoom) as i32;
        let top = self.position[1] + (h as f32 / zoom) as i32;
        let right = self.position[0] + (w as f32 / zoom) as i32;
        let bottom = self.position[1] - (h as f32 / zoom) as i32;
        self.window = [
            self.position[0] - (w as f32 / zoom) as i32,
            self.position[1] - (h as f32 / zoom) as i32,
            ((w * 2) as f32 / zoom) as i32,
            ((h * 2) as f32 / zoom) as i32,
        ];
        let o = self.origin();
        self.map.overview_active = false;
        self.map.set_view(View {
            left: o[0] + left,
            top: o[1] + top,
            right: o[0] + right,
            bottom: o[1] + bottom,
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h + 1,
        });
        let locs = !self.disable_types[DISABLE_LOCS as usize];
        let icons = !self.disable_types[DISABLE_ICONS as usize];
        self.map
            .draw_chunks(canvas, locs, icons, self.members, false);
        self.map.project_elements();
        self.draw_elements(canvas);
        // The FPS/memory lines are drawn by the caller, which owns the debug
        // stats.
        self.map.main_cache.clean(world_map::CHUNK_IDLE_CLEANS);
    }

    /// Draws every visible element and records its hit boxes.
    fn draw_elements(&mut self, canvas: &mut dyn Canvas) {
        let Some(mut containers) = self.containers.take() else {
            return;
        };
        containers.clear();
        if !self.disable_elements {
            let count = self.map.area.as_ref().map_or(0, |a| a.elements.len());
            for index in 0..count {
                let id = self.map.area.as_ref().unwrap().elements[index].id;
                let Some(t) = self.visible_type(id) else {
                    continue;
                };
                if self.draw_element(canvas, index, &t, &mut containers) {
                    // The arrow reads the unresolved type.
                    if let Some(original) = self.element_types.get(id).cloned() {
                        self.draw_arrow(canvas, index, &original, &mut containers);
                    }
                }
            }
        }
        self.containers = Some(containers);
    }

    /// The label font for this zoom.
    fn label_font(&self, text_size: i32) -> Option<i32> {
        if !(0..3).contains(&text_size) {
            return None;
        }
        let z = f64::from(self.map.target_zoom);
        let column = if z == 2.0 {
            0
        } else if z == 3.0 {
            1
        } else if z == 4.0 {
            2
        } else if z == 6.0 {
            3
        } else if z >= 8.0 {
            4
        } else {
            return None;
        };
        Some(self.map.defaults.fonts[text_size as usize][column])
    }

    fn sprite(&mut self, id: i32) -> Option<Rc<crate::ui_sprites::Sprite>> {
        let pack = self.map.pack.clone()?;
        self.element_types.sprite(&pack, id)
    }

    /// Draws one element; true when it lies outside
    /// the view (the caller then draws its edge arrow).
    fn draw_element(
        &mut self,
        canvas: &mut dyn Canvas,
        index: usize,
        t: &MapElement,
        containers: &mut Vec<Container>,
    ) -> bool {
        let v = self.map.view;
        let area = self.map.area.as_ref().unwrap();
        let e = area.elements[index].clone();
        if self.map.members_map && !self.members && !area.member_block(e.x, e.z) {
            return false;
        }
        let [span_x, tiles_x, span_y, tiles_z] = v.span();
        let (tiles_x, tiles_z) = (
            if tiles_x == 0 { 1 } else { tiles_x },
            if tiles_z == 0 { 1 } else { tiles_z },
        );
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        if !t.polygon.is_empty() {
            min_x = (e.x + t.bounds[0] - v.left).wrapping_mul(span_x) / tiles_x + v.x0;
            max_x = (e.x + t.bounds[2] - v.left).wrapping_mul(span_x) / tiles_x + v.x0;
            max_y = v.y1 - (e.z + t.bounds[1] - v.bottom).wrapping_mul(span_y) / tiles_z;
            min_y = v.y1 - (e.z + t.bounds[3] - v.bottom).wrapping_mul(span_y) / tiles_z;
        }
        let [sx, sy] = e.screen;
        let mut sprite = None;
        let (mut box_x0, mut box_x1, mut box_y0, mut box_y1) = (0, 0, 0, 0);
        if t.sprite != -1 {
            sprite = if e.hovered && t.sprite2 != -1 {
                self.sprite(t.sprite2)
            } else {
                self.sprite(t.sprite)
            };
            if let Some(s) = sprite.as_ref() {
                let [gx, gy] = s.full_size();
                match t.align[0] {
                    0 => {
                        box_x0 = sx - gx;
                        box_x1 = sx;
                    }
                    1 => {
                        box_x0 = sx;
                        box_x1 = sx + gx;
                    }
                    2 => {
                        box_x0 = sx - ((gx + 1) >> 1);
                        box_x1 = sx + ((gx + 1) >> 1);
                    }
                    _ => {}
                }
                match t.align[1] {
                    0 => {
                        box_y0 = sy - gy;
                        box_y1 = sy;
                    }
                    1 => {
                        box_y0 = sy;
                        box_y1 = sy + gy;
                    }
                    2 => {
                        box_y0 = sy - ((gy + 1) >> 1);
                        box_y1 = sy + ((gy + 1) >> 1);
                    }
                    _ => {}
                }
                min_x = min_x.min(box_x0);
                max_x = max_x.max(box_x1);
                min_y = min_y.min(box_y0);
                max_y = max_y.max(box_y1);
            }
        }
        let flash_sprite = if t.flash_sprite != -1 {
            self.sprite(t.flash_sprite)
        } else {
            None
        };
        let (mut text_x, mut text_y, mut text_w, mut text_h) = (0, 0, 0, 0);
        let mut text_box = [0; 4];
        let font = t.text.as_ref().and_then(|_| self.label_font(t.text_size));
        let mut measured = None;
        if let (Some(text), Some(font)) = (t.text.as_ref(), font) {
            measured = canvas.measure(font, text);
            if let Some([height, width]) = measured {
                text_h = height;
                text_w = width;
                text_x = t.text_offset[0].wrapping_mul(span_x) / tiles_x + (sx - width / 2);
                let anchor = sy - t.text_offset[1].wrapping_mul(span_y) / tiles_z;
                text_y = match sprite.as_ref() {
                    None => anchor - height / 2,
                    Some(s) => anchor - ((s.full_size()[1] >> 1) + height),
                };
                text_box = [text_x, text_x + width, text_y, text_y + height];
                min_x = min_x.min(text_box[0]);
                max_x = max_x.max(text_box[1]);
                min_y = min_y.min(text_box[2]);
                max_y = max_y.max(text_box[3]);
            }
        }
        if max_x < v.x0 || min_x > v.x1 || max_y < v.y0 || min_y > v.y1 {
            return true;
        }
        self.draw_polygon(canvas, &e, t);
        if let Some(s) = sprite.as_ref() {
            let [gx, gy] = s.full_size();
            let width = s.size[0];
            let (mut rx, mut ry, mut dx, mut dy) = (0, 0, 0, 0);
            match t.align[0] {
                0 => {
                    rx = width;
                    dx = gx;
                }
                2 => {
                    rx = width / 2;
                    dx = gx >> 1;
                }
                _ => {}
            }
            match t.align[1] {
                0 => {
                    ry = width;
                    dy = gy;
                }
                2 => {
                    ry = width / 2;
                    dy = gy >> 1;
                }
                _ => {}
            }
            let flash = self.flash_of(e.id, t.category);
            if let Some(flash) = flash.filter(|_| t.flash_sprite == -1) {
                // The GPU renderer's flash box; the software renderer's
                // circles are `TODO(#gap-worldmap-sw-flash)`.
                let colour = self.flash_alpha(flash) << 24 | 0xFFFF00;
                for grow in [7, 5, 3, 1, 0] {
                    canvas.fill(
                        [
                            sx - rx - grow,
                            sy - ry - grow,
                            width + grow * 2,
                            width + grow * 2,
                        ],
                        colour,
                    );
                }
            }
            canvas.sprite(s, [sx - dx, sy - dy]);
            if let (Some(flash), Some(fs)) = (flash, flash_sprite.as_ref()) {
                let [fx, fy] = fs.full_size();
                let ox = match t.align[0] {
                    1 => fx,
                    2 => fx >> 1,
                    _ => 0,
                };
                let oy = match t.align[1] {
                    0 => (fy + gy) / 2,
                    2 => (fy / 2 + gy) / 2,
                    _ => 0,
                };
                let colour = self.flash_alpha(flash) << 24 | 0xFFFF00;
                canvas.sprite_tinted(fs, [sx - ox, sy - oy], colour);
            }
        }
        if let (Some(text), Some(font), Some(_)) = (t.text.as_ref(), font, measured) {
            // The label box, fill and outline.
            let bx = text_x - 5;
            let by = text_y + 2;
            if t.text_fill != 0 {
                canvas.fill([bx, by, text_w + 10, text_y + text_h - by + 1], t.text_fill);
            }
            if t.text_outline != 0 {
                canvas.outline(
                    [bx, by, text_w + 10, text_y + text_h - by + 1],
                    t.text_outline,
                );
            }
            let colour = if e.hovered && t.hover_text_colour != -1 {
                t.hover_text_colour
            } else {
                t.text_colour
            };
            canvas.text(
                font,
                text,
                [text_x, text_y, text_w, text_h],
                colour | 0xFF00_0000u32 as i32,
                self.map.defaults.text_shadow,
            );
        }
        if t.sprite != -1 || t.text.is_some() {
            containers.push(Container {
                element: index,
                sprite: [box_x0, box_x1, box_y0, box_y1],
                text: if measured.is_some() { text_box } else { [0; 4] },
            });
        }
        false
    }

    /// Draws an element's polygon and its outline.
    fn draw_polygon(&self, canvas: &mut dyn Canvas, e: &Element, t: &MapElement) {
        if t.polygon.is_empty() {
            return;
        }
        let v = self.map.view;
        let points: Vec<i32> = t
            .polygon
            .chunks_exact(2)
            .flat_map(|p| v.project_exact(e.x + p[0], e.z + p[1]))
            .collect();
        canvas.polygon(&points, t.polygon_fill);
        let n = points.len() / 2;
        let colour = |edge: usize| -> i32 {
            let index = t.line_index.get(edge).map_or(0, |i| usize::from(*i as u8));
            t.line_colours.get(index).copied().unwrap_or(0)
        };
        let style = t.line_style;
        for i in 0..n.saturating_sub(1) {
            canvas.line(
                [points[i * 2], points[i * 2 + 1]],
                [points[(i + 1) * 2], points[(i + 1) * 2 + 1]],
                colour(i),
                style,
            );
        }
        if n > 0 {
            canvas.line(
                [points[points.len() - 2], points[points.len() - 1]],
                [points[0], points[1]],
                colour(t.line_index.len().saturating_sub(1)),
                style,
            );
        }
    }

    /// Draws the edge arrow of an off-screen element.
    fn draw_arrow(
        &mut self,
        canvas: &mut dyn Canvas,
        index: usize,
        t: &MapElement,
        containers: &mut Vec<Container>,
    ) {
        if t.arrow_sprite == -1 {
            return;
        }
        let Some(arrow) = self.sprite(t.arrow_sprite) else {
            return;
        };
        let v = self.map.view;
        let e = self.map.area.as_ref().unwrap().elements[index].clone();
        let size = arrow.size[0].max(arrow.size[1]);
        let pad = 10;
        let [mut ax, mut ay] = e.screen;
        let (mut text_w, mut text_h) = (0, 0);
        if let Some(text) = t.text.as_ref() {
            // The metrics come from the font chosen for this element's
            // text size.
            if let Some(font) = self.label_font(t.text_size) {
                if let Some([h, w]) = canvas.measure(font, text) {
                    text_h = h;
                    text_w = w;
                }
            }
        }
        let mut label_x = size / 2 + e.screen[0];
        let mut label_y = e.screen[1];
        if ax < v.x0 + size {
            ax = v.x0;
            label_x = text_w / 2 + size / 2 + v.x0 + pad + 5;
        } else if ax > v.x1 - size {
            ax = v.x1 - size;
            label_x = v.x1 - size / 2 - pad - text_w / 2 - 5;
        }
        if ay < v.y0 + size {
            ay = v.y0;
            label_y = size / 2 + v.y0 + pad;
        } else if ay > v.y1 - size {
            ay = v.y1 - size;
            label_y = v.y1 - size / 2 - pad - text_h;
        }
        let angle = ((f64::from(ax - e.screen[0]).atan2(f64::from(ay - e.screen[1]))
            / std::f64::consts::PI
            * 32767.0) as i32)
            & 0xFFFF;
        canvas.rotated(
            &arrow,
            [size as f32 / 2.0 + ax as f32, size as f32 / 2.0 + ay as f32],
            angle,
        );
        let mut text_box = [-2; 4];
        if let Some(text) = t.text.as_ref() {
            let x0 = label_x - text_w / 2 - 5;
            let x1 = text_w + x0 + 10;
            let y1 = text_h + label_y + 3;
            text_box = [x0, x1, label_y, y1];
            if t.text_fill != 0 {
                canvas.fill([x0, label_y, x1 - x0, y1 - label_y], t.text_fill);
            }
            if t.text_outline != 0 {
                canvas.outline([x0, label_y, x1 - x0, y1 - label_y], t.text_outline);
            }
            if let Some(font) = self.label_font(t.text_size) {
                canvas.text(
                    font,
                    text,
                    [label_x, label_y, text_w, text_h],
                    t.text_colour | 0xFF00_0000u32 as i32,
                    self.map.defaults.text_shadow,
                );
            }
        }
        if t.sprite == -1 && t.text.is_none() {
            return;
        }
        containers.push(Container {
            element: index,
            sprite: [ax - size / 2, size / 2 + ax, ay - size, ay],
            text: text_box,
        });
    }

    /// Draws the overview into `[x, y, w, h]`: the main-cache window over
    /// the aspect-fitted whole-area render.
    pub fn draw_overview(&mut self, canvas: &mut dyn Canvas, [x, y, w, h]: [i32; 4]) {
        canvas.fill([x, y, w, h], -16_777_216);
        if self.loading < 100 {
            return;
        }
        let Some([aw, ah]) = self.map.area.as_ref().map(|a| a.size) else {
            return;
        };
        let aspect = ah as f32 / aw as f32;
        let (mut vw, mut vh) = (w, h);
        if aspect < 1.0 {
            vh = (w as f32 * aspect) as i32;
        } else {
            vw = (h as f32 / aspect) as i32;
        }
        let ox = (w - vw) / 2 + x;
        let oy = (h - vh) / 2 + y;
        // There is no render-target capture of the overview; the chunk
        // sprites stay in the overview cache and are redrawn instead.
        let o = self.origin();
        self.map.overview_active = true;
        self.map.set_view(View {
            left: o[0],
            top: ah + o[1],
            right: aw + o[0],
            bottom: o[1],
            x0: ox,
            y0: oy,
            x1: vw + ox,
            y1: vh + oy,
        });
        self.map
            .draw_chunks(canvas, false, false, self.members, true);
        self.map.overview_active = false;
        self.overview_drawn = Some([vw, vh]);
        let [wx, wz, ww, wh] = self.window;
        let box_w = ww * vw / aw;
        let box_h = wh * vh / ah;
        let box_x = wx * vw / aw + ox;
        let box_y = vh + oy - wz * vh / ah - box_h;
        let colour = -1_996_554_240;
        canvas.fill([box_x, box_y, box_w, box_h], colour);
        canvas.outline([box_x, box_y, box_w, box_h], colour);
        let Some(area) = self.map.area.as_ref() else {
            return;
        };
        let elements: Vec<Element> = area.elements.clone();
        for e in elements {
            let Some(t) = self.visible_type(e.id) else {
                continue;
            };
            let Some(flash) = self.flash_of(e.id, t.category) else {
                continue;
            };
            let alpha = self.flash_alpha(flash);
            let px = e.x * vw / aw + ox;
            let py = (ah - e.z) * vh / ah + oy;
            canvas.fill([px - 2, py - 2, 4, 4], alpha << 24 | 0xFFFF00);
        }
        self.map.overview_cache.clean(world_map::CHUNK_IDLE_CLEANS);
    }

    /// The per-cycle update: zoom and jump animation, flash countdown and
    /// the hovered element's options and triggers.
    pub fn update(&mut self, mouse: [i32; 2]) -> UpdateEvents {
        let mut events = UpdateEvents::default();
        let m = &mut self.map;
        if m.zoom < m.target_zoom {
            m.zoom = (f64::from(m.zoom) / 30.0 + f64::from(m.zoom)) as f32;
            if m.zoom > m.target_zoom {
                m.zoom = m.target_zoom;
            }
            self.clamp();
            self.map.shape_size = self.map.target_zoom as i32 >> 1;
            self.map.shapes = world_map::tile_shapes(self.map.shape_size);
        } else if m.zoom > m.target_zoom {
            m.zoom = (f64::from(m.zoom) - f64::from(m.zoom) / 30.0) as f32;
            if m.zoom < m.target_zoom {
                m.zoom = m.target_zoom;
            }
            self.clamp();
            self.map.shape_size = self.map.target_zoom as i32 >> 1;
            self.map.shapes = world_map::tile_shapes(self.map.shape_size);
        }
        if self.jump[0] != -1 && self.jump[1] != -1 {
            let mut dx = self.jump[0] - self.position[0];
            if dx != 0 {
                dx /= JUMP_STEPS.min(dx.abs());
            }
            let mut dz = self.jump[1] - self.position[1];
            if dz != 0 {
                dz /= JUMP_STEPS.min(dz.abs());
            }
            self.position[0] += dx;
            self.position[1] += dz;
            if dx == 0 && dz == 0 {
                self.jump = [-1, -1];
            }
            self.clamp();
        }
        let tics = self.flash_tics;
        let perpetual = self.perpetual_flash;
        for flashes in [&mut self.flash_elements, &mut self.flash_categories] {
            flashes.retain(|_, f| {
                f.ticks -= 1;
                if f.ticks != 0 {
                    return true;
                }
                if f.loops > 1 || perpetual {
                    f.loops -= 1;
                    f.ticks = tics;
                    true
                } else {
                    false
                }
            });
        }
        if !self.mouse_over {
            return events;
        }
        let Some(containers) = self.containers.clone() else {
            return events;
        };
        for c in containers {
            let Some(area) = self.map.area.as_ref() else {
                break;
            };
            let Some(e) = area.elements.get(c.element) else {
                continue;
            };
            let (id, hovered) = (e.id, e.hovered);
            let Some(t) = self.element_types.get(id).cloned() else {
                continue;
            };
            if c.contains(mouse[0], mouse[1]) {
                for slot in (0..5).rev() {
                    if let Some(Some(op)) = t.ops.get(slot) {
                        events.options.push(ElementOption {
                            op: op.clone(),
                            target: t.target.clone(),
                            action: 1008 + slot as i32,
                            element: id,
                            category: t.category,
                        });
                    }
                }
                if !hovered {
                    self.set_hovered(c.element, true);
                    events.triggers.push(ElementTrigger {
                        trigger: 15,
                        element: id,
                        category: t.category,
                    });
                }
                events.triggers.push(ElementTrigger {
                    trigger: 17,
                    element: id,
                    category: t.category,
                });
            } else if hovered {
                self.set_hovered(c.element, false);
                events.triggers.push(ElementTrigger {
                    trigger: 16,
                    element: id,
                    category: t.category,
                });
            }
        }
        events
    }
    fn set_hovered(&mut self, index: usize, value: bool) {
        if let Some(e) = self
            .map
            .area
            .as_mut()
            .and_then(|a| a.elements.get_mut(index))
        {
            e.hovered = value;
        }
    }

    /// The world-map component input. `rect` is the component
    /// `[x, y, w, h]`; `over` is whether the mouse is over it, `held` whether
    /// the button is held and `click` the queued press inside the component.
    pub fn component_input(
        &mut self,
        rect: [i32; 4],
        mouse: [i32; 2],
        over: bool,
        held: bool,
        click: Option<[i32; 2]>,
        menu_should_open: bool,
    ) -> InputEvents {
        let mut out = InputEvents::default();
        if over {
            self.mouse_over = true;
        }
        let [x, y, w, h] = rect;
        if let Some(at) = click {
            let zoom = f64::from(self.map.zoom);
            let dx = (f64::from(at[0] - x - w / 2) * 2.0 / zoom) as i32;
            let dz = -(f64::from(at[1] - y - h / 2) * 2.0 / zoom) as i32;
            let o = self.origin();
            let tx = self.position[0] + dx + o[0];
            let tz = self.position[1] + dz + o[1];
            let Some(meta) = self.map.metadata() else {
                return out;
            };
            // A press outside every subarea still clicks, at the origin: the
            // original reads a zeroed coordinate whatever the lookup answered.
            let [level, sx, sz] = world_map::display_to_source(meta, tx, tz).unwrap_or([0; 3]);
            out.click = Some(level << 28 | sx << 14 | sz);
            self.click_state = 1;
            self.dragged = false;
            self.press = mouse;
            return out;
        }
        if held && self.click_state > 0 {
            if self.click_state == 1 && self.press != mouse {
                self.drag_base = self.position;
                self.click_state = 2;
            }
            if self.click_state == 2 {
                self.dragged = true;
                let zoom = f64::from(self.map.target_zoom);
                self.set_position_x(
                    self.drag_base[0] + (f64::from(self.press[0] - mouse[0]) * 2.0 / zoom) as i32,
                );
                self.set_position_z(
                    self.drag_base[1] - (f64::from(self.press[1] - mouse[1]) * 2.0 / zoom) as i32,
                );
            }
            out.dragged = self.dragged;
            return out;
        }
        if self.click_state > 0 && !self.dragged && menu_should_open {
            out.show_menu = Some(self.press);
        }
        out.dragged = self.dragged;
        self.click_state = 0;
        out
    }

    /// Centres the view on an overview click.
    pub fn overview_click(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let Some([aw, ah]) = self.map.area.as_ref().map(|a| a.size) else {
            return;
        };
        let aspect = ah as f32 / aw as f32;
        let (mut vw, mut vh) = (w, h);
        if aspect < 1.0 {
            vh = (w as f32 * aspect) as i32;
        } else {
            vw = (h as f32 / aspect) as i32;
        }
        let px = x - (w - vw) / 2;
        let py = y - (h - vh) / 2;
        if vw == 0 || vh == 0 {
            return;
        }
        self.position = [aw * px / vw, ah - ah * py / vh];
        self.jump = [-1, -1];
        self.clamp();
    }
}

/// A varp (`is_varbit == false`) or varbit value over a varp snapshot.
pub fn read_var(
    varps: &[i32],
    varbits: Option<&crate::scenery_varbits::Inputs>,
    is_varbit: bool,
    id: i32,
) -> i32 {
    if !is_varbit {
        return varps.get(id as usize).copied().unwrap_or(0);
    }
    let Some(defs) = varbits else { return 0 };
    let Ok(bit) = defs.get(id, false) else {
        return 0;
    };
    let Some(base) = bit.binding.as_ref() else {
        return 0;
    };
    let raw = varps.get(base.id as usize).copied().unwrap_or(0);
    bit.get(raw).unwrap_or(0)
}

#[cfg(test)]
mod tests;
