//! The modern renderer's post chain on the GPU (effects and constants in
//! [`crate::post`]): the SSAO passes before the forward pass (the
//! normal/depth pass, the occlusion pass, the X/Y blur; the result is the
//! forward pass's `ssao_map`), and after it the scene luminance and
//! adaptation, the bright pass and Kawase blur, the composite (exposure,
//! bloom, tonemap, grading) and FXAA, in that order.
use crate::frame::compile::Job;
use crate::frame::encoding::EncodeInputs;
use crate::frame::*;
use crate::post::{Effects, PassParams, PostFrame};

/// The geometry pass outputs: the view-space normal and position.
pub(crate) const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const POSITION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
pub(crate) const GEOMETRY_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The ambient occlusion (a single-channel target).
pub(crate) const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// The luminance chain and the adapted luminance.
pub(crate) const LUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// The bright pass and its blur.
pub(crate) const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The graded frame FXAA reads (display values).
pub(crate) const LDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The `Pass` block's dynamic slots (256-byte aligned).
pub(crate) const PASS_SLOT: u64 = 256;

/// Scene-pixel reach of the geometry-aware occlusion blur.
const AO_BLUR_RADIUS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PostKey {
    scene: [u32; 2],
    bloom: [u32; 2],
    ao: [u32; 2],
}
/// Slot layout: 0/1 blur X/Y, 2-4 the luminance levels, 5.. the Kawase
/// iterations.
pub(crate) const SLOT_BLUR_X: u32 = 0;
pub(crate) const SLOT_BLUR_Y: u32 = 1;
pub(crate) const SLOT_LUM: u32 = 2;
pub(crate) const SLOT_KAWASE: u32 = 5;
/// Lane Q-AOENV: the horizon-based occlusion's parameter sets A and B (`post::ao`).
pub(crate) const SLOT_HBAO_A: u32 = SLOT_KAWASE + 16;
pub(crate) const SLOT_HBAO_B: u32 = SLOT_HBAO_A + 1;
/// Lane P4-GPU: the render scale's upscale (`frame::scale`).
pub(crate) const SLOT_UPSCALE: u32 = SLOT_HBAO_B + 1;
pub(crate) const PASS_SLOTS: u32 = SLOT_UPSCALE + 1;

/// A 1x1 `R16Float` texture holding 1.0: no occlusion (the forward pass's
/// `ssao_map` while SSAO is off, and the sky's).
pub(crate) fn white_view(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
) -> wgpu::TextureView {
    let one = 0x3C00u16.to_le_bytes();
    device
        .create_texture_with_data(
            queue.queue(),
            &wgpu::TextureDescriptor {
                label: Some("modern ao white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: AO_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &one,
        )
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// The forward frame group (group 0): the `Frame` block and the ambient
/// occlusion map.
pub(crate) fn frame_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    ao: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("modern frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(ao),
            },
        ],
    })
}

/// The forward frame group's ambient occlusion entry (binding 1).
pub(crate) const AO_ENTRY: wgpu::BindGroupLayoutEntry = wgpu::BindGroupLayoutEntry {
    binding: 1,
    visibility: wgpu::ShaderStages::FRAGMENT,
    ty: wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    },
    count: None,
};

