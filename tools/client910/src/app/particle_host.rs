//! `ViewerApp::particles` (code-quality programme Phase 4.4).
use super::*;

/// The particle system and GPU particle renderer state and the particle
/// bindings of scene entities.
pub(super) struct ParticleHost {
    /// NPC body anchors per NPC index.
    pub(super) npc_bindings: HashMap<usize, crate::particle::Binding>,
    /// Scene temporary index -> (particle owner key, whether this entity is
    /// the owner's draw that `drawParticles` follows). Actor spot models
    /// join the actor's system but draw inside the actor.
    pub(super) temporary: HashMap<usize, (u64, bool)>,
    /// Last applied `ActorState::particle_resets` per actor owner key.
    pub(super) resets_seen: HashMap<u64, u32>,
    /// Spot, projectile and actor-spot model anchors.
    pub(super) effect_bindings: Vec<crate::particle::Binding>,
    /// The particle systems, including the entity-owned ones.
    pub(super) runtime: Option<crate::particle::Runtime>,
    /// GPU particle renderer CPU state.
    pub(super) builder: crate::particle_render::Builder,
    /// The scene (`floor_base`, terrain generation) the runtime's owner keys
    /// and scene-local coordinates belong to.
    pub(super) base: Option<((i32, i32), u64)>,
}

/// The scene view moving particles read: heightmaps, tile levels and the
/// entity bounds of walls, dynamic walls, ground decorations and entities.
pub(super) struct ParticleSceneAdapter<'a> {
    pub(super) scene: &'a mut crate::scene::Scene,
    pub(super) floors: &'a [Option<&'a crate::floor::FloorHeights>],
}

impl ParticleSceneAdapter<'_> {
    /// The entity bounds test over the bounds built from the transform and model.
    pub(super) fn contains(
        model: Option<&crate::gpumodel::GpuModel>,
        trans: [i32; 3],
        p: [i32; 3],
    ) -> bool {
        let Some([x0, x1, y0, y1, z0, z1, r]) = model.and_then(|m| m.cached_bounds()) else {
            return false;
        };
        if p[0] < trans[0] + x0 || p[0] > trans[0] + x1 {
            return false;
        }
        if p[1] < trans[1] + y0 || p[1] > trans[1] + y1 {
            return false;
        }
        if p[2] < trans[2] + z0 || p[2] > trans[2] + z1 {
            return false;
        }
        let (dx, dz) = (p[0] - trans[0], p[2] - trans[2]);
        dx * dx + dz * dz < r * r
    }
}

impl crate::particle::ParticleScene for ParticleSceneAdapter<'_> {
    fn size(&self) -> i32 {
        self.scene.size
    }
    fn max_tile_x(&self) -> i32 {
        self.scene.max_x as i32
    }
    fn max_tile_z(&self) -> i32 {
        self.scene.max_z as i32
    }
    fn max_level(&self) -> i32 {
        self.scene.max_level as i32
    }
    fn tile_height(&self, level: i32, x: i32, z: i32) -> i32 {
        self.floors
            .get(level as usize)
            .copied()
            .flatten()
            .filter(|f| (x as usize) <= f.tiles_x && (z as usize) <= f.tiles_z)
            .map_or(0, |f| f.get_tile_height(x as usize, z as usize))
    }
    fn tile_level(&self, level: i32, x: i32, z: i32) -> Option<i32> {
        self.scene
            .tile(level as usize, x as usize, z as usize)
            .map(|t| i32::from(t.level))
    }
    fn bridged(&self, x: i32, z: i32) -> bool {
        self.scene
            .tile(0, x as usize, z as usize)
            .is_some_and(|t| t.bridge.is_some())
    }
    fn create_tile(&mut self, plane: i32, x: i32, z: i32, level: i32) {
        self.scene
            .put_tile(plane as usize, x as usize, z as usize, level);
    }
    /// The bounds test over the tile `particle::allocate_tiles`
    /// selected.
    fn bounds_contain(&self, plane: i32, x: i32, z: i32, p: [i32; 3]) -> bool {
        let Some(tile) = self.scene.tile(plane as usize, x as usize, z as usize) else {
            return false;
        };
        let s = &*self.scene;
        let wall = |i: Option<usize>| {
            i.is_some_and(|i| {
                let w = &s.walls[i];
                Self::contains(w.model.as_ref(), [w.x, w.y, w.z], p)
            })
        };
        if wall(tile.wall) || wall(tile.dynamic_wall) {
            return true;
        }
        if let Some(i) = tile.ground_decoration {
            let g = &s.ground_decors[i];
            if Self::contains(g.model.as_ref(), [g.x, g.y, g.z], p) {
                return true;
            }
        }
        tile.entities.iter().any(|e| match *e {
            crate::scene::PrimaryRef::Scenery(i) => {
                let e = &s.scenery[i];
                Self::contains(e.model.as_ref(), [e.x, e.y, e.z], p)
            }
            crate::scene::PrimaryRef::Temporary(i) => {
                let e = &s.temporary[i];
                Self::contains(e.model.as_ref(), e.position.map(|v| v as i32), p)
            }
        })
    }
}

