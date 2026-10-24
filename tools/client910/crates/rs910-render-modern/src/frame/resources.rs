//! The NXT renderer's scene resources (renderer plan M1; see the parent
//! module's "Resources"): uploaded meshes, the loc model cache by
//! `EntityKey`, the floors per level with their tile selection, the
//! per-frame arena, and the frame's draw records built from them.
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::frame::*;

/// A loc's mesh in the shared loc pages (`frame::arenas`): where it lives
/// and its batches `(material, first index, count)` relative to it. It is
/// rewritten in place when its model changes (a dynamic loc's pose) and
/// still fits.
pub(crate) struct Mesh {
    pub(crate) alloc: crate::frame::arenas::Alloc,
    pub(crate) batches: Vec<(i32, u32, u32)>,
    /// Its model-space box (`None`: no vertices).
    pub(crate) bounds: Option<crate::models::bounds::Bounds>,
}

/// `s`'s indices padded to an even count (4-byte buffer writes).
pub(crate) fn padded_indices(s: &ModelStreams) -> std::borrow::Cow<'_, [u16]> {
    if s.indices.len().is_multiple_of(2) {
        std::borrow::Cow::Borrowed(&s.indices)
    } else {
        let mut indices = Vec::with_capacity(s.indices.len() + 1);
        indices.extend_from_slice(&s.indices);
        indices.push(0);
        std::borrow::Cow::Owned(indices)
    }
}

/// A loc's cache slot: one per scene entity (its `live.entities` slot and
/// scene-graph source), whatever model the entity holds; the model itself
/// is identified by the full [`EntityKey`] in [`StaticModel::key`].
pub(crate) type LocSlot = (usize, crate::scene::EntityRef);

/// The slot `key` caches under.
pub(crate) fn loc_slot(key: &EntityKey) -> LocSlot {
    (key.id, key.source)
}

/// A cached loc model: the key it was built for (a dynamic loc's model
/// revision, the slot's loc changes), its streams' identity (model address
/// and counts, a guard against a changed model under an unchanged key) and
/// its buffers.
pub(crate) struct StaticModel {
    pub(crate) key: EntityKey,
    pub(crate) fingerprint: (usize, i32, i32),
    /// `None`: the entity draws nothing (no streams). A mesh whose model
    /// lost its streams keeps its buffers with no batches.
    pub(crate) mesh: Option<Mesh>,
    /// The last frame that drew it.
    pub(crate) used: u64,
    pub(crate) fade: crate::streaming::ResidencyFade,
    /// The model's billboards and the scene-local matrix they are placed
    /// with (M9, `crate::sprites::billboards::of_entity`; `None`: none).
    pub(crate) billboards: Option<(crate::mesh_billboards::MeshBillboards, [f32; 16])>,
    /// Where in the world it stands and what it draws, for the next scene
    /// to take it over ([`PreviousScene`]).
    pub(crate) placement: Placement,
}

/// A loc mesh's world placement (its matrix with the scene's base added,
/// as bits) and its model's content ([`model_content`]): a loc of the next
/// scene with both equal draws the same mesh.
pub(crate) type Placement = ([u32; 16], u64);

/// Frames the previous scene's loc meshes wait for the new scene to take
/// them over before they are freed.
pub(crate) const PREVIOUS_SCENE_FRAMES: u64 = 300;

/// The previous scene's loc meshes (see [`ModernRenderer::adopt_previous`]).
/// A scene rebuild as the player walks on overlaps the last one: most of
/// its locs are the same models at the same world places, at new scene
/// slots and a shifted scene-local origin. Their meshes are model-local, so
/// they draw as they are.
#[derive(Default)]
pub(crate) struct PreviousScene {
    pub(crate) meshes: crate::fast_hash::FastMap<Placement, StaticModel>,
    /// The frame the scene changed.
    pub(crate) since: u64,
    /// The new scene's slots looked up and not found (with the model they
    /// were looked up for).
    pub(crate) missed: crate::fast_hash::FastMap<LocSlot, (usize, i32, i32)>,
}

