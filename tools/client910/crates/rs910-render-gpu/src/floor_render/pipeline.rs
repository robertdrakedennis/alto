//! Floor and model pipeline variants and frame uniform resources.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::cache::Pack;

use crate::texture::MaterialStore;

// `FloorUniforms` and `GAME_TO_WORLD` moved to rs910-scene (Phase 3.2).
pub use rs910_scene::floor_uniforms::*;

use super::{
    empty_cube, make_sampler, material_frame_bindings, upload_environment_cube, upload_volume_mips,
    FrameViews, FLOOR_SHADER,
};

/// Group-1 per-batch uniform (`BatchUniforms`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct BatchUniforms {
    tex_scale: [f32; 4],
    origin: [f32; 4],
    pub(super) shader: [f32; 4],
    pub(super) scroll: [f32; 4],
    pub(super) water: [f32; 4],
    pub(super) light_pos: [[f32; 4]; 4],
    pub(super) light_colour: [[f32; 4]; 4],
    /// The height-fog plane per floor batch: zero outside the underwater
    /// pass.
    pub(super) height_fog_plane: [f32; 4],
    /// The height-fog colour.
    pub(super) height_fog_colour: [f32; 4],
}

impl BatchUniforms {
    /// A batch's initial uniforms: UV scale, alpha reference (`-1` disables),
    /// reflective-alpha flag and origin; every other block starts zeroed
    /// (shader `[0, 0, 1, 0]`) and is filled in by the caller.
    pub(super) fn initial(
        tex_scale: f32,
        alpha_ref: f32,
        reflective_alpha: bool,
        origin_fine: [f32; 3],
    ) -> Self {
        Self {
            tex_scale: [
                tex_scale,
                tex_scale,
                alpha_ref,
                u8::from(reflective_alpha) as f32,
            ],
            origin: [origin_fine[0], origin_fine[1], origin_fine[2], 0.],
            shader: [0., 0., 1., 0.],
            scroll: [0.; 4],
            water: [0.; 4],
            light_pos: [[0.; 4]; 4],
            light_colour: [[0.; 4]; 4],
            height_fog_plane: [0.; 4],
            height_fog_colour: [0.; 4],
        }
    }
}

/// Stream-0 vertex as uploaded: `pos, uv, depth, normal` (the stride-36
/// layout; floors without depth/normals are padded to it at upload).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FloorVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub depth: f32,
    pub normal: [f32; 3],
}

pub(super) const FLOOR_VERTEX_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x2,
    2 => Float32,
    3 => Float32x3
];

pub(super) const FLOOR_COLOUR_ATTRS: [wgpu::VertexAttribute; 1] =
    wgpu::vertex_attr_array![4 => Unorm8x4];

/// Pipeline + shared bindings for floor draws.
pub struct FloorPipeline {
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// The model pipelines of [`floor_pipelines`] by `(samples, format)`
    /// ([`crate::pipelines::Variant::FloorModels`]), shared with every other
    /// owner of this cache.
    pipelines: crate::pipelines::PipelineCache,
    scene_variants: Option<std::sync::Arc<[wgpu::RenderPipeline; 4]>>,
    pub sample_count: u32,
    base_format: wgpu::TextureFormat,
    scene_format: wgpu::TextureFormat,
    /// One sample at `base_format`: plain, alpha test, no depth write,
    /// alpha test without depth write.
    base: std::sync::Arc<[wgpu::RenderPipeline; 4]>,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    pub(super) uniform_layout: wgpu::BindGroupLayout,
    pub(super) batch_layout: wgpu::BindGroupLayout,
    pub(super) environment_view: wgpu::TextureView,
    pub(super) environment_sampler: wgpu::Sampler,
    pub(super) environment_id: i32,
    environment_cache: HashMap<i32, wgpu::TextureView>,
    noise_texture: wgpu::Texture,
    pub(super) noise_view: wgpu::TextureView,
    pub(super) noise_sampler: wgpu::Sampler,
    noise_uploaded: std::cell::Cell<bool>,
    /// The water normal volume the environment-mapped water program
    /// samples; uploaded on first use.
    water_normal_texture: wgpu::Texture,
    pub(super) water_normal_view: wgpu::TextureView,
    water_normal_uploaded: std::cell::Cell<bool>,
    clock: std::time::Instant,
    frame_millis: i32,
    scene_base: [f32; 3],
}