pub(crate) fn texture(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// A render target that later passes read (and tests copy back).
pub(crate) fn target(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let t = texture(
        device,
        label,
        size,
        format,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
    (t, v)
}

pub(crate) fn target_view(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    target(device, label, size, format).1
}

/// One fullscreen post pass: what it draws into and with.
pub(crate) struct PostPass<'a> {
    pub(crate) which: crate::frame::passes::Pass,
    pub(crate) view: &'a wgpu::TextureView,
    pub(crate) pipeline: &'a wgpu::RenderPipeline,
    pub(crate) bind: &'a wgpu::BindGroup,
    /// The pass block's slot the bind group reads.
    pub(crate) slot: u32,
    /// The viewport `[x, y, w, h]` and scissor `[l, t, r, b]` (`None`: the
    /// whole target).
    pub(crate) area: Option<([f32; 4], [u32; 4])>,
    /// Clear the target to this grey first.
    pub(crate) clear: Option<f64>,
}

/// The size-dependent targets and their bind groups.
pub(crate) struct PostTargets {
    /// Frame size and bloom size they were made for.
    pub(crate) key: PostKey,
    pub(crate) normal: wgpu::TextureView,
    pub(crate) position: wgpu::TextureView,
    pub(crate) depth: wgpu::TextureView,
    pub(crate) ao_raw: wgpu::TextureView,
    pub(crate) ao_tmp: wgpu::TextureView,
    pub(crate) ao: wgpu::TextureView,
    /// The bright pass and the Kawase ping-pong.
    pub(crate) bloom: [wgpu::TextureView; 2],
    /// The texture of the views beside it (the tests read it back).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) ao_tex: wgpu::Texture,
    pub(crate) ldr: wgpu::TextureView,
    /// At a render scale (`frame::scale`): FXAA's output, the upscale's
    /// source (made the first frame that needs it).
    pub(crate) ldr_fxaa: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    /// The upscale's bind group over `ldr`.
    pub(crate) upscale_bind: wgpu::BindGroup,
    pub(crate) ssao_bind: wgpu::BindGroup,
    pub(crate) blur_x_bind: wgpu::BindGroup,
    pub(crate) blur_y_bind: wgpu::BindGroup,
    /// Per HDR source (the frame's resolve or its twin, the one the
    /// atmosphere's passes wrote last: `ModernRenderer::hdr_source`).
    pub(crate) lum_log_bind: [wgpu::BindGroup; 2],
    /// Per HDR source, then per adapted slot (the one written this frame).
    pub(crate) bright_bind: [[wgpu::BindGroup; 2]; 2],
    pub(crate) kawase_bind: [wgpu::BindGroup; 2],
    pub(crate) composite_bind: [[wgpu::BindGroup; 2]; 2],
    pub(crate) fxaa_bind: wgpu::BindGroup,
}

/// See the module docs.
pub(crate) struct PostGpu {
    pub(crate) frame_layout: wgpu::BindGroupLayout,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) geometry: wgpu::RenderPipeline,
    pub(crate) ssao: wgpu::RenderPipeline,
    pub(crate) blur: wgpu::RenderPipeline,
    pub(crate) lum_log: wgpu::RenderPipeline,
    pub(crate) lum_down: wgpu::RenderPipeline,
    pub(crate) adapt: wgpu::RenderPipeline,
    pub(crate) bright: wgpu::RenderPipeline,
    pub(crate) kawase: wgpu::RenderPipeline,
    pub(crate) composite: wgpu::RenderPipeline,
    pub(crate) composite_ldr: wgpu::RenderPipeline,
    pub(crate) fxaa: wgpu::RenderPipeline,
    /// At a render scale: FXAA into the scaled frame, and the upscale into
    /// the frame (`frame::scale`).
    pub(crate) fxaa_ldr: wgpu::RenderPipeline,
    pub(crate) upscale: wgpu::RenderPipeline,
    /// Lane Q-AOENV: the horizon-based occlusion (set A replacing the target,
    /// set B blended over it).
    pub(crate) hbao: [wgpu::RenderPipeline; 2],
    pub(crate) frame: wgpu::Buffer,
    /// Occlusion-space viewport and parameters; the post chain keeps scene pixels.
    pub(crate) ao_frame: wgpu::Buffer,
    pub(crate) ao_area: crate::post::ao::Area,
    pub(crate) passes: wgpu::Buffer,
    pub(crate) white: wgpu::TextureView,
    /// The luminance levels (64, 16, 4, 1) and their bind groups (each
    /// level reads the previous; level 0 is `PostTargets::lum_log_bind`).
    pub(crate) lum: [wgpu::TextureView; 4],
    pub(crate) lum_down_bind: [wgpu::BindGroup; 3],
    /// The current and previous adapted luminance: written alternately.
    pub(crate) adapted: [wgpu::TextureView; 2],
    pub(crate) adapt_bind: [wgpu::BindGroup; 2],
    pub(crate) targets: Option<PostTargets>,
    /// The adapted slot the last frame wrote.
    pub(crate) adapted_slot: usize,
    /// The logic time of the last adapted frame (`None`: the next frame
    /// takes its measured luminance as is, as with cleared targets).
    pub(crate) last_ms: Option<i64>,
    /// This frame's effects.
    pub(crate) effects: Effects,
    /// The draws the geometry pass renders: the opaque entities and floors
    /// (`draws[..geometry_draws]` with depth writes).
    pub(crate) geometry_draws: usize,
    /// The clamped scissor of this frame `[l, t, r, b]`.
    pub(crate) clip: [u32; 4],
    /// What the forward frame group binds as its occlusion map: `None` the
    /// white texel, else the targets of this key.
    pub(crate) frame_ao: Option<PostKey>,
    /// This frame's ambient occlusion mode (the settings').
    pub(crate) ao: crate::settings::AoMode,
}

