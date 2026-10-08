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
    /// The model's billboards and the scene-local matrix they are placed
    /// with (M9, `crate::sprites::billboards::of_entity`; `None`: none).
    pub(crate) billboards: Option<(crate::mesh_billboards::MeshBillboards, [f32; 16])>,
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
pub(crate) struct FloorBatchGpu {
    pub(crate) source: usize,
    pub(crate) material: i32,
    pub(crate) uv_scale: f32,
    pub(crate) colours: wgpu::Buffer,
    pub(crate) indices: wgpu::Buffer,
    pub(crate) count: u32,
}

/// One uploaded floor level.
pub(crate) struct FloorGpu {
    pub(crate) token: FloorToken,
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) batches: Vec<FloorBatchGpu>,
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

/// GPU cache installation remains ordered; these immutable recipes can be
/// assembled on the pool without touching arenas, counters or texture caches.
struct EntityRecipe {
    geometry: Geometry,
    batches: Vec<(i32, u32, u32, crate::models::materials::MaterialInfo, u32)>,
    matrix: [f32; 16],
    pass: Pass,
    casts: bool,
    lights: [f32; 4],
    bounds: Option<crate::models::bounds::Bounds>,
}
struct EntityRecords {
    instances: Vec<Instance>,
    draws: Vec<Draw>,
    bounds: Option<crate::models::bounds::Bounds>,
}
const ENTITIES_PER_JOB: usize = 16;
impl EntityRecipe {
    fn assemble(&self) -> EntityRecords {
        let mut instances = Vec::with_capacity(self.batches.len());
        let mut draws = Vec::with_capacity(self.batches.len());
        for &(material, first_index, count, info, slot_bits) in &self.batches {
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
            });
        }
        EntityRecords {
            instances,
            draws,
            bounds: self
                .bounds
                .filter(|_| !self.batches.is_empty())
                .map(|bounds| bounds.transformed(&self.matrix)),
        }
    }
}