/// A hash of what `model`'s mesh is built from: its geometry, normals,
/// texture coordinates, face colours and materials.
pub(crate) fn model_content(model: &crate::gpumodel::GpuModel) -> u64 {
    use std::hash::Hasher;
    let mut h = crate::fast_hash::FastHasher::default();
    h.write_u64(model.unique_count as u64);
    h.write_u64(model.face_count as u64);
    h.write_u64(model.draw_face_count as u64);
    h.write_u64(model.flags as u64);
    for words in [&model.vx, &model.vy, &model.vz] {
        h.write(bytemuck::cast_slice(words));
    }
    for halves in [
        &model.unique_vertex,
        &model.nx,
        &model.ny,
        &model.nz,
        &model.face_colour,
        &model.face_material,
        &model.idx1,
        &model.idx2,
        &model.idx3,
    ] {
        h.write(bytemuck::cast_slice(halves));
    }
    h.write(bytemuck::cast_slice(&model.face_alpha));
    h.write(bytemuck::cast_slice(&model.u));
    h.write(bytemuck::cast_slice(&model.v));
    h.finish()
}

/// `entity`'s [`Placement`] in `snapshot`'s scene.
pub(crate) fn placement(snapshot: &SceneSnapshot<'_>, entity: &EntityDraw<'_>) -> Placement {
    let mut world = entity.matrix;
    world[12] += snapshot.floor_base[0] as f32 * crate::terrain::GRID_SIZE;
    world[14] += snapshot.floor_base[1] as f32 * crate::terrain::GRID_SIZE;
    (world.map(f32::to_bits), model_content(entity.model))
}

/// A floor level's identity: the retained construction calls (held, so
/// the address cannot be reused while cached) or the stream's address and
/// length.
pub(crate) enum FloorToken {
    Calls(Arc<crate::floor::FloorCalls>),
    Stream(usize, usize),
}

impl FloorToken {
    pub(crate) fn of(g: &crate::floor::FloorGeometry) -> Self {
        match &g.calls {
            Some(calls) => Self::Calls(calls.clone()),
            None => Self::Stream(g.stream0.as_ptr() as usize, g.stream0.len()),
        }
    }
    pub(crate) fn matches(&self, g: &crate::floor::FloorGeometry) -> bool {
        match (self, &g.calls) {
            (Self::Calls(a), Some(b)) => Arc::ptr_eq(a, b),
            (Self::Stream(p, n), None) => {
                *p == g.stream0.as_ptr() as usize && *n == g.stream0.len()
            }
            _ => false,
        }
    }
}

/// One uploaded floor batch.
pub(crate) struct TerrainPatchBatch {
    pub(crate) source: usize,
    pub(crate) material: i32,
    pub(crate) uv_scale: f32,
    pub(crate) colours: wgpu::Buffer,
    pub(crate) indices: wgpu::Buffer,
    pub(crate) count: u32,
}

/// One uploaded floor level.
pub(crate) struct TerrainPatch {
    pub(crate) token: FloorToken,
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) batches: Vec<TerrainPatchBatch>,
    pub(crate) selection: Option<crate::draw::FloorSelection>,
}

/// A per-frame geometry arena (models posed this frame and the sky
/// models): one vertex, colour and index buffer, rewritten each frame.
#[derive(Default)]
pub(crate) struct Arena {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) colours: Vec<u32>,
    pub(crate) indices: Vec<u16>,
    pub(crate) gpu: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, [u64; 3])>,
}

impl Arena {
    pub(crate) fn clear(&mut self) {
        self.vertices.clear();
        self.colours.clear();
        self.indices.clear();
    }
    /// Append `s`; returns `(base vertex, first index)`.
    pub(crate) fn push(&mut self, s: &ModelStreams) -> (i32, u32) {
        let base = self.vertices.len() as i32;
        let first = self.indices.len() as u32;
        self.vertices.extend_from_slice(&s.vertices);
        self.colours.extend_from_slice(&s.colours);
        self.indices.extend_from_slice(&s.indices);
        (base, first)
    }
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        if !self.indices.len().is_multiple_of(2) {
            self.indices.push(0);
        }
        let need = [
            (self.vertices.len() * std::mem::size_of::<Vertex>()).max(64) as u64,
            (self.colours.len() * 4).max(64) as u64,
            (self.indices.len() * 2).max(64) as u64,
        ];
        let grow = self
            .gpu
            .as_ref()
            .is_none_or(|(_, _, _, cap)| (0..3).any(|i| cap[i] < need[i]));
        if grow {
            let cap = need.map(|n| n.next_power_of_two());
            let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: usage | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.gpu = Some((
                buffer("modern arena vertices", cap[0], wgpu::BufferUsages::VERTEX),
                buffer("modern arena colours", cap[1], wgpu::BufferUsages::VERTEX),
                buffer("modern arena indices", cap[2], wgpu::BufferUsages::INDEX),
                cap,
            ));
        }
        let (v, c, i, _) = self.gpu.as_ref().expect("arena buffers");
        if !self.vertices.is_empty() {
            queue.write_buffer(v, 0, bytemuck::cast_slice(&self.vertices));
            queue.write_buffer(c, 0, bytemuck::cast_slice(&self.colours));
            queue.write_buffer(i, 0, bytemuck::cast_slice(&self.indices));
        }
    }
}

