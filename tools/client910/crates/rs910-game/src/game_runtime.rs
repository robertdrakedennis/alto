//! Production inputs and retained packet/map ownership for the live packet read.
//! Actor scheduling and external receipts are owned by the application adapter.
use crate::{
    animation_playback::AnimationRandom,
    cache::Pack,
    entity_runtime::{Inputs, PacketContext, Runtime},
    protocol910::{
        self,
        live::{Applied, Feed},
        rebuild_state::{Kind, SceneBounds, World},
        zone_state::Snapshot,
    },
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarpEffect {
    pub id: i32,
    pub value: i32,
    pub client_code: i32,
}

pub struct Game {
    pub inputs: Inputs,
    pub runtime: Runtime,
    pub cycle: i32,
    pub camera: crate::camera_follow::Follow,
    pub rebuild_started_ms: Option<i64>,
    pub packets_applied: u64,
    /// Ground-stack scene work stays owned until the renderer applies it.
    pub pending_refresh: BTreeSet<(i32, i32, i32)>,
    /// Retained for interface transmit hooks and positioned-sound refresh.
    pub varp_effects: VecDeque<VarpEffect>,
    /// Bumped once per client-variable update; the audio owner refreshes its
    /// positioned sounds when it moves.
    pub varp_serial: u64,
    /// Sequence sound requests emitted by the actor logic owner before
    /// the retained UI/audio tick consumes them.
    pub pending_actor_sounds: VecDeque<crate::actor::SequenceSound>,
    pub single_mouse_button: i32,
    pub chat_effects: i32,
    pub scene_locs: BTreeMap<(i32, i32, i32, i32), Snapshot>,
    /// Location animation commands move from packet state to the live scene
    /// owner at the next frame boundary.
    pub pending_loc_animations: VecDeque<crate::protocol910::zone_state::LocAnimation>,
    /// Positional sound packets move from the zone decoder to the retained
    /// audio owner at the same packet boundary.
    pub pending_sounds: VecDeque<crate::protocol910::zone_state::SoundArea>,
    /// The cutscene manager plus the cutscene-owned client state (scene
    /// state, cutscene id, fade, cutscene camera flag).
    pub cutscene: crate::cutscene::Manager,
    pub attachment_y: BTreeMap<(i32, i32), i32>,
    /// The members flag as installed into the loc/obj/NPC factories; the
    /// scene builder's `LocStore` reads it.
    pub allow_members: bool,
    transient_cycle: i32,
    pub random: AnimationRandom,
}

/// The sound emitter of a world spot animation.
fn spot_emitter(
    level: i32,
    position: [f32; 3],
    targeted: i32,
    listener: Option<(i32, i32)>,
) -> crate::sequence_sound::Emitter {
    crate::sequence_sound::Emitter {
        level,
        fine_x: position[0],
        fine_z: position[2],
        local: false,
        visible: true,
        targeted,
        listener,
    }
}

impl Game {
    /// Players, NPCs, then chat expiry, once per logic cycle.
    pub fn update_actors(&mut self) -> Result<(), protocol910::Error> {
        if self.runtime.map_request.is_some() || !self.runtime.feed.state.initialized {
            return Ok(());
        }
        let state = &mut self.runtime.feed.state;
        let varps = state.varps.as_ref();
        let bits = &self.inputs.bits;
        // Multi-NPC resolution reads the local player's variable state.
        let vars = |bit: bool, id: i32| -> Option<i32> {
            if bit {
                let definition = bits.get(id, false).ok()?;
                varps?.get_bit(&definition).ok()
            } else {
                varps?.get(id).ok()
            }
        };
        let c = crate::actor::Inputs {
            selection: &self.inputs.selection,
            sequences: &self.inputs.animation.sequences.sequences,
            bases: &self.inputs.bas,
            npcs: &self.inputs.appearance.types.npcs,
            vars: Some(&vars),
        };
        let mut sounds = Vec::new();
        crate::actor::tick_with_sounds(
            &mut state.players,
            &mut state.npcs,
            &crate::actor::TickContext {
                inputs: &c,
                map: &self.runtime.map,
                scene: self.runtime.terrain.as_ref(),
                cycle: self.cycle,
            },
            &mut crate::actor::TickOutput {
                rng: &mut self.random,
                sounds: &mut sounds,
            },
        )?;
        self.pending_actor_sounds.extend(sounds);
        Ok(())
    }
    /// The projectile lifecycle: advance active projectiles
    /// against the installed terrain and remove expired nodes. Rendering owns
    /// the retained positions; packet decode alone does not complete this.
    pub fn update_transients(&mut self) -> Result<(), protocol910::Error> {
        if self.runtime.map_request.is_some() || !self.runtime.feed.state.initialized {
            return Ok(());
        }
        let cycle = self.cycle;
        let previous = self.transient_cycle;
        self.transient_cycle = cycle;
        let terrain = self.runtime.terrain.as_ref();
        let height = |x: i32, z: i32, level: i32| {
            crate::protocol910::terrain::height(terrain, x, z, level).unwrap_or(0)
        };
        let world = [self.runtime.map.width * 512, self.runtime.map.height * 512];
        let local = self.runtime.map.local;
        let cutscene = (self.cutscene.scene_state == 0).then_some(&self.cutscene);
        let bases = &self.inputs.bas;
        let npc_types = &self.inputs.appearance.types.npcs;
        let bits = &self.inputs.bits;
        let state = &mut self.runtime.feed.state;
        let (players, npcs, varps) = (&state.players, &state.npcs, state.varps.as_ref());
        let transients = &mut state.zones.transients;
        // A cutscene entity, none when it does not exist.
        let cutscene_entity = |index: i32| -> Option<&crate::entities910::Player> {
            let manager = cutscene?;
            let index = usize::try_from(index).ok()?;
            manager.entity(index, local)
        };
        // An actor's BAS: the player's appearance BAS or the NPC's resolved
        // BAS over the local player's variable state.
        let bas_of = |actor: &crate::entities910::Player,
                      npc: Option<&crate::entities910::Npc>|
         -> Option<&crate::protocol910::bas_types::Bas> {
            let id = match npc {
                None => actor.appearance.bas,
                Some(n) if n.bas_override != -1 => n.bas_override,
                Some(n) => {
                    let t = npc_types.get(&n.type_id);
                    let multi = t
                        .and_then(|t| t.multinpc.as_deref().map(|list| (t, list)))
                        .and_then(|(t, list)| {
                            let read = |bit: bool, id: i32| -> Option<i32> {
                                let varps = varps?;
                                if bit {
                                    varps.get_bit(&bits.get(id, false).ok()?).ok()
                                } else {
                                    varps.get(id).ok()
                                }
                            };
                            crate::config::select_multi(t.multivarbit, t.multivarp, list, &read)
                        })
                        .and_then(|id| npc_types.get(&(id as i32)))
                        .map(|t| t.bas)
                        .filter(|&bas| bas != -1);
                    multi.or(t.map(|t| t.bas)).unwrap_or(-1)
                }
            };
            bases.get(&id)
        };
        // The local player's level and target id, for the sequence sounds.
        let listener = players
            .players
            .get(local)
            .and_then(Option::as_ref)
            .map(|p| (p.level, -(local as i32) - 1));
        let sequences = &self.inputs.animation.sequences.sequences;
        let mut sounds = Vec::new();
        transients.projectiles.retain_mut(|projectile| {
            let emitter = |p: &crate::entities910::transient::Projectile| {
                Some(spot_emitter(p.level, p.position, p.targeted, listener))
            };
            let at_creation = emitter(projectile);
            // The sound of the sequence's first frame plays when the
            // projectile is created, before it starts moving.
            crate::sequence_sound::play_triggers(
                &mut projectile.animation,
                at_creation,
                sequences,
                &mut self.random,
                &mut sounds,
            );
            if cycle > projectile.end {
                return false;
            }
            if cycle < projectile.start {
                return true;
            }
            // Pushed ahead of the animation update: an unmoved projectile
            // sits on its source actor plus the BAS wear-slot offset.
            if !projectile.mobile && projectile.source != 0 {
                let source = if cutscene.is_some() {
                    cutscene_entity(projectile.source - 1).map(|e| (e, None))
                } else if projectile.source < 0 {
                    players
                        .players
                        .get((-projectile.source - 1) as usize)
                        .and_then(Option::as_ref)
                        .map(|e| (e, None))
                } else {
                    npcs.entities
                        .get(&((projectile.source - 1) as usize))
                        .map(|n| (&n.path, Some(n)))
                };
                if let Some((actor, npc)) = source {
                    let (x, z) = (actor.fine_x, actor.fine_z);
                    projectile.position = [
                        x,
                        height(x as i32, z as i32, projectile.level)
                            .wrapping_sub(projectile.offset_start) as f32,
                        z,
                    ];
                    if projectile.slot >= 0 {
                        let slot = projectile.slot as usize;
                        let mut dx = 0;
                        let mut dz = 0;
                        if let Some(bas) = bas_of(actor, npc) {
                            for table in [&bas.slot_transforms, &bas.slot_offsets] {
                                if let Some(t) = table
                                    .as_ref()
                                    .and_then(|t| t.get(slot))
                                    .and_then(Option::as_ref)
                                {
                                    dx += t.first().copied().unwrap_or(0);
                                    dz += t.get(2).copied().unwrap_or(0);
                                }
                            }
                        }
                        if dx != 0 || dz != 0 {
                            let angle = actor.angle;
                            let slot_angle = actor
                                .actor
                                .wear_angles
                                .as_ref()
                                .and_then(|a| a.get(slot))
                                .copied()
                                .filter(|&a| a != -1)
                                .unwrap_or(angle);
                            let delta = slot_angle.wrapping_sub(angle) & 0x3FFF;
                            let (sin, cos) = (crate::trig::sin(delta), crate::trig::cos(delta));
                            projectile.position[0] += ((dx * cos + dz * sin) >> 14) as f32;
                            projectile.position[2] += ((dz * cos - dx * sin) >> 14) as f32;
                        }
                    }
                }
            }
            // The target actor re-aims the projectile each draw.
            let in_world = |e: &crate::entities910::Player| {
                let (x, z) = (e.fine_x as i32, e.fine_z as i32);
                x >= 0 && x < world[0] && z >= 0 && z < world[1]
            };
            if projectile.target > 0 {
                let target = if cutscene.is_some() {
                    cutscene_entity(projectile.target - 1).map(|e| (e, e.level))
                } else {
                    npcs.entities
                        .get(&((projectile.target - 1) as usize))
                        .map(|n| (&n.path, projectile.level))
                };
                if let Some((e, level)) = target.filter(|(e, _)| in_world(e)) {
                    let (x, z) = (e.fine_x as i32, e.fine_z as i32);
                    let y = height(x, z, level).wrapping_sub(projectile.offset_end);
                    projectile.velocity(x, z, y, cycle, &height);
                }
            }
            if projectile.target < 0 {
                if let Some(e) = players
                    .players
                    .get((-projectile.target - 1) as usize)
                    .and_then(Option::as_ref)
                    .filter(|e| in_world(e))
                {
                    let (x, z) = (e.fine_x as i32, e.fine_z as i32);
                    let y = height(x, z, projectile.level).wrapping_sub(projectile.offset_end);
                    projectile.velocity(x, z, y, cycle, &height);
                }
            }
            let from = previous.max(projectile.start);
            let ticks = cycle.wrapping_sub(from);
            if ticks > 0 {
                projectile.advance_physics(ticks, &height);
                let sequence = projectile.animation.id();
                let range = projectile.animation.skeletal_range;
                if let Some(sequence) = self.inputs.animation.sequences.sequences.get(&sequence) {
                    crate::animation_playback::advance(
                        &mut projectile.animation,
                        sequence,
                        ticks,
                        range,
                        &mut self.random,
                    );
                    // A completed
                    // effect node while the projectile remains in flight.
                    if projectile.animation.finished {
                        projectile.animation.restart(0);
                    }
                    let moved = emitter(projectile);
                    crate::sequence_sound::play_triggers(
                        &mut projectile.animation,
                        moved,
                        sequences,
                        &mut self.random,
                        &mut sounds,
                    );
                }
            }
            true
        });
        // A spot animation's first-frame sound plays when it is created.
        for spot in &mut transients.spots {
            if let Some(node) = spot.animation.as_mut() {
                let emitter = spot_emitter(spot.level, spot.position, spot.targeted, listener);
                crate::sequence_sound::play_triggers(
                    node,
                    Some(emitter),
                    sequences,
                    &mut self.random,
                    &mut sounds,
                );
            }
        }
        if cycle > previous {
            let ticks = cycle.wrapping_sub(previous);
            let effects = &self.inputs.animation.effects;
            transients.spots.retain_mut(|spot| {
                let emitter = spot_emitter(spot.level, spot.position, spot.targeted, listener);
                let Some(node) = spot.animation.as_mut() else {
                    return true;
                };
                let sequence_id = node.id();
                let range = node.skeletal_range;
                let Some(sequence) = sequences.get(&sequence_id) else {
                    return true;
                };
                crate::animation_playback::advance(node, sequence, ticks, range, &mut self.random);
                crate::sequence_sound::play_triggers(
                    node,
                    Some(emitter),
                    sequences,
                    &mut self.random,
                    &mut sounds,
                );
                if !node.finished {
                    return true;
                }
                if effects
                    .get(&spot.effect)
                    .is_some_and(|effect| effect.looping)
                {
                    node.restart(0);
                    true
                } else {
                    // Non-looping effects are marked
                    // complete so the scene can release them.
                    false
                }
            });
        }
        self.pending_actor_sounds.extend(sounds);
        Ok(())
    }
    /// Fresh world login; unlike a reconnect, no previous world/Scene is installed.
    /// The default origins apply; the standard area is configured by loading
    /// before entry. The wire rebuild selects its actual area and slots.
    pub fn login(
        pack: &Pack,
        local: usize,
        feed: Feed,
        seed: u64,
        logged_in_members: bool,
    ) -> anyhow::Result<Self> {
        let configs = rs910_config::login_configs::LoginConfigs::read(pack);
        Self::login_with(pack, &configs, local, feed, seed, logged_in_members)
    }
    /// [`Self::login`] decoding the config archives `configs` has read, which
    /// the interface engine decodes from too.
    pub fn login_with(
        pack: &Pack,
        configs: &rs910_config::login_configs::LoginConfigs,
        local: usize,
        mut feed: Feed,
        seed: u64,
        logged_in_members: bool,
    ) -> anyhow::Result<Self> {
        // The members flag is set on the object type list
        // runs before the first PLAYER_INFO, so worn-object `team` gating
        // uses the world's membership.
        let inputs = Inputs::load_with(
            pack,
            configs,
            logged_in_members,
            false,
            crate::logic_clock::RATE,
        )?;
        let world = World {
            base_x: 0,
            base_z: 0,
            region_x: 0,
            region_z: 0,
            width: 104,
            height: 104,
            area: None,
            last_kind: Kind::Other,
            npc_bits: 0,
            map_squares: vec![],
            groups: vec![],
            group_count: 0,
        };
        let mut runtime = Runtime::new(&inputs, world, local, None)
            .map_err(|e| anyhow::anyhow!("initialize entity runtime: {e:?}"))?;
        anyhow::ensure!(
            !feed.state.initialized,
            "fresh login cannot reuse initialized entity state"
        );
        feed.state.varps = runtime.feed.state.varps.take();
        feed.state.world = runtime.feed.state.world.take();
        runtime.feed = feed;
        Ok(Self {
            inputs,
            runtime,
            cycle: 0,
            camera: Default::default(),
            rebuild_started_ms: None,
            packets_applied: 0,
            pending_refresh: BTreeSet::new(),
            varp_effects: VecDeque::new(),
            varp_serial: 0,
            pending_actor_sounds: VecDeque::new(),
            single_mouse_button: 0,
            chat_effects: 0,
            scene_locs: BTreeMap::new(),
            pending_loc_animations: VecDeque::new(),
            pending_sounds: VecDeque::new(),
            cutscene: crate::cutscene::Manager::default(),
            attachment_y: BTreeMap::new(),
            allow_members: logged_in_members,
            transient_cycle: 0,
            random: AnimationRandom::new(seed),
        })
    }
    /// A reconnect the server resumed in place (login reply 15): drop the
    /// frames of the lost connection and re-initialise the player list from the
    /// server's player-positions block; the map, npcs and variables stay.
    pub fn resume_players(&mut self, block: &[u8]) -> Result<(), protocol910::Error> {
        self.runtime.feed.discard_pending();
        let state = protocol910::rebuild_state::resume_players(
            block,
            &self.runtime.feed.state,
            &self.runtime.map,
            Some(&self.inputs.appearance.appearance),
        )?;
        self.runtime.feed.state = state;
        // A cutscene the client was showing ends with the connection.
        self.cutscene.reset_cutscene();
        Ok(())
    }
    /// Actual CPU contexts, never fixture constants. The decoder retains the front
    /// packet on failure; RNG advances only for accepted NPC creations.
    pub fn apply_next(
        &mut self,
        now_ms: i64,
        textures: bool,
    ) -> Result<Option<Applied>, protocol910::Error> {
        let npc = self
            .runtime
            .feed
            .front()
            .is_some_and(|f| f.opcode == crate::proto::server::NPC_INFO);
        // Each fresh NPC draws four values, taken lazily
        // in creation order from a speculative copy (the NPC array is bounded
        // at 1024); unused draws never happen and do not advance the
        // committed source.
        let speculative = std::cell::RefCell::new(self.random.clone());
        let draw = || {
            let mut random = speculative.borrow_mut();
            std::array::from_fn(|_| random.next())
        };
        let random = if npc {
            protocol910::npc::Random::Stream(&draw)
        } else {
            protocol910::npc::Random::Samples(&[])
        };
        let applied = self.apply_with(now_ms, random, textures)?;
        if let Some(a) = &applied {
            for _ in 0..a.random_used * 4 {
                self.random.next();
            }
        }
        Ok(applied)
    }
    /// Replay supplies the same external random samples as the reference client; live callers
    /// use apply_next, which commits only the consumed prefix of the RNG stream.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn apply_with_random(
        &mut self,
        now_ms: i64,
        random: &[[f64; 4]],
        textures: bool,
    ) -> Result<Option<Applied>, protocol910::Error> {
        self.apply_with(now_ms, protocol910::npc::Random::Samples(random), textures)
    }
    fn apply_with(
        &mut self,
        now_ms: i64,
        random: protocol910::npc::Random<'_>,
        textures: bool,
    ) -> Result<Option<Applied>, protocol910::Error> {
        let context = PacketContext {
            cycle: self.cycle,
            now_ms,
            // `Preferences.textures`, read by the caller (client_game).
            textures,
            random,
            zone_allow_outside: false,
            // Zone packets are gated on the last rebuild type being a cutscene.
            cutscene: self.cutscene.rebuild_type_cutscene,
            scene_bounds: self.runtime.terrain.as_ref().map(|t| SceneBounds {
                width: t.width as i32,
                height: t.height as i32,
                level_tiles: true,
            }),
            scene_locs: self.runtime.terrain.as_ref().map(|_| &self.scene_locs),
            attachment_y: Some(&self.attachment_y),
        };
        let applied = self.runtime.apply_next(&self.inputs, &context)?;
        if let Some(a) = &applied {
            self.packets_applied += 1;
            if a.rebuild.is_some() {
                // A rebuild stores the packet's rebuild type and ends a
                // finished cutscene.
                self.cutscene.rebuild_type_cutscene = false;
                if self.cutscene.scene_state == 4 {
                    self.cutscene.scene_state = 3;
                    self.cutscene.client_id = -1;
                }
            }
            if let Some((_, effect)) = a.rebuild.as_ref().filter(|(_, e)| e.rebased) {
                if effect.mode == 3 {
                    self.camera.rebase(effect.delta_x, effect.delta_z);
                }
                self.rebuild_started_ms = None;
                self.pending_refresh.clear();
                self.pending_loc_animations.clear();
                self.pending_actor_sounds.clear();
            }
            self.pending_refresh.extend(a.refresh.iter().copied());
            // Ownership moves to this adapter: RNG and refresh work above,
            // rebuild effects in MapRequest, and read-batch return value below.
            // Do not retain a second growing copy inside the Feed.
            for (_, receipt) in self.runtime.feed.drain_completed() {
                debug_assert_eq!(&receipt, a);
            }
        }
        if applied.is_some() {
            self.pending_loc_animations
                .extend(self.runtime.feed.state.zones.loc_animations.drain(..));
            self.pending_sounds
                .extend(self.runtime.feed.state.zones.sounds.drain(..));
        }
        Ok(applied)
    }
    pub fn take_loc_animations(&mut self) -> Vec<crate::protocol910::zone_state::LocAnimation> {
        self.pending_loc_animations.drain(..).collect()
    }
    pub fn take_sounds(&mut self) -> Vec<crate::protocol910::zone_state::SoundArea> {
        self.pending_sounds.drain(..).collect()
    }
    pub fn take_actor_sounds(&mut self) -> Vec<crate::actor::SequenceSound> {
        self.pending_actor_sounds.drain(..).collect()
    }
    /// The `CUTSCENE` packet starts a cutscene.
    pub fn begin_cutscene(&mut self, id: u16, parameter: u16, appearance: &[u8]) {
        self.cutscene.begin(id, parameter, appearance);
    }
    /// The client-variable poll. Poll one
    /// server value, apply its client-code state, then append the transmit ring.
    /// Audio/UI work remains explicitly owned in varp_effects for its consumers.
    pub fn poll_vars(&mut self, mut now: impl FnMut() -> i64) -> Result<(), protocol910::Error> {
        let vars = self
            .runtime
            .feed
            .state
            .varps
            .as_mut()
            .ok_or(protocol910::Error::UnsupportedContext("local varps"))?;
        // The client-variable update has no queued consumer:
        // codes 5/6 apply inline and the positioned-sound refresh reads
        // `varp_serial`. Keep only this poll's effects (read by the oracle)
        // so a live session does not grow the queue without bound.
        self.varp_effects.clear();
        let mut restart = true;
        loop {
            let id = vars.poll(restart, now())?;
            restart = false;
            if id == -1 {
                return Ok(());
            }
            let code = self
                .inputs
                .variables
                .players
                .get(&id)
                .ok_or(protocol910::Error::Invalid("varp transmit definition"))?
                .client_code;
            let value = vars.get(id)?;
            self.varp_serial = self.varp_serial.wrapping_add(1);
            match code {
                5 => self.single_mouse_button = value,
                6 => self.chat_effects = value,
                _ => {}
            }
            self.varp_effects.push_back(VarpEffect {
                id,
                value,
                client_code: code,
            });
            self.runtime.varp_transmit_num = self.runtime.varp_transmit_num.wrapping_add(1);
            self.runtime.varp_transmitted
                [(self.runtime.varp_transmit_num.wrapping_sub(1) & 63) as usize] = id;
        }
    }
    /// Thin-replay helper: install one server varp value through the real
    /// player-variable server path and run the poll
    /// loop above so the shared CS2 getter
    /// (scripts 2524->2526) observes it on the next tick. Generic over all
    /// varps (5863/5967/8172); the caller supplies id/value from cache varbit
    /// defs, so new Inventory rows are data, not new branches.
    #[cfg(any(test, feature = "test-hooks"))] // thin-replay helpers
    pub fn install_server_varp(
        &mut self,
        id: i32,
        value: i32,
        now_ms: i64,
    ) -> Result<(), protocol910::Error> {
        let varps = self
            .runtime
            .feed
            .state
            .varps
            .as_mut()
            .ok_or(protocol910::Error::UnsupportedContext("local varps"))?;
        varps.set_server(id, value, now_ms)?;
        self.poll_vars(|| now_ms)?;
        Ok(())
    }
    /// Generic varbit read through the retained player variables
    /// (the CS2 push_varbit path used by getter 2526).
    /// No per-setting branches; the cache definition supplies base/start/end.
    #[cfg(any(test, feature = "test-hooks"))] // thin-replay helpers
    pub fn varbit_value(&self, id: u16) -> Result<i32, protocol910::Error> {
        let varps = self
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .ok_or(protocol910::Error::UnsupportedContext("local varps"))?;
        let bit = self
            .inputs
            .bits
            .get(i32::from(id), false)
            .map_err(|failure| failure.cause)?;
        varps.get_bit(&bit)
    }
    pub fn local_position(&self) -> Option<(i32, f32, f32, f32)> {
        let p = self
            .runtime
            .feed
            .state
            .players
            .players
            .get(self.runtime.map.local)?
            .as_ref()?;
        let w = self.runtime.feed.state.world.as_ref()?;
        Some((
            p.level,
            (w.base_x * 512) as f32 + p.fine_x,
            p.motion.y,
            (w.base_z * 512) as f32 + p.fine_z,
        ))
    }

    /// Apply retained loc change-request state to the scene snapshot
    /// consumed by loc menus and CS2 position/bounds queries. Rendering owns
    /// the replacement mesh; this keeps the interaction owner on the same
    /// location id/shape/angle after a zone update. Called where the game
    /// update applies loc changes (`update_session_logic`), before the actors
    /// and the UI tick.
    pub fn apply_location_snapshots(&mut self) {
        for request in self
            .runtime
            .feed
            .state
            .zones
            .locations
            .iter()
            .chain(&self.runtime.feed.state.zones.customisations)
        {
            let key = (request.level, request.layer, request.x, request.z);
            if request.remove || request.id < 0 {
                self.scene_locs.remove(&key);
            } else {
                self.scene_locs.insert(
                    key,
                    Snapshot {
                        id: request.id,
                        shape: request.shape,
                        angle: request.angle,
                        transform: request.transform,
                    },
                );
            }
        }
    }
}
