//! Scene-only colour/depth attachments. Resolve multisampling before post
//! processing and copy only the viewport clip, preserving the surrounding UI.
use rs910_gpu_device::desc;

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
        let layout = desc::layout(
            device,
            "scene copy",
            &[desc::float_texture(
                0,
                wgpu::ShaderStages::FRAGMENT,
                false,
                wgpu::TextureViewDimension::D2,
            )],
        );
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
        let pl = desc::pipeline_layout(device, "scene copy", &[Some(&layout)]);
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
                sample_count: samples,
                ..desc::texture_2d(label, [size[0], size[1]], format, usage)
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
        let binding = desc::bind_group(
            device,
            "scene copy",
            &pipelines.layout,
            &[(0, wgpu::BindingResource::TextureView(&view))],
        );
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
        let binding = desc::bind_group(
            device,
            "UI MSAA background",
            &self.pipelines.layout,
            &[(0, wgpu::BindingResource::TextureView(source))],
        );
        let mut pass = desc::colour_pass(
            encoder,
            "seed UI model samples",
            self.attachment(),
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        );
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
        let mut pass = desc::colour_pass(
            encoder,
            "compose scene before UI",
            target,
            wgpu::LoadOp::Load,
        );
        pass.set_pipeline(&self.pipelines.presenter);
        pass.set_bind_group(0, &self.binding, &[]);
        pass.set_scissor_rect(clip[0], clip[1], clip[2], clip[3]);
        pass.draw(0..3, 0..1);
    }
}
