//! The modern renderer's sun shadow resources (renderer plan M3; the maths in
//! [`crate::shadows`], the passes and the cascade cache in
//! `frame::gpu::shadow_cache`): the shadow atlas (one depth texture, the
//! cascades as its 2x2 tiles), the caster pipeline (pass type 0) and the
//! forward pass's receive bindings (the `Shadow` block, the atlas and a
//! `LESS_EQUAL` comparison sampler with clamp-to-edge).
use crate::frame::*;
use rs910_gpu_device::desc;

/// The forward pass's group 2: the `Shadow` block, the atlas and its
/// comparison sampler.
pub(crate) fn shadow_receive_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    desc::layout(
        device,
        "modern shadow receive",
        &[
            desc::uniform(0, wgpu::ShaderStages::FRAGMENT),
            desc::texture(
                1,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::TextureSampleType::Depth,
                wgpu::TextureViewDimension::D2,
                false,
            ),
            desc::sampler(
                2,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::SamplerBindingType::Comparison,
            ),
        ],
    )
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
        let caster_layout = desc::layout(
            device,
            "modern shadow caster",
            &[desc::uniform_sized(
                3,
                wgpu::ShaderStages::VERTEX,
                true,
                std::mem::size_of::<CasterUniforms>() as u64,
            )],
        );
        let uniforms = desc::uniform_buffer(
            device,
            "modern shadow uniforms",
            std::mem::size_of::<ShadowUniforms>() as u64,
        );
        let casters = desc::uniform_buffer(
            device,
            "modern shadow casters",
            crate::shadows::CASTER_SLOT * crate::shadows::MAX_CASCADES as u64,
        );
        let caster_bind = desc::bind_group(
            device,
            "modern shadow caster",
            &caster_layout,
            &[(
                3,
                wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &casters,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CasterUniforms>() as u64),
                }),
            )],
        );
        let layout = desc::pipeline_layout(
            device,
            "modern shadow caster",
            &[
                Some(frame_layout),
                Some(&textures.layout),
                Some(&caster_layout),
                None,
                Some(&textures.arrays.layout),
            ],
        );
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
                depth_stencil: Some(desc::depth_state(
                    crate::shadows::SHADOW_FORMAT,
                    true,
                    wgpu::CompareFunction::LessEqual,
                )),
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
    let texture = device.create_texture(&desc::texture_2d(
        "modern shadow atlas",
        [size, size],
        crate::shadows::SHADOW_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    ));
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = desc::bind_group(
        device,
        "modern shadow receive",
        layout,
        &[
            (0, uniforms.as_entire_binding()),
            (1, wgpu::BindingResource::TextureView(&view)),
            (2, wgpu::BindingResource::Sampler(sampler)),
        ],
    );
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
        if self.history.shadow.atlas.0 != size {
            self.history.shadow.atlas = atlas(
                device,
                &self.history.shadow.receive_layout,
                &self.history.shadow.uniforms,
                &self.history.shadow.sampler,
                size,
            );
            self.history.shadow.sun.state.forget_tiles();
        }
        self.frame_resources.shadow_frame = profile.map(|profile| {
            let mut camera = snapshot.camera.clone();
            camera.viewport = viewport;
            camera.target = [0; 3];
            let basis = crate::shadows::LightBasis::new(sun_dir);
            let fresh = SunlightShadows::fit(
                &profile,
                &basis,
                &camera.view_entries(),
                &camera.projection(),
                origin,
            );
            let fits = self.history.shadow.sun.state.choose_fits(
                (profile.quality, sun_dir.map(f32::to_bits)),
                &fresh,
                self.history.frame,
            );
            SunlightShadows::from_fits(profile, basis, fits, origin)
        });
        let uniforms = self
            .frame_resources
            .shadow_frame
            .as_ref()
            .map_or_else(ShadowUniforms::default, |f| f.uniforms);
        queue.write_buffer(
            &self.history.shadow.uniforms,
            0,
            bytemuck::bytes_of(&uniforms),
        );
        if let Some(frame) = &self.frame_resources.shadow_frame {
            queue.write_buffer(
                &self.history.shadow.casters,
                0,
                bytemuck::cast_slice(&frame.casters),
            );
        }
    }
}