impl PostGpu {
    /// The pipelines (the geometry pass over the forward layout and vertex
    /// streams, the fullscreen passes over one post layout) and the
    /// size-independent targets. `modules`: the geometry pass's and the post
    /// chain's (`shaders::Module::AoGeometry`, `Module::Post`).
    pub(crate) fn new(
        device: &wgpu::Device,
        frame_layout: wgpu::BindGroupLayout,
        forward_layout: &wgpu::PipelineLayout,
        vertex_layouts: &[Option<wgpu::VertexBufferLayout<'_>>],
        output_format: wgpu::TextureFormat,
        white: wgpu::TextureView,
        (geometry_module, module): (
            std::sync::Arc<wgpu::ShaderModule>,
            std::sync::Arc<wgpu::ShaderModule>,
        ),
    ) -> Self {
        let geometry = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern geometry"),
                layout: Some(forward_layout),
                vertex: wgpu::VertexState {
                    module: &geometry_module,
                    entry_point: Some("vs_main"),
                    buffers: vertex_layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &geometry_module,
                    entry_point: Some("fs_geometry"),
                    targets: &[Some(NORMAL_FORMAT.into()), Some(POSITION_FORMAT.into())],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: GEOMETRY_DEPTH,
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

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = |binding, dynamic, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern post"),
            entries: &[
                uniform_entry(0, false, std::mem::size_of::<PostFrame>() as u64),
                uniform_entry(1, true, std::mem::size_of::<PassParams>() as u64),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                uniform_entry(
                    6,
                    false,
                    std::mem::size_of::<crate::post::grading::PostUniforms>() as u64,
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(format.into())],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame = buffer("modern post frame", std::mem::size_of::<PostFrame>() as u64);
        let ao_frame = buffer(
            "modern occlusion frame",
            std::mem::size_of::<PostFrame>() as u64,
        );
        let passes = buffer("modern post passes", PASS_SLOT * u64::from(PASS_SLOTS));
        let [l0, l1, l2, l3] = [
            crate::post::LUMINANCE_SIZE,
            crate::post::LUMINANCE_LEVELS[0],
            crate::post::LUMINANCE_LEVELS[1],
            crate::post::LUMINANCE_LEVELS[2],
        ]
        .map(|s| target(device, "modern luminance", [s, s], LUM_FORMAT));
        let lum = [l0.1, l1.1, l2.1, l3.1];
        let [a0, a1] =
            [0, 1].map(|_| target(device, "modern adapted luminance", [1, 1], LUM_FORMAT));
        let adapted = [a0.1, a1.1];
        let w = &white;
        let lum_down_bind = [0, 1, 2]
            .map(|k| post_bind(device, &layout, &frame, &passes, [&lum[k], w, w, w], None));
        // Slot `s` adapts from the other slot's value into its own.
        let adapt_bind = [0, 1].map(|slot| {
            post_bind(
                device,
                &layout,
                &frame,
                &passes,
                [&lum[3], &adapted[1 - slot], w, w],
                None,
            )
        });
        let blended = |blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("fs_hbao"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_hbao"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: AO_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Every pipeline of the chain, created at once (`frame::compile`).
        let pipeline = &pipeline;
        let blended = &blended;
        let fullscreen: [(&str, wgpu::TextureFormat); 12] = [
            ("fs_ssao", AO_FORMAT),
            ("fs_blur", AO_FORMAT),
            ("fs_lum_log", LUM_FORMAT),
            ("fs_lum_down", LUM_FORMAT),
            ("fs_adapt", LUM_FORMAT),
            ("fs_bright", BLOOM_FORMAT),
            ("fs_kawase", BLOOM_FORMAT),
            ("fs_composite", output_format),
            ("fs_composite", LDR_FORMAT),
            ("fs_fxaa", output_format),
            ("fs_fxaa", LDR_FORMAT),
            ("fs_upscale", output_format),
        ];
        let mut jobs: Vec<Job<'_, wgpu::RenderPipeline>> = vec![
            Box::new(geometry),
            Box::new(move || blended(None)),
            Box::new(move || blended(Some(crate::post::ao::PASS_B_BLEND))),
        ];
        jobs.extend(
            fullscreen.map(|(entry, format)| -> Job<'_, wgpu::RenderPipeline> {
                Box::new(move || pipeline(entry, format))
            }),
        );
        let mut built = crate::frame::compile::all(jobs).into_iter();
        let mut next = || built.next().expect("every pipeline is built");
        let geometry = next();
        let hbao = [next(), next()];
        Self {
            frame_layout,
            geometry,
            hbao,
            ssao: next(),
            blur: next(),
            lum_log: next(),
            lum_down: next(),
            adapt: next(),
            bright: next(),
            kawase: next(),
            composite: next(),
            composite_ldr: next(),
            fxaa: next(),
            fxaa_ldr: next(),
            upscale: next(),
            layout,
            frame,
            ao_frame,
            ao_area: crate::post::ao::Area {
                rect: [0.0; 4],
                clip: [0; 4],
            },
            passes,
            lum_down_bind,
            adapt_bind,
            lum,
            adapted,
            white,
            targets: None,
            adapted_slot: 0,
            last_ms: None,
            effects: Effects::default(),
            geometry_draws: 0,
            clip: [0; 4],
            frame_ao: None,
            ao: crate::settings::AoMode::Hbao,
        }
    }

    /// A post bind group over `textures` (t0..t3) and the grading block.
    pub(crate) fn bind(
        &self,
        device: &wgpu::Device,
        textures: [&wgpu::TextureView; 4],
        grading: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        post_bind(
            device,
            &self.layout,
            &self.frame,
            &self.passes,
            textures,
            grading,
        )
    }

    /// The size-dependent targets for a `size` frame whose bright pass is
    /// `bloom` texels, bound over the frame's two HDR resolves, the grading
    /// LUTs and block, and the forward frame buffer.
    pub(crate) fn ensure_targets(
        &mut self,
        device: &wgpu::Device,
        key: PostKey,
        hdr: [&wgpu::TextureView; 2],
        lut: &wgpu::TextureView,
        grading: &wgpu::Buffer,
    ) {
        if self.targets.as_ref().is_some_and(|t| t.key == key) {
            return;
        }
        let PostKey {
            scene: size,
            bloom,
            ao: ao_size,
        } = key;
        let normal = target_view(device, "modern geometry normal", ao_size, NORMAL_FORMAT);
        let position = target_view(device, "modern geometry position", ao_size, POSITION_FORMAT);
        let depth = texture(
            device,
            "modern geometry depth",
            ao_size,
            GEOMETRY_DEPTH,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let ao_tmp = target_view(device, "modern ssao blur", ao_size, AO_FORMAT);
        let (ao_tex, ao) = target(device, "modern ssao map", ao_size, AO_FORMAT);
        // The occlusion before its blur shares the map's texture: the blur's
        // first pass reads it into `ao_tmp` before the second writes the
        // map (`frame::passes::ALIASES`).
        let ao_raw = ao_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bloom_views = [0, 1].map(|_| target(device, "modern bloom", bloom, BLOOM_FORMAT).1);
        let ldr = target_view(device, "modern post ldr", size, LDR_FORMAT);
        let w = &self.white;
        let last = crate::post::BLOOM_KERNEL.len() % 2;
        let targets = PostTargets {
            key,
            ssao_bind: post_bind(
                device,
                &self.layout,
                &self.ao_frame,
                &self.passes,
                [&normal, &position, w, w],
                None,
            ),
            blur_x_bind: post_bind(
                device,
                &self.layout,
                &self.ao_frame,
                &self.passes,
                [&ao_raw, &normal, &position, w],
                None,
            ),
            blur_y_bind: post_bind(
                device,
                &self.layout,
                &self.ao_frame,
                &self.passes,
                [&ao_tmp, &normal, &position, w],
                None,
            ),
            lum_log_bind: hdr.map(|hdr| self.bind(device, [hdr, w, w, w], None)),
            bright_bind: hdr.map(|hdr| {
                [0, 1].map(|slot| self.bind(device, [hdr, &self.adapted[slot], w, w], None))
            }),
            kawase_bind: [0, 1].map(|k| self.bind(device, [&bloom_views[k], w, w, w], None)),
            composite_bind: hdr.map(|hdr| {
                [0, 1].map(|slot| {
                    self.bind(
                        device,
                        [hdr, &bloom_views[last], &self.adapted[slot], lut],
                        Some(grading),
                    )
                })
            }),
            fxaa_bind: self.bind(device, [&ldr, w, w, w], Some(grading)),
            upscale_bind: self.bind(device, [&ldr, w, w, w], Some(grading)),
            ldr_fxaa: None,
            normal,
            position,
            depth,
            ao_raw,
            ao_tmp,
            ao,
            bloom: bloom_views,
            ao_tex,
            ldr,
        };
        self.targets = Some(targets);
    }
}

/// A post bind group over `textures` (t0..t3), the `PostFrame` and `Pass`
/// blocks and the grading block (`None`: the frame block in its place,
/// unused).
pub(crate) fn post_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    frame: &wgpu::Buffer,
    passes: &wgpu::Buffer,
    textures: [&wgpu::TextureView; 4],
    grading: Option<&wgpu::Buffer>,
) -> wgpu::BindGroup {
    let [a, b, c, d] = textures;
    let pass = wgpu::BufferBinding {
        buffer: passes,
        offset: 0,
        size: wgpu::BufferSize::new(std::mem::size_of::<PassParams>() as u64),
    };
    let texture = |binding, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("modern post"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(pass),
            },
            texture(2, a),
            texture(3, b),
            texture(4, c),
            texture(5, d),
            wgpu::BindGroupEntry {
                binding: 6,
                resource: grading.unwrap_or(frame).as_entire_binding(),
            },
        ],
    })
}

