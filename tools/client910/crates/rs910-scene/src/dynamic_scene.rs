//! Dynamic loc models (animated, multiloc, changed locs): model build, per-draw pose and
//! particle updates.
//! Static scene construction stays independent; this state begins at the first frame.
use crate::{
    animation_assets::AnimationAssets,
    animation_playback::AnimationRandom,
    cache::Pack,
    config::{Loc, LocStore},
    draw::DrawEntity,
    dynamic_loc::{DynamicLoc, LocContext, Pick},
    entities910::varps::Varps,
    floor::{FloorGeometry, SunLighting},
    gpumodel::GpuModel,
    hardshadow::HardShadow,
    scene::{EntityRef, Scene},
    texture::MaterialStore,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// The loc a scene entity was placed as and the loc it currently resolves to
/// (through its varbit or varp).
#[derive(Clone, Copy)]
struct LocPair<'a> {
    base: &'a Loc,
    resolved: Option<&'a Loc>,
}

/// A tile position on a level, in fine units for the shadow it holds.
#[derive(Clone, Copy)]
struct Spot {
    level: usize,
    x: i32,
    z: i32,
}

/// The scene state a dynamic loc refresh writes: the scene graph, the floors
/// (whose hard shadows it updates), the materials and the sun.
pub struct RefreshWorld<'a> {
    pub scene: &'a mut Scene,
    pub floors: &'a mut [Option<FloorGeometry>],
    pub materials: &'a MaterialStore,
    pub sun: &'a SunLighting,
}

/// Which loc an animation request names: level, scene tile, layer, shape and
/// angle. All of them matter when several layers share a tile.
#[derive(Clone, Copy, Debug)]
pub struct LocTarget {
    pub level: i32,
    pub tile: [i32; 2],
    pub layer: i32,
    pub shape: i32,
    pub angle: i32,
}

/// The animation a request starts: the client cycle, the sequence id and the
/// start delay.
#[derive(Clone, Copy, Debug)]
pub struct AnimationStart {
    pub cycle: i32,
    pub sequence: i32,
    pub delay: i32,
}