pub(super) fn floor_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    surface_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    sample_count: u32,
) -> [wgpu::RenderPipeline; 4] {
    let descriptor = wgpu::RenderPipelineDescriptor {
        label: Some("floor"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FloorVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &FLOOR_VERTEX_ATTRS,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &FLOOR_COLOUR_ATTRS,
                }),
            ],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: if crate::render_debug_flags::flags().profile_shared_discard {
                Some("fs_shared_discard")
            } else {
                Some("fs_main")
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                // Blend (SRC_ALPHA, ONE_MINUS_SRC_ALPHA)
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::Zero,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(true),
            // Depth test LEQUAL
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    };
    let pipeline = device.create_render_pipeline(&descriptor);
    // A nonzero alpha ref selects the cutout mode: RGBA replacement and
    // depth write retained. Blending is disabled; the separate alpha
    // factors therefore have no effect.
    let cutout_targets = [Some(wgpu::ColorTargetState {
        format: surface_format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let model_no_depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("character shadow"),
        depth_stencil: Some(wgpu::DepthStencilState {
            depth_write_enabled: Some(false),
            ..descriptor.depth_stencil.clone().unwrap()
        }),
        ..descriptor.clone()
    });
    let model_cutout_no_depth_pipeline =
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("character shadow alpha test"),
            fragment: Some(wgpu::FragmentState {
                targets: &cutout_targets,
                entry_point: Some("fs_cutout"),
                ..descriptor.fragment.clone().unwrap()
            }),
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                ..descriptor.depth_stencil.clone().unwrap()
            }),
            ..descriptor.clone()
        });
    let model_cutout_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model alpha test"),
        fragment: Some(wgpu::FragmentState {
            targets: &cutout_targets,
            entry_point: Some("fs_cutout"),
            ..descriptor.fragment.clone().unwrap()
        }),
        ..descriptor
    });
    [
        pipeline,
        model_cutout_pipeline,
        model_no_depth_pipeline,
        model_cutout_no_depth_pipeline,
    ]
}

impl FloorPipeline {
    pub fn set_sample_count(
        &mut self,
        device: &wgpu::Device,
        colour: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        count: u32,
    ) {
        if self.sample_count == count && self.scene_format == colour {
            return;
        }
        self.scene_variants = (count > 1 || colour != self.base_format).then(|| {
            self.pipelines.get(
                count,
                colour,
                crate::pipelines::Variant::FloorModels,
                || floor_pipelines(device, &self.shader, &self.layout, colour, depth, count),
            )
        });
        self.sample_count = count;
        self.scene_format = colour;
    }
    /// Colour format the scene variants target.
    #[must_use]
    pub fn scene_format(&self) -> wgpu::TextureFormat {
        self.scene_format
    }
    pub fn scene_pipeline(&self) -> &wgpu::RenderPipeline {
        self.scene_variants
            .as_ref()
            .map_or(&self.base[0], |p| &p[0])
    }

