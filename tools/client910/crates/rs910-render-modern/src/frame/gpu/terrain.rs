//! The NXT terrain's GPU half (renderer plan M10; the CPU half, the data
//! and the WGSL are [`crate::terrain`]): the terrain pipelines (the
//! forward pass, the sun's caster pass, M8's normal/depth pre-pass and
//! M7's water reflection), the scene's texture array and material table,
//! the per-level meshes with their indices over the frame's tile
//! selection, and the hook each pass calls where the floors draw.
//!
//! The terrain replaces the classic floor's non-water batches of every level
//! it covers (their draws are not recorded); the water batches stay the
//! water pass's (M7).
use wgpu::util::DeviceExt;

use crate::frame::*;
use crate::terrain::{TerrainScene, TerrainVertex};

/// Which pass a terrain draw is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerrainPass {
    Forward,
    Shadow,
    Geometry,
    Reflection,
}

/// The terrain's uniform block (`Terrain` of [`crate::terrain::wgsl`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct TerrainUniforms {
    pub(crate) origin: [f32; 4],
    pub(crate) params: [f32; 4],
}

/// One level's buffers.
pub(crate) struct LevelGpu {
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) indices: wgpu::Buffer,
    pub(crate) selection: Option<crate::draw::FloorSelection>,
    pub(crate) count: u32,
}

/// The installed scene's terrain on the GPU.
pub(crate) struct SceneGpu {
    pub(crate) key: crate::terrain::SceneKey,
    pub(crate) cpu: TerrainScene,
    pub(crate) levels: Vec<Option<LevelGpu>>,
    pub(crate) bind: wgpu::BindGroup,
}

pub(crate) struct Pipes {
    pub(crate) layout: wgpu::BindGroupLayout,
    /// The lit pass into the forward target, per sample count.
    pub(crate) forward: crate::frame::pipelines::Variants<u32, wgpu::RenderPipeline>,
    pub(crate) module: std::sync::Arc<wgpu::ShaderModule>,
    pub(crate) forward_layout: wgpu::PipelineLayout,
    pub(crate) shadow: wgpu::RenderPipeline,
    pub(crate) geometry: wgpu::RenderPipeline,
    pub(crate) reflection: wgpu::RenderPipeline,
    pub(crate) sampler: wgpu::Sampler,
    /// The module has the caustics on the bed (bindings 12
    /// and 13, `water_body::caustics`).
    pub(crate) caustics: bool,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct TerrainGpu {
    pub(crate) pipes: Option<Pipes>,
    pub(crate) uniforms: Option<wgpu::Buffer>,
    pub(crate) scene: Option<SceneGpu>,
    /// This frame's draws: `(level, index count)`.
    pub(crate) draws: Vec<(usize, u32)>,
    /// The classic floor batches the terrain replaced this frame.
    pub(crate) replaced: usize,
    /// Whether the terrain draws this frame (switch on, a scene built).
    pub(crate) active: bool,
    /// Frames drawn with the terrain (the check's log cadence).
    pub(crate) frames: u64,
    pub(crate) mismatches: u64,
    pub(crate) rt7_mismatches: u64,
    /// Tests: draw this scene (every level, every tile) instead of the
    /// snapshot's.
    #[cfg(test)]
    pub(crate) test_scene: Option<TerrainScene>,
    pub(crate) test_all: bool,
}

pub(crate) fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Uint16x4, 4 => Float32x4, 5 => Unorm8x4
    ];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TerrainVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &A,
    }
}