impl ViewerApp {
    /// The particle tick at the top of the game and title/lobby draws,
    /// before the interface tree (and the scene inside it) binds and draws
    /// particles. The runtime is created on first use; a rebuild kills
    /// every system; the particle preferences feed the detail level.
    pub(super) fn tick_particles(&mut self) {
        if self.scene.pack_root.is_none() {
            return;
        }
        if self.particles.runtime.is_none() {
            let pack = self.pack.clone();
            let loaded = crate::particle::EmitterStore::load(&pack)
                .and_then(|emitters| Ok((emitters, crate::particle::EffectorStore::load(&pack)?)));
            match loaded {
                Ok((emitters, effectors)) => {
                    self.particles.runtime = Some(crate::particle::Runtime::new(
                        emitters,
                        effectors,
                        0x5eed_7a27,
                    ))
                }
                Err(error) => {
                    crate::logging::warn_repeated!(
                        "[client910] particle types unavailable: {error:#}"
                    );
                    return;
                }
            }
        }
        let runtime = self.particles.runtime.as_mut().unwrap();
        let generation = self
            .core
            .session
            .game()
            .map_or(0, |g| g.runtime.terrain_generation);
        if self.particles.base != Some((self.scene.floor_base, generation)) {
            runtime.reset();
            self.particles.base = Some((self.scene.floor_base, generation));
        }
        if let Some(options) = self
            .core
            .session
            .game()
            .map(|g| &g.ui_variables.queries.preferences.options)
        {
            runtime.set_detail(options.particle_level);
        }
        // The entity updates made since the last tick are applied.
        if let Some(game) = self.core.session.game() {
            let state = &game.runtime.feed.state;
            let players = state
                .players
                .players
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    p.as_ref()
                        .map(|p| (crate::particle::keys::player(i), &p.actor))
                });
            let npcs = state
                .npcs
                .entities
                .iter()
                .map(|(&i, n)| (crate::particle::keys::npc(i), &n.path.actor));
            for (key, actor) in players.chain(npcs) {
                let seen = self
                    .particles
                    .resets_seen
                    .insert(key, actor.particle_resets);
                if seen.is_some_and(|seen| seen != actor.particle_resets) {
                    runtime.restart(key);
                }
            }
        }
        runtime.tick(i64::from(self.core.cycle));
    }

    /// The scene draw's binds and particle update, run once per drawn frame on
    /// the logic cycle after the owners refreshed their models (dynamic
    /// locs, players, NPCs, spot and projectile effects), then the
    /// GPU particle batches in the frame plan's draw order.
    pub(super) fn update_particles(&mut self) {
        let started = std::time::Instant::now();
        if self.scene.pack_root.is_none() {
            return;
        }
        let Some(scene) = self.scene.graph.as_ref() else {
            return;
        };
        let Some(runtime) = self.particles.runtime.as_mut() else {
            return;
        };
        let cycle = i64::from(self.core.cycle);
        // The scene draw binds drawn entities (actors, dynamic locs,
        // projectiles and spots); culled entities near the eye are handed to
        // the particle update, which keeps an already bound system alive. Other entities do not
        // touch their system. Each live entity maps to (owner key, whether it
        // is the draw that the particle draw follows).
        let owner_of = |source: crate::scene::EntityRef| -> Option<(u64, bool)> {
            Some(match source {
                crate::scene::EntityRef::Scenery(i) => (crate::particle::keys::loc(0, i), true),
                crate::scene::EntityRef::Wall(i) => (crate::particle::keys::loc(1, i), true),
                crate::scene::EntityRef::WallDecor(i) => (crate::particle::keys::loc(2, i), true),
                crate::scene::EntityRef::GroundDecor(i) => (crate::particle::keys::loc(3, i), true),
                crate::scene::EntityRef::Temporary(i) => {
                    let t = scene.temporary.get(i)?;
                    if t.player != usize::MAX {
                        (crate::particle::keys::player(t.player), true)
                    } else if t.location_key.is_some() && !t.transient {
                        // Promoted into its static slot, which draws it.
                        return None;
                    } else {
                        *self.particles.temporary.get(&i)?
                    }
                }
            })
        };
        let plan_owners = |ids: &[usize]| -> Vec<Option<(u64, bool)>> {
            let Some(live) = self.scene.live.as_ref() else {
                return Vec::new();
            };
            ids.iter()
                .map(|&id| live.entities.get(id).and_then(|e| owner_of(e.source)))
                .collect()
        };
        let plan = self.scene.live.as_ref().map(|live| {
            [
                plan_owners(&live.draw.plan.opaque),
                plan_owners(&live.draw.plan.transparent),
            ]
        });
        let drawn: Option<std::collections::HashSet<u64>> = plan.as_ref().map(|lists| {
            lists
                .iter()
                .flatten()
                .flatten()
                .filter(|(_, draws)| *draws)
                .map(|(key, _)| *key)
                .collect()
        });
        let culled: std::collections::HashSet<u64> = self
            .scene
            .live
            .as_ref()
            .map(|live| {
                plan_owners(&live.draw.plan.culled_updates)
                    .into_iter()
                    .flatten()
                    .filter(|(_, draws)| *draws)
                    .map(|(key, _)| key)
                    .collect()
            })
            .unwrap_or_default();
        let mut bindings: std::collections::BTreeMap<u64, crate::particle::Binding> =
            std::collections::BTreeMap::new();
        let mut add = |binding: &crate::particle::Binding| {
            bindings
                .entry(binding.key)
                .and_modify(|b| b.merge(binding))
                .or_insert_with(|| binding.clone());
        };
        // Only dynamic locs own systems (static
        // entities never bind); the rotation is baked into the loc model.
        let locs = scene
            .scenery
            .iter()
            .enumerate()
            .filter(|(_, e)| e.dynamic)
            .map(|(i, e)| (0, i, e.model.as_ref(), e.occlude_level, [e.x, e.y, e.z]))
            .chain(
                scene
                    .walls
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.dynamic)
                    .map(|(i, e)| (1, i, e.model.as_ref(), e.occlude_level, [e.x, e.y, e.z])),
            )
            .chain(
                scene
                    .wall_decors
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.dynamic)
                    .map(|(i, e)| {
                        (
                            2,
                            i,
                            e.model.as_ref(),
                            e.occlude_level,
                            [e.x + e.offset_x, e.y, e.z + e.offset_z],
                        )
                    }),
            )
            .chain(
                scene
                    .ground_decors
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.dynamic)
                    .map(|(i, e)| (3, i, e.model.as_ref(), e.occlude_level, [e.x, e.y, e.z])),
            );
        for (kind, index, model, level, position) in locs {
            let Some(model) = model else {
                continue;
            };
            let mut binding = crate::particle::Binding {
                key: crate::particle::keys::loc(kind, index),
                level,
                ..Default::default()
            };
            binding.add_model(
                model,
                &crate::particle::translation(position.map(|v| v as f32)),
                crate::particle::Rotation::IDENTITY,
                crate::particle::keys::BODY,
            );
            add(&binding);
        }
        if let Some(players) = self.entities.players.as_ref() {
            for entry in players.meshes.values() {
                add(&entry.particles);
            }
        }
        for binding in self.particles.npc_bindings.values() {
            add(binding);
        }
        for binding in &self.particles.effect_bindings {
            add(binding);
        }
        for binding in bindings.values() {
            if drawn
                .as_ref()
                .is_none_or(|drawn| drawn.contains(&binding.key))
            {
                runtime.bind(
                    binding.key,
                    cycle,
                    binding.level,
                    &binding.emitters,
                    &binding.effectors,
                );
            } else if culled.contains(&binding.key) {
                runtime.touch(binding.key, cycle, binding.level);
            }
        }
        // The full redraws before this one (R13 follows the scene cycle increment).
        let frames_before = self.core.scene_cycle.wrapping_sub(1) as u32;
        if crate::debug_flags::flags().particle_trace && frames_before == 20 {
            for b in bindings.values().filter(|b| !b.emitters.is_empty()) {
                let v = b.emitters[0].vertices[0];
                log::info!(
                    "[particles] owner {:#x} level {} emitters {:?} effectors {} at tile {},{} y {}",
                    b.key,
                    b.level,
                    b.emitters.iter().map(|e| e.particle).collect::<Vec<_>>(),
                    b.effectors.len(),
                    (v[0] >> 9) + self.scene.floor_base.0,
                    (v[2] >> 9) + self.scene.floor_base.1,
                    v[1]
                );
            }
        }
        let floors: Vec<Option<&crate::floor::FloorHeights>> = (0..scene.max_level)
            .map(|level| {
                self.scene
                    .floors
                    .get(level)
                    .and_then(Option::as_ref)
                    .map(|f| &f.heights)
            })
            .collect();
        let Some(scene) = self.scene.graph.as_mut() else {
            return;
        };
        let mut adapter = ParticleSceneAdapter {
            scene,
            floors: &floors,
        };
        runtime.collect(&mut adapter);
        let scene = &*adapter.scene;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let camera = self.view.camera.scene_camera(renderer.scene_size());
        let mut local = camera.clone();
        local.target = [0; 3];
        let offset = [
            self.scene.floor_base.0 * 512 - camera.target[0],
            -camera.target[1],
            self.scene.floor_base.1 * 512 - camera.target[2],
        ];
        // The draw lists draw each owner's list right after its
        // entity; owners that are not drawn this frame draw nothing.
        let view = local.view_entries();
        let frame = match plan.as_ref() {
            Some(lists) => {
                let mut frame = crate::particle_render::Frame::default();
                // Every drawn entry of an owner draws its particles.
                for (list, owners) in lists.iter().enumerate() {
                    for (entity, owner) in owners.iter().enumerate() {
                        let Some((key, true)) = *owner else {
                            continue;
                        };
                        self.particles.builder.add_segment(
                            &mut frame,
                            list as u8,
                            entity,
                            runtime.list(key),
                            &view,
                            offset,
                        );
                    }
                }
                frame
            }
            None => {
                let lists: Vec<&[crate::particle::DrawParticle]> =
                    runtime.lists().map(|(_, list)| list).collect();
                self.particles.builder.build(&lists, &view, offset)
            }
        };
        if crate::debug_flags::flags().particle_trace && frames_before.is_multiple_of(50) {
            let stats = runtime.stats();
            log::info!(
                "[particles] cycle {cycle} cpu {:.3} ms detail {} systems {} emitters {} effectors {} counted {} live {} drawn {} batches {} quads {} dropped {}",
                started.elapsed().as_secs_f64() * 1000.0,
                runtime.detail(),
                stats.systems,
                stats.emitters,
                stats.effectors,
                stats.counted,
                stats.live,
                stats.drawn,
                frame.batches.len(),
                frame.vertices.len() / 4,
                frame.dropped,
            );
            let cpu = crate::camera::CpuProjection::new(
                camera.view_entries(),
                camera.projection(),
                [0, 0, camera.viewport.0, camera.viewport.1],
            );
            for (key, list) in runtime.lists() {
                let n = list.len() as i64;
                let screen: Vec<[i32; 2]> = list
                    .iter()
                    .take(3)
                    .map(|p| {
                        let q = cpu.project(std::array::from_fn(|i| {
                            ((p.pos[i] >> 12)
                                + [
                                    self.scene.floor_base.0 * 512,
                                    0,
                                    self.scene.floor_base.1 * 512,
                                ][i]) as f32
                        }));
                        [q[0] as i32, q[1] as i32]
                    })
                    .collect();
                let loc = match key >> 56 {
                    10 => scene
                        .scenery
                        .get((key & 0xFFFF_FFFF) as usize)
                        .map(|e| e.loc_id as i64),
                    _ => None,
                };
                let mut alphas: Vec<u32> = list.iter().map(|p| (p.colour as u32) >> 24).collect();
                alphas.sort_unstable();
                log::info!(
                    "[particles]   system {key:#x} loc {loc:?} screen {screen:?} alpha min/median/max {}/{}/{} textures {:?}",
                    alphas[0],
                    alphas[alphas.len() / 2],
                    alphas[alphas.len() - 1],
                    list.iter().map(|p| p.texture).collect::<std::collections::BTreeSet<_>>(),
                );
                let mean: Vec<i64> = (0..3)
                    .map(|i| list.iter().map(|p| i64::from(p.pos[i] >> 12)).sum::<i64>() / n)
                    .collect();
                log::info!(
                    "[particles]   system {key:#x}: {n} particles, mean scene-local {mean:?} (tile {},{}), first colour {:#010x} size {} texture {} ; camera target {:?} base {:?}",
                    mean[0] >> 9,
                    mean[2] >> 9,
                    list[0].colour,
                    list[0].size >> 12,
                    list[0].texture,
                    camera.target,
                    self.scene.floor_base,
                );
            }
        }
        if let Some(materials) = self.scene.material_store.as_ref() {
            if let Err(error) = renderer.prepare_particles(&self.pack, materials, &frame) {
                crate::logging::warn_repeated!("[client910] particle upload: {error:#}");
            }
        }
    }
}
