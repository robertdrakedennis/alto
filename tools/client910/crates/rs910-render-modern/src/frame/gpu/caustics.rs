//! The caustics' GPU half ([`crate::water_body::caustics`]): the ray
//! map, the ray pass (the frame's water draws from straight above, in the
//! caustics-compute variant) and the resolve, before the forward pass; the terrain
//! reads the resolved light (`crate::frame::terrain`, bindings 12 and 13).
//! Nothing is created or drawn unless the caustics are on with the terrain
//! (`crate::water_body::caustics::enabled`).
use crate::frame::*;
use crate::water_body::caustics::{CausticUniforms, COMPUTE_RES, MAP_RES};

/// The ray map, the resolved light and the uniforms (created once, when the
/// terrain module takes the caustics).
pub(crate) struct CausticBuffers {
    pub(crate) rays: wgpu::Buffer,
    pub(crate) light: wgpu::Buffer,
    pub(crate) uniforms: wgpu::Buffer,
    pub(crate) target: wgpu::TextureView,
    pub(crate) resolve: wgpu::ComputePipeline,
    pub(crate) resolve_bind: wgpu::BindGroup,
}

/// The ray pass's pipeline and its group 2 layout (in the water pipelines).
pub(crate) struct CausticPipes {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) group2: wgpu::BindGroupLayout,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct CausticsGpu {
    pub(crate) buffers: Option<CausticBuffers>,
    /// This frame's ray pass bindings (`None`: no ray pass this frame).
    pub(crate) bind: Option<wgpu::BindGroup>,
    /// Frames with the ray pass (the log's cadence).
    pub(crate) frames: u64,
}

impl CausticBuffers {
    pub(crate) fn new(device: &wgpu::Device, module: &wgpu::ShaderModule) -> Self {
        let texels = u64::from(MAP_RES * MAP_RES);
        let rays = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern caustic rays"),
            size: texels * 8,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern caustic light"),
            size: texels * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern caustics"),
            size: std::mem::size_of::<CausticUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let target = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("modern caustic ray raster"),
                size: wgpu::Extent3d {
                    width: COMPUTE_RES,
                    height: COMPUTE_RES,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern caustics resolve"),
            entries: &[
                storage(0, true),
                storage(1, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let resolve = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("modern caustics resolve"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("modern caustics resolve"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            module,
            entry_point: Some("cs_resolve"),
            compilation_options: Default::default(),
            cache: None,
        });
        let resolve_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern caustics resolve"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: rays.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        Self {
            rays,
            light,
            uniforms,
            target,
            resolve,
            resolve_bind,
        }
    }
}

impl CausticPipes {
    /// The ray pass over `water_module` (compiled with
    /// `crate::water_body::caustics::WATER_CAUSTICS_WGSL`), groups 0 and 1 of the forward
    /// pipeline.
    pub(crate) fn new(
        device: &wgpu::Device,
        water_module: &wgpu::ShaderModule,
        g0: &wgpu::BindGroupLayout,
        g1: &wgpu::BindGroupLayout,
        buffers: [Option<wgpu::VertexBufferLayout<'static>>; 4],
    ) -> Self {
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let texture = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        // The water bindings the rays read (its uniforms, the normal maps,
        // their sampler) and the ray map and caustics uniforms.
        let group2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern caustic rays group 2"),
            entries: &[
                entry(10, both, uniform),
                entry(14, fragment, texture),
                entry(15, fragment, texture),
                entry(
                    17,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(
                    19,
                    fragment,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(20, both, uniform),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern caustic rays"),
            bind_group_layouts: &[Some(g0), Some(g1), Some(&group2)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("modern caustic rays"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: water_module,
                entry_point: Some("vs_caustics"),
                buffers: &buffers,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: water_module,
                entry_point: Some("fs_caustics"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, group2 }
    }
}

impl ModernRenderer {
    /// The caustic buffers (the terrain's bind group reads them).
    pub(crate) fn ensure_caustics(&mut self, device: &wgpu::Device) {
        if self.water.caustics.buffers.is_none() {
            self.water.caustics.buffers = Some(self.caustic_buffers(device));
        }
    }

    /// A new set of caustic buffers.
    pub(crate) fn caustic_buffers(&self, device: &wgpu::Device) -> CausticBuffers {
        let module = self
            .shaders
            .get(device, crate::shaders::Module::CausticsResolve);
        CausticBuffers::new(device, &module)
    }

    /// This frame's caustics (after `prepare_water`): the uniforms (off
    /// without water, a terrain or a plane) and the ray pass's bindings.
    pub(crate) fn prepare_caustics(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        origin: [f32; 3],
    ) {
        self.water.caustics.bind = None;
        let Some(buffers) = self.water.caustics.buffers.as_ref() else {
            return;
        };
        let pipes = self.water.pipes.current();
        let rays = pipes.and_then(|p| p.caustics.as_ref());
        let on = self.terrain.active && !self.water.draws.is_empty() && rays.is_some();
        let u = crate::water_body::caustics::uniforms(origin, self.water.plane_y, on);
        queue.write_buffer(&buffers.uniforms, 0, bytemuck::bytes_of(&u));
        let (true, Some(pipes), Some(rays)) = (u.extra[3] > 0.5, pipes, rays) else {
            return;
        };
        let normal = |m: Option<u16>| {
            m.and_then(|m| self.water.normal_maps.get(&m))
                .unwrap_or(&pipes.flat_normal)
        };
        let [na, nb] = self.water.normal_pair;
        let tex = wgpu::BindingResource::TextureView;
        self.water.caustics.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern caustic rays"),
            layout: &rays.group2,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: pipes.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: tex(normal(na)),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: tex(normal(nb.or(na))),
                },
                wgpu::BindGroupEntry {
                    binding: 17,
                    resource: wgpu::BindingResource::Sampler(&pipes.water_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 19,
                    resource: buffers.rays.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 20,
                    resource: buffers.uniforms.as_entire_binding(),
                },
            ],
        }));
        self.water.caustics.frames += 1;
        if self.water.caustics.frames == 1 || self.water.caustics.frames.is_multiple_of(600) {
            log::info!(
                "[modern] caustics: {} water draws into the {MAP_RES}x{MAP_RES} ray map (plane {:?})",
                self.water.draws.len(),
                self.water.plane_y
            );
        }
    }

    /// The ray pass and the resolve (before the forward pass), when
    /// [`Self::prepare_caustics`] set them up.
    pub(crate) fn encode_caustics(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(buffers), Some(bind), Some(rays)) = (
            self.water.caustics.buffers.as_ref(),
            self.water.caustics.bind.as_ref(),
            self.water.pipes.current().and_then(|p| p.caustics.as_ref()),
        ) else {
            return;
        };
        encoder.clear_buffer(&buffers.rays, 0, None);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::CausticRays)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &buffers.target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&rays.pipeline);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(2, bind, &[]);
            for d in &self.water.draws {
                self.draw_water(&mut pass, d);
            }
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::CausticsResolve)),
            timestamp_writes: None,
        });
        pass.set_pipeline(&buffers.resolve);
        pass.set_bind_group(0, &buffers.resolve_bind, &[]);
        pass.dispatch_workgroups(MAP_RES.div_ceil(8), MAP_RES.div_ceil(8), 1);
    }
}