impl Pipes {
    pub(crate) fn new(
        device: &wgpu::Device,
        forward: &wgpu::RenderPipeline,
        caster: &wgpu::RenderPipeline,
        samples: u32,
        caustics: bool,
        module: std::sync::Arc<wgpu::ShaderModule>,
    ) -> Self {
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        // The caustics' uniforms and resolved light.
        let caustic_entries = [
            entry(
                12,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(13, wgpu::ShaderStages::FRAGMENT, storage),
        ];
        let mut entries = vec![
            entry(
                8,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                9,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            ),
            entry(
                10,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            ),
            entry(11, wgpu::ShaderStages::FRAGMENT, storage),
        ];
        if caustics {
            entries.extend(caustic_entries);
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern terrain"),
            entries: &entries,
        });
        let g = |i| forward.get_bind_group_layout(i);
        let (g0, g2, g3) = (g(0), g(2), g(3));
        let forward_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern terrain"),
            bind_group_layouts: &[Some(&g0), Some(&layout), Some(&g2), Some(&g3)],
            immediate_size: 0,
        });
        let caster_group = caster.get_bind_group_layout(2);
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern terrain caster"),
            bind_group_layouts: &[Some(&g0), Some(&layout), Some(&caster_group)],
            immediate_size: 0,
        });
        let buffers = [Some(vertex_layout())];
        let depth = |format, write| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let primitive = |front_face, cull_mode| wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face,
            cull_mode,
            ..Default::default()
        };
        let lit = |label, samples, front_face| {
            lit_pipeline(device, &module, &forward_layout, label, samples, front_face)
        };
        let shadow = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern terrain caster"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_terrain_shadow"),
                    buffers: &buffers,
                    compilation_options: Default::default(),
                },
                fragment: None,
                primitive: primitive(wgpu::FrontFace::Ccw, None),
                depth_stencil: Some(depth(crate::shadows::SHADOW_FORMAT, true)),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let geometry = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern terrain geometry"),
                layout: Some(&forward_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_terrain"),
                    buffers: &buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_terrain_geometry"),
                    targets: &[
                        Some(crate::frame::gpu::post::NORMAL_FORMAT.into()),
                        Some(crate::frame::gpu::post::POSITION_FORMAT.into()),
                    ],
                    compilation_options: Default::default(),
                }),
                primitive: primitive(wgpu::FrontFace::Ccw, Some(wgpu::Face::Back)),
                depth_stencil: Some(depth(crate::frame::gpu::post::GEOMETRY_DEPTH, true)),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // The four pipelines are created at once (`frame::compile`).
        let jobs: Vec<crate::frame::compile::Job<'_, wgpu::RenderPipeline>> = vec![
            Box::new(|| lit("modern terrain", samples, wgpu::FrontFace::Ccw)),
            Box::new(|| lit("modern terrain reflection", 1, wgpu::FrontFace::Cw)),
            Box::new(shadow),
            Box::new(geometry),
        ];
        let mut built = crate::frame::compile::all(jobs).into_iter();
        let mut next = || built.next().expect("every pipeline is built");
        let lit_forward = next();
        let mut forward = crate::frame::pipelines::Variants::default();
        forward.select(samples, || lit_forward);
        let (reflection, shadow, geometry) = (next(), next(), next());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern terrain"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            layout,
            forward,
            module,
            forward_layout,
            shadow,
            geometry,
            reflection,
            sampler,
            caustics,
        }
    }
}

impl Pipes {
    /// Draw into a forward target of `samples` (an anti-aliasing change):
    /// the lit pass for that count, built once.
    pub(crate) fn use_samples(&mut self, device: &wgpu::Device, samples: u32) {
        let (module, layout) = (&self.module, &self.forward_layout);
        self.forward.select(samples, || {
            lit_pipeline(
                device,
                module,
                layout,
                "modern terrain",
                samples,
                wgpu::FrontFace::Ccw,
            )
        });
    }
}

/// A lit terrain pipeline (the forward pass, or the water reflection's at
/// one sample with the winding mirrored).
pub(crate) fn lit_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    label: &str,
    samples: u32,
    front_face: wgpu::FrontFace,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_terrain"),
            buffers: &[Some(vertex_layout())],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_terrain"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// The scene's layers as one texture array (a white layer when there are
