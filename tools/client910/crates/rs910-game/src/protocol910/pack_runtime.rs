//! CPU packet/context assembly using the verified cache providers. No graphics
//! objects or socket polling. Logic cycles,
//! entity scheduling and varp polling are driven by the game owner
//! (`game_runtime::Game`, on the app's `logic_clock` cadence);
//! SERVER_TICK_END only ends a read batch, it is not a logic tick.
use crate::{
    cache::Pack,
    entities910::{animation_state, varps::Varps},
    protocol910::{
        self as protocol,
        live::{self, Applied, Feed},
        npc, npc_custom,
        rebuild_state::{SceneBounds, World},
        terrain::Terrain,
        variable_types::Domain,
        variables, zone, zone_state, Error,
    },
};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
#[path = "pack_animation.rs"]
mod animation_pack;
#[path = "pack_appearance.rs"]
pub mod appearance_pack;
#[path = "pack_bas.rs"]
mod bas_pack;
pub use rs910_config::scenery_varbits as bits_pack;
#[path = "pack_combat.rs"]
pub(crate) mod combat_pack;
#[path = "pack_locs.rs"]
mod locs_pack;
#[path = "pack_terrain.rs"]
mod terrain_pack;
#[path = "pack_variables.rs"]
mod variables_pack;
pub struct Inputs {
    pub appearance: appearance_pack::Inputs,
    pub animation: animation_pack::Inputs,
    pub selection: animation_state::Config,
    pub bas: BTreeMap<i32, protocol::bas_types::Bas>,
    pub combat: protocol::combat::Config,
    /// Full hitmark and headbar type lists for the 2D entity element pass.
    pub combat_types: combat_pack::Types,
    pub variables: variables_pack::Inputs,
    pub player_variables: variables::Config,
    pub npc_variables: variables::Config,
    pub bits: std::sync::Arc<bits_pack::Inputs>,
    pub locs: locs_pack::Inputs,
    pub land_groups: BTreeSet<i32>,
    pub npc_types: BTreeMap<i32, npc::NpcType>,
    pub npc_custom: npc_custom::Config,
    pub objects: BTreeMap<i32, zone::ObjectType>,
    pub logic_rate: i32,
}
impl Inputs {
    pub fn load(
        pack: &Pack,
        members: bool,
        staff_live_override: bool,
        logic_rate: i32,
    ) -> Result<Self> {
        Self::load_with(
            pack,
            &rs910_config::login_configs::LoginConfigs::read(pack),
            members,
            staff_live_override,
            logic_rate,
        )
    }
    /// [`Self::load`] decoding the config archives `configs` has read.
    pub fn load_with(
        pack: &Pack,
        configs: &rs910_config::login_configs::LoginConfigs,
        members: bool,
        staff_live_override: bool,
        logic_rate: i32,
    ) -> Result<Self> {
        anyhow::ensure!(logic_rate > 0, "logic rate must be supplied");
        let types = protocol::pack_types::decode(
            configs.required_objs()?,
            configs.required_npcs()?,
            members,
        )?;
        let appearance = appearance_pack::load_inputs_with(pack, types, staff_live_override)?;
        let animation = animation_pack::load(pack, configs.required_seqs()?)?;
        let (loc_store, loc_count) = configs.required_locs()?;
        let selection = animation.selection(appearance.appearance.wear.len());
        let combat_types = combat_pack::load(pack)?;
        let combat = combat_types.config(&appearance.defaults.graphics, logic_rate);
        let variables = variables_pack::load(pack)?;
        let player_variables = variables.packet_config(Domain::Player);
        let npc_variables = variables.packet_config(Domain::Npc);
        let npc_types = appearance.types.npc_types();
        let npc_custom = appearance.types.npc_customisations();
        let objects = appearance.types.ground_types();
        Ok(Self {
            appearance,
            animation,
            selection,
            bas: bas_pack::load(pack)?,
            combat,
            combat_types,
            variables,
            player_variables,
            npc_variables,
            bits: configs.required_varbits()?,
            locs: locs_pack::footprints(loc_store, loc_count),
            land_groups: terrain_pack::land_groups(pack)?,
            npc_types,
            npc_custom,
            objects,
            logic_rate,
        })
    }
}
pub struct PacketContext<'a> {
    pub cycle: i32,
    pub now_ms: i64,
    pub textures: bool,
    pub random: npc::Random<'a>,
    pub zone_allow_outside: bool,
    pub cutscene: bool,
    pub scene_bounds: Option<SceneBounds>,
    pub scene_locs: Option<&'a zone_state::SceneLocs>,
    pub attachment_y: Option<&'a BTreeMap<(i32, i32), i32>>,
}
pub struct MapRequest {
    pub generation: u64,
    pub world: World,
    pub effects: protocol::rebuild_state::Effects,
}
#[derive(Clone)]
pub struct PreparedMap {
    generation: u64,
    world: World,
    pub data: terrain_pack::Inputs,
}
pub struct Runtime {
    pub feed: Feed,
    pub map: protocol::Context,
    pub terrain: Option<Terrain>,
    pub installed_world: World,
    /// The instanced-region templates of the installed map
    /// (the world's region data); `None` for a normal map. A rebuild
    /// rebuilds the same window from them.
    pub installed_region: Option<protocol::rebuild_state::RegionLayout>,
    pub map_request: Option<MapRequest>,
    pub varp_transmit_num: i32,
    pub varp_transmitted: [i32; 64],
    /// Bumped on every whole-terrain install so per-cycle consumers (cam2's
    /// heightmap copy) know when the level heightmap was replaced.
    pub terrain_generation: u64,
    generation: u64,
}
/// [`Runtime::height`] over borrowed parts, so packet decode can read heights
/// while it mutates the feed.
fn height(
    terrain: Option<&Terrain>,
    map: &protocol::Context,
    scene: bool,
    x: i32,
    z: i32,
    level: i32,
) -> std::result::Result<i32, Error> {
    if !scene {
        return Ok(0);
    }
    let t = terrain.ok_or(Error::UnsupportedContext("installed scene terrain"))?;
    let (tx, tz) = (x >> 9, z >> 9);
    if tx < 0 || tz < 0 || tx >= map.width || tz >= map.height {
        return Ok(0);
    }
    let level = if level < 3 && map.bridges.contains(&(tx, tz)) {
        level.wrapping_add(1)
    } else {
        level
    };
    if !(0..4).contains(&level) {
        return Err(Error::Invalid("heightmap level"));
    }
    t.fine_height(x, z, level)
}
impl Runtime {
    /// The old world/map is explicit. None terrain means no scene, not a
    /// fabricated flat map. Existing sessions may replace `feed` with explicit state.
    pub fn new(
        inputs: &Inputs,
        world: World,
        local: usize,
        terrain: Option<Terrain>,
    ) -> std::result::Result<Self, Error> {
        let map = if let Some(t) = &terrain {
            if (t.width as i32, t.height as i32) != (world.width, world.height) {
                return Err(Error::Invalid("initial terrain/world mismatch"));
            }
            t.context(local, world.base_x, world.base_z)?
        } else {
            if local >= 2048 || world.width <= 0 || world.height <= 0 {
                return Err(Error::Invalid("initial world"));
            }
            protocol::Context {
                local,
                base_x: world.base_x,
                base_z: world.base_z,
                width: world.width,
                height: world.height,
                bridges: vec![],
            }
        };
        let mut feed = Feed::default();
        feed.state.varps = Some(Varps::new(inputs.variables.players.len()));
        feed.state.world = Some(world.clone());
        Ok(Self {
            feed,
            map,
            terrain,
            installed_world: world,
            installed_region: None,
            map_request: None,
            varp_transmit_num: 0,
            terrain_generation: 0,
            varp_transmitted: [0; 64],
            generation: 0,
        })
    }
    pub fn apply_next(
        &mut self,
        inputs: &Inputs,
        c: &PacketContext,
    ) -> std::result::Result<Option<Applied>, Error> {
        if self.map_request.is_some() {
            let error = Error::UnsupportedContext("map installation pending");
            self.feed.blocked = Some(error.clone());
            return Err(error);
        }
        let Some(opcode) = self.feed.front().map(|f| f.opcode) else {
            return Ok(None);
        };
        // No staged copy of the feed: every decoder leaves the retained state
        // untouched when it rejects a packet (live.rs `Change`), which keeps the
        // failed packet at the front with `blocked` set.
        let result = if protocol::varp::handles(opcode) {
            self.feed.apply_varp_next(c.now_ms, &|id| {
                inputs.bits.get(id, false).map_err(|e| e.cause)
            })
        } else {
            if c.scene_bounds.is_some() && self.terrain.is_none() {
                let error = Error::UnsupportedContext("installed scene terrain");
                self.feed.blocked = Some(error.clone());
                return Err(error);
            }
            let limits = inputs
                .appearance
                .defaults
                .graphics
                .entity_limits(inputs.logic_rate);
            let player = protocol::PlayerContext {
                world: &self.map,
                appearance: Some(&inputs.appearance.appearance),
                combat: Some(&inputs.combat),
                animation: Some(&inputs.selection),
                variables: Some(&inputs.player_variables),
                chat_timeout: Some(limits.player_chat_ticks),
                cycle: c.cycle,
            };
            let local = self
                .feed
                .state
                .players
                .players
                .get(self.map.local)
                .and_then(Option::as_ref);
            let npc = npc::NpcContext {
                map: &self.map,
                local_x: local.map_or(0, |p| p.x[0]),
                local_z: local.map_or(0, |p| p.z[0]),
                view_bits: self.installed_world.npc_bits,
                loop_cycle: c.cycle,
                textures: c.textures,
                combat: Some(&inputs.combat),
                animation: Some(&inputs.selection),
                variables: Some(&inputs.npc_variables),
                customisation: Some(&inputs.npc_custom),
                wear_slots: Some(inputs.appearance.appearance.wear.len()),
                chat_timeout: Some(limits.npc_chat_ticks),
                random: c.random,
                types: &inputs.npc_types,
            };
            let height_error = std::cell::RefCell::new(None);
            let (terrain, map) = (self.terrain.as_ref(), &self.map);
            let scene = c.scene_bounds.is_some();
            let get_height = |x, z, l| match height(terrain, map, scene, x, z, l) {
                Ok(h) => h,
                Err(e) => {
                    *height_error.borrow_mut() = Some(e);
                    0
                }
            };
            let transient = protocol::transient::Config {
                cycle: c.cycle,
                cutscene: c.cutscene,
                animation: &inputs.selection,
                height: &get_height,
                attachment_y: c.attachment_y,
            };
            let zone = zone_state::Config {
                map: &self.map,
                cycle: c.cycle,
                cutscene: c.cutscene,
                allow_outside: c.zone_allow_outside,
                objects: &inputs.objects,
                scene: c.scene_locs,
                transients: Some(&transient),
            };
            let rebuild = protocol::rebuild_state::Config {
                prior: &self.installed_world,
                old_map: &self.map,
                appearance: Some(&inputs.appearance.appearance),
                login: !self.feed.state.initialized,
                land_groups: &inputs.land_groups,
                loc_sizes: &inputs.locs.sizes,
                scene: c.scene_bounds,
            };
            // A height lookup failure inside the decode rejects the packet
            // before it commits.
            let r = self.feed.apply_next_checked(
                &live::Contexts {
                    rebuild: Some(&rebuild),
                    player: Some(&player),
                    npc: Some(&npc),
                    zone: Some(&zone),
                },
                || height_error.borrow_mut().take().map_or(Ok(()), Err),
            );
            let error = height_error.into_inner();
            if let Some(e) = error {
                Err(e)
            } else {
                r
            }
        };
        match result {
            Err(e) => {
                self.feed.blocked = Some(e.clone());
                Err(e)
            }
            Ok(applied) => {
                if let Some(a) = &applied {
                    if let Some(v) = &a.varp {
                        self.varp_transmit_num =
                            self.varp_transmit_num.wrapping_add(v.transmit_increment);
                    }
                    if let Some((w, e)) = &a.rebuild {
                        if e.rebased {
                            self.generation = self.generation.wrapping_add(1);
                            self.map_request = Some(MapRequest {
                                generation: self.generation,
                                world: w.clone(),
                                effects: e.clone(),
                            });
                        } else {
                            self.installed_world = w.clone();
                            // A region rebuild writes the region data and the
                            // rebuild type before the same-region early
                            // return.
                            self.installed_region = e.region.clone();
                            if e.resized {
                                self.map.width = w.width;
                                self.map.height = w.height;
                                self.map.bridges.clear();
                            }
                        }
                    }
                }
                Ok(applied)
            }
        }
    }
    /// The CPU terrain a region/cutscene map request installs
    /// (`terrain_pack::load_region`), for scene/renderer agreement checks.
    #[cfg(any(test, feature = "test-hooks"))] // test-only terrain loader
    pub fn load_region_terrain(
        pack: &Pack,
        world: &World,
        layout: &protocol::rebuild_state::RegionLayout,
    ) -> Result<Terrain> {
        Ok(terrain_pack::load_region(pack, world, layout)?.terrain)
    }
    /// A client-initiated map transaction (the cutscene rebuild): the same
    /// pending-install owner packet rebuilds use.
    pub fn request_map(&mut self, world: World, effects: protocol::rebuild_state::Effects) {
        self.generation = self.generation.wrapping_add(1);
        self.map_request = Some(MapRequest {
            generation: self.generation,
            world,
            effects,
        });
    }
    /// The heightmap lookup uses the world's bounds/flags before the old
    /// scene floor-model bounds. A build-area size change can reset the former
    /// while a same-region rebuild is skipped and retains the latter.
    #[cfg(any(test, feature = "test-hooks"))] // test-only terrain loader
    pub fn height(
        &self,
        scene: bool,
        x: i32,
        z: i32,
        level: i32,
    ) -> std::result::Result<i32, Error> {
        height(self.terrain.as_ref(), &self.map, scene, x, z, level)
    }
    pub fn prepare_map(&self, pack: &Pack) -> Result<PreparedMap> {
        let r = self
            .map_request
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no pending map"))?;
        let data = if let Some(layout) = &r.effects.region {
            terrain_pack::load_region(pack, &r.world, layout)?
        } else {
            terrain_pack::load_normal(pack, &r.world)?
        };
        Ok(PreparedMap {
            generation: r.generation,
            world: r.world.clone(),
            data,
        })
    }
    /// Call after the external map/environment/effect installation succeeds. This
    /// does not send MAP_BUILD_COMPLETE; that remains an explicit transport action.
    pub fn install_map(&mut self, p: PreparedMap) -> std::result::Result<(), Error> {
        let r = self
            .map_request
            .as_ref()
            .ok_or(Error::Invalid("no pending map"))?;
        if r.generation != p.generation || r.world != p.world {
            return Err(Error::Invalid("stale prepared map"));
        }
        if (p.data.terrain.width as i32, p.data.terrain.height as i32)
            != (p.world.width, p.world.height)
        {
            return Err(Error::Invalid("prepared terrain/world mismatch"));
        }
        let map = p
            .data
            .terrain
            .context(self.map.local, p.world.base_x, p.world.base_z)?;
        self.map = map;
        self.terrain = Some(p.data.terrain);
        self.terrain_generation = self.terrain_generation.wrapping_add(1);
        self.installed_world = p.world;
        self.installed_region = r.effects.region.clone();
        self.map_request = None;
        Ok(())
    }
}
