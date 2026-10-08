//! Floor and model GPU mesh uploads, material batches and draw resources.

use wgpu::util::DeviceExt;

use crate::cache::Pack;

use crate::floor::FloorGeometry;

use crate::texture::{AlphaMode, Material, MaterialStore};

// `FloorUniforms` and `GAME_TO_WORLD` moved to rs910-scene (Phase 3.2).
pub use rs910_scene::floor_uniforms::*;

use super::{
    material_frame_bindings, BatchUniforms, FloorPipeline, FloorTexture, FloorTextureCache,
    FloorVertex, FrameViews,
};

/// What a mesh upload creates buffers and textures through: the device and
/// queue, the floor pipeline, the material texture cache and the cache and
/// material data textures load from.
pub struct MeshUpload<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub pipeline: &'a FloorPipeline,
    pub textures: &'a mut FloorTextureCache,
    pub pack: &'a Pack,
    pub materials: &'a MaterialStore,
}

/// One uploaded batch (a floor batch, or one material range of a model).
pub struct FloorGpuBatch {
    /// The batch's own colour stream (floors); `None` when the mesh shares
    /// one colour stream (models).
    pub(super) colour_buffer: Option<wgpu::Buffer>,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) index_count: u32,
    pub(super) bind_group: wgpu::BindGroup,
    /// Material id.
    pub material: i32,
    pub(super) alpha_test: bool,
    pub(super) floor_batch: Option<usize>,
    pub(super) payload: Option<MaterialPayload>,
}

pub(super) struct MaterialPayload {
    buffer: wgpu::Buffer,
    values: BatchUniforms,
    spec: crate::material::MaterialSpec,
}

/// One uploaded floor level, or one uploaded model (same streams, same
/// shader).
pub struct FloorMesh {
    pub(super) render_id: u64,
    pub(super) vertex_buffer: wgpu::Buffer,
    /// The single colour stream of a model.
    pub(super) shared_colours: Option<wgpu::Buffer>,
    pub(super) batches: Vec<FloorGpuBatch>,
    /// Vertices uploaded.
    pub vertex_count: usize,
    pub(super) model_uniform: Option<(wgpu::Buffer, wgpu::BindGroup, i32)>,
    pub(super) depth_write: bool,
    /// The billboards of an uploaded model: what is drawn after the batches
    /// (`crate::billboard_render`).
    pub billboards: Option<crate::mesh_billboards::MeshBillboards>,
}

/// Only alpha-tested materials supply a nonzero ref. The -1 sentinel chooses
/// the ordinary pipeline, which still rejects alpha zero under the standard
/// blend.
pub(super) fn model_alpha_ref(material: Option<&Material>) -> f32 {
    match material {
        Some(m) if m.alpha == AlphaMode::AlphaTested && m.alpha_threshold != 0 => {
            f32::from(m.alpha_threshold) / 255.0
        }
        _ => -1.0,
    }
}

/// A model batch's material state as [`FloorMesh::from_model`] sets it up:
/// the alpha reference, the reflective-alpha flag and the material spec.
pub(super) fn model_batch_state(
    materials: &MaterialStore,
    material: i16,
    model: &crate::gpumodel::GpuModel,
) -> (f32, bool, crate::material::MaterialSpec) {
    let material_info = if material == -1 {
        None
    } else {
        materials.get(u32::from(material as u16))
    };
    let pre_alpha = crate::frame_profile::FrameProfile::enabled()
        && crate::render_debug_flags::flags().profile_pre_alpha;
    let alpha_ref = if pre_alpha {
        -1.0
    } else {
        model_alpha_ref(material_info)
    };
    let reflective_alpha = !pre_alpha && material_info.is_some_and(|m| matches!(m.effect, 1 | 7));
    let spec = crate::material::MaterialSpec::new(material_info, model.detail & 0x37 != 0, true);
    (alpha_ref, reflective_alpha, spec)
}

pub(super) fn next_render_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Immutable commands for an ordered list of uploaded scene models. Camera
/// and environment uniform buffers remain live. Rebuild whenever the ordered
/// mesh identities change, including a world rebuild or future visible list.
pub struct ModelBundle {
    ids: Vec<u64>,
    pub bundle: wgpu::RenderBundle,
}