struct EntityBatch {
    material: i32,
    first_index: u32,
    count: u32,
    info: crate::models::materials::MaterialInfo,
    slot_bits: u32,
    residency: [f32; 3],
}

/// GPU cache installation remains ordered; these immutable recipes can be
/// assembled on the pool without touching arenas, counters or texture caches.
struct EntityRecipe {
    geometry: Geometry,
    residency: f32,
    batches: Vec<EntityBatch>,
    matrix: [f32; 16],
    pass: Pass,
    casts: bool,
    lights: [f32; 4],
    bounds: Option<crate::models::bounds::Bounds>,
}
impl EntityRecipe {
    /// Append to retained frame storage. Building these small records does not
    /// justify allocating a pair of vectors and dispatching a job per entity.
    fn append(
        &self,
        instances: &mut Vec<Instance>,
        draws: &mut Vec<Draw>,
    ) -> Option<crate::models::bounds::Bounds> {
        for &EntityBatch {
            material,
            first_index,
            count,
            info,
            slot_bits,
            residency: material_residency,
        } in &self.batches
        {
            let mut flags = info.flags | slot_bits;
            if info.alpha_ref == 0.5 && flags & crate::models::materials::FLAG_ALPHA_IS_MASK == 0 {
                flags |= FLAG_TEXTURE_CUTOUT;
            }
            instances.push(Instance {
                model: self.matrix,
                p0: [1.0, 1.0, info.alpha_ref, flags as f32],
                p1: [
                    info.scroll[0],
                    info.scroll[1],
                    info.spec_power,
                    info.spec_strength,
                ],
                p2: self.lights,
                residency: [
                    self.residency,
                    material_residency[0],
                    material_residency[1],
                    material_residency[2],
                ],
            });
            draws.push(Draw {
                geometry: self.geometry,
                material,
                first_index,
                count,
                instance: (instances.len() - 1) as u32,
                pass: self.pass,
                casts: self.casts,
                indirect: None,
                batched: false,
            });
        }
        self.bounds
            .filter(|_| !self.batches.is_empty())
            .map(|bounds| bounds.transformed(&self.matrix))
    }
}