impl ModernRenderer {
    /// Drop the loc model cache when a new scene is installed: a new map
    /// build (level 0's floor identity) or a different set of static slots.
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
        if self.scene_token != Some(token) {
            if self.scene_token.is_some() {
                log::info!(
                    "[modern] scene changed: {} cached loc models dropped",
                    self.statics.len()
                );
            }
            self.statics.clear();
            self.underwater.meshes.clear();
            self.loc_arena.reset();
            self.scene_token = Some(token);
        }
    }

    /// The loc meshes cached (one per scene slot drawn since the scene was
    /// installed) and the GPU buffers created for loc meshes so far.
    #[must_use]
    pub fn loc_mesh_cache(&self) -> (usize, u64) {
        (self.statics.len(), self.loc_buffers_created)
    }

    /// The cached loc model built for exactly `key` (`None`: none, or one
    /// built for an earlier model of its slot).
    pub(crate) fn static_model(&self, key: &EntityKey) -> Option<&StaticModel> {
        self.statics
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
        if self.floors.len() <= floor.level {
            self.floors.resize_with(floor.level + 1, || None);
        }
        let stale = self.floors[floor.level]
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
                self.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    snapshot.materials,
                    batch.material,
                );
                batches.push(FloorBatchGpu {
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
            self.floors[floor.level] = Some(FloorGpu {
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
        let gpu = self.floors[floor.level].as_mut().expect("floor uploaded");
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
            .textures
            .get(material)
            .map(|m| (m.info, m.slot_bits))
            .unwrap_or_default();
        let mut flags = info.flags | extra | slot_bits;
        if info.alpha_ref == 0.5 && flags & crate::models::materials::FLAG_ALPHA_IS_MASK == 0 {
            flags |= FLAG_TEXTURE_CUTOUT;
        }
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
        let casts = self.shadow_frame.is_some()
            && match entity.kind {
                Kind::SpotShadow | Kind::HintArrow => false,
                Kind::Body => settings.characters,
                Kind::Model if entity.key.is_some() => settings.scenery,
                Kind::Model => settings.characters,
            };
        let mut batches = std::mem::take(&mut self.batch_scratch);
        batches.clear();
        let model_bounds;
        let geometry = match entity.key {
            Some(key) => {
                let fingerprint = (
                    entity.model as *const _ as usize,
                    entity.model.unique_count,
                    entity.model.draw_face_count,
                );
                let slot = loc_slot(&key);
                let stale = self
                    .statics
                    .get(&slot)
                    .is_none_or(|s| s.key != key || s.fingerprint != fingerprint);
                if stale {
                    // M10: a static loc's RT7 geometry (`models::rt7`) when it
                    // corresponds to its classic model, else the classic mesh.
                    // (Q-RT7A: a dynamic loc's posed RT7 mesh is checked in this frame.)
                    self.rt7.anim.begin_frame(self.frame);
                    // Built on the threads ahead of the draw (`prebuild`),
                    // else here.
                    let streams = match self.take_prebuilt(entity) {
                        Some(streams) => streams.map(Arc::new),
                        None if self.posing.has(entity) => {
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
                    let old = self.statics.remove(&slot).and_then(|s| s.mesh);
                    let mesh = match (old, streams) {
                        (old, Some(s)) => {
                            let (at, mut batches) =
                                old.map_or((None, Vec::new()), |m| (Some(m.alloc), m.batches));
                            let before = self.loc_arena.buffers_created;
                            let alloc = self.loc_arena.store(device, queue, at, &s);
                            self.loc_buffers_created += self.loc_arena.buffers_created - before;
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
                    self.statics.insert(
                        slot,
                        StaticModel {
                            key,
                            fingerprint,
                            mesh,
                            used: self.frame,
                            billboards,
                        },
                    );
                }
                let entry = self.statics.get_mut(&slot).expect("static model");
                entry.used = self.frame;
                let Some(mesh) = entry.mesh.as_ref() else {
                    self.batch_scratch = batches;
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
                    self.batch_scratch = batches;
                    return None;
                };
                model_bounds = crate::models::bounds::Bounds::of(&streams.vertices);
                let (base_vertex, first) = self.arena.push(&streams);
                batches.extend(
                    streams
                        .batches
                        .iter()
                        .map(|&(m, start, count)| (m, start + first, count)),
                );
                Geometry::Arena { base_vertex }
            }
        };
        // The entity's point lights (M4): the classic selection for it, the
        // lights the faithful model shader receives (`lighting::point_lights`).
        let lights = match snapshot.live_frame() {
            Some(live) if entity.id < live.entities.len() => {
                crate::lighting::point_lights::model_slots(live.model_lights, entity.id)
            }
            _ => [0.0; 4],
        };
        let mut recipes = Vec::with_capacity(batches.len());
        for &(material, first_index, count) in &batches {
            self.textures
                .ensure(device, queue, snapshot.pack, Some(materials), material);
            let (info, slot_bits) = self
                .textures
                .get(material)
                .map(|texture| (texture.info, texture.slot_bits))
                .unwrap_or_default();
            recipes.push((material, first_index, count, info, slot_bits));
        }
        self.batch_scratch = batches;
        Some(EntityRecipe {
            geometry,
            batches: recipes,
            matrix,
            pass,
            casts,
            lights,
            bounds: model_bounds,
        })
    }
    fn append_entity(&mut self, records: EntityRecords) -> Option<crate::models::bounds::Bounds> {
        let first = self.instances.len() as u32;
        self.instances.extend(records.instances);
        self.draws.extend(records.draws.into_iter().map(|mut draw| {
            draw.instance += first;
            draw
        }));
        records.bounds
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
        self.append_entity(recipe.assemble())
    }
    /// Resolve mutable caches in input order, assemble immutable records on the
    /// pool, then append all draws/instances/bounds in that same order.
    pub(crate) fn prepare_entities(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        entities: &[EntityDraw<'_>],
        origin: [f32; 3],
    ) -> Vec<(usize, usize, Option<crate::models::bounds::Bounds>)> {
        let recipes: Vec<_> = entities
            .iter()
            .map(|entity| self.resolve_entity(device, queue, snapshot, entity, origin))
            .collect();
        let jobs = recipes.len().div_ceil(ENTITIES_PER_JOB);
        let records = self.jobs.map(jobs, |job| {
            let end = ((job + 1) * ENTITIES_PER_JOB).min(recipes.len());
            recipes[job * ENTITIES_PER_JOB..end]
                .iter()
                .map(|recipe| recipe.as_ref().map(EntityRecipe::assemble))
                .collect::<Vec<_>>()
        });
        records
            .into_iter()
            .flatten()
            .map(|records| {
                let start = self.draws.len();
                let bounds = records.and_then(|records| self.append_entity(records));
                (start, self.draws.len(), bounds)
            })
            .collect()
    }
}
