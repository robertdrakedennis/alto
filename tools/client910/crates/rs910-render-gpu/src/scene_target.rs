//! Scene-only colour/depth attachments. Resolve multisampling before post
//! processing and copy only the viewport clip, preserving the surrounding UI.

pub struct Target {
    pub colour: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub multisampled: Option<wgpu::TextureView>,
    pub depth: wgpu::TextureView,
    pub samples: u32,
    pipelines: std::sync::Arc<CopyPipelines>,
    binding: wgpu::BindGroup,
}

/// The scene-copy pipelines for one sample count and colour format
/// ([`crate::pipelines::Variant::SceneCopy`]): `presenter` copies the
/// resolved scene into the frame, `seed_pipeline` seeds every sample from
/// the composed interface.
pub struct CopyPipelines {
    presenter: wgpu::RenderPipeline,
    seed_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl CopyPipelines {
    /// The one set for `(samples, format)` in `cache`.
    pub fn cached(
        cache: &crate::pipelines::PipelineCache,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> std::sync::Arc<Self> {
        cache.get(
            samples,
            format,
            crate::pipelines::Variant::SceneCopy,
            || Self::new(device, format, samples),
        )
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene copy"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(
                r#"
@group(0) @binding(0) var scene: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) id:u32) -> @builtin(position) vec4<f32> {
    let x = f32((id << 1u) & 2u); let y = f32(id & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) position:vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(scene, vec2<i32>(position.xy), 0);
}
"#,
            )),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("scene copy"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        };
        let presenter = device.create_render_pipeline(&descriptor);
        let seed_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            ..descriptor
        });
        Self {
            presenter,
            seed_pipeline,
            layout,
        }
    }
}

impl Target {
    pub fn new(
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let pipelines = std::sync::Arc::new(CopyPipelines::new(device, format, samples));
        Self::with_pipelines(device, size, format, depth_format, samples, pipelines)
    }

    /// [`Target::new`] drawing with shared copy pipelines
    /// (`CopyPipelines::cached`).
    pub fn with_pipelines(
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
        pipelines: std::sync::Arc<CopyPipelines>,
    ) -> Self {
        let make = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let colour = make(
            "scene resolve",
            format,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        );
        let view = colour.create_view(&Default::default());
        let multisampled = (samples > 1).then(|| {
            make(
                "multisampled scene",
                format,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let depth = make(
            "scene depth",
            depth_format,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene copy"),
            layout: &pipelines.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Self {
            colour,
            view,
            multisampled,
            depth,
            samples,
            pipelines,
            binding,
        }
    }
    pub fn matches(&self, size: [u32; 2], samples: u32) -> bool {
        self.colour.width() == size[0] && self.colour.height() == size[1] && self.samples == samples
    }
    pub fn attachment(&self) -> &wgpu::TextureView {
        self.multisampled.as_ref().unwrap_or(&self.view)
    }
    pub fn resolve(&self) -> Option<&wgpu::TextureView> {
        self.multisampled.as_ref().map(|_| &self.view)
    }
    /// Seed every sample from the already composed interface before a 3D
    /// component draw. The subsequent resolve preserves all prior 2D pixels.
    pub fn seed_from(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
    ) {
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI MSAA background"),
            layout: &self.pipelines.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("seed UI model samples"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.attachment(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipelines.seed_pipeline);
        pass.set_bind_group(0, &binding, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn present(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: [u32; 4],
    ) {
        if clip[2] == 0 || clip[3] == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compose scene before UI"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipelines.presenter);
        pass.set_bind_group(0, &self.binding, &[]);
        pass.set_scissor_rect(clip[0], clip[1], clip[2], clip[3]);
        pass.draw(0..3, 0..1);
    }
}