impl ModernRenderer {
    /// Start the loc model cache over when a new scene is installed: a new
    /// map build (level 0's floor identity) or a different set of static
    /// slots. The static loc meshes wait for the new scene to take them over
    /// ([`PreviousScene`]).
    /// The frame's temporaries (NPCs, players, projectiles, spot anims) come
    /// and go after the static slots and do not count; a loc change
    /// invalidates only its slot ([`EntityKey::changes`]).
    pub(crate) fn check_scene(&mut self, snapshot: &SceneSnapshot<'_>) {
        let floor = snapshot
            .floors
            .first()
            .and_then(Option::as_ref)
            .map_or(0, |g| {
                g.calls
                    .as_ref()
                    .map_or(g.stream0.as_ptr() as usize, |c| Arc::as_ptr(c) as usize)
            });
        let floor = snapshot.owned.map_or(floor, |data| data.scene_identity);
        let token = (
            floor,
            snapshot.live_frame().map_or(0, |l| l.static_entity_count),
        );
        if self.scene_resources.scene_token != Some(token) {
            if self.scene_resources.scene_token.is_some() {
                log::info!(
                    "[modern] scene changed: {} cached loc models dropped",
                    self.scene_resources.statics.len()
                );
            }
            self.preparation.prebuilds.reset();
            self.scene_resources.terrain.stream.reset();
            // The static loc meshes wait for the new scene to take them
            // over (`PreviousScene`); the rest go.
            let resources = &mut self.scene_resources;
            let arena = &mut resources.loc_arena;
            let free = |arena: &mut crate::frame::arenas::LocArena, m: &StaticModel| {
                if let Some(mesh) = m.mesh.as_ref() {
                    arena.free(mesh.alloc);
                }
            };
            for m in resources.previous.meshes.values() {
                free(arena, m);
            }
            resources.previous.meshes.clear();
            resources.previous.missed.clear();
            resources.previous.since = self.history.frame;
            for (_, m) in resources.statics.drain() {
                if m.key.revision.is_some() {
                    free(arena, &m);
                } else if let Some(m) = resources.previous.meshes.insert(m.placement, m) {
                    free(arena, &m);
                }
            }
            resources.dynamic_meshes.release_all(arena);
            for mesh in resources.underwater.meshes.drain(..).flatten() {
                if let Some(mesh) = mesh.mesh {
                    arena.free(mesh.alloc);
                }
            }
            self.scene_resources.scene_token = Some(token);
        }
        // The previous scene's meshes the new one did not take over.
        let previous = &mut self.scene_resources.previous;
        if !previous.meshes.is_empty()
            && self.history.frame.saturating_sub(previous.since) > PREVIOUS_SCENE_FRAMES
        {
            for (_, m) in previous.meshes.drain() {
                if let Some(mesh) = m.mesh {
                    self.scene_resources.loc_arena.free(mesh.alloc);
                }
            }
            previous.missed.clear();
        }
    }

    /// Take over the previous scene's mesh of `entity`, a static loc of
    /// the new scene at the same world place with the same model
    /// ([`PreviousScene`]); whether it did.
    pub(crate) fn adopt_previous(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
    ) -> bool {
        let Some(key) = entity.key else {
            return false;
        };
        let previous = &self.scene_resources.previous;
        let slot = loc_slot(&key);
        let fingerprint = crate::frame::prebuild::fingerprint(entity);
        if previous.meshes.is_empty()
            || previous.missed.get(&slot) == Some(&fingerprint)
            || key.revision.is_some()
            || crate::models::rt7::Rt7Cache::is_dynamic(snapshot, entity)
            || self.frame_resources.posing.has(entity)
        {
            return false;
        }
        let placement = placement(snapshot, entity);
        let Some(mut m) = self.scene_resources.previous.meshes.remove(&placement) else {
            self.scene_resources
                .previous
                .missed
                .insert(slot, fingerprint);
            return false;
        };
        m.key = key;
        m.fingerprint = fingerprint;
        m.used = self.history.frame;
        m.billboards = crate::sprites::billboards::of_entity(snapshot, entity);
        if let Some(old) = self.scene_resources.statics.insert(slot, m) {
            if let Some(mesh) = old.mesh {
                self.scene_resources.loc_arena.free(mesh.alloc);
            }
        }
        true
    }

    /// The loc meshes cached (one per scene slot drawn since the scene was
    /// installed) and the GPU buffers created for loc meshes so far.
    #[must_use]
    pub fn loc_mesh_cache(&self) -> (usize, u64) {
        (
            self.scene_resources.statics.len(),
            self.scene_resources.loc_buffers_created,
        )
    }

    /// The cached loc model built for exactly `key` (`None`: none, or one
    /// built for an earlier model of its slot).
    pub(crate) fn static_model(&self, key: &EntityKey) -> Option<&StaticModel> {
        self.scene_resources
            .statics
            .get(&loc_slot(key))
            .filter(|entry| entry.key == *key)
    }

