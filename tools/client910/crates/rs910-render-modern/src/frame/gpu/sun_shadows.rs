//! The modern renderer's sun shadow resources (renderer plan M3; the maths in
//! [`crate::shadows`], the passes and the cascade cache in
//! `frame::gpu::shadow_cache`): the shadow atlas (one depth texture, the
//! cascades as its 2x2 tiles), the caster pipeline (pass type 0) and the
//! forward pass's receive bindings (the `Shadow` block, the atlas and a
//! `LESS_EQUAL` comparison sampler with clamp-to-edge).
use crate::frame::*;

/// The forward pass's group 2: the `Shadow` block, the atlas and its
/// comparison sampler.
pub(crate) fn shadow_receive_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("modern shadow receive"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}

impl ShadowGpu {
    /// The caster pipeline over the forward module's `vs_shadow`/
    /// `fs_shadow`, the uniform buffers and a 1-texel atlas (shadows off
    /// until a frame asks for a profile).
    pub(crate) fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        frame_layout: &wgpu::BindGroupLayout,
        textures: &crate::models::materials::Textures,
        receive_layout: wgpu::BindGroupLayout,
        vertex_layouts: &[Option<wgpu::VertexBufferLayout<'_>>],
        fill_module: &wgpu::ShaderModule,
    ) -> Self {
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern shadow caster"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CasterUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern shadow uniforms"),
            size: std::mem::size_of::<ShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let casters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern shadow casters"),
            size: crate::shadows::CASTER_SLOT * crate::shadows::MAX_CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let caster_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern shadow caster"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &casters,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CasterUniforms>() as u64),
                }),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern shadow caster"),
            bind_group_layouts: &[
                Some(frame_layout),
                Some(&textures.layout),
                Some(&caster_layout),
                None,
                Some(&textures.arrays.layout),
            ],
            immediate_size: 0,
        });
        // Two-sided casters (see `shadows` "What this crate does"); the
        // bias is the receiver's, per cascade.
        let caster = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern shadow caster"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_shadow"),
                    buffers: vertex_layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_shadow"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::shadows::SHADOW_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern shadow"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let atlas = atlas(device, &receive_layout, &uniforms, &sampler, 1);
        // The caster, the point casters and the sun cache's fill pipelines
        // are created at once (`frame::compile`).
        let (pipeline, (point, sun)) = crate::frame::compile::join(caster, || {
            crate::frame::compile::join(
                || {
                    crate::frame::gpu::point_shadows::PointCasterGpu::new(
                        device,
                        module,
                        frame_layout,
                        &textures.layout,
                        &textures.arrays.layout,
                        vertex_layouts,
                    )
                },
                || crate::frame::gpu::shadow_cache::SunCacheGpu::new(device, fill_module),
            )
        });
        Self {
            settings: ShadowSettings::default(),
            receive_layout,
            uniforms,
            casters,
            caster_bind,
            sampler,
            pipeline,
            atlas,
            point,
            sun,
        }
    }
}

/// An atlas of `size` texels square and the receive bind group over it.
pub(crate) fn atlas(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    size: u32,
) -> (u32, wgpu::TextureView, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("modern shadow atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::shadows::SHADOW_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("modern shadow receive"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (size, view, bind)
}

impl ModernRenderer {
    /// This frame's cascades (M3): the settings' profile fitted to the
    /// camera (the frame's camera-local view and projection, the scene-local
    /// camera `origin`) and the frame's sun (`sun_dir`, towards the sun;
    /// the environment's, turned by map file 6, M5), each cascade's fit the
    /// one its map keeps where the cascade cache allows
    /// (`shadows::cache`), written to the uniform buffers; the atlas resized
    /// when the quality changes. Off: a zero cascade count, which the
    /// forward pass reads as fully lit.
    pub(crate) fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        viewport: (i32, i32),
        origin: [f32; 3],
        sun_dir: [f32; 3],
    ) {
        let settings = self.shadow_settings();
        // The preset of the level.
        let profile = settings.profile();
        let size = profile.map_or(1, |p| p.atlas_size());
        if self.shadow.atlas.0 != size {
            self.shadow.atlas = atlas(
                device,
                &self.shadow.receive_layout,
                &self.shadow.uniforms,
                &self.shadow.sampler,
                size,
            );
            self.shadow.sun.state.forget_tiles();
        }
        self.shadow_frame = profile.map(|profile| {
            let mut camera = snapshot.camera.clone();
            camera.viewport = viewport;
            camera.target = [0; 3];
            let basis = crate::shadows::LightBasis::new(sun_dir);
            let fresh = ShadowFrame::fit(
                &profile,
                &basis,
                &camera.view_entries(),
                &camera.projection(),
                origin,
            );
            let fits = self.shadow.sun.state.choose_fits(
                (profile.quality, sun_dir.map(f32::to_bits)),
                &fresh,
                self.frame,
            );
            ShadowFrame::from_fits(profile, basis, fits, origin)
        });
        let uniforms = self
            .shadow_frame
            .as_ref()
            .map_or_else(ShadowUniforms::default, |f| f.uniforms);
        queue.write_buffer(&self.shadow.uniforms, 0, bytemuck::bytes_of(&uniforms));
        if let Some(frame) = &self.shadow_frame {
            queue.write_buffer(
                &self.shadow.casters,
                0,
                bytemuck::cast_slice(&frame.casters),
            );
        }
    }
}