    /// The one-sample, `base_format` floor pipeline (the minimap base).
    pub fn base_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.base[0]
    }

    /// The model pipeline for a batch: the scene variants when set, else
    /// the base ones; both hold plain, alpha test, no depth write, alpha
    /// test without depth write, in that order.
    pub(super) fn model_pipeline(
        &self,
        alpha_test: bool,
        depth_write: bool,
    ) -> &wgpu::RenderPipeline {
        let pipelines = self.scene_variants.as_ref().unwrap_or(&self.base);
        &pipelines[match (alpha_test, depth_write) {
            (false, true) => 0,
            (true, true) => 1,
            (false, false) => 2,
            (true, false) => 3,
        }]
    }

    /// The pipeline cache this pipeline's variants come from.
    pub fn cache(&self) -> &crate::pipelines::PipelineCache {
        &self.pipelines
    }
    pub fn frame_millis(&self) -> i32 {
        self.frame_millis
    }
    /// Create the pipeline for the given surface format (depth `Depth24Plus`
    /// to match the renderer), with a cache of its own.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        Self::with_cache(
            device,
            surface_format,
            depth_format,
            crate::pipelines::PipelineCache::default(),
        )
    }

    /// [`FloorPipeline::new`] taking its model pipelines from `pipelines`.
    pub fn with_cache(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        pipelines: crate::pipelines::PipelineCache,
    ) -> Self {
        let fragment_uniform = crate::frame_profile::FrameProfile::enabled()
            && crate::render_debug_flags::flags().profile_fragment_uniform;
        let source = if fragment_uniform {
            std::borrow::Cow::Owned(
                FLOOR_SHADER
                    .replace("in.alpha_state.x", "b.tex_scale.z")
                    .replace("in.alpha_state.y", "b.tex_scale.w"),
            )
        } else {
            std::borrow::Cow::Borrowed(FLOOR_SHADER)
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor (Model.glsl ShaderMode 0)"),
            source: wgpu::ShaderSource::Wgsl(source),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor uniforms"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let batch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor batch"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: if fragment_uniform {
                        wgpu::ShaderStages::VERTEX_FRAGMENT
                    } else {
                        wgpu::ShaderStages::VERTEX
                    },
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&batch_layout)],
            immediate_size: 0,
        });
        let base = pipelines.get(
            1,
            surface_format,
            crate::pipelines::Variant::FloorModels,
            || floor_pipelines(device, &shader, &layout, surface_format, depth_format, 1),
        );
        let ident = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let env = crate::env::EnvFrame::default_for(1.0, 0.0, &ident);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("floor uniforms"),
            contents: bytemuck::bytes_of(&FloorUniforms::new(glam::Mat4::IDENTITY, &env)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let environment_view = empty_cube(device);
        let environment_sampler = make_sampler(device, true, true, true);
        let noise_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waterfall billow"),
            size: wgpu::Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 16,
            },
            mip_level_count: 8,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let noise_view = noise_texture.create_view(&Default::default());
        let water_normal_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water normal volume"),
            size: wgpu::Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 16,
            },
            mip_level_count: 8,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let water_normal_view = water_normal_texture.create_view(&Default::default());
        let noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniform_bind_group = material_frame_bindings(
            device,
            &uniform_layout,
            &uniform_buffer,
            FrameViews {
                cube: &environment_view,
                sampler: &environment_sampler,
                noise: &noise_view,
                noise_sampler: &noise_sampler,
                water_normals: &water_normal_view,
            },
        );
        Self {
            shader,
            layout,
            pipelines,
            scene_variants: None,
            sample_count: 1,
            base_format: surface_format,
            scene_format: surface_format,
            base,
            uniform_buffer,
            uniform_bind_group,
            uniform_layout,
            batch_layout,
            environment_view,
            environment_sampler,
            environment_id: -1,
            environment_cache: HashMap::new(),
            noise_texture,
            noise_view,
            noise_sampler,
            noise_uploaded: std::cell::Cell::new(false),
            water_normal_texture,
            water_normal_view,
            water_normal_uploaded: std::cell::Cell::new(false),
            clock: crate::logic_clock::now(),
            frame_millis: 0,
            scene_base: [0.; 3],
        }
    }

    /// Upload the normal volume with the box mip chain the billow volume
    /// uses.
    pub(super) fn upload_water_normals(&self, queue: &wgpu::Queue) {
        if self.water_normal_uploaded.replace(true) {
            return;
        }
        upload_volume_mips(
            queue,
            &self.water_normal_texture,
            crate::water::normal_volume(),
            4,
        );
    }

    pub(super) fn upload_noise(&self, queue: &wgpu::Queue) {
        self.upload_water_normals(queue);
        if self.noise_uploaded.replace(true) {
            return;
        }
        let mut bytes = crate::billow::volume().to_vec();
        let (mut w, mut h, mut d) = (
            crate::billow::WIDTH,
            crate::billow::HEIGHT,
            crate::billow::DEPTH,
        );
        for level in 0..8 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.noise_texture,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w as u32 * 2),
                    rows_per_image: Some(h as u32),
                },
                wgpu::Extent3d {
                    width: w as u32,
                    height: h as u32,
                    depth_or_array_layers: d as u32,
                },
            );
            if level == 7 {
                break;
            }
            let (nw, nh, nd) = ((w / 2).max(1), (h / 2).max(1), (d / 2).max(1));
            let mut next = Vec::with_capacity(nw * nh * nd * 2);
            for z in 0..nd {
                for y in 0..nh {
                    for x in 0..nw {
                        for channel in 0..2 {
                            let mut sum = 0u32;
                            for dz in 0..2 {
                                for dy in 0..2 {
                                    for dx in 0..2 {
                                        sum += bytes[(((z * 2 + dz).min(d - 1) * h
                                            + (y * 2 + dy).min(h - 1))
                                            * w
                                            + (x * 2 + dx).min(w - 1))
                                            * 2
                                            + channel]
                                            as u32;
                                    }
                                }
                            }
                            next.push(((sum + 4) / 8) as u8);
                        }
                    }
                }
            }
            bytes = next;
            w = nw;
            h = nh;
            d = nd;
        }
    }
    /// Sample the clock once per frame: scrolling and waterfall uniforms
    /// must use the same time even at the 128-second UV wrap.
    pub fn begin_material_frame(&mut self) -> i32 {
        self.frame_millis = (crate::logic_clock::now()
            .duration_since(self.clock)
            .as_millis()
            & 0x7fffffff) as i32;
        self.frame_millis
    }
    pub fn set_environment(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pack: &Pack,
        materials: &MaterialStore,
        id: i32,
        base: [f32; 3],
    ) -> anyhow::Result<bool> {
        self.scene_base = base;
        if self.environment_id == id {
            return Ok(false);
        }
        let view = if let Some(view) = self.environment_cache.remove(&id) {
            view
        } else if id < 0 {
            empty_cube(device)
        } else {
            let material = materials
                .get(id as u32)
                .ok_or_else(|| anyhow::anyhow!("environment material {id} missing"))?;
            if !material.environment_cube {
                empty_cube(device)
            } else {
                upload_environment_cube(device, queue, pack, material)?
            }
        };
        let previous = std::mem::replace(&mut self.environment_view, view);
        self.environment_cache.insert(self.environment_id, previous);
        self.environment_id = id;
        self.uniform_bind_group = material_frame_bindings(
            device,
            &self.uniform_layout,
            &self.uniform_buffer,
            FrameViews {
                cube: &self.environment_view,
                sampler: &self.environment_sampler,
                noise: &self.noise_view,
                noise_sampler: &self.noise_sampler,
                water_normals: &self.water_normal_view,
            },
        );
        Ok(true)
    }

    /// Group-0 layout (`FloorUniforms`), shared by the extra floor passes.
    #[must_use]
    /// A standalone group-0 bind group over `values` with this pipeline's
    /// environment/noise/water bindings (an interface model's particle
    /// draw, whose quads are already in view space).
    pub fn uniform_group(
        &self,
        device: &wgpu::Device,
        values: &FloorUniforms,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle frame"),
            contents: bytemuck::bytes_of(values),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let group = material_frame_bindings(
            device,
            &self.uniform_layout,
            &buffer,
            FrameViews {
                cube: &self.environment_view,
                sampler: &self.environment_sampler,
                noise: &self.noise_view,
                noise_sampler: &self.noise_sampler,
                water_normals: &self.water_normal_view,
            },
        );
        (buffer, group)
    }

    pub fn uniform_layout(&self) -> &wgpu::BindGroupLayout {
        &self.uniform_layout
    }

    /// The top-down minimap pass binds the identity view, the orthographic
    /// `view_proj` built by the caller, and zeroes the sun/ambient/fog terms
    /// so the vertex colours show as is; the shader's lighting factor is
    /// therefore 1.
    pub fn upload_minimap_uniforms(
        &self,
        queue: &dyn crate::uploads::Uploader,
        view_proj: glam::Mat4,
        env: &crate::env::EnvFrame,
    ) {
        self.upload_noise(queue.queue());
        let mut uniforms = FloorUniforms::new(view_proj, env);
        uniforms.sun_dir = [0.0; 4];
        uniforms.sun_colour = [0.0; 4];
        uniforms.anti_sun_colour = [0.0; 4];
        uniforms.ambient_colour = [1.0, 1.0, 1.0, 0.0];
        uniforms.height_fog_plane = [0.0; 4];
        uniforms.distance_fog_plane = [0.0; 4];
        uniforms.scene_origin = [0.0; 4];
        uniforms.eye_time[3] = (self.frame_millis % 128000) as f32 / 1000.;
        uniforms.scene_base = [
            self.scene_base[0],
            self.scene_base[1],
            self.scene_base[2],
            0.,
        ];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }
    pub fn update_camera(
        &self,
        queue: &dyn crate::uploads::Uploader,
        camera: &crate::camera::SceneCamera,
        env: &crate::env::EnvFrame,
    ) {
        self.upload_noise(queue.queue());
        let mut uniforms = FloorUniforms::for_camera(camera, env);
        uniforms.eye_time[3] = (self.frame_millis % 128000) as f32 / 1000.;
        uniforms.scene_base = [
            self.scene_base[0],
            self.scene_base[1],
            self.scene_base[2],
            0.,
        ];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }
}