impl ModernRenderer {
    /// The last frame's post effects.
    #[must_use]
    pub fn post_effects(&self) -> Effects {
        self.history.post.effects
    }

    /// This frame's effects, targets and uniforms (renderer plan M8). The
    /// forward frame group gets this frame's ambient occlusion map (or the
    /// white texel); returns whether the forward pass applies SSAO.
    pub(crate) fn prepare_post(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        uniforms: &FrameUniforms,
        size: [u32; 2],
        rect: [i32; 4],
        clip: [i32; 4],
    ) -> bool {
        let fx = Effects::resolve(
            &self.preparation.settings,
            self.preparation.faithful_bloom,
            self.device_resources.samples,
        );
        if fx != self.history.post.effects {
            log::info!("[modern] post effects {fx:?}");
        }
        self.history.post.effects = fx;
        let [x, y, w, h] = rect;
        let bloom = [
            (w.max(1) as u32).div_ceil(crate::post::BLOOM_DOWNSAMPLE),
            (h.max(1) as u32).div_ceil(crate::post::BLOOM_DOWNSAMPLE),
        ];
        // The occlusion mode (`Off` draws no occlusion).
        self.history.post.ao = self.preparation.settings.ao;
        let ao = self.history.post.ao != crate::settings::AoMode::Off;
        let targets = self.frame_resources.targets.as_ref().expect("targets");
        let divisor = self.preparation.settings.ao_resolution.divisor();
        let key = PostKey {
            scene: size,
            bloom,
            ao: self.preparation.settings.ao_resolution.size(size),
        };
        self.history.post.ensure_targets(
            device,
            key,
            targets.hdr_views(),
            &self.device_resources.lut_view,
            &self.device_resources.post_buffer,
        );
        self.bind_ao(device, ao.then_some(key));
        // At a render scale with FXAA: FXAA's scaled output, the upscale's
        // source (`frame::scale`).
        if self.frame_resources.scaled.is_some() && fx.fxaa {
            let post = &mut self.history.post;
            if post.targets.as_ref().is_some_and(|t| t.ldr_fxaa.is_none()) {
                let view = target_view(device, "modern post ldr (fxaa)", size, LDR_FORMAT);
                let w = &post.white;
                let bind = post_bind(
                    device,
                    &post.layout,
                    &post.frame,
                    &post.passes,
                    [&view, w, w, w],
                    Some(&self.device_resources.post_buffer),
                );
                post.targets.as_mut().expect("post targets").ldr_fxaa = Some((view, bind));
            }
        }
        // The scissor as `encode` clamps it.
        let (tw, th) = (size[0] as i32, size[1] as i32);
        let [l, t, r, b] = clip;
        let (l, t) = (l.clamp(0, tw), t.clamp(0, th));
        let (r, b) = (r.clamp(l, tw), b.clamp(t, th));
        self.history.post.clip = [l, t, r, b].map(|v| v as u32);
        // Adaptation on the logic clock (fixed-clock frames repeat).
        let (dt, reset) = {
            let now = self.frame_millis();
            let step =
                self.history.post.last_ms.map(|last| {
                    ((now - last) as f32 / 1000.0).clamp(0.0, crate::post::MAX_TIME_STEP)
                });
            self.history.post.last_ms = Some(now);
            self.history.post.adapted_slot = 1 - self.history.post.adapted_slot;
            (step.unwrap_or(0.0), step.is_none())
        };
        let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
        let view = glam::Mat4::from_cols_array_2d(&uniforms.view);
        // Classic view space to clip: the SSAO radius's pixel scale at unit
        // depth (the projection's y scale times half the viewport height).
        let proj = view_proj * view.inverse();
        let pixels_per_unit = proj.y_axis.y.abs() * h as f32 * 0.5;
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let region = [(r - l).max(1) as f32, (b - t).max(1) as f32];
        let frame = PostFrame {
            rect: [x as f32, y as f32, w as f32, h as f32],
            clip: [l as f32, t as f32, r as f32, b as f32],
            sizes: [
                bloom[0] as f32,
                bloom[1] as f32,
                region[0] / crate::post::LUMINANCE_SIZE as f32,
                region[1] / crate::post::LUMINANCE_SIZE as f32,
            ],
            ssao: [
                crate::post::SSAO_RADIUS * pixels_per_unit,
                crate::post::SSAO_INTENSITY,
                crate::post::SSAO_BIAS,
                crate::post::SSAO_BASE_SAMPLES,
            ],
            ssao2: [
                crate::post::SSAO_Z_FAR,
                crate::post::SSAO_MIN_SAMPLES,
                crate::post::SSAO_BLUR_SHARPNESS,
                0.0,
            ],
            directions: crate::post::ssao_directions(),
            bloom: [
                crate::post::BLOOM_THRESHOLD,
                crate::post::BLOOM_SCALE,
                on(fx.bloom),
                0.0,
            ],
            exposure: [
                crate::post::EXPOSURE_KEY,
                crate::post::EXPOSURE_MIN,
                crate::post::EXPOSURE_MAX,
                1.0,
            ],
            adapt: [dt, 1.0 / crate::post::EXPOSURE_SPEED, on(reset), 0.0],
            tonemap: [
                crate::post::MIN_BLACK_LUM,
                crate::post::MAX_WHITE_LUM,
                crate::post::REINHARD_KEY,
                crate::post::MIN_AUTO_EXPOSURE,
            ],
            // z: the scene's light unit (the patched module reads it; 1 unless
            // the look sets it).
            tonemap2: [crate::post::MAX_AUTO_EXPOSURE, 1.0, 1.0, 0.0],
            fxaa: [
                crate::post::FXAA_SUBPIX,
                crate::post::FXAA_EDGE_THRESHOLD,
                crate::post::FXAA_EDGE_THRESHOLD_MIN,
                // The composite writes the display frame for a pass after it
                // (FXAA, or the render scale's upscale).
                on(fx.fxaa || self.frame_resources.scaled.is_some()),
            ],
        };
        let mut frame = frame;
        self.look.post_frame(&mut frame, pixels_per_unit);
        queue.write_buffer(&self.history.post.frame, 0, bytemuck::bytes_of(&frame));
        let area = crate::post::ao::Area::new(rect, self.history.post.clip, divisor);
        self.history.post.ao_area = area;
        frame.rect = area.rect;
        frame.clip = area.clip.map(|v| v as f32);
        let view = crate::post::ao::View {
            viewport: [area.rect[2], area.rect[3]],
            proj,
        };
        crate::post::ao::post_frame(&mut frame, self.history.post.ao, &view);
        queue.write_buffer(&self.history.post.ao_frame, 0, bytemuck::bytes_of(&frame));
        let mut slots = vec![PassParams::default(); PASS_SLOTS as usize];
        slots[SLOT_BLUR_X as usize] = PassParams::new([1.0, 0.0, 0.0, 0.0]);
        slots[SLOT_BLUR_Y as usize] = PassParams::new([0.0, 1.0, 0.0, 0.0]);
        for slot in [SLOT_BLUR_X, SLOT_BLUR_Y] {
            slots[slot as usize].pad[0] =
                [divisor as f32, (AO_BLUR_RADIUS / divisor) as f32, 0.0, 0.0];
        }
        slots[SLOT_LUM as usize + 2] = PassParams::new([1.0, 0.0, 0.0, 0.0]);
        for (k, &it) in crate::post::BLOOM_KERNEL.iter().enumerate() {
            slots[(SLOT_KAWASE as usize) + k] = PassParams::new([it as f32, 0.0, 0.0, 0.0]);
        }
        slots[SLOT_HBAO_A as usize] = PassParams::new(crate::post::ao::HBAO_A.params());
        slots[SLOT_HBAO_B as usize] = PassParams::new(crate::post::ao::HBAO_B.params());
        if let Some(s) = self.frame_resources.scaled {
            // The viewport in the frame's pixels; encode for an sRGB target
            // unless FXAA's output already is linear.
            let mut p = PassParams::new(s.native_rect.map(|v| v as f32));
            p.pad[0][0] = on(self.device_resources.output_format.is_srgb() && !fx.fxaa);
            slots[SLOT_UPSCALE as usize] = p;
        }
        queue.write_buffer(&self.history.post.passes, 0, bytemuck::cast_slice(&slots));
        ao
    }