impl ModelBundle {
    pub fn matches(&self, meshes: &[&FloorMesh]) -> bool {
        self.ids.len() == meshes.len()
            && self
                .ids
                .iter()
                .zip(meshes)
                .all(|(id, mesh)| *id == mesh.render_id)
    }

    pub fn new(
        device: &wgpu::Device,
        pipeline: &FloorPipeline,
        meshes: &[&FloorMesh],
        format: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> Self {
        let mut encoder =
            device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("ordered scene models"),
                color_formats: &[Some(format)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: depth,
                    depth_read_only: false,
                    stencil_read_only: true,
                }),
                sample_count: pipeline.sample_count,
                multiview: None,
            });
        encoder.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
        let mut current_alpha_test = None;
        let mut transitions = 0;
        let mut cutouts = 0;
        for mesh in meshes {
            if let Some((_, group, _)) = &mesh.model_uniform {
                encoder.set_bind_group(0, group, &[]);
            }
            encoder.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            if let Some(colours) = &mesh.shared_colours {
                encoder.set_vertex_buffer(1, colours.slice(..));
            }
            for batch in &mesh.batches {
                cutouts += usize::from(batch.alpha_test);
                if current_alpha_test != Some((batch.alpha_test, mesh.depth_write)) {
                    transitions += 1;
                    encoder
                        .set_pipeline(pipeline.model_pipeline(batch.alpha_test, mesh.depth_write));
                    current_alpha_test = Some((batch.alpha_test, mesh.depth_write));
                }
                encoder.set_bind_group(1, &batch.bind_group, &[]);
                if let Some(colours) = &batch.colour_buffer {
                    encoder.set_vertex_buffer(1, colours.slice(..));
                }
                encoder.set_index_buffer(batch.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                encoder.draw_indexed(0..batch.index_count, 0, 0..1);
            }
            if mesh.model_uniform.is_some() {
                encoder.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
            }
        }
        if crate::frame_profile::FrameProfile::enabled() {
            log::info!(
                "[perf] bundle meshes={} cutout_batches={} pipeline_transitions={}",
                meshes.len(),
                cutouts,
                transitions
            );
        }
        Self {
            ids: meshes.iter().map(|mesh| mesh.render_id).collect(),
            bundle: encoder.finish(&wgpu::RenderBundleDescriptor {
                label: Some("ordered scene models"),
            }),
        }
    }
}

#[cfg(test)] // test-only helper
pub(super) fn batch_bind_group(
    device: &wgpu::Device,
    pipeline: &FloorPipeline,
    tex: &FloorTexture,
    values: BatchUniforms,
    label: &str,
) -> wgpu::BindGroup {
    batch_bind_group_full(device, pipeline, tex, values, label).0
}

pub(super) fn batch_bind_group_full(
    device: &wgpu::Device,
    pipeline: &FloorPipeline,
    tex: &FloorTexture,
    values: BatchUniforms,
    label: &str,
) -> (wgpu::BindGroup, wgpu::Buffer, BatchUniforms) {
    let batch_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(&values),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.batch_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&tex.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&tex.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: batch_uniform.as_entire_binding(),
            },
        ],
    });
    (group, batch_uniform, values)
}

pub(super) fn index_buffer(device: &wgpu::Device, label: &str, indices: &[u16]) -> wgpu::Buffer {
    // u16 indices padded to a 4-byte multiple.
    let mut idx = indices.to_vec();
    if idx.len() % 2 == 1 {
        idx.push(0);
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(&idx),
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
    })
}