struct Entry {
    loc: DynamicLoc,
    shadow: Option<HardShadow>,
    shadow_levels: Option<[bool; 4]>,
    pose: Option<(i32, i32, i32, i32, i32, u64, u64)>,
    pub revision: u64,
    returned: bool,
}
pub struct DynamicScene {
    pub pack: Pack,
    pub assets: AnimationAssets,
    pub variables: Varps,
    /// Cutscene variable-domain overrides consulted while `sceneState == 0`:
    /// varp ids, and varbit ids tagged with `1 << 32`.
    pub var_overrides: std::collections::BTreeMap<i64, i32>,
    locs: LocStore,
    bits: crate::scenery_varbits::Inputs,
    bound_bits: BTreeMap<i32, crate::protocol910::varbits::Type>,
    billboards: crate::billboard::BillboardStore,
    emitters: crate::particle::EmitterStore,
    // The placement model cache has 50 live entries. Eviction resets retained
    // render capabilities, which can change later transparency decisions.
    bases: HashMap<(u32, i32, i32), (i32, u64)>,
    base_order: VecDeque<(u32, i32, i32)>,
    base_models: HashMap<(u32, i32, i32, i32), Option<GpuModel>>,
    raw_models: crate::loctype::SharedModelCache,
    next_base_revision: u64,
    entries: Vec<Option<Entry>>,
    rng: AnimationRandom,
    pub dirty_shadows: Vec<HashSet<(usize, usize)>>,
    pub loaded_models: HashSet<u32>,
    model_detail: i32,
    scenery_shadows: i32,
    animation_detail: i32,
    raster_hard_shadows: bool,
}
impl DynamicScene {
    #[cfg(any(test, feature = "test-hooks"))] // test-only constructors
    pub fn new(
        pack: &Pack,
        locs: &LocStore,
        entities: &[DrawEntity],
        seed: u64,
        raw_models: crate::loctype::SharedModelCache,
    ) -> anyhow::Result<Self> {
        Self::new_with_variables(pack, locs, entities, seed, raw_models, None)
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only constructors
    pub fn new_with_variables(
        pack: &Pack,
        locs: &LocStore,
        entities: &[DrawEntity],
        seed: u64,
        raw_models: crate::loctype::SharedModelCache,
        variables: Option<&Varps>,
    ) -> anyhow::Result<Self> {
        Self::new_with_preferences(
            pack,
            locs,
            entities,
            seed,
            raw_models,
            variables,
            &crate::rebuild::BuildPrefs::default(),
        )
    }
    pub fn new_with_preferences(
        pack: &Pack,
        locs: &LocStore,
        entities: &[DrawEntity],
        seed: u64,
        raw_models: crate::loctype::SharedModelCache,
        variables: Option<&Varps>,
        prefs: &crate::rebuild::BuildPrefs,
    ) -> anyhow::Result<Self> {
        let bits = crate::scenery_varbits::load(pack)?;
        let count = bits.definitions.get(&0).map_or(0, BTreeMap::len);
        let mut s = Self {
            pack: pack.clone(),
            assets: AnimationAssets::load(pack)?,
            variables: variables.cloned().unwrap_or_else(|| Varps::new(count)),
            var_overrides: std::collections::BTreeMap::new(),
            locs: locs.clone(),
            bits,
            bound_bits: BTreeMap::new(),
            billboards: crate::billboard::BillboardStore::load(pack)?,
            emitters: crate::particle::EmitterStore::load(pack)?,
            bases: HashMap::new(),
            base_order: VecDeque::new(),
            base_models: HashMap::new(),
            next_base_revision: 0,
            raw_models,
            entries: (0..entities.len()).map(|_| None).collect(),
            rng: AnimationRandom::new(seed),
            dirty_shadows: (0..4).map(|_| HashSet::new()).collect(),
            loaded_models: HashSet::new(),
            model_detail: prefs.model_detail(),
            scenery_shadows: prefs.scenery_shadows,
            animation_detail: prefs.anim_detail,
            raster_hard_shadows: true,
        };
        // The pending traversal is the reverse of construction order (the scene lists prepend).
        for (id, e) in entities.iter().enumerate().rev().filter(|(_, e)| e.dynamic) {
            s.activate_entity(id, e)?;
        }
        Ok(s)
    }

    /// Online drawing borrows the authoritative player variable domain.
    /// Moving ownership for the call avoids a full array copy on each redraw;
    /// both normal return and Result errors restore it to the runtime.
    pub fn with_variables<T>(
        &mut self,
        mut variables: Option<&mut Varps>,
        run: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if let Some(v) = variables.as_deref_mut() {
            std::mem::swap(v, &mut self.variables);
        }
        let result = run(self);
        if let Some(v) = variables {
            std::mem::swap(v, &mut self.variables);
        }
        result
    }
    fn resolve(&mut self, loc: &Loc) -> anyhow::Result<Option<u32>> {
        let value = if loc.multivarbit != -1 {
            if !self.bound_bits.contains_key(&loc.multivarbit) {
                self.bound_bits.insert(
                    loc.multivarbit,
                    self.bits.get(loc.multivarbit, false).map_err(|e| {
                        anyhow::anyhow!("loc {} varbit {}: {e:?}", loc.id, loc.multivarbit)
                    })?,
                );
            }
            let bit = &self.bound_bits[&loc.multivarbit];
            // Only PLAYER varbits are exposed to the multiloc lookup; other domains take its
            // fallback branch.
            if let Some(&value) = self
                .var_overrides
                .get(&(i64::from(loc.multivarbit) | 0x1_0000_0000))
            {
                value
            } else if bit.binding.as_ref().is_some_and(|base| base.domain != 0) {
                -1
            } else {
                self.variables
                    .get_bit(bit)
                    .map_err(|e| anyhow::anyhow!("loc {} variable: {e:?}", loc.id))?
            }
        } else if loc.multivarp != -1 {
            if let Some(&value) = self.var_overrides.get(&i64::from(loc.multivarp)) {
                value
            } else {
                self.variables
                    .get(loc.multivarp)
                    .map_err(|e| anyhow::anyhow!("loc {} varp: {e:?}", loc.id))?
            }
        } else {
            -1
        };
        Ok(u32::try_from(crate::dynamic_loc::morph_id(loc, value)).ok())
    }
    /// The modern scene casts its own GPU shadows. Keep animation and model
    /// state identical while omitting the reference floor-mask raster.
    pub fn set_hard_shadow_raster(&mut self, enabled: bool) {
        if enabled && !self.raster_hard_shadows {
            for entry in self.entries.iter_mut().flatten() {
                entry.loc.shadow_current = false;
            }
        }
        self.raster_hard_shadows = enabled;
    }

    pub fn contains(&self, id: usize) -> bool {
        self.entries.get(id).is_some_and(Option::is_some)
    }
    fn activate_entity(&mut self, id: usize, entity: &DrawEntity) -> anyhow::Result<()> {
        if self.contains(id) {
            return Ok(());
        }
        let loc_id = u32::try_from(entity.loc_id)
            .map_err(|_| anyhow::anyhow!("scene loc id {} is negative", entity.loc_id))?;
        let base = self
            .locs
            .get(loc_id)
            .ok_or_else(|| anyhow::anyhow!("scene loc {} missing", entity.loc_id))?
            .clone();
        let selected = self.resolve(&base)?;
        let resolved = selected.and_then(|id| self.locs.get(id));
        let mut loc = DynamicLoc::default();
        let ctx = LocContext {
            base: &base,
            resolved,
            assets: &self.assets,
            cycle: 0,
            detail: self.animation_detail,
        };
        let pick = Pick {
            preserve: false,
            explicit: -1,
            mode: 1,
            delay: 0,
        };
        loc.select(&ctx, pick, &mut self.rng)?;
        self.entries[id] = Some(Entry {
            loc,
            shadow: None,
            shadow_levels: None,
            pose: None,
            revision: 0,
            returned: false,
        });
        Ok(())
    }
    /// Removing a dynamic entity's loc: the loc's current shadow is taken back out of
    /// the floors, and the entity leaves the scene, so its dynamic loc
    /// stops updating. Returns whether the entity was live.
    pub fn remove_entity(
        &mut self,
        id: usize,
        entity: &DrawEntity,
        floors: &mut [Option<FloorGeometry>],
        sun: &SunLighting,
    ) -> bool {
        let Some(mut entry) = self.entries.get_mut(id).and_then(Option::take) else {
            return false;
        };
        let (level, x, z) = (entity.occlude_level as usize, entity.x, entity.z);
        self.shadow(&mut entry, floors, Spot { level, x, z }, sun, false);
        true
    }
    /// The same entity re-added by a later loc replacement back to its
    /// original loc: a fresh dynamic loc, which builds its shadow on first draw.
    pub fn restore_entity(&mut self, id: usize, entity: &DrawEntity) -> anyhow::Result<()> {
        if id < self.entries.len() {
            self.activate_entity(id, entity)?;
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only constructors
    pub fn animation_id(&self, id: usize) -> i32 {
        self.entries[id]
            .as_ref()
            .map_or(-1, |e| e.loc.animation.node.id())
    }
    pub fn start_animation(
        &mut self,
        id: usize,
        entity: &DrawEntity,
        cycle: i32,
        seq: i32,
        delay: i32,
    ) -> anyhow::Result<()> {
        if !self.contains(id) {
            self.activate_entity(id, entity)?;
        }
        let base = self.locs.get(entity.loc_id as u32).unwrap().clone();
        let selected = self.resolve(&base)?;
        let resolved = selected.and_then(|id| self.locs.get(id));
        let ctx = LocContext {
            base: &base,
            resolved,
            assets: &self.assets,
            cycle,
            detail: 1,
        };
        self.entries[id]
            .as_mut()
            .unwrap()
            .loc
            .start(&ctx, seq, delay, &mut self.rng)
    }
    /// Start a loc animation on every scene owner at a tile/layer/shape.
    /// The shape and angle are part of the loc position key; matching them
    /// avoids animating a different loc when several layers share a tile.
    pub fn start_animation_at(
        &mut self,
        entities: &[DrawEntity],
        target: LocTarget,
        start: AnimationStart,
    ) -> anyhow::Result<usize> {
        let LocTarget {
            level,
            tile: [x, z],
            layer,
            shape,
            angle,
        } = target;
        let AnimationStart {
            cycle,
            sequence,
            delay,
        } = start;
        let mut hits = 0;
        for (id, entity) in entities.iter().enumerate() {
            if entity.level != level
                || entity.kind != layer
                || entity.shape != shape
                || entity.angle != angle
                || (entity.x >> 9) != x
                || (entity.z >> 9) != z
            {
                continue;
            }
            let entity = entity.clone();
            self.start_animation(id, &entity, cycle, sequence, delay)?;
            hits += 1;
        }
        Ok(hits)
    }
    pub fn revision(&self, id: usize) -> u64 {
        self.entries[id].as_ref().map_or(0, |e| e.revision)
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn oracle_state(&self, id: usize) -> Vec<i32> {
        let e = self.entries[id].as_ref().unwrap();
        let l = &e.loc;
        let n = &l.animation.node;
        vec![
            l.selected,
            l.overlay_height,
            l.last_cycle,
            i32::from(l.forced),
            i32::from(l.shadow_current),
            n.id(),
            n.time,
            n.delay,
            n.loops,
            n.frame,
            n.next,
            i32::from(n.finished),
            n.mode,
            i32::from(e.returned),
        ]
    }
    pub fn refresh(
        &mut self,
        id: usize,
        draw: bool,
        cycle: i32,
        entity: &mut DrawEntity,
        world: RefreshWorld<'_>,
    ) -> anyhow::Result<()> {
        if !self.contains(id) {
            return Ok(());
        }
        let base = self.locs.get(entity.loc_id as u32).unwrap().clone();
        let resolved = self
            .resolve(&base)?
            .and_then(|id| self.locs.get(id))
            .cloned();
        // Take temporarily so cache/provider lookups can borrow the whole runtime.
        let mut entry = self.entries[id].take().unwrap();
        let locs = LocPair {
            base: &base,
            resolved: resolved.as_ref(),
        };
        let result = self.refresh_entry(&mut entry, locs, draw, cycle, entity, world);
        self.entries[id] = Some(entry);
        result
    }
    fn shadow(
        &mut self,
        entry: &mut Entry,
        floors: &mut [Option<FloorGeometry>],
        spot: Spot,
        sun: &SunLighting,
        add: bool,
    ) {
        let Spot { level, x, z } = spot;
        let Some(shadow) = entry.shadow.as_ref() else {
            return;
        };
        let Some(top) = floors.get(level).and_then(Option::as_ref) else {
            return;
        };
        let height = top.heights.get_fine_height(x, z);
        for l in 0..=level {
            let Some(floor) = floors[l].as_mut() else {
                continue;
            };
            let dy = height - floor.heights.get_fine_height(x, z);
            let (tx, tz) = crate::hardshadow::shadow_texel(sun, x, dy, z);
            let Some(mask) = floor.hard_shadows.as_mut() else {
                continue;
            };
            if let Some(levels) = &mut entry.shadow_levels {
                if add {
                    levels[l] = mask.test(shadow, tx, tz);
                }
                if !levels[l] {
                    continue;
                }
            }
            if add {
                mask.add(shadow, tx, tz);
            } else {
                mask.sub(shadow, tx, tz);
            }
            // Include the 4-neighbour alpha fringe around changed coverage.
            crate::hardshadow::mark_dirty_blocks(shadow, tx, tz, &mut self.dirty_shadows[l]);
        }
    }
    fn refresh_entry(
        &mut self,
        entry: &mut Entry,
        locs: LocPair<'_>,
        draw: bool,
        cycle: i32,
        entity: &mut DrawEntity,
        world: RefreshWorld<'_>,
    ) -> anyhow::Result<()> {
        let LocPair { base, resolved } = locs;
        let RefreshWorld {
            scene,
            floors,
            materials,
            sun,
        } = world;
        let [x, y, z] = [entity.x, entity.y, entity.z];
        let level = entity.occlude_level as usize;
        entry.returned = false;
        let ctx = LocContext {
            base,
            resolved,
            assets: &self.assets,
            cycle,
            detail: self.animation_detail,
        };
        if !entry.loc.begin(&ctx, self.scenery_shadows, &mut self.rng)? {
            self.shadow(entry, floors, Spot { level, x, z }, sun, false);
            entry.shadow = None;
            entry.shadow_levels = None;
            *model_mut(scene, entity.source) = None;
            entry.pose = None;
            entry.revision += 1;
            return Ok(());
        }
        let loc = resolved.unwrap();
        if self.scenery_shadows == 0 && entry.shadow.is_some() {
            self.shadow(entry, floors, Spot { level, x, z }, sun, false);
            entry.shadow = None;
            entry.shadow_levels = None;
            entry.loc.shadow_current = false;
        }
        let update_shadow =
            base.hardshadow && self.scenery_shadows != 0 && !entry.loc.shadow_current;
        if !draw && !update_shadow {
            entry.loc.selected = loc.id as i32;
            return Ok(());
        }
        if update_shadow {
            self.shadow(entry, floors, Spot { level, x, z }, sun, false);
            entry.loc.shadow_current = false;
        }
        let animated = entry.loc.animation.node.id() != -1;
        let sh = if entity.shape == 11 { 10 } else { entity.shape };
        let angle = entity.angle + if entity.shape == 11 { 4 } else { 0 };
        let floor = &floors[level]
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("dynamic floor {level} missing"))?
            .heights;
        let above = floors
            .get(level + 1)
            .and_then(Option::as_ref)
            .map(|f| &f.heights);
        let fy = floor.get_fine_height(x, z);
        let flags = if draw { 2048 } else { 262144 };
        let source = crate::loctype::ModelSource::new(
            &self.pack,
            materials,
            &self.billboards,
            &self.emitters,
            self.model_detail,
        )
        .with_model_cache(self.raw_models.clone());
        let mut animation_pose = None;
        let mut model_key = None;
        let mut base_revision = 0;
        let shape = if crate::loctype::shape::is_wall_decor(sh) {
            4
        } else {
            sh
        };
        if animated {
            let pose = self
                .assets
                .prepare(&self.pack, &mut entry.loc.animation, angle & 3)?;
            let key = (loc.id, shape, angle);
            let mut requested = pose.flags | flags | if update_shadow { 0x40000 } else { 0 };
            if loc.hillchange == 3 {
                requested |= 7;
            } else {
                if loc.hillchange != 0 || loc.post_yoff != 0 {
                    requested |= 2;
                }
                if loc.post_xoff != 0 {
                    requested |= 1;
                }
                if loc.post_zoff != 0 {
                    requested |= 4;
                }
            }
            if shape == 10 && angle > 3 {
                requested |= 5;
            }
            let previous = self.bases.get(&key).map_or(0, |b| b.0);
            let flags = previous | requested;
            let physical = (loc.id, shape, angle, flags);
            if !self.bases.contains_key(&key) || previous & requested != requested {
                self.base_models.insert(
                    physical,
                    crate::loctype::build_base_model(&source, loc, flags, shape, angle)?,
                );
                if self.base_models[&physical].is_some() {
                    self.next_base_revision += 1;
                    self.bases.insert(key, (flags, self.next_base_revision));
                }
            }
            if let Some((_, revision)) = self.bases.get(&key) {
                base_revision = *revision;
                self.base_order.retain(|k| *k != key);
                self.base_order.push_back(key);
                while self.base_order.len() > 50 {
                    let old = self.base_order.pop_front().unwrap();
                    self.bases.remove(&old);
                }
            }
            model_key = Some(physical);
            animation_pose = Some(pose);
        }
        let n = &entry.loc.animation.node;
        let pose_key = (
            loc.id as i32,
            n.id(),
            n.frame,
            n.time,
            n.next,
            entry.loc.cache_revision,
            base_revision,
        );
        if entry.pose != Some(pose_key) {
            if let Some(pose) = &animation_pose {
                // The base model's copy, finished for this pose, written over
                // last frame's model in place (`GpuModel::copy_from`: the same
                // copy, reusing its allocations; programme Phase 6).
                let slot = model_mut(scene, entity.source);
                match (&self.base_models[&model_key.unwrap()], slot.as_mut()) {
                    (Some(base), Some(model)) => model.copy_from(base),
                    (base, _) => *slot = base.clone(),
                }
                if let Some(m) = slot.as_mut() {
                    crate::loctype::finish_animated_model(
                        m,
                        loc,
                        [shape, angle],
                        pose,
                        crate::gpumodel::TerrainHeights { floor, above },
                        [x, fy, z],
                    );
                }
            } else {
                let request = crate::loctype::ModelRequest {
                    flags,
                    shape: sh,
                    rotation: angle,
                };
                let ground = crate::loctype::LocGround {
                    floor: Some(floor),
                    above,
                };
                *model_mut(scene, entity.source) =
                    crate::loctype::get_dynamic_model(&source, loc, request, ground, [x, fy, z])?;
            }
            entry.pose = Some(pose_key);
            entry.revision += 1;
        }
        self.loaded_models.extend(source.loaded_ids());
        let model = model_mut(scene, entity.source);
        if let Some(m) = model {
            entry.returned = true;
            if update_shadow {
                entry.shadow = self
                    .raster_hard_shadows
                    .then(|| m.hard_shadow(sun))
                    .flatten();
                entry.shadow_levels = if animated { Some([false; 4]) } else { None };
                self.shadow(entry, floors, Spot { level, x, z }, sun, true);
                entry.loc.shadow_current = true;
            }
            entry.loc.overlay_height = m.min_y();
            m.radius();
            if draw {
                entity.transparent = m.has_transparency || m.has_particles;
                entity.bounds = Some([
                    x + m.min_x(),
                    y + m.min_y(),
                    z + m.min_z(),
                    m.max_x() - m.min_x(),
                    m.max_y() - m.min_y(),
                    m.max_z() - m.min_z(),
                ]);
                entity.cylinder = if m.unique_count == 0 {
                    None
                } else {
                    Some([x, y, z, m.min_y(), m.max_y(), m.horizontal_radius()])
                };
                if let (EntityRef::WallDecor(i), Some(c)) = (entity.source, &mut entity.cylinder) {
                    c[0] += scene.wall_decors[i].offset_x;
                    c[2] += scene.wall_decors[i].offset_z;
                }
            }
        } else {
            entry.shadow = None;
            entry.shadow_levels = None;
            entry.loc.overlay_height = 0;
        }
        entity.overlay_height = entry.loc.overlay_height;
        entry.loc.selected = loc.id as i32;
        Ok(())
    }
}
pub fn model_mut(scene: &mut Scene, e: EntityRef) -> &mut Option<GpuModel> {
    match e {
        EntityRef::Scenery(i) => &mut scene.scenery[i].model,
        EntityRef::Wall(i) => &mut scene.walls[i].model,
        EntityRef::WallDecor(i) => &mut scene.wall_decors[i].model,
        EntityRef::GroundDecor(i) => &mut scene.ground_decors[i].model,
        EntityRef::Temporary(i) => &mut scene.temporary[i].model,
    }
}
pub fn model(scene: &Scene, e: EntityRef) -> Option<&GpuModel> {
    match e {
        EntityRef::Scenery(i) => scene.scenery[i].model.as_ref(),
        EntityRef::Wall(i) => scene.walls[i].model.as_ref(),
        EntityRef::WallDecor(i) => scene.wall_decors[i].model.as_ref(),
        EntityRef::GroundDecor(i) => scene.ground_decors[i].model.as_ref(),
        EntityRef::Temporary(i) => scene.temporary[i].model.as_ref(),
    }
}

/// What loc removal and re-placement read from a static scene
/// entity: its loc id, shape, angle, transform x/z, `occludeLevel`
/// and `hasHardShadow`.
#[derive(Clone, Copy, Debug)]
pub struct SlotLoc {
    pub loc_id: u32,
    pub shape: i32,
    pub angle: i32,
    pub x: i32,
    pub z: i32,
    pub occlude_level: usize,
    pub has_hard_shadow: bool,
}
pub fn slot_loc(scene: &Scene, e: EntityRef) -> SlotLoc {
    let (loc_id, shape, angle, x, z, level, shadow) = match e {
        EntityRef::Scenery(i) => {
            let e = &scene.scenery[i];
            (
                e.loc_id,
                e.shape,
                e.angle,
                e.x,
                e.z,
                e.occlude_level,
                e.has_hard_shadow,
            )
        }
        EntityRef::Wall(i) => {
            let e = &scene.walls[i];
            (
                e.loc_id,
                e.shape,
                e.angle,
                e.x,
                e.z,
                e.occlude_level,
                e.has_hard_shadow,
            )
        }
        EntityRef::WallDecor(i) => {
            let e = &scene.wall_decors[i];
            (
                e.loc_id,
                e.shape,
                e.angle,
                e.x,
                e.z,
                e.occlude_level,
                e.has_hard_shadow,
            )
        }
        EntityRef::GroundDecor(i) => {
            let e = &scene.ground_decors[i];
            (
                e.loc_id,
                22,
                e.angle,
                e.x,
                e.z,
                e.occlude_level,
                e.has_hard_shadow,
            )
        }
        EntityRef::Temporary(i) => {
            let t = &scene.temporary[i];
            (
                0,
                0,
                0,
                t.position[0] as i32,
                t.position[2] as i32,
                t.occlude_level,
                false,
            )
        }
    };
    SlotLoc {
        loc_id,
        shape,
        angle,
        x,
        z,
        occlude_level: level.max(0) as usize,
        has_hard_shadow: shadow,
    }
}