    /// Point the forward frame group's occlusion map at the post targets of
    /// `key` (`None`: the white texel) when it does not already.
    pub(crate) fn bind_ao(&mut self, device: &wgpu::Device, key: Option<PostKey>) {
        if self.history.post.frame_ao == key {
            return;
        }
        let ao = match (key, self.history.post.targets.as_ref()) {
            (Some(_), Some(t)) => &t.ao,
            _ => &self.history.post.white,
        };
        self.device_resources.frame_bind = frame_bind(
            device,
            &self.history.post.frame_layout,
            &self.device_resources.frame_buffer,
            ao,
        );
        self.history.post.frame_ao = key;
    }
}

impl<'a> EncodeInputs<'a> {
    /// A fullscreen post pass into `view` over `[x, y, w, h]` with the
    /// scissor `clip` (`None`: the whole target), cleared to `clear`.
    pub(crate) fn post_pass(&self, encoder: &mut wgpu::CommandEncoder, draw: PostPass<'_>) {
        let PostPass {
            which,
            view,
            pipeline,
            bind,
            slot,
            area,
            clear,
        } = draw;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(which)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, |c| {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: c,
                            g: c,
                            b: c,
                            a: 1.0,
                        })
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        if let Some(([x, y, w, h], [l, t, r, b])) = area {
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_scissor_rect(l, t, r - l, b - t);
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[(u64::from(slot) * PASS_SLOT) as u32]);
        pass.draw(0..3, 0..1);
    }

    /// The post chain after the forward pass into `view`.
    pub(crate) fn encode_post(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rect: [i32; 4],
    ) {
        let fx = self.post.effects;
        let Some(t) = self.post.targets.as_ref() else {
            return;
        };
        let [l, tp, r, b] = self.post.clip;
        if r <= l || b <= tp {
            return;
        }
        let slot = self.post.adapted_slot;
        let src = self.hdr_source();
        {
            // Scene luminance: 64x64 log, 4x4 boxes to 1x1, adaptation.
            let lum_binds = std::iter::once(&t.lum_log_bind[src]).chain(&self.post.lum_down_bind);
            for (k, bind) in lum_binds.enumerate() {
                let (pipeline, pass_slot) = if k == 0 {
                    (&self.post.lum_log, 0)
                } else {
                    (&self.post.lum_down, SLOT_LUM + k as u32 - 1)
                };
                self.post_pass(
                    encoder,
                    PostPass {
                        which: crate::frame::passes::Pass::Luminance,
                        view: &self.post.lum[k],
                        pipeline,
                        bind,
                        slot: pass_slot,
                        area: None,
                        clear: None,
                    },
                );
            }
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Adaptation,
                    view: &self.post.adapted[slot],
                    pipeline: &self.post.adapt,
                    bind: &self.post.adapt_bind[slot],
                    slot: 0,
                    area: None,
                    clear: None,
                },
            );
        }
        if fx.bloom {
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::BrightPass,
                    view: &t.bloom[0],
                    pipeline: &self.post.bright,
                    bind: &t.bright_bind[src][slot],
                    slot: 0,
                    area: None,
                    clear: None,
                },
            );
            for k in 0..crate::post::BLOOM_KERNEL.len() {
                self.post_pass(
                    encoder,
                    PostPass {
                        which: crate::frame::passes::Pass::Bloom,
                        view: &t.bloom[(k + 1) % 2],
                        pipeline: &self.post.kawase,
                        bind: &t.kawase_bind[k % 2],
                        slot: SLOT_KAWASE + k as u32,
                        area: None,
                        clear: None,
                    },
                );
            }
        }
        let [x, y, w, h] = rect.map(|v| v as f32);
        let area = Some(([x, y, w, h], self.post.clip));
        if let Some(s) = self.scaled {
            // The render scale (`frame::scale`): the composite (and FXAA)
            // at the scene's scale, then the upscale into the frame's
            // viewport.
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Composite,
                    view: &t.ldr,
                    pipeline: &self.post.composite_ldr,
                    bind: &t.composite_bind[src][slot],
                    slot: 0,
                    area,
                    clear: None,
                },
            );
            let mut source = &t.upscale_bind;
            if let Some((out, bind)) = t.ldr_fxaa.as_ref().filter(|_| fx.fxaa) {
                self.post_pass(
                    encoder,
                    PostPass {
                        which: crate::frame::passes::Pass::Fxaa,
                        view: out,
                        pipeline: &self.post.fxaa_ldr,
                        bind: &t.fxaa_bind,
                        slot: 0,
                        area,
                        clear: None,
                    },
                );
                source = bind;
            }
            let [nx, ny, nw, nh] = s.native_rect.map(|v| v as f32);
            let [sw, sh] = s.native_size.map(|v| v as i32);
            let [l, t, r, b] = s.native_clip;
            let (l, t) = (l.clamp(0, sw), t.clamp(0, sh));
            let (r, b) = (r.clamp(l, sw), b.clamp(t, sh));
            if r > l && b > t {
                self.post_pass(
                    encoder,
                    PostPass {
                        which: crate::frame::passes::Pass::Upscale,
                        view,
                        pipeline: &self.post.upscale,
                        bind: source,
                        slot: SLOT_UPSCALE,
                        area: Some(([nx, ny, nw, nh], [l, t, r, b].map(|v| v as u32))),
                        clear: None,
                    },
                );
            }
        } else if fx.fxaa {
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Composite,
                    view: &t.ldr,
                    pipeline: &self.post.composite_ldr,
                    bind: &t.composite_bind[src][slot],
                    slot: 0,
                    area,
                    clear: None,
                },
            );
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Fxaa,
                    view,
                    pipeline: &self.post.fxaa,
                    bind: &t.fxaa_bind,
                    slot: 0,
                    area,
                    clear: None,
                },
            );
        } else {
            self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Composite,
                    view,
                    pipeline: &self.post.composite,
                    bind: &t.composite_bind[src][slot],
                    slot: 0,
                    area,
                    clear: None,
                },
            );
        }
    }

    /// The SSAO passes (before the forward pass): the opaque entities and
    /// floors into the normal/position targets, the occlusion, its X and Y
    /// blur into the forward pass's `ssao_map`.
    pub(crate) fn encode_ssao(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        rect: [i32; 4],
        clip: [i32; 4],
    ) {
        if self.post.ao == crate::settings::AoMode::Off {
            return;
        }
        let Some(t) = self.post.targets.as_ref() else {
            return;
        };
        let [l, tp, r, b] = self.post.ao_area.clip;
        if r <= l || b <= tp {
            return;
        }
        let [x, y, w, h] = self.post.ao_area.rect;
        let _ = rect;
        let _ = clip;
        {
            let clear = wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::AoGeometry)),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &t.normal,
                        resolve_target: None,
                        depth_slice: None,
                        ops: clear,
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &t.position,
                        resolve_target: None,
                        depth_slice: None,
                        ops: clear,
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_scissor_rect(l, tp, r - l, b - tp);
            pass.set_pipeline(&self.post.geometry);
            pass.set_bind_group(0, self.frame_bind, &[]);
            pass.set_bind_group(2, &self.shadow.atlas.2, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            // M10: the terrain's normal and depth.
            self.encode_terrain(&mut pass, crate::frame::TerrainPass::Geometry);
            pass.set_pipeline(&self.post.geometry);
            // In the draw order: colour ties keep the last draw (`submit`).
            let end = self.post.geometry_draws.min(self.draws.len());
            let mut bound = crate::frame::submit::Bound::default();
            for d in self.draws[..end]
                .iter()
                .filter(|d| d.pass == Pass::Opaque && !matches!(d.geometry, Geometry::Far { .. }))
            {
                self.submit(&mut pass, d, &mut bound);
            }
        }
        let area = Some(([x, y, w, h], self.post.ao_area.clip));
        let mode = self.post.ao;
        self.encode_ao(encoder, t, mode, area);
        if !crate::post::ao::blurs(mode) {
            return;
        }
        self.post_pass(
            encoder,
            PostPass {
                which: crate::frame::passes::Pass::AoBlurX,
                view: &t.ao_tmp,
                pipeline: &self.post.blur,
                bind: &t.blur_x_bind,
                slot: SLOT_BLUR_X,
                area,
                clear: Some(1.0),
            },
        );
        self.post_pass(
            encoder,
            PostPass {
                which: crate::frame::passes::Pass::AoBlurY,
                view: &t.ao,
                pipeline: &self.post.blur,
                bind: &t.blur_y_bind,
                slot: SLOT_BLUR_Y,
                area,
                clear: Some(1.0),
            },
        );
    }

    /// The occlusion pass(es) (`post::ao`): SSAO straight into the forward
    /// pass's map (no blur), or horizon-based set A (then set B over it for
    /// HBAO Ultra) into the blur's input.
    pub(crate) fn encode_ao(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        t: &PostTargets,
        mode: crate::settings::AoMode,
        area: Option<([f32; 4], [u32; 4])>,
    ) {
        use crate::settings::AoMode as Mode;
        let hbao = &self.post.hbao;
        match mode {
            Mode::Off => {}
            Mode::Ssao => self.post_pass(
                encoder,
                PostPass {
                    which: crate::frame::passes::Pass::Ao,
                    view: &t.ao,
                    pipeline: &self.post.ssao,
                    bind: &t.ssao_bind,
                    slot: 0,
                    area,
                    clear: Some(1.0),
                },
            ),
            Mode::Hbao | Mode::HbaoUltra => {
                self.post_pass(
                    encoder,
                    PostPass {
                        which: crate::frame::passes::Pass::Ao,
                        view: &t.ao_raw,
                        pipeline: &hbao[0],
                        bind: &t.ssao_bind,
                        slot: SLOT_HBAO_A,
                        area,
                        clear: Some(1.0),
                    },
                );
                if mode == Mode::HbaoUltra {
                    self.post_pass(
                        encoder,
                        PostPass {
                            which: crate::frame::passes::Pass::Ao,
                            view: &t.ao_raw,
                            pipeline: &hbao[1],
                            bind: &t.ssao_bind,
                            slot: SLOT_HBAO_B,
                            area,
                            clear: None,
                        },
                    );
                }
            }
        }
    }
}