    /// Upload (or re-select) the floor of `level`.
    pub(crate) fn prepare_floor(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        floor: &crate::models::draw_list::FloorDraw<'_>,
    ) {
        let g = floor.geometry;
        if self.scene_resources.floors.len() <= floor.level {
            self.scene_resources
                .floors
                .resize_with(floor.level + 1, || None);
        }
        let stale = self.scene_resources.floors[floor.level]
            .as_ref()
            .is_none_or(|f| !f.token.matches(g));
        if stale {
            // The batches' materials decoded on the threads (`prebuild`).
            self.prefetch_materials(snapshot, g.batches.iter().map(|b| b.material));
            let mut vertices = crate::models::mesh::floor_vertices(g);
            let all: Vec<usize> = (0..g.tiles_x * g.tiles_z).collect();
            let mut batches = Vec::new();
            let mut triangles = Vec::new();
            for (source, batch) in g.batches.iter().enumerate() {
                let (mut indices, _, _) = batch.build_indices(g, &all);
                if indices.is_empty() {
                    continue;
                }
                triangles.extend_from_slice(&indices);
                if !indices.len().is_multiple_of(2) {
                    indices.push(0);
                }
                let colours: Vec<u32> = batch.colours.iter().map(|&c| c as u32).collect();
                self.device_resources.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    snapshot.materials,
                    batch.material,
                );
                batches.push(TerrainPatchBatch {
                    source,
                    material: batch.material,
                    uv_scale: crate::models::mesh::floor_uv_scale(batch.scale),
                    colours: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("modern floor colours"),
                        contents: bytemuck::cast_slice(&colours),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("modern floor indices"),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    }),
                    count: 0,
                });
            }
            // Tangents over every batch's triangles (the batches share the
            // level's vertices; M2 normal maps).
            crate::models::mesh::add_tangents(&mut vertices, &triangles);
            log::info!(
                "[modern] floor l{}: {} vertices, {} batches uploaded (grid normals {})",
                floor.level,
                vertices.len(),
                batches.len(),
                g.calls.is_some()
            );
            let pad = [Vertex::default()];
            self.scene_resources.floors[floor.level] = Some(TerrainPatch {
                token: FloorToken::of(g),
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("modern floor"),
                    contents: bytemuck::cast_slice(if vertices.is_empty() {
                        &pad[..]
                    } else {
                        &vertices
                    }),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                batches,
                selection: None,
            });
        }
        let gpu = self.scene_resources.floors[floor.level]
            .as_mut()
            .expect("floor uploaded");
        if gpu.selection.as_ref() != Some(floor.selection) {
            let tiles = floor.selection.tiles(g.tiles_x, g.tiles_z);
            for batch in &mut gpu.batches {
                let (mut indices, _, _) = g.batches[batch.source].build_indices(g, &tiles);
                batch.count = indices.len() as u32;
                if !indices.len().is_multiple_of(2) {
                    indices.push(0);
                }
                if !indices.is_empty() {
                    queue.write_buffer(&batch.indices, 0, bytemuck::cast_slice(&indices));
                }
            }
            gpu.selection = Some(floor.selection.clone());
        }
    }

    /// The instance record of a draw with `material`'s parameters.
    pub(crate) fn instance(
        &self,
        matrix: [f32; 16],
        material: i32,
        uv_scale: f32,
        extra: u32,
    ) -> Instance {
        let (info, slot_bits) = self
            .device_resources
            .textures
            .get(material)
            .map(|m| (m.info, m.slot_bits))
            .unwrap_or_default();
        let mut flags = info.flags | extra | slot_bits;
        if info.alpha_ref == 0.5 && flags & crate::models::materials::FLAG_ALPHA_IS_MASK == 0 {
            flags |= FLAG_TEXTURE_CUTOUT;
        }
        let residency = self.device_resources.textures.residency(material);
        Instance {
            model: matrix,
            p0: [uv_scale, uv_scale, info.alpha_ref, flags as f32],
            p1: [
                info.scroll[0],
                info.scroll[1],
                info.spec_power,
                info.spec_strength,
            ],
            p2: [0.0; 4],
            residency: [
                crate::streaming::FULLY_RESIDENT,
                residency[0],
                residency[1],
                residency[2],
            ],
        }
    }

    /// Record the draws of `entity` (uploading its model if needed);
    /// returns their camera-local box (`models::bounds`; `None`: no
    /// draws, or no box).
    fn resolve_entity(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
        origin: [f32; 3],
    ) -> Option<EntityRecipe> {
        let materials = snapshot.materials?;
        let matrix = local_matrix(&entity.matrix, origin);
        let pass = if entity.depth_write {
            Pass::Opaque
        } else {
            Pass::NoDepthWrite
        };
        // The sun shadow casters (M3, `shadows` "Settings"): locs with
        // the scenery setting, players and the transient models (NPC
        // bodies, projectiles, spot anims) with the character setting;
        // spot shadows and hint arrows never.
        let settings = self.shadow_settings();
        let casts = self.frame_resources.shadow_frame.is_some()
            && match entity.kind {
                Kind::SpotShadow | Kind::HintArrow => false,
                Kind::Body => settings.characters,
                Kind::Model if entity.key.is_some() => settings.scenery,
                Kind::Model => settings.characters,
            };
        let mut batches = std::mem::take(&mut self.frame_resources.batch_scratch);
        batches.clear();
        let model_bounds;
        let now = self.frame_resources.residency_time;
        let mut residency = crate::streaming::FULLY_RESIDENT;
        let geometry = match entity.key {
            Some(key) => {
                let fingerprint = (
                    entity.model as *const _ as usize,
                    entity.model.unique_count,
                    entity.model.draw_face_count,
                );
                let slot = loc_slot(&key);
                let stale = self
                    .scene_resources
                    .statics
                    .get(&slot)
                    .is_none_or(|s| s.key != key || s.fingerprint != fingerprint)
                    && !self.adopt_previous(snapshot, entity);
                if stale {
                    let streaming = !self.preparation.prebuilds.sync
                        && !crate::models::rt7::Rt7Cache::is_dynamic(snapshot, entity)
                        && snapshot.pack.is_some();
                    if streaming && !self.preparation.prebuilds.ready_for(entity) {
                        return None;
                    }
                    let _upload = if streaming {
                        Some(self.preparation.prebuilds.uploads.begin_installation()?)
                    } else {
                        None
                    };
                    // M10: a static loc's RT7 geometry (`models::rt7`) when it
                    // corresponds to its classic model, else the classic mesh.
                    // (Q-RT7A: a dynamic loc's posed RT7 mesh is checked in this frame.)
                    self.rt7.anim.begin_frame(self.history.frame);
                    // Built on the threads ahead of the draw (`prebuild`),
                    // else here.
                    let streams = match self.take_prebuilt(entity) {
                        Some(streams) => streams.map(Arc::new),
                        // The shared allowance can expire after the readiness
                        // check. Never fall back to a blocking build in that gap.
                        None if streaming => return None,
                        None if self.frame_resources.posing.has(entity) => {
                            self.rt7.stats.not_static += 1;
                            self.posed_streams(snapshot, materials, entity)
                        }
                        None => self
                            .rt7
                            .streams(snapshot, materials, entity)
                            .or_else(|| {
                                crate::models::mesh::model_streams(
                                    entity.model,
                                    materials,
                                    Colour::Classic,
                                )
                            })
                            .map(Arc::new),
                    };
                    let billboards = crate::sprites::billboards::of_entity(snapshot, entity);
                    // The slot's ranges take the new model in place (a
                    // dynamic loc's next pose, a loc change) while it fits.
                    let old = self
                        .scene_resources
                        .statics
                        .remove(&slot)
                        .and_then(|s| s.mesh);
                    let mesh = match (old, streams) {
                        (old, Some(s)) => {
                            let (at, mut batches) =
                                old.map_or((None, Vec::new()), |m| (Some(m.alloc), m.batches));
                            let before = self.scene_resources.loc_arena.buffers_created;
                            let alloc = self.scene_resources.loc_arena.store(device, queue, at, &s);
                            self.scene_resources.loc_buffers_created +=
                                self.scene_resources.loc_arena.buffers_created - before;
                            batches.clear();
                            batches.extend_from_slice(&s.batches);
                            Some(Mesh {
                                alloc,
                                batches,
                                bounds: crate::models::bounds::Bounds::of(&s.vertices),
                            })
                        }
                        (Some(mut mesh), None) => {
                            mesh.batches.clear();
                            Some(mesh)
                        }
                        (None, None) => None,
                    };
                    self.scene_resources.statics.insert(
                        slot,
                        StaticModel {
                            key,
                            fingerprint,
                            mesh,
                            used: self.history.frame,
                            fade: crate::streaming::ResidencyFade::new(now, streaming),
                            billboards,
                            placement: placement(snapshot, entity),
                        },
                    );
                }
                let entry = self
                    .scene_resources
                    .statics
                    .get_mut(&slot)
                    .expect("static model");
                entry.used = self.history.frame;
                residency = entry.fade.weight(now);
                let Some(mesh) = entry.mesh.as_ref() else {
                    self.frame_resources.batch_scratch = batches;
                    return None;
                };
                let first = mesh.alloc.first_index();
                batches.extend(
                    mesh.batches
                        .iter()
                        .map(|&(m, start, count)| (m, start + first, count)),
                );
                model_bounds = mesh.bounds;
                Geometry::Loc {
                    page: mesh.alloc.page,
                    base_vertex: mesh.alloc.vertex as i32,
                }
            }
            None => {
                // An animated model's RT7 mesh posed as its
                // classic model (`models::rt7_anim`), else the classic mesh; posed
                // on the renderer's threads (`frame::posing`).
                let Some(streams) = self.posed_streams(snapshot, materials, entity) else {
                    self.frame_resources.batch_scratch = batches;
                    return None;
                };
                if let Some(owned) = snapshot.owned {
                    let resources = &mut self.scene_resources;
                    let mesh = resources.dynamic_meshes.resolve(
                        &mut resources.loc_arena,
                        crate::frame::dynamic_meshes::MeshInput {
                            device,
                            queue,
                            entity,
                            streams: &streams,
                            revision: &owned.frame_token,
                            frame: self.history.frame,
                        },
                    );
                    model_bounds = mesh.bounds;
                    let first = mesh.alloc.first_index();
                    batches.extend(
                        mesh.batches
                            .iter()
                            .map(|&(material, start, count)| (material, start + first, count)),
                    );
                    Geometry::Dynamic {
                        page: mesh.alloc.page,
                        base_vertex: mesh.alloc.vertex as i32,
                    }
                } else {
                    model_bounds = crate::models::bounds::Bounds::of(&streams.vertices);
                    let (base_vertex, first) = self.frame_resources.arena.push(&streams);
                    batches.extend(
                        streams
                            .batches
                            .iter()
                            .map(|&(material, start, count)| (material, start + first, count)),
                    );
                    Geometry::Arena { base_vertex }
                }
            }
        };
        // The entity's point lights : the classic selection for it, the
        // lights the classic model shader receives (`lighting::point_lights`).
        let lights = match snapshot.live_frame() {
            Some(live) if entity.id < live.entities.len() => {
                crate::lighting::point_lights::model_slots(live.model_lights, entity.id)
            }
            _ => [0.0; 4],
        };
        let mut recipes = Vec::with_capacity(batches.len());
        for &(material, first_index, count) in &batches {
            self.device_resources.textures.ensure(
                device,
                queue,
                snapshot.pack,
                Some(materials),
                material,
            );
            let (info, slot_bits) = self
                .device_resources
                .textures
                .get(material)
                .map(|texture| (texture.info, texture.slot_bits))
                .unwrap_or_default();
            recipes.push(EntityBatch {
                material,
                first_index,
                count,
                info,
                slot_bits,
                residency: self.device_resources.textures.residency(material),
            });
        }
        self.frame_resources.batch_scratch = batches;
        Some(EntityRecipe {
            geometry,
            residency,
            batches: recipes,
            matrix,
            pass,
            casts,
            lights,
            bounds: model_bounds,
        })
    }
    fn append_entity(&mut self, recipe: &EntityRecipe) -> Option<crate::models::bounds::Bounds> {
        recipe.append(
            &mut self.frame_resources.instances,
            &mut self.frame_resources.draws,
        )
    }
    pub(crate) fn prepare_entity(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
        origin: [f32; 3],
    ) -> Option<crate::models::bounds::Bounds> {
        let recipe = self.resolve_entity(device, queue, snapshot, entity, origin)?;
        self.append_entity(&recipe)
    }
    /// Resolve caches and append directly to reusable frame storage in input
    /// order. Expensive model posing remains on the worker pool.
    pub(crate) fn prepare_entities(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        entities: &[EntityDraw<'_>],
        origin: [f32; 3],
    ) -> Vec<(usize, usize, Option<crate::models::bounds::Bounds>)> {
        let mut ranges = Vec::with_capacity(entities.len());
        for entity in entities {
            let start = self.frame_resources.draws.len();
            let recipe = self.resolve_entity(device, queue, snapshot, entity, origin);
            let bounds = recipe
                .as_ref()
                .and_then(|recipe| self.append_entity(recipe));
            ranges.push((start, self.frame_resources.draws.len(), bounds));
        }
        ranges
    }
}