impl FloorMesh {
    /// Enable or disable depth writes for the mesh: character-shadow depth
    /// writes are disabled, including alpha-tested materials; the following
    /// body restores them. (A model uses one model-local shader frame for
    /// the transform, sun, eye and fog; uniform buffers stay stable across
    /// actor movement/animation.)
    pub fn set_depth_write(&mut self, enabled: bool) {
        if self.depth_write != enabled {
            self.depth_write = enabled;
            self.render_id = next_render_id();
        }
    }
    pub fn set_model_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        pipeline: &FloorPipeline,
        values: &FloorUniforms,
    ) {
        if self
            .model_uniform
            .as_ref()
            .is_none_or(|(_, _, id)| *id != pipeline.environment_id)
        {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("actor shader frame"),
                contents: bytemuck::bytes_of(values),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let group = material_frame_bindings(
                device,
                &pipeline.uniform_layout,
                &buffer,
                FrameViews {
                    cube: &pipeline.environment_view,
                    sampler: &pipeline.environment_sampler,
                    noise: &pipeline.noise_view,
                    noise_sampler: &pipeline.noise_sampler,
                    water_normals: &pipeline.water_normal_view,
                },
            );
            self.model_uniform = Some((buffer, group, pipeline.environment_id));
            self.render_id = next_render_id();
        } else {
            queue.write_buffer(
                &self.model_uniform.as_ref().unwrap().0,
                0,
                bytemuck::bytes_of(values),
            );
        }
    }
    /// A model update rewrites the position/normal/colour streams without
    /// changing material bindings. Stable buffers also keep recorded model
    /// bundles reusable.
    pub fn update_model(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        materials: &MaterialStore,
        model: &crate::gpumodel::GpuModel,
    ) -> anyhow::Result<bool> {
        let batches = model.batches();
        if model.unique_count as usize != self.vertex_count
            || batches.len() != self.batches.len()
            || batches
                .iter()
                .zip(&self.batches)
                .any(|(b, g)| i32::from(b.0) != g.material || b.2 as u32 * 3 != g.index_count)
        {
            return Ok(false);
        }
        let positions = model.position_stream();
        let uvs = model.uv_stream();
        let normals = model.normal_stream();
        let verts: Vec<_> = (0..positions.len())
            .map(|i| FloorVertex {
                pos: positions[i],
                uv: uvs[i],
                depth: 0.,
                normal: normals[i],
            })
            .collect();
        if !verts.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));
        }
        crate::mesh_billboards::MeshBillboards::refresh(&mut self.billboards, model, materials);
        let colours = model.colour_stream(materials)?;
        if !colours.is_empty() {
            queue.write_buffer(
                self.shared_colours.as_ref().expect("model colours"),
                0,
                bytemuck::cast_slice(&colours),
            );
        }
        let indices = model.index_stream();
        for (b, g) in batches.iter().zip(&self.batches) {
            let mut idx = indices[b.1 as usize * 3..(b.1 + b.2) as usize * 3].to_vec();
            if !idx.len().is_multiple_of(2) {
                idx.push(0);
            }
            if !idx.is_empty() {
                queue.write_buffer(&g.index_buffer, 0, bytemuck::cast_slice(&idx));
            }
        }
        Ok(true)
    }
    /// Reuse a [`FloorMesh::from_model`] mesh for `model` at `origin_fine`
    /// (programme Phase 6: the per-frame transient models). When `model`
    /// has the mesh's structure (as [`FloorMesh::update_model`]: vertex
    /// count, batch materials and index counts, with no batch that
    /// `from_model` would skip) and the mesh is still as `from_model` left
    /// it (no actor frame, depth writes on), its streams are rewritten and
    /// every batch's uniforms, material spec and alpha test, and the
    /// billboards, are reset to what `from_model(model, origin_fine)`
    /// builds. The buffers, bind groups and render id stay, so the draws
    /// are `from_model`'s with the same contents. Returns false, with the
    /// mesh unchanged, when it does not fit.
    pub fn reuse_for_model(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        materials: &MaterialStore,
        model: &crate::gpumodel::GpuModel,
        origin_fine: [f32; 3],
    ) -> anyhow::Result<bool> {
        let faces = model.draw_face_count;
        if self.model_uniform.is_some()
            || !self.depth_write
            || self.shared_colours.is_none()
            || self.batches.iter().any(|b| b.payload.is_none())
            || model
                .batches()
                .iter()
                .any(|&(_, start, count, _, _)| count <= 0 || start + count > faces)
            || !self.update_model(queue, materials, model)?
        {
            return Ok(false);
        }
        for (batch, &(material, ..)) in self.batches.iter_mut().zip(&model.batches()) {
            let (alpha_ref, reflective_alpha, spec) = model_batch_state(materials, material, model);
            let initial = BatchUniforms::initial(1.0, alpha_ref, reflective_alpha, origin_fine);
            let payload = batch.payload.as_mut().expect("checked above");
            if bytemuck::bytes_of(&payload.values) != bytemuck::bytes_of(&initial) {
                queue.write_buffer(&payload.buffer, 0, bytemuck::bytes_of(&initial));
                payload.values = initial;
            }
            payload.spec = spec;
            batch.alpha_test = alpha_ref >= 0.0;
        }
        self.billboards =
            crate::mesh_billboards::MeshBillboards::from_model(model, materials, origin_fine);
        Ok(true)
    }

    /// Upload a lit model: stream 0 = `(pos, uv, 0, normal)`, one shared
    /// colour stream, one index range per material batch. `origin_fine` is
    /// the entity's world translation in fine units (a loc's rotation/scale
    /// is applied to the model beforehand, [`crate::scene::srt_model`]). The
    /// texture matrix is the identity for models (at t = 0).
    pub fn from_model(
        upload: MeshUpload<'_>,
        model: &crate::gpumodel::GpuModel,
        origin_fine: [f32; 3],
        label: &str,
    ) -> anyhow::Result<Self> {
        let MeshUpload {
            device,
            queue,
            pipeline,
            textures,
            pack,
            materials,
        } = upload;
        let positions = model.position_stream();
        let uvs = model.uv_stream();
        let normals = model.normal_stream();
        let mut verts = Vec::with_capacity(positions.len());
        for i in 0..positions.len() {
            verts.push(FloorVertex {
                pos: positions[i],
                uv: uvs[i],
                depth: 0.0,
                normal: normals[i],
            });
        }
        let pad = [FloorVertex {
            pos: [0.0; 3],
            uv: [0.0; 2],
            depth: 0.0,
            normal: [0.0, -1.0, 0.0],
        }];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(if verts.is_empty() {
                &pad
            } else {
                verts.as_slice()
            }),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let colours: Vec<u32> = model
            .colour_stream(materials)?
            .iter()
            .map(|&c| c as u32)
            .collect();
        let shared_colours = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(if colours.is_empty() {
                &[0_u32][..]
            } else {
                &colours
            }),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let indices = model.index_stream();
        let mut batches = Vec::new();
        for (material, face_start, face_count, _, _) in model.batches() {
            let start = face_start as usize * 3;
            let end = (face_start + face_count) as usize * 3;
            if end <= start || end > indices.len() {
                continue;
            }
            let tex = textures.get_or_load(device, queue, pack, materials, i32::from(material))?;
            let (alpha_ref, reflective_alpha, spec) = model_batch_state(materials, material, model);
            let (bind_group, buffer, values) = batch_bind_group_full(
                device,
                pipeline,
                tex,
                BatchUniforms::initial(1.0, alpha_ref, reflective_alpha, origin_fine),
                label,
            );
            batches.push(FloorGpuBatch {
                colour_buffer: None,
                index_buffer: index_buffer(device, label, &indices[start..end]),
                index_count: (end - start) as u32,
                bind_group,
                material: i32::from(material),
                floor_batch: None,
                payload: Some(MaterialPayload {
                    buffer,
                    values,
                    spec,
                }),
                alpha_test: alpha_ref >= 0.0,
            });
        }
        Ok(Self {
            render_id: next_render_id(),
            model_uniform: None,
            depth_write: true,
            vertex_buffer,
            shared_colours: Some(shared_colours),
            batches,
            vertex_count: positions.len(),
            billboards: crate::mesh_billboards::MeshBillboards::from_model(
                model,
                materials,
                origin_fine,
            ),
        })
    }

    /// Upload a finished floor: stream 0, one colour stream + index list +
    /// bind group per batch. Every tile is included (no visibility mask).
    pub fn build(
        upload: MeshUpload<'_>,
        geometry: &FloorGeometry,
        origin_fine: [f32; 3],
        label: &str,
    ) -> anyhow::Result<Self> {
        let MeshUpload {
            device,
            queue,
            pipeline,
            textures,
            pack,
            materials,
        } = upload;
        let stride = geometry.stride_floats;
        let mut verts = Vec::with_capacity(geometry.vertex_count);
        for i in 0..geometry.vertex_count {
            let f = &geometry.stream0[i * stride..(i + 1) * stride];
            let mut k = 5;
            let depth = if geometry.has_depth {
                k += 1;
                f[5]
            } else {
                0.0
            };
            let normal = if geometry.has_normals {
                [f[k], f[k + 1], f[k + 2]]
            } else {
                [0.0, -1.0, 0.0]
            };
            verts.push(FloorVertex {
                pos: [f[0], f[1], f[2]],
                uv: [f[3], f[4]],
                depth,
                normal,
            });
        }
        let pad = [FloorVertex {
            pos: [0.0; 3],
            uv: [0.0; 2],
            depth: 0.0,
            normal: [0.0, -1.0, 0.0],
        }];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(if verts.is_empty() {
                &pad
            } else {
                verts.as_slice()
            }),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let all_tiles: Vec<usize> = (0..geometry.tiles_x * geometry.tiles_z).collect();
        let mut batches = Vec::with_capacity(geometry.batches.len());
        for (batch_index, batch) in geometry.batches.iter().enumerate() {
            let (indices, _, _) = batch.build_indices(geometry, &all_tiles);
            if indices.is_empty() {
                continue;
            }
            let colours: Vec<u32> = batch.colours.iter().map(|&c| c as u32).collect();
            let colour_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(&colours),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let tex = textures.get_or_load(device, queue, pack, materials, batch.material)?;
            let scale = if batch.scale != 0.0 {
                1.0 / batch.scale
            } else {
                0.0
            };
            let (bind_group, buffer, mut values) = batch_bind_group_full(
                device,
                pipeline,
                tex,
                BatchUniforms::initial(scale, -1.0, false, origin_fine),
                label,
            );
            let water = if geometry.underwater {
                crate::material::FloorWater::Underwater
            } else if geometry.water_detail {
                crate::material::FloorWater::WaterDetail
            } else {
                crate::material::FloorWater::Normal
            };
            let spec = crate::material::MaterialSpec::new(
                materials.get(batch.material as u32),
                geometry.has_normals,
                false,
            )
            .with_floor_water(water);
            if geometry.underwater {
                // The underwater pass sets per-batch water fog.
                let (plane, colour) = if geometry.has_normals {
                    crate::water::underwater_lit_fog(&batch.water_fog)
                } else {
                    crate::water::underwater_unlit_fog(
                        &batch.water_fog,
                        crate::water::UNDERWATER_HEIGHT_BIAS,
                    )
                };
                values.height_fog_plane = plane;
                values.height_fog_colour = colour;
                queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&values));
            }
            batches.push(FloorGpuBatch {
                colour_buffer: Some(colour_buffer),
                index_buffer: index_buffer(device, label, &indices),
                index_count: indices.len() as u32,
                bind_group,
                material: batch.material,
                floor_batch: Some(batch_index),
                payload: Some(MaterialPayload {
                    buffer,
                    values,
                    spec,
                }),
                alpha_test: false,
            });
        }
        Ok(Self {
            render_id: next_render_id(),
            model_uniform: None,
            depth_write: true,
            billboards: None,
            vertex_buffer,
            shared_colours: None,
            batches,
            vertex_count: geometry.vertex_count,
        })
    }

    /// Reuse the immutable vertex/colour streams and upload only the
    /// selected index prefix. Buffers retain full-scene capacity.
    pub fn select_tiles(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        geometry: &FloorGeometry,
        tiles: &[usize],
    ) {
        for batch in &mut self.batches {
            let Some(source) = batch.floor_batch else {
                continue;
            };
            let (mut indices, _, _) = geometry.batches[source].build_indices(geometry, tiles);
            batch.index_count = indices.len() as u32;
            if indices.len() % 2 != 0 {
                indices.push(0);
            }
            if !indices.is_empty() {
                queue.write_buffer(&batch.index_buffer, 0, bytemuck::cast_slice(&indices));
            }
        }
    }

    /// The model's height-fog plane and colour from the toolkit's current
    /// water fog (the underwater pass); updated buffers remain valid in
    /// cached bundles.
    pub fn set_height_fog(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        plane: [f32; 4],
        colour: [f32; 4],
    ) {
        for batch in &mut self.batches {
            if let Some(payload) = batch.payload.as_mut() {
                payload.values.height_fog_plane = plane;
                payload.values.height_fog_colour = colour;
                queue.write_buffer(&payload.buffer, 0, bytemuck::bytes_of(&payload.values));
            }
        }
    }

    pub fn prepare_materials(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        state: &mut crate::material::MaterialState,
        millis: i32,
        lights: &[[f32; 4]; 8],
        count: usize,
    ) {
        if self.vertex_count == 0 {
            return;
        }
        state.begin_3d();
        for batch in &mut self.batches {
            if batch.index_count == 0 {
                continue;
            }
            let Some(payload) = &mut batch.payload else {
                continue;
            };
            let mut values = payload.values;
            let program = state.material(payload.spec);
            values.shader = [
                program as f32,
                state.exponent,
                if payload.spec.lit { 1. } else { 64. },
                if matches!(program, 0..=2) {
                    count as f32
                } else {
                    0.
                },
            ];
            values.scroll = [payload.spec.scroll[0], payload.spec.scroll[1], 0., 0.];
            values.water = if program == 5 {
                crate::material::waterfall_parameters(payload.spec.argument, millis)
            } else if matches!(
                program,
                crate::material::PROGRAM_ENV_WATER | crate::material::PROGRAM_ENV_SEA
            ) {
                // The time fraction of the effect argument.
                [
                    crate::water::time_fraction(millis, payload.spec.argument),
                    0.,
                    0.,
                    0.,
                ]
            } else {
                [0.; 4]
            };
            values.light_pos.copy_from_slice(&lights[..4]);
            values.light_colour.copy_from_slice(&lights[4..]);
            if values != payload.values {
                queue.write_buffer(&payload.buffer, 0, bytemuck::bytes_of(&values));
                payload.values = values;
            }
        }
    }

    /// Batches with geometry.
    #[must_use]
    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }

    /// Each batch's material and the index count it draws (a floor's over
    /// its current tile selection): the structure another backend's draw
    /// list is compared with (renderer plan M1, `CLIENT910_MODERN_CHECK`).
    #[must_use]
    pub fn batch_index_counts(&self) -> Vec<(i32, u32)> {
        self.batches
            .iter()
            .map(|b| (b.material, b.index_count))
            .collect()
    }

    /// Batches per water program: `[EnvMappedWater, EnvMappedSea,
    /// UnderwaterGround, UnderwaterGroundSpecular]` (upload diagnostics).
    #[must_use]
    pub fn water_program_counts(&self) -> [usize; 4] {
        let mut out = [0; 4];
        for batch in &self.batches {
            if let Some(payload) = &batch.payload {
                match payload.spec.program() {
                    crate::material::PROGRAM_ENV_WATER => out[0] += 1,
                    crate::material::PROGRAM_ENV_SEA => out[1] += 1,
                    crate::material::PROGRAM_UNDERWATER_GROUND => out[2] += 1,
                    crate::material::PROGRAM_UNDERWATER_SPECULAR => out[3] += 1,
                    _ => {}
                }
            }
        }
        out
    }

    /// Issue the per-batch draws. The caller has set the pipeline and group
    /// 0.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.draw_batches(pass, None, &mut None, false);
    }

    /// Draw with the model material state.
    pub fn draw_model<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, pipeline: &'a FloorPipeline) {
        self.draw_model_cached(pass, pipeline, &mut None, true);
    }

    /// Preserve the current model pipeline across meshes. Material order,
    /// bindings and draws are unchanged; only redundant state calls vanish.
    pub fn draw_model_cached<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a FloorPipeline,
        current_alpha_test: &mut Option<(bool, bool)>,
        cache: bool,
    ) {
        self.draw_batches(pass, Some(pipeline), current_alpha_test, cache);
    }

    pub(super) fn draw_batches<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: Option<&'a FloorPipeline>,
        current_alpha_test: &mut Option<(bool, bool)>,
        cache: bool,
    ) {
        if let Some((_, group, _)) = &self.model_uniform {
            pass.set_bind_group(0, group, &[]);
        }
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        if let Some(shared) = &self.shared_colours {
            pass.set_vertex_buffer(1, shared.slice(..));
        }
        for batch in &self.batches {
            if batch.index_count == 0 {
                continue;
            }
            if let Some(pipeline) = pipeline {
                if !cache || *current_alpha_test != Some((batch.alpha_test, self.depth_write)) {
                    pass.set_pipeline(pipeline.model_pipeline(batch.alpha_test, self.depth_write));
                    *current_alpha_test = Some((batch.alpha_test, self.depth_write));
                }
            }
            pass.set_bind_group(1, &batch.bind_group, &[]);
            if let Some(colours) = &batch.colour_buffer {
                pass.set_vertex_buffer(1, colours.slice(..));
            }
            pass.set_index_buffer(batch.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..batch.index_count, 0, 0..1);
        }
        if self.model_uniform.is_some() {
            if let Some(pipeline) = pipeline {
                pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
            }
        }
    }
}