/// none, so the binding is always valid).
pub(crate) fn upload_layers(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    layers: &[crate::terrain::atlas::LayerLevels],
) -> wgpu::TextureView {
    let size = crate::terrain::atlas::LAYER_SIZE;
    let count = layers.len().max(1) as u32;
    let mips = size.ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("modern terrain layers"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: count,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let white: crate::terrain::atlas::LayerLevels = (0..mips)
        .map(|m| {
            let s = (size >> m).max(1);
            (s, s, vec![255_u8; (s * s * 4) as usize])
        })
        .collect();
    let all = if layers.is_empty() {
        std::slice::from_ref(&white)
    } else {
        layers
    };
    for (layer, levels) in all.iter().enumerate() {
        for (mip, (w, h, px)) in levels.iter().enumerate().take(mips as usize) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(*h),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl ModernRenderer {
    /// The terrain's pipelines, the lit pass at the current sample count
    /// (built the first time: at renderer creation, and on an
    /// anti-aliasing change).
    pub(crate) fn select_terrain_pipes(&mut self, device: &wgpu::Device) {
        let samples = self.samples;
        match self.terrain.pipes.as_mut() {
            Some(pipes) => pipes.use_samples(device, samples),
            None => self.terrain.pipes = Some(self.terrain_pipes(device)),
        }
    }

    /// The terrain's pipelines at the current sample count (a new set).
    pub(crate) fn terrain_pipes(&self, device: &wgpu::Device) -> Pipes {
        Pipes::new(
            device,
            &self.pipes().forward,
            &self.shadow.pipeline,
            self.samples,
            true,
            self.shaders.get(device, crate::shaders::Module::Terrain),
        )
    }

    /// The frame's terrain (before the floor loop): the installed scene's terrain
    /// (built when the scene changes), each level's indices re-selected
    /// when the plan's tile selection changes.
    pub(crate) fn prepare_terrain(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        list: &DrawList<'_>,
        origin: [f32; 3],
    ) {
        if matches!(self.frame, 2 | 5 | 20) || self.frame.is_multiple_of(600) {
            log::info!(
                "[modern] rt7 models: {:?}, built in {:.1} ms",
                self.rt7.stats,
                self.rt7.build_time.as_secs_f64() * 1000.0
            );
        }
        // CLIENT910_MODERN_CHECK (M10): the RT7 entities of the last frame
        // (their meshes are built in this frame's entity loop, after).
        if crate::modern_debug_flags::flags().check {
            let (n, bad) = self.rt7.check(&list.opaque);
            let (m, bad2) = self.rt7.check(&list.transparent);
            if bad + bad2 > 0 {
                self.terrain.rt7_mismatches += 1;
                log::warn!(
                    "[modern] rt7 check: {} of {} RT7 entities differ from their classic faces ({} mismatching frames)",
                    bad + bad2,
                    n + m,
                    self.terrain.rt7_mismatches
                );
            } else if self.frame == 2 || self.frame.is_multiple_of(600) {
                log::info!(
                    "[modern] rt7 check: {} RT7 entities draw their classic faces",
                    n + m
                );
            }
        }
        let t = &mut self.terrain;
        t.draws.clear();
        t.replaced = 0;
        t.active = false;
        // The caustic buffers the terrain's bind group reads.
        self.ensure_caustics(device);
        self.select_terrain_pipes(device);
        let t = &mut self.terrain;
        #[cfg(test)]
        let test_scene = t.test_scene.take();
        #[cfg(not(test))]
        let test_scene: Option<TerrainScene> = None;
        let testing = test_scene.is_some() || t.test_all;
        let key = crate::terrain::SceneKey::of(snapshot);
        let fresh = test_scene.is_some() || t.scene.as_ref().map(|s| &s.key) != Some(&key);
        if fresh {
            t.scene = None;
            let cpu = match test_scene {
                Some(cpu) => {
                    t.test_all = true;
                    cpu
                }
                None => {
                    let (Some(pack), Some(materials)) = (snapshot.pack, snapshot.materials) else {
                        return;
                    };
                    let start = std::time::Instant::now();
                    // The layers' texels on the renderer's threads.
                    let jobs = &self.jobs;
                    let cpu = TerrainScene::build_with(pack, materials, snapshot, |ids| {
                        jobs.map(ids.len(), |i| {
                            crate::terrain::layer_of(pack, materials, ids[i])
                        })
                    });
                    log::info!(
                        "[modern] terrain built in {:.1} ms",
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                    cpu
                }
            };
            log::info!("[modern] terrain: {}", cpu.summary());
            if t.uniforms.is_none() {
                t.uniforms = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern terrain"),
                    size: std::mem::size_of::<TerrainUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let (Some(pipes), Some(uniforms)) = (t.pipes.as_ref(), t.uniforms.as_ref()) else {
                return;
            };
            let layers = upload_layers(device, queue, &cpu.layer_texels);
            let mut table: Vec<[f32; 4]> = cpu.layer_params.clone();
            if table.is_empty() {
                table.push([0.0; 4]);
            }
            let materials_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("modern terrain materials"),
                contents: bytemuck::cast_slice(&table),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&layers),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&pipes.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: materials_buffer.as_entire_binding(),
                },
            ];
            if let (true, Some(b)) = (pipes.caustics, self.water.caustics.buffers.as_ref()) {
                entries.push(wgpu::BindGroupEntry {
                    binding: 12,
                    resource: b.uniforms.as_entire_binding(),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: 13,
                    resource: b.light.as_entire_binding(),
                });
            }
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern terrain"),
                layout: &pipes.layout,
                entries: &entries,
            });
            let levels = cpu
                .levels
                .iter()
                .map(|l| {
                    l.as_ref().filter(|m| !m.indices.is_empty()).map(|m| {
                        let mut padded = m.indices.clone();
                        padded.resize(padded.len().max(4), 0);
                        LevelGpu {
                            vertices: device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("modern terrain vertices"),
                                    contents: bytemuck::cast_slice(&m.vertices),
                                    usage: wgpu::BufferUsages::VERTEX,
                                },
                            ),
                            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("modern terrain indices"),
                                contents: bytemuck::cast_slice(&padded),
                                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                            }),
                            selection: None,
                            count: 0,
                        }
                    })
                })
                .collect();
            t.scene = Some(SceneGpu {
                key,
                cpu,
                levels,
                bind,
            });
        }
        let Some(scene) = t.scene.as_mut() else {
            return;
        };
        if !scene.cpu.usable {
            return;
        }
        t.active = true;
        if let Some(uniforms) = t.uniforms.as_ref() {
            let u = TerrainUniforms {
                origin: [origin[0], origin[1], origin[2], 0.0],
                params: [
                    crate::terrain::GRID_SIZE,
                    if crate::terrain::specular_on() {
                        1.0
                    } else {
                        0.0
                    },
                    0.0,
                    0.0,
                ],
            };
            queue.write_buffer(uniforms, 0, bytemuck::bytes_of(&u));
        }
        if testing {
            // Tests: every level, every tile.
            for (level, gpu) in scene.levels.iter_mut().enumerate() {
                if let (Some(gpu), Some(Some(mesh))) = (gpu.as_mut(), scene.cpu.levels.get(level)) {
                    gpu.count = mesh.indices.len() as u32;
                    queue.write_buffer(&gpu.indices, 0, bytemuck::cast_slice(&mesh.indices));
                    t.draws.push((level, gpu.count));
                }
            }
            return;
        }
        for floor in &list.floors {
            let (Some(Some(gpu)), Some(Some(mesh))) = (
                scene.levels.get_mut(floor.level),
                scene.cpu.levels.get(floor.level),
            ) else {
                continue;
            };
            if gpu.selection.as_ref() != Some(floor.selection) {
                let indices = mesh.select(floor.selection);
                gpu.count = indices.len() as u32;
                if !indices.is_empty() {
                    queue.write_buffer(&gpu.indices, 0, bytemuck::cast_slice(&indices));
                }
                gpu.selection = Some(floor.selection.clone());
            }
            if gpu.count > 0 {
                t.draws.push((floor.level, gpu.count));
            }
        }
        t.frames += 1;
        if crate::modern_debug_flags::flags().check {
            let (ok, report) = scene.cpu.check(list, snapshot.materials);
            if !ok {
                t.mismatches += 1;
                log::warn!(
                    "[modern] terrain check: {report} ({} mismatching frames)",
                    t.mismatches
                );
            } else if t.frames == 1 || t.frames.is_multiple_of(600) {
                log::info!("[modern] terrain check: {report}");
            }
        }
    }

    /// Whether the terrain replaces the classic floor batch of `material` on
    /// `level` this frame (non-water batches of a level the terrain
    /// covers); counts it.
    pub(crate) fn terrain_replaces(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        level: usize,
        material: i32,
    ) -> bool {
        let t = &mut self.terrain;
        if !t.active {
            return false;
        }
        let covered = t
            .scene
            .as_ref()
            .is_some_and(|s| s.cpu.levels.get(level).is_some_and(Option::is_some));
        let water = snapshot
            .materials
            .is_some_and(|m| crate::water_body::is_water_material(m, material));
        if covered && !water {
            t.replaced += 1;
            return true;
        }
        false
    }

    /// Draw this frame's terrain into `pass` (a pass whose groups 0, 2
    /// and 3 are set; the caller re-sets its own pipeline afterwards).
    pub(crate) fn encode_terrain<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, kind: TerrainPass) {
        // The far scene's ground first (`far`; nothing when off).
        self.encode_far(pass, kind);
        let t = &self.terrain;
        let (true, Some(pipes), Some(scene)) = (t.active, t.pipes.as_ref(), t.scene.as_ref())
        else {
            return;
        };
        if t.draws.is_empty() {
            return;
        }
        pass.set_pipeline(match kind {
            TerrainPass::Forward => pipes.forward.current().expect("terrain lit pipeline"),
            TerrainPass::Shadow => &pipes.shadow,
            TerrainPass::Geometry => &pipes.geometry,
            TerrainPass::Reflection => &pipes.reflection,
        });
        pass.set_bind_group(1, &scene.bind, &[]);
        for &(level, count) in &t.draws {
            if let Some(crate::modern_debug_flags::TerrainDebug::Level(only)) =
                crate::modern_debug_flags::flags().terrain
            {
                if level != only {
                    continue;
                }
            }
            let Some(Some(gpu)) = scene.levels.get(level) else {
                continue;
            };
            pass.set_vertex_buffer(0, gpu.vertices.slice(..));
            pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..count, 0, 0..1);
        }
    }

    /// Whether the terrain casts sun shadows this frame (the floors'
    /// scenery setting, M3).
    pub(crate) fn terrain_casts(&self) -> bool {
        self.terrain.active && self.shadow_frame.is_some() && self.shadow_settings().scenery
    }
}
