//! Light probes and image-based lighting on the GPU (renderer plan M6; the
//! maths, evidence and choices in [`crate::lighting::probes`]): the forward
//! pass's probe bindings (group 3, beside the point lights: the probes'
//! SH, their grid, the prefiltered environment cube, the BRDF LUT), the
//! capture (the scene drawn into a cube
//! per probe with the forward shading, projected to SH), the environment
//! cube's capture and GGX prefilter, and the LUT at start-up.
//!
//! A capture runs inside a frame, before its sky pass: the sky's six faces
//! (probe size) → each probe's six faces in one atlas pass (the sky copied
//! under them, then the scene's floors and locs near the probe) → the
//! projection (compute) into the SH buffer the forward pass reads → the
//! environment cube's faces (128) → its mip chain → the prefilter into the
//! bound cube. It runs when the scene or the settled environment changes
//! (`lighting::probes` "When"), never per frame.
use crate::frame::encoding::EncodeInputs;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use wgpu::util::DeviceExt;

use crate::frame::gpu::sky_layers::SkyLayerUniforms;
use crate::frame::*;
use crate::lighting::probes::ProbeGrid;

/// The `Probes` block of [`crate::lighting::probes::PROBES_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ProbeUniforms {
    pub(crate) origin: [f32; 4],
    pub(crate) grid: [f32; 4],
    pub(crate) dims: [u32; 4],
    pub(crate) env: [f32; 4],
    /// The per-square ambient: x 1 when on, yz the scene-local origin's
    /// offset from the first cell's corner (fine units).
    pub(crate) ambient: [f32; 4],
    /// x: the cells per side.
    pub(crate) ambient_dims: [u32; 4],
}

/// The `Project` block of the projection module.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ProjectUniforms {
    pub(crate) params: [f32; 4],
    pub(crate) fill: [f32; 4],
}

/// The `Filter` block of the filter module (256-byte slots).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FilterUniforms {
    pub(crate) params: [f32; 4],
    pub(crate) pad: [[f32; 4]; 15],
}

/// The capture formats (the forward target's and depth's).
pub(crate) const CAPTURE_FORMAT: wgpu::TextureFormat = HDR_FORMAT;
/// The environment cube's and the LUT's formats.
pub(crate) const ENV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The frame slots' stride (a `FrameUniforms` is 256 bytes).
pub(crate) const SLOT: u64 = 256;

/// The group 3 entries of the probes (bindings 3-7) and the per-square
/// ambient blocks (binding 13).
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    let fragment = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    };
    [
        fragment(
            3,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        ),
        fragment(
            4,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        ),
        fragment(
            5,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::Cube,
                multisampled: false,
            },
        ),
        fragment(
            6,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ),
        fragment(
            7,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        ),
        fragment(
            crate::frame::gpu::ambient::BINDING,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        ),
    ]
}

/// The probe resources the forward pass binds (group 3): fixed-size, so
/// the light group never rebuilds for them; a capture writes into them.
pub(crate) struct ProbeBindings {
    pub(crate) sh: wgpu::Buffer,
    pub(crate) uniforms: wgpu::Buffer,
    /// The prefiltered environment cube (all levels) and its cube view.
    pub(crate) env: wgpu::Texture,
    pub(crate) env_view: wgpu::TextureView,
    pub(crate) lut_view: wgpu::TextureView,
    pub(crate) sampler: wgpu::Sampler,
    /// The per-square ambient blocks the shader reads (`frame::gpu::ambient`).
    pub(crate) squares: wgpu::Buffer,
}

pub(crate) fn cube_texture(
    device: &wgpu::Device,
    label: &str,
    size: u32,
    mips: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ENV_FORMAT,
        usage,
        view_formats: &[],
    })
}

pub(crate) fn cube_view(
    texture: &wgpu::Texture,
    base_mip: u32,
    mips: Option<u32>,
) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: base_mip,
        mip_level_count: mips,
        ..Default::default()
    })
}

/// One face (layer) of one level of a cube texture, as a render target.
pub(crate) fn face_view(texture: &wgpu::Texture, face: u32, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level: mip,
        mip_level_count: Some(1),
        base_array_layer: face,
        array_layer_count: Some(1),
        ..Default::default()
    })
}

/// The filter module, its layout and one fullscreen pipeline per entry.
pub(crate) struct FilterGpu {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) downsample: wgpu::RenderPipeline,
    pub(crate) prefilter: wgpu::RenderPipeline,
    pub(crate) lut: wgpu::RenderPipeline,
    pub(crate) sampler: wgpu::Sampler,
}

impl FilterGpu {
    pub(crate) fn new(device: &wgpu::Device, shaders: &crate::shaders::Library) -> Self {
        let module = shaders.get(device, crate::shaders::Module::ProbeFilters);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern probe filter"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<FilterUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern probe filter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format| {
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
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern probe filter"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let pipeline = &pipeline;
        let jobs: Vec<crate::frame::compile::Job<'_, wgpu::RenderPipeline>> = vec![
            Box::new(move || pipeline("fs_downsample", ENV_FORMAT)),
            Box::new(move || pipeline("fs_prefilter", ENV_FORMAT)),
            Box::new(move || pipeline("fs_lut", LUT_FORMAT)),
        ];
        let mut built = crate::frame::compile::all(jobs).into_iter();
        let mut next = || built.next().expect("every pipeline is built");
        Self {
            downsample: next(),
            prefilter: next(),
            lut: next(),
            layout,
            sampler,
        }
    }

    /// A slot buffer holding `params` per pass.
    pub(crate) fn slots(device: &wgpu::Device, params: &[[f32; 4]]) -> wgpu::Buffer {
        let records: Vec<FilterUniforms> = params
            .iter()
            .map(|&params| FilterUniforms {
                params,
                ..FilterUniforms::default()
            })
            .collect();
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern probe filter slots"),
            contents: bytemuck::cast_slice(&records),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    }

    pub(crate) fn bind(
        &self,
        device: &wgpu::Device,
        slots: &wgpu::Buffer,
        source: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern probe filter"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: slots,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<FilterUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// One fullscreen pass of `pipeline` with filter slot `slot` into
    /// `target`.
    pub(crate) fn pass(
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
        slot: usize,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(crate::frame::passes::Pass::ProbeFilter.label()),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[(slot as u64 * SLOT) as u32]);
        pass.draw(0..3, 0..1);
    }
}

impl ProbeBindings {
    /// The bindings; with the probes on, the environment cube at its size
    /// and the BRDF LUT rendered now (start-up),
    /// else 1-texel stand-ins.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        shaders: &crate::shaders::Library,
    ) -> Self {
        let on = true;
        let sh = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern probe SH"),
            size: if on {
                (crate::lighting::probes::MAX_PROBES * crate::lighting::probes::PROBE_STRIDE * 16)
                    as u64
            } else {
                16 * crate::lighting::probes::PROBE_STRIDE as u64
            },
            // COPY_SRC: the tests read the coefficients back.
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern probes"),
            contents: bytemuck::bytes_of(&ProbeUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let (size, mips) = if on {
            (
                crate::lighting::probes::ENV_RES,
                crate::lighting::probes::ENV_MIPS,
            )
        } else {
            (1, 1)
        };
        let env = cube_texture(
            device,
            "modern environment cube",
            size,
            mips,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST,
        );
        let env_view = cube_view(&env, 0, None);
        let lut_size = if on {
            crate::lighting::probes::LUT_RES
        } else {
            1
        };
        let lut = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modern BRDF LUT"),
            size: wgpu::Extent3d {
                width: lut_size,
                height: lut_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let lut_view = lut.create_view(&wgpu::TextureViewDescriptor::default());
        if on {
            let filters = FilterGpu::new(device, shaders);
            let slots = FilterGpu::slots(device, &[[0.0, lut_size as f32, 0.0, 0.0]]);
            let bind = filters.bind(device, &slots, &env_view);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("modern BRDF LUT"),
            });
            FilterGpu::pass(&mut encoder, &filters.lut, &bind, 0, &lut_view);
            queue.submit_uploads(vec![encoder.finish()]);
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern environment"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let squares = crate::frame::gpu::ambient::cell_buffer(device);
        Self {
            sh,
            uniforms,
            env,
            env_view,
            lut_view,
            sampler,
            squares,
        }
    }

    /// The group 3 entries (bindings 3-7, 13).
    pub(crate) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 6] {
        [
            wgpu::BindGroupEntry {
                binding: 3,
                resource: self.sh.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: self.uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&self.env_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&self.lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
            wgpu::BindGroupEntry {
                binding: crate::frame::gpu::ambient::BINDING,
                resource: self.squares.as_entire_binding(),
            },
        ]
    }
}

/// What the last capture did (diagnostics, the plan's measurements).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeStats {
    /// Captures so far.
    pub captures: usize,
    /// The grid (probes along x and z) and its probes (plus the global
    /// one).
    pub nx: usize,
    pub nz: usize,
    /// The capture's draws: locs and floor batches recorded, draw calls
    /// over every probe face, draw calls into the environment cube.
    pub locs: usize,
    pub floor_batches: usize,
    pub probe_draw_calls: usize,
    pub env_draw_calls: usize,
    /// CPU time recording the capture (ms): the draws, the culling, the
    /// slots and bind groups.
    pub record_ms: f32,
}

/// The capture pipelines and targets (built on the first capture).
pub(crate) struct CapturePipes {
    /// `[winding][depth write]`: the forward shading of the pass before
    /// this lane (`fs_forward_ambient`), one sample; winding 0 counter-clockwise
    /// (the frame's), 1 clockwise.
    pub(crate) forward: [[wgpu::RenderPipeline; 2]; 2],
    /// The sky models and layers (no depth writes, depth always passes).
    pub(crate) sky_model: [wgpu::RenderPipeline; 2],
    pub(crate) sky_layer: wgpu::RenderPipeline,
    pub(crate) project: wgpu::ComputePipeline,
    pub(crate) project_layout: wgpu::BindGroupLayout,
    pub(crate) filters: FilterGpu,
    /// No sun shadows in the captures: a `Shadow` block with no cascades.
    pub(crate) no_shadow: wgpu::BindGroup,
    pub(crate) ao_white: wgpu::TextureView,
    /// The captured environment cube (level 0 rendered, the rest its
    /// downsampled chain) and its depth.
    pub(crate) env_source: wgpu::Texture,
    pub(crate) env_depth: wgpu::TextureView,
    /// The sky's six faces at the probe size, side by side.
    pub(crate) sky_atlas: wgpu::Texture,
    pub(crate) sky_depth: wgpu::TextureView,
}

/// The probe faces' atlas (six faces per row, a row per probe).
pub(crate) struct Atlas {
    pub(crate) rows: u32,
    pub(crate) colour: wgpu::Texture,
    pub(crate) depth: wgpu::TextureView,
}

/// A floor batch's zones: index range and bounding sphere.
pub(crate) type FloorParts = Vec<(u32, u32, [f32; 4])>;

/// The sky layers per capture target (0 the probe size, 1 the environment
/// cube) and face: their uniform slot and texture.
pub(crate) type SkyFaceDraws = [[Vec<(u32, Option<SkyTextureKey>)>; 6]; 2];

/// One capture draw.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CaptureDraw {
    /// A loc (the forward pass's draw record).
    Loc(Draw),
    /// One zone's part of a floor batch (all its tiles there): level,
    /// `FloorGpu::batches` index, instance, index range.
    Floor {
        level: usize,
        batch: usize,
        instance: u32,
        first: u32,
        count: u32,
    },
}

/// A level's floor batches over all tiles (the captures' floors), each
/// split by zone (8x8 tiles, so the probes cull what is far): the
/// batch's index buffer and per zone its index range and bounding sphere
/// (scene-local).
pub(crate) struct FullFloor {
    pub(crate) token: crate::frame::resources::FloorToken,
    pub(crate) batches: Vec<Option<(wgpu::Buffer, FloorParts)>>,
}

/// What the faces of a capture draw: the scene's draws and the sky
/// (shared by the probe captures and the per-square ambient captures).
pub(crate) struct SceneCapture {
    pub(crate) draws: Vec<CaptureDraw>,
    /// The sky layers per target (0 the probe size, 1 the environment
    /// cube) and face: their uniform slots and textures.
    pub(crate) sky_layers: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    pub(crate) sky_draws: SkyFaceDraws,
    pub(crate) sky_models: Vec<Draw>,
    /// The faces' winding against the frame's (1: clockwise).
    pub(crate) winding: [usize; 6],
}

/// This frame's capture (recorded in `draw`, encoded in `encode`).
pub(crate) struct Capture {
    pub(crate) scene: SceneCapture,
    /// Per probe (the global one last) and face: the draws it sees.
    pub(crate) faces: Vec<[Vec<u32>; 6]>,
    /// The environment cube's faces' draws.
    pub(crate) env_faces: [Vec<u32>; 6],
    /// The frame slots (probe faces, then the environment faces, then the
    /// sky faces) and their bind groups.
    pub(crate) _frames: wgpu::Buffer,
    pub(crate) binds: Vec<wgpu::BindGroup>,
    pub(crate) probes: u32,
    pub(crate) project: wgpu::BindGroup,
    /// The environment cube's filter passes: one bind group per downsampled
    /// level (its source level), then the prefilter's (the whole chain).
    pub(crate) downsample: Vec<wgpu::BindGroup>,
    pub(crate) prefilter: wgpu::BindGroup,
    /// The first frame of a capture: the environment cube too.
    pub(crate) env: bool,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct ProbeGpu {
    pub(crate) pipes: Option<CapturePipes>,
    pub(crate) atlas: Option<Atlas>,
    pub(crate) grid: Option<ProbeGrid>,
    /// The captured scene and environment, and the last frame's
    /// environment (a change recaptures once it holds for two frames).
    pub(crate) captured: Option<((usize, usize), u64)>,
    pub(crate) last_env: Option<u64>,
    pub(crate) capture: Option<Capture>,
    /// The capture in progress: its grid, the probes still to capture
    /// (nearest the camera first) and whether its first frame (the
    /// environment cube and the global probe) is still to come.
    pub(crate) job: Option<Job>,
    pub(crate) floors: Vec<Option<FullFloor>>,
    /// Per loc slot: the key its radius was measured for, and the radius.
    pub(crate) radii: HashMap<crate::frame::resources::LocSlot, (EntityKey, f32)>,
    pub(crate) radii_scene: Option<(usize, usize)>,
    pub(crate) stats: ProbeStats,
}

/// A capture spread over frames (the modern client renders one probe face
/// per frame; here the environment
/// cube and the global probe in the first frame, then
/// [`crate::lighting::probes::PROBES_PER_FRAME`] probes per frame).
#[derive(Clone)]
pub(crate) struct Job {
    pub(crate) grid: ProbeGrid,
    pub(crate) order: Vec<usize>,
    pub(crate) first: bool,
    pub(crate) frames: usize,
    pub(crate) draw_calls: usize,
    pub(crate) record_ms: f32,
}

/// The environment's key: its colours, intensities and skybox (quantised),
/// what a capture depends on besides the scene.
pub(crate) fn env_key(snapshot: &SceneSnapshot<'_>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let q = |v: f32| (v * 1024.0).round() as i64;
    let env = snapshot.env;
    for v in env
        .sun_rgb
        .iter()
        .chain(&env.fog.distance_colour)
        .chain(&env.clear)
        .chain(&[env.sun.ambient, env.sun.diffuse_half])
    {
        q(*v).hash(&mut h);
    }
    if let Some(sky) = snapshot.sky {
        for layer in sky.layers {
            match layer {
                crate::skybox::SkyLayer::Fill { argb, .. } => (0, *argb).hash(&mut h),
                crate::skybox::SkyLayer::Clear { rgb } => (1, *rgb).hash(&mut h),
                crate::skybox::SkyLayer::Material { key, alpha, .. } => {
                    (2, *key, *alpha).hash(&mut h);
                }
                crate::skybox::SkyLayer::Model { key, fade, .. } => (3, *key, *fade).hash(&mut h),
                crate::skybox::SkyLayer::Decor {
                    key, decor, alpha, ..
                } => (4, *key, *decor, *alpha).hash(&mut h),
            }
        }
    }
    h.finish()
}

/// A loc's bounding radius about its origin (fine units, scaled by its
/// matrix).
pub(crate) fn loc_radius(model: &crate::gpumodel::GpuModel, m: &[f32; 16]) -> f32 {
    let n = (model.vertex_count.max(0) as usize)
        .min(model.vx.len())
        .min(model.vy.len())
        .min(model.vz.len());
    let r2 = (0..n)
        .map(|i| {
            let (x, y, z) = (model.vx[i] as f32, model.vy[i] as f32, model.vz[i] as f32);
            x * x + y * y + z * z
        })
        .fold(0.0_f32, f32::max);
    let scale = (0..3)
        .map(|c| {
            (m[c * 4] * m[c * 4] + m[c * 4 + 1] * m[c * 4 + 1] + m[c * 4 + 2] * m[c * 4 + 2]).sqrt()
        })
        .fold(0.0_f32, f32::max);
    r2.sqrt() * scale.max(1e-3)
}

/// Whether a sphere (centre `q` relative to the eye, radius `r`) meets
/// face `face`'s view pyramid within `far`.
pub(crate) fn in_face(face: usize, q: glam::Vec3, r: f32, far: f32) -> bool {
    let [m, s, t] = crate::lighting::probes::face_axes(face).map(glam::Vec3::from);
    let (ma, sc, tc) = (q.dot(m), q.dot(s), q.dot(t));
    let k = std::f32::consts::FRAC_1_SQRT_2;
    q.length() - r < far
        && ma > -r
        && (ma - sc) * k > -r
        && (ma + sc) * k > -r
        && (ma - tc) * k > -r
        && (ma + tc) * k > -r
}

impl ModernRenderer {
    /// The probes' statistics (renderer plan M6).
    #[must_use]
    pub fn probe_stats(&self) -> ProbeStats {
        self.history.probes.stats
    }

    /// This frame's probes (after the frame's draws, before the uploads):
    /// the uniforms, and a capture's draws when the scene or the settled
    /// environment changed (see the module docs).
    pub(crate) fn prepare_probes(&mut self, prep: &PrepareFrame<'_, '_>, frame: &FrameUniforms) {
        let PrepareFrame {
            queue,
            snapshot,
            origin,
            ..
        } = *prep;
        self.history.probes.capture = None;
        let debug = match crate::lighting::probes::mode() {
            crate::lighting::probes::Mode::Debug(n) => n,
            _ => 0,
        };
        let key = env_key(snapshot);
        let scene = self.scene_resources.scene_token.unwrap_or((0, 0));
        let has_scene = snapshot.floors.first().and_then(Option::as_ref).is_some();
        let settled = self.history.probes.last_env == Some(key);
        self.history.probes.last_env = Some(key);
        let wanted = has_scene
            && match self.history.probes.captured {
                None => true,
                Some((s, _)) if s == (usize::MAX, 0) => false,
                Some((s, e)) => s != scene || (e != key && settled),
            };
        if wanted {
            if let Some(g0) = snapshot.floors.first().and_then(Option::as_ref) {
                // The grid over level 0 around the camera target (the modern
                // zones and 9x9 probes), probes above its ground; the
                // nearest probes first.
                let grid =
                    ProbeGrid::new(g0.tiles_x, g0.tiles_z, [origin[0], origin[2]], |x, z| {
                        g0.heights.get_fine_height_clamped(x, z)
                    });
                let mut order: Vec<usize> = (0..grid.positions.len()).collect();
                let distance = |i: usize| {
                    let p = grid.positions[i];
                    let (dx, dz) = (p[0] - origin[0], p[2] - origin[2]);
                    dx * dx + dz * dz
                };
                order.sort_by(|&a, &b| distance(a).total_cmp(&distance(b)).then(a.cmp(&b)));
                // With the captured ambient the zone probes are not captured:
                // the global probe (every probe's fill) and the environment
                // cube are what the water, reflections and env masks read.
                if self.preparation.settings.look.captured_ambient() {
                    order.clear();
                }
                self.history.probes.job = Some(Job {
                    grid,
                    order,
                    first: true,
                    frames: 0,
                    draw_calls: 0,
                    record_ms: 0.0,
                });
                self.history.probes.captured = Some((scene, key));
                self.history.probes.stats.captures += 1;
            }
        }
        if let Some(mut job) = self.history.probes.job.take() {
            let started = std::time::Instant::now();
            let subset: Vec<usize> = if job.first {
                Vec::new()
            } else {
                let n = job
                    .order
                    .len()
                    .min(crate::lighting::probes::PROBES_PER_FRAME);
                job.order.drain(..n).collect()
            };
            self.record_capture(prep, frame, &job.grid, &subset, job.first);
            if job.first {
                self.history.probes.grid = Some(job.grid.clone());
            }
            job.first = false;
            job.frames += 1;
            job.draw_calls += self.history.probes.stats.probe_draw_calls
                + self.history.probes.stats.env_draw_calls;
            job.record_ms += started.elapsed().as_secs_f32() * 1000.0;
            if job.order.is_empty() {
                self.history.probes.stats.record_ms = job.record_ms;
                log::info!(
                    "[modern] probes: capture {} done in {} frames: {} draw calls, {:.1} ms recording; {:?}",
                    self.history.probes.stats.captures,
                    job.frames,
                    job.draw_calls,
                    job.record_ms,
                    self.history.probes.stats
                );
            } else {
                self.history.probes.job = Some(job);
            }
        }
        let least_metal = 0.0;
        let (ambient, ambient_dims) = self.history.ambient.shader_params();
        let (grid, captured) = match &self.history.probes.grid {
            Some(g) => (g.clone(), self.history.probes.captured.is_some()),
            None => (ProbeGrid::default(), false),
        };
        let uniforms = ProbeUniforms {
            origin: [origin[0], origin[1], origin[2], 0.0],
            grid: [
                grid.x0,
                grid.z0,
                1.0 / crate::lighting::probes::ZONE,
                crate::lighting::probes::NORMAL_BIAS,
            ],
            dims: [
                grid.nx.max(1) as u32,
                grid.nz.max(1) as u32,
                (grid.nx * grid.nz) as u32,
                debug,
            ],
            env: [
                (crate::lighting::probes::ENV_MIPS - 1) as f32,
                if captured && grid.nx > 0 { 1.0 } else { 0.0 },
                least_metal,
                0.0,
            ],
            ambient,
            ambient_dims,
        };
        queue.write_buffer(
            &self.scene_resources.lights.probes.uniforms,
            0,
            bytemuck::bytes_of(&uniforms),
        );
    }

    /// Record one frame of a capture: probes `subset` of `grid` (their
    /// draws, culling and slots), and with `first` the environment cube and
    /// the global probe (whose SH also fills every probe until captured).
    pub(crate) fn record_capture(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        frame: &FrameUniforms,
        grid: &ProbeGrid,
        subset: &[usize],
        first: bool,
    ) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        let started = std::time::Instant::now();
        if self.history.probes.pipes.is_none() {
            self.history.probes.pipes =
                Some(CapturePipes::new(device, queue, &self.encoding_inputs()));
        }
        let count = grid.nx * grid.nz;
        // The global probe with the environment cube: over the camera
        // target (camera-local 0), in the first frame.
        let env_eye = [0.0, -crate::lighting::probes::ENV_EYE_HEIGHT, 0.0];
        let mut eyes: Vec<[f32; 3]> = subset
            .iter()
            .map(|&i| {
                let p = grid.positions[i];
                [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]]
            })
            .collect();
        let mut targets: Vec<u32> = subset.iter().map(|&i| i as u32).collect();
        if first {
            eyes.push(env_eye);
            targets.push(count as u32);
        }
        let rows = eyes.len() as u32;
        let res = crate::lighting::probes::CAPTURE_RES;
        if self
            .history
            .probes
            .atlas
            .as_ref()
            .is_none_or(|a| a.rows < rows)
        {
            let size = wgpu::Extent3d {
                width: 6 * res,
                height: rows * res,
                depth_or_array_layers: 1,
            };
            let texture = |label, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            self.history.probes.atlas = Some(Atlas {
                rows,
                colour: texture(
                    "modern probe atlas",
                    CAPTURE_FORMAT,
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                ),
                depth: texture(
                    "modern probe atlas depth",
                    DEPTH_FORMAT,
                    wgpu::TextureUsages::RENDER_ATTACHMENT,
                )
                .create_view(&Default::default()),
            });
        }

        let CaptureCandidates {
            locs,
            spheres,
            parts,
        } = self.capture_candidates(prep);

        // Culling: per probe (the global one too) and the environment
        // cube, the candidates within reach and at least about a texel wide
        // (`crate::lighting::probes::SMALL_ANGLE`), then per face its view pyramid.
        let cull = |eye: [f32; 3], far: f32, angle: f32| cull_faces(&spheres, eye, far, angle);
        let candidates: Vec<[Vec<u32>; 6]> = eyes
            .iter()
            .map(|&eye| {
                cull(
                    eye,
                    crate::lighting::probes::CAPTURE_RADIUS,
                    crate::lighting::probes::SMALL_ANGLE,
                )
            })
            .collect();
        let env_candidates = if first {
            cull(
                env_eye,
                crate::lighting::probes::ENV_RADIUS,
                crate::lighting::probes::ENV_SMALL_ANGLE,
            )
        } else {
            Default::default()
        };

        // The draws of the candidates some face sees (a loc's model uploaded
        // only then): each candidate's range of capture draws.
        let mut used = vec![false; spheres.len()];
        for list in candidates
            .iter()
            .chain(std::iter::once(&env_candidates))
            .flatten()
        {
            for &i in list {
                used[i as usize] = true;
            }
        }
        let culled = std::time::Instant::now();
        let BuiltDraws {
            draws,
            ranges,
            loc_count,
            floor_batches,
        } = self.capture_draws(prep, &locs, &parts, &used);
        let uploaded = std::time::Instant::now();
        log::debug!(
            "[modern] probes: culling {:.1} ms, loc draws and uploads {:.1} ms",
            (culled - started).as_secs_f32() * 1000.0,
            (uploaded - culled).as_secs_f32() * 1000.0
        );
        // Every floor batch of every level is among the candidates (the
        // check below).
        let mut part_batches: Vec<(usize, usize)> = parts.iter().map(|p| (p.0, p.1)).collect();
        part_batches.dedup();
        let expand = |lists: &[Vec<u32>; 6]| -> [Vec<u32>; 6] {
            std::array::from_fn(|face| {
                lists[face]
                    .iter()
                    .flat_map(|&i| {
                        let (a, b) = ranges[i as usize];
                        a..b
                    })
                    .collect()
            })
        };
        let faces: Vec<[Vec<u32>; 6]> = candidates.iter().map(expand).collect();
        let env_faces = expand(&env_candidates);

        // The frame slots: probe faces, environment faces, sky faces.
        let near = crate::lighting::probes::CAPTURE_NEAR;
        let slot = |eye: [f32; 3], face: usize, far: f32, sky: bool| {
            capture_frame(frame, eye, face, (near, far), sky)
        };
        let mut slots = Vec::with_capacity(eyes.len() * 6 + 12);
        for &eye in &eyes {
            for face in 0..6 {
                slots.push(slot(
                    eye,
                    face,
                    crate::lighting::probes::CAPTURE_RADIUS,
                    false,
                ));
            }
        }
        for face in 0..6 {
            slots.push(slot(env_eye, face, crate::lighting::probes::ENV_FAR, false));
        }
        for face in 0..6 {
            slots.push(slot([0.0; 3], face, crate::lighting::probes::ENV_FAR, true));
        }
        let sky_models = self.capture_sky_models(device, queue, snapshot);
        let frames = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern probe frames"),
            contents: bytemuck::cast_slice(&slots),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pipes = self
            .history
            .probes
            .pipes
            .as_ref()
            .expect("capture pipelines");
        let frame_layout = self.pipes().forward.get_bind_group_layout(0);
        let binds = (0..slots.len())
            .map(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("modern probe frame"),
                    layout: &frame_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &frames,
                                offset: i as u64 * SLOT,
                                size: wgpu::BufferSize::new(SLOT),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&pipes.ao_white),
                        },
                    ],
                })
            })
            .collect();
        // Each face's winding against the frame's (a mirrored face draws
        // with the clockwise pipelines).
        let winding = face_windings(frame, near);

        // The sky: the layers per target and face.
        let (sky_layers, sky_draws) = self.capture_sky_layers(device, snapshot);

        let project = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern probe projection"),
            layout: &pipes.project_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .history
                            .probes
                            .atlas
                            .as_ref()
                            .expect("atlas")
                            .colour
                            .create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.scene_resources.lights.probes.sh.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("modern probe projection"),
                            contents: bytemuck::bytes_of(&ProjectUniforms {
                                params: [
                                    rows as f32,
                                    res as f32,
                                    1.0,
                                    crate::post::tonemap::EXPOSURE,
                                ],
                                fill: [if first { count as f32 } else { 0.0 }, 0.0, 0.0, 0.0],
                            }),
                            usage: wgpu::BufferUsages::UNIFORM,
                        })
                        .as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("modern probe targets"),
                            contents: bytemuck::cast_slice(&targets),
                            usage: wgpu::BufferUsages::STORAGE,
                        })
                        .as_entire_binding(),
                },
            ],
        });

        // The environment cube's filter passes (slots: the downsample's
        // faces per level, then the prefilter's; the roughness per level
        // is sqrt(level / (levels - 1)), as the IBL specular reads it).
        let mips = crate::lighting::probes::ENV_MIPS;
        let mut params = Vec::new();
        for _ in 1..mips {
            for face in 0..6 {
                params.push([face as f32, 0.0, 0.0, 0.0]);
            }
        }
        for level in 1..mips {
            let rough = (level as f32 / (mips - 1) as f32).sqrt();
            for face in 0..6 {
                params.push([
                    face as f32,
                    crate::lighting::probes::ENV_RES as f32,
                    rough,
                    level as f32,
                ]);
            }
        }
        let filter_slots = FilterGpu::slots(device, &params);
        let downsample = (1..mips)
            .map(|level| {
                pipes.filters.bind(
                    device,
                    &filter_slots,
                    &cube_view(&pipes.env_source, level - 1, Some(1)),
                )
            })
            .collect();
        let prefilter = pipes.filters.bind(
            device,
            &filter_slots,
            &cube_view(&pipes.env_source, 0, None),
        );

        self.history.probes.stats.nx = grid.nx;
        self.history.probes.stats.nz = grid.nz;
        self.history.probes.stats.locs = loc_count;
        self.history.probes.stats.floor_batches = floor_batches;
        self.history.probes.stats.probe_draw_calls = faces
            .iter()
            .map(|f| f.iter().map(Vec::len).sum::<usize>())
            .sum();
        self.history.probes.stats.env_draw_calls = env_faces.iter().map(Vec::len).sum();
        if crate::modern_debug_flags::flags().check {
            // CLIENT910_MODERN_CHECK: every probe is captured (a face row per
            // probe plus the global one), every floor batch of every level
            // is a capture draw once.
            let expected: usize = snapshot
                .floors
                .iter()
                .enumerate()
                .filter_map(|(l, g)| {
                    let g = g.as_ref()?;
                    let gpu = self.scene_resources.floors.get(l)?.as_ref()?;
                    if g.vertex_count == 0 || !gpu.token.matches(g) {
                        return None;
                    }
                    let all: Vec<usize> = (0..g.tiles_x * g.tiles_z).collect();
                    Some(
                        g.batches
                            .iter()
                            .filter(|b| !b.build_indices(g, &all).0.is_empty())
                            .count(),
                    )
                })
                .sum();
            if part_batches.len() != expected || faces.len() != subset.len() + usize::from(first) {
                log::warn!(
                    "[modern] probe check: {} floor batches in the capture of {expected}, {} probe rows for {} probes",
                    part_batches.len(),
                    faces.len(),
                    subset.len() + usize::from(first)
                );
            } else {
                log::info!(
                    "[modern] probe check: {} probes of {count} + the global one, {floor_batches} floor batches, {loc_count} locs",
                    subset.len()
                );
            }
        }
        self.history.probes.capture = Some(Capture {
            scene: SceneCapture {
                draws,
                sky_layers,
                sky_draws,
                sky_models,
                winding,
            },
            faces,
            env_faces,
            _frames: frames,
            binds,
            probes: rows,
            project,
            downsample,
            prefilter,
            env: first,
        });
    }

    /// The sky layers of the captures (`crate::frame::gpu::sky_layers::prepare_sky`'s layers per
    /// target and face: the sky box's tiles at the face's pitch and
    /// yaw, the face as the viewport).
    pub(crate) fn capture_sky_layers(
        &self,
        device: &wgpu::Device,
        snapshot: &SceneSnapshot<'_>,
    ) -> (Option<(wgpu::Buffer, wgpu::BindGroup)>, SkyFaceDraws) {
        use crate::skybox::SkyLayer;
        let mut layers: Vec<SkyLayerUniforms> = Vec::new();
        let mut draws: SkyFaceDraws = Default::default();
        let Some(sky) = snapshot.sky else {
            return (None, draws);
        };
        let argb = |c: u32| {
            [
                ((c >> 16) & 0xff) as f32 / 255.0,
                ((c >> 8) & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
                ((c >> 24) & 0xff) as f32 / 255.0,
            ]
        };
        let cam_yaw = snapshot.camera.yaw_int();
        for (target, res) in [
            crate::lighting::probes::CAPTURE_RES,
            crate::lighting::probes::ENV_RES,
        ]
        .into_iter()
        .enumerate()
        {
            for (face, face_draws) in draws[target].iter_mut().enumerate() {
                let x = if target == 0 { face as u32 * res } else { 0 };
                let size = res as i32;
                let (face_pitch, face_yaw) = crate::lighting::probes::face_angles(face);
                let base = SkyLayerUniforms {
                    rect: [x as f32, 0.0, res as f32, res as f32],
                    params: [crate::post::tonemap::EXPOSURE, 0.0, 0.0, 0.0],
                    ..SkyLayerUniforms::default()
                };
                let mut push = |u: SkyLayerUniforms, texture| {
                    face_draws.push((layers.len() as u32, texture));
                    layers.push(u);
                };
                for layer in sky.layers {
                    match layer {
                        SkyLayer::Fill { argb: c, blend } => {
                            let mut colour = argb(*c);
                            if !blend {
                                colour[3] = 1.0;
                            }
                            push(SkyLayerUniforms { colour, ..base }, None);
                        }
                        SkyLayer::Clear { rgb } => {
                            let mut colour = argb(*rgb as u32);
                            colour[3] = 1.0;
                            push(SkyLayerUniforms { colour, ..base }, None);
                        }
                        // The probes see the sky's fills, textures and
                        // models; a decor is a small sprite over them.
                        SkyLayer::Model { .. } | SkyLayer::Decor { .. } => {}
                        SkyLayer::Material {
                            key,
                            material,
                            alpha,
                            first,
                            fog,
                            yaw,
                            fill,
                            ..
                        } => {
                            let multiply = snapshot
                                .materials
                                .and_then(|m| m.get(*material as u32))
                                .is_some_and(|m| m.alpha == crate::texture::AlphaMode::Multiply);
                            let translucent = *alpha != 255 || multiply;
                            if translucent && *first {
                                let mut colour = argb(*fog as u32);
                                colour[3] = 1.0;
                                push(SkyLayerUniforms { colour, ..base }, None);
                            }
                            let Some(texture) = self
                                .scene_resources
                                .sky_textures
                                .get(&SkyTextureKey::Material(*key))
                            else {
                                continue;
                            };
                            let horizon = *fill == Some(crate::skybox::SkyBoxFillMode::Horizon);
                            // The layer's yaw is the camera's plus the box's
                            // offset: the face's yaw plus that offset.
                            let yaw = (yaw - cam_yaw + face_yaw) & 0x3FFF;
                            let mut tile_y = size.wrapping_mul(face_pitch) / -4096;
                            let mut tile_x = size.wrapping_mul(yaw) / 4096;
                            tile_x = tile_x.rem_euclid(size);
                            if !horizon {
                                tile_y = tile_y.rem_euclid(size);
                            }
                            let tint = if translucent {
                                *alpha as f32 / 255.0
                            } else {
                                1.0
                            };
                            push(
                                SkyLayerUniforms {
                                    colour: [1.0, 1.0, 1.0, tint],
                                    tile: [
                                        tile_x as f32,
                                        tile_y as f32,
                                        size as f32,
                                        if horizon { 2.0 } else { 1.0 },
                                    ],
                                    top: argb(texture.first as u32),
                                    bottom: argb(texture.last as u32),
                                    ..base
                                },
                                Some(SkyTextureKey::Material(*key)),
                            );
                        }
                    }
                }
            }
        }
        if layers.is_empty() {
            return (None, draws);
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern probe sky layers"),
            contents: bytemuck::cast_slice(&layers),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern probe sky layers"),
            layout: &self.device_resources.sky_layer_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<SkyLayerUniforms>() as u64),
                }),
            }],
        });
        (Some((buffer, bind)), draws)
    }
}

/// What a capture can draw: the scene's locs (visible or not) and every
/// zone of every floor batch over all its tiles, with their bounding
/// spheres (camera-local; the locs first, then the floor parts).
pub(crate) struct CaptureCandidates<'a> {
    pub(crate) locs: Vec<crate::models::draw_list::EntityDraw<'a>>,
    pub(crate) spheres: Vec<(glam::Vec3, f32)>,
    /// Floor parts: (level, batch, first index, index count).
    pub(crate) parts: Vec<(usize, usize, u32, u32)>,
}

/// The capture draws recorded for the candidates some face sees: each
/// candidate's range of draws.
pub(crate) struct BuiltDraws {
    pub(crate) draws: Vec<CaptureDraw>,
    pub(crate) ranges: Vec<(u32, u32)>,
    pub(crate) loc_count: usize,
    pub(crate) floor_batches: usize,
}

/// The candidates within reach of `eye` (`far`, and at least `angle`
/// wide), then per face those in its view pyramid.
pub(crate) fn cull_faces(
    spheres: &[(glam::Vec3, f32)],
    eye: [f32; 3],
    far: f32,
    angle: f32,
) -> [Vec<u32>; 6] {
    let e = glam::Vec3::from(eye);
    let near: Vec<u32> = spheres
        .iter()
        .enumerate()
        .filter(|(_, (c, r))| {
            let d = (*c - e).length();
            d - r < far && *r >= angle * d
        })
        .map(|(i, _)| i as u32)
        .collect();
    std::array::from_fn(|face| {
        near.iter()
            .copied()
            .filter(|&i| {
                let (c, r) = spheres[i as usize];
                in_face(face, c - e, r, far)
            })
            .collect()
    })
}

impl Capture {
    /// The bind group of the sky's frame slot of `face` (the last six).
    pub(crate) fn sky_bind(&self, face: usize) -> &wgpu::BindGroup {
        &self.binds[self.binds.len() - 6 + face]
    }
}

/// Each face's winding against the frame's (a mirrored face draws with the
/// clockwise pipelines).
pub(crate) fn face_windings(frame: &FrameUniforms, near: f32) -> [usize; 6] {
    let main = glam::Mat4::from_cols_array_2d(&frame.view_proj).determinant();
    std::array::from_fn(|face| {
        let d = crate::lighting::probes::face_view_proj(face, [0.0; 3], near, 1000.0).determinant();
        usize::from((d > 0.0) != (main > 0.0))
    })
}

/// The frame block of a capture face seen from camera-local `eye` (`sky`:
/// the sky's rotation-only view): no SSAO, the sky at the capture's own
/// exposure (the frame's exposure follows the modern composite; the capture
/// keeps the display encode it projects).
pub(crate) fn capture_frame(
    frame: &FrameUniforms,
    eye: [f32; 3],
    face: usize,
    (near, far): (f32, f32),
    sky: bool,
) -> FrameUniforms {
    let mut f = *frame;
    f.view_proj = crate::lighting::probes::face_view_proj(face, eye, near, far).to_cols_array_2d();
    f.view = crate::lighting::probes::face_view(face, eye).to_cols_array_2d();
    f.eye = [eye[0], eye[1], eye[2], frame.eye[3]];
    f.params = [crate::post::tonemap::EXPOSURE, 0.0, 0.0, 0.0];
    if sky {
        f.view = glam::Mat4::IDENTITY.to_cols_array_2d();
        f.eye = [0.0, 0.0, 0.0, 0.0];
    }
    f
}

impl ModernRenderer {
    /// The sky models of a capture (the sky box's dome, drawn from the face's eye under the
    /// face's own rotation): recorded here, for a capture, and not in every frame: nothing else
    /// of the frame reads them (the sky cube's bake draws its own).
    pub(crate) fn capture_sky_models(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
    ) -> Vec<Draw> {
        use crate::skybox::SkyLayer;
        let mut sky_models = Vec::new();
        let (Some(sky), Some(materials)) = (snapshot.sky, snapshot.materials) else {
            return sky_models;
        };
        for layer in sky.layers {
            let SkyLayer::Model { key, .. } = layer else {
                continue;
            };
            let Some(model) = sky.model(*key) else {
                continue;
            };
            let Some(streams) =
                crate::models::mesh::model_streams(model, materials, Colour::Classic)
            else {
                continue;
            };
            let (base_vertex, first) = self.frame_resources.arena.push(&streams);
            for &(material, start, count) in &streams.batches {
                self.device_resources.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    Some(materials),
                    material,
                );
                let instance = self.frame_resources.instances.len() as u32;
                self.frame_resources.instances.push(self.instance(
                    [
                        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                    ],
                    material,
                    1.0,
                    FLAG_SKY | FLAG_UNLIT,
                ));
                sky_models.push(Draw {
                    geometry: Geometry::Arena { base_vertex },
                    material,
                    first_index: start + first,
                    count,
                    instance,
                    pass: Pass::NoDepthWrite,
                    casts: false,
                    indirect: None,
                });
            }
        }
        sky_models
    }

    /// The capture candidates of this frame's scene (the loc radii cached
    /// per scene).
    pub(crate) fn capture_candidates<'a>(
        &mut self,
        prep: &PrepareFrame<'_, 'a>,
    ) -> CaptureCandidates<'a> {
        let PrepareFrame {
            device,
            snapshot,
            origin,
            ..
        } = *prep;
        // The candidates: every loc of the scene (visible or not) and every
        // zone of every floor batch over all its tiles, with their bounding
        // spheres (camera-local).
        if self.history.probes.radii_scene != self.scene_resources.scene_token {
            self.history.probes.radii.clear();
            self.history.probes.radii_scene = self.scene_resources.scene_token;
        }
        let locs = crate::models::draw_list::DrawList::scene_locs(snapshot);
        let mut spheres: Vec<(glam::Vec3, f32)> = Vec::with_capacity(locs.len());
        for entity in &locs {
            let radius = match entity.key {
                Some(key) => {
                    let slot = self
                        .history
                        .probes
                        .radii
                        .entry(crate::frame::resources::loc_slot(&key))
                        .or_insert_with(|| (key, loc_radius(entity.model, &entity.matrix)));
                    if slot.0 != key {
                        *slot = (key, loc_radius(entity.model, &entity.matrix));
                    }
                    slot.1
                }
                None => loc_radius(entity.model, &entity.matrix),
            };
            let m = &entity.matrix;
            spheres.push((
                glam::Vec3::new(m[12] - origin[0], m[13] - origin[1], m[14] - origin[2]),
                radius,
            ));
        }
        // Floor parts: (level, batch, first index, index count).
        let mut parts: Vec<(usize, usize, u32, u32)> = Vec::new();
        for (level, g) in snapshot.floors.iter().enumerate() {
            let Some(g) = g.as_ref() else {
                continue;
            };
            let Some(Some(gpu)) = self.scene_resources.floors.get(level) else {
                continue;
            };
            // Only a level this scene uploaded (a level left over from the
            // previous scene holds other batches).
            if g.vertex_count == 0 || !gpu.token.matches(g) {
                continue;
            }
            if self.history.probes.floors.len() <= level {
                self.history.probes.floors.resize_with(level + 1, || None);
            }
            if self.history.probes.floors[level]
                .as_ref()
                .is_none_or(|f| !f.token.matches(g))
            {
                let stride = g.stride_floats;
                let zone = (crate::lighting::probes::ZONE / 512.0) as usize;
                let chunks: Vec<Vec<usize>> = (0..g.tiles_z.div_ceil(zone))
                    .flat_map(|cz| (0..g.tiles_x.div_ceil(zone)).map(move |cx| (cx, cz)))
                    .map(|(cx, cz)| {
                        let mut tiles = Vec::new();
                        for z in cz * zone..((cz + 1) * zone).min(g.tiles_z) {
                            for x in cx * zone..((cx + 1) * zone).min(g.tiles_x) {
                                tiles.push(z * g.tiles_x + x);
                            }
                        }
                        tiles
                    })
                    .collect();
                let batches = gpu
                    .batches
                    .iter()
                    .map(|b| {
                        let mut indices: Vec<u16> = Vec::new();
                        let mut ranges = Vec::new();
                        for tiles in &chunks {
                            let (part, _, _) = g.batches[b.source].build_indices(g, tiles);
                            if part.is_empty() {
                                continue;
                            }
                            let (mut lo, mut hi) =
                                (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
                            for &i in &part {
                                let f = &g.stream0
                                    [usize::from(i) * stride..usize::from(i) * stride + 3];
                                let p = glam::Vec3::new(f[0], f[1], f[2]);
                                lo = lo.min(p);
                                hi = hi.max(p);
                            }
                            let centre = (lo + hi) * 0.5;
                            ranges.push((
                                indices.len() as u32,
                                part.len() as u32,
                                [centre.x, centre.y, centre.z, (hi - lo).length() * 0.5],
                            ));
                            indices.extend_from_slice(&part);
                        }
                        if indices.is_empty() {
                            return None;
                        }
                        if !indices.len().is_multiple_of(2) {
                            indices.push(0);
                        }
                        Some((
                            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("modern probe floor indices"),
                                contents: bytemuck::cast_slice(&indices),
                                usage: wgpu::BufferUsages::INDEX,
                            }),
                            ranges,
                        ))
                    })
                    .collect();
                self.history.probes.floors[level] = Some(FullFloor {
                    token: crate::frame::resources::FloorToken::of(g),
                    batches,
                });
            }
            let full = self.history.probes.floors[level]
                .as_ref()
                .expect("full floor");
            for (batch, entry) in full.batches.iter().enumerate() {
                for &(first, count, s) in entry.iter().flat_map(|(_, r)| r) {
                    parts.push((level, batch, first, count));
                    spheres.push((
                        glam::Vec3::new(s[0] - origin[0], s[1] - origin[1], s[2] - origin[2]),
                        s[3],
                    ));
                }
            }
        }

        CaptureCandidates {
            locs,
            spheres,
            parts,
        }
    }

    /// The draws of the candidates `used` marks (a loc's model uploaded
    /// only then).
    pub(crate) fn capture_draws(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        locs: &[crate::models::draw_list::EntityDraw<'_>],
        parts: &[(usize, usize, u32, u32)],
        used: &[bool],
    ) -> BuiltDraws {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        // The seen locs' meshes not cached yet, built on the threads
        // (`frame::prebuild`).
        self.prebuild_locs(
            snapshot,
            locs.iter()
                .enumerate()
                .filter(|&(i, _)| used[i])
                .map(|(_, e)| e),
        );
        let mut draws = Vec::new();
        let mut ranges = vec![(0_u32, 0_u32); locs.len() + parts.len()];
        let mut loc_count = 0;
        for (i, entity) in locs.iter().enumerate() {
            if !used[i] {
                continue;
            }
            let start = self.frame_resources.draws.len();
            self.prepare_entity(device, queue, snapshot, entity, origin);
            let recorded: Vec<Draw> = self.frame_resources.draws.drain(start..).collect();
            if !recorded.is_empty() {
                loc_count += 1;
            }
            let first = draws.len() as u32;
            draws.extend(recorded.into_iter().map(CaptureDraw::Loc));
            ranges[i] = (first, draws.len() as u32);
        }
        let floor_matrix = local_matrix(
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            origin,
        );
        let mut floor_instances: HashMap<(usize, usize), u32> = HashMap::new();
        for (k, &(level, batch, first, count)) in parts.iter().enumerate() {
            let i = locs.len() + k;
            if !used[i] {
                continue;
            }
            let instance = match floor_instances.get(&(level, batch)) {
                Some(&instance) => instance,
                None => {
                    let Some(b) = self
                        .scene_resources
                        .floors
                        .get(level)
                        .and_then(Option::as_ref)
                        .and_then(|f| f.batches.get(batch))
                    else {
                        continue;
                    };
                    let (material, uv_scale) = (b.material, b.uv_scale);
                    let instance = self.frame_resources.instances.len() as u32;
                    let mut record = self.instance(floor_matrix, material, uv_scale, FLAG_FLOOR);
                    record.p2 = [level as f32, 0.0, 0.0, 0.0];
                    self.frame_resources.instances.push(record);
                    floor_instances.insert((level, batch), instance);
                    instance
                }
            };
            let at = draws.len() as u32;
            draws.push(CaptureDraw::Floor {
                level,
                batch,
                instance,
                first,
                count,
            });
            ranges[i] = (at, at + 1);
        }
        let floor_batches = floor_instances.len();
        BuiltDraws {
            draws,
            ranges,
            loc_count,
            floor_batches,
        }
    }
}

impl CapturePipes {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        r: &EncodeInputs<'_>,
    ) -> Self {
        // The forward module (compiled once, `crate::frame::pipelines::PipelineInputs`).
        let module = &r.pipeline_inputs.forward_module;
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern probe capture"),
            bind_group_layouts: &[
                Some(&r.pipes().forward.get_bind_group_layout(0)),
                Some(&r.pipes().forward.get_bind_group_layout(1)),
                Some(&r.pipes().forward.get_bind_group_layout(2)),
                Some(&r.pipes().forward.get_bind_group_layout(3)),
                Some(&r.pipes().forward.get_bind_group_layout(4)),
            ],
            immediate_size: 0,
        });
        let layouts = vertex_layouts();
        // The capture shades with the ambient (the probes it fills are
        // not read while they are captured).
        let fs = "fs_forward_ambient";
        let pipeline = |label: &str, cw: bool, depth: Option<bool>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_forward"),
                    buffers: &layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: CAPTURE_FORMAT,
                        blend: Some(ALPHA_BLEND),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: if cw {
                        wgpu::FrontFace::Cw
                    } else {
                        wgpu::FrontFace::Ccw
                    },
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth.unwrap_or(false)),
                    depth_compare: Some(if depth.is_some() {
                        wgpu::CompareFunction::LessEqual
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sky_module = r.shaders.get(device, crate::shaders::Module::SkyLayers);
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern probe sky"),
            bind_group_layouts: &[Some(r.sky_layer_layout), Some(r.sky_texture_layout)],
            immediate_size: 0,
        });
        let sky_layer = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern probe sky layer"),
                layout: Some(&sky_layout),
                vertex: wgpu::VertexState {
                    module: &sky_module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sky_module,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: CAPTURE_FORMAT,
                        blend: Some(ALPHA_BLEND),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let project_module = r
            .shaders
            .get(device, crate::shaders::Module::ProbeProjection);
        let project_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern probe projection"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let project = || {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("modern probe projection"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("modern probe projection"),
                        bind_group_layouts: &[Some(&project_layout)],
                        immediate_size: 0,
                    }),
                ),
                module: &project_module,
                entry_point: Some("cs_project"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        // The capture, sky, projection and filter pipelines are created at
        // once (`frame::compile`).
        let pipeline = &pipeline;
        let model_jobs: Vec<crate::frame::compile::Job<'_, wgpu::RenderPipeline>> = vec![
            Box::new(move || pipeline("modern probe capture", false, Some(false))),
            Box::new(move || pipeline("modern probe capture", false, Some(true))),
            Box::new(move || pipeline("modern probe capture", true, Some(false))),
            Box::new(move || pipeline("modern probe capture", true, Some(true))),
            Box::new(move || pipeline("modern probe sky model", false, None)),
            Box::new(move || pipeline("modern probe sky model", true, None)),
        ];
        let (models, (sky_layer, (project, filters))) = crate::frame::compile::join(
            || crate::frame::compile::all(model_jobs),
            || {
                crate::frame::compile::join(sky_layer, || {
                    crate::frame::compile::join(project, || FilterGpu::new(device, r.shaders))
                })
            },
        );
        let mut models = models.into_iter();
        let mut next = || models.next().expect("every pipeline is built");
        let forward = [[next(), next()], [next(), next()]];
        let sky_model = [next(), next()];
        let no_shadow_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern probe no shadow"),
            contents: bytemuck::bytes_of(&ShadowUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let no_shadow = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern probe no shadow"),
            layout: &r.pipes().forward.get_bind_group_layout(2),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: no_shadow_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&r.shadow.atlas.1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&r.shadow.sampler),
                },
            ],
        });
        let depth = |label, w, h| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let res = crate::lighting::probes::CAPTURE_RES;
        let sky_atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modern probe sky"),
            size: wgpu::Extent3d {
                width: 6 * res,
                height: res,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CAPTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            forward,
            sky_model,
            sky_layer,
            project,
            project_layout,
            filters,
            no_shadow,
            ao_white: crate::frame::gpu::post::white_view(device, queue),
            env_source: cube_texture(
                device,
                "modern environment capture",
                crate::lighting::probes::ENV_RES,
                crate::lighting::probes::ENV_MIPS,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
            ),
            env_depth: depth(
                "modern environment capture depth",
                crate::lighting::probes::ENV_RES,
                crate::lighting::probes::ENV_RES,
            ),
            sky_atlas,
            sky_depth: depth("modern probe sky depth", 6 * res, res),
        }
    }
}

#[cfg(test)]
impl ModernRenderer {
    /// Tests: whether a capture is in progress (the probes', or the
    /// per-square ambient's when it is on).
    pub(crate) fn probe_capture_pending(&self) -> bool {
        self.history.probes.job.is_some() || self.ambient_pending(self.frame_millis())
    }
}

impl<'a> EncodeInputs<'a> {
    /// Draw one capture draw (`bound`: what the face's draws have bound,
    /// `frame::submit`).
    pub(crate) fn draw_capture<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'p>,
        d: &CaptureDraw,
        bound: &mut crate::frame::submit::Bound,
    ) {
        match *d {
            CaptureDraw::Loc(ref d) => self.submit(pass, d, bound),
            CaptureDraw::Floor {
                level,
                batch,
                instance,
                first,
                count,
            } => {
                let (Some(Some(floor)), Some(Some(full)), Some((instances, _))) = (
                    self.floors.get(level),
                    self.probes.floors.get(level),
                    self.instance_buffer.as_ref(),
                ) else {
                    return;
                };
                let (Some(b), Some(Some((indices, _)))) =
                    (floor.batches.get(batch), full.batches.get(batch))
                else {
                    return;
                };
                let Some(material) = self.textures.get(b.material) else {
                    return;
                };
                bound.reset();
                pass.set_bind_group(1, &material.bind_group, &[]);
                pass.set_bind_group(4, &self.textures.arrays.bind, &[]);
                pass.set_vertex_buffer(0, floor.vertices.slice(..));
                pass.set_vertex_buffer(1, b.colours.slice(..));
                pass.set_vertex_buffer(2, instances.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(first..first + count, 0, instance..instance + 1);
            }
        }
    }

    /// Encode this frame's capture (before the frame's passes).
    pub(crate) fn encode_probes(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(capture), Some(pipes), Some(atlas)) = (
            self.probes.capture.as_ref(),
            self.probes.pipes.as_ref(),
            self.probes.atlas.as_ref(),
        ) else {
            return;
        };
        let started = std::time::Instant::now();
        let res = crate::lighting::probes::CAPTURE_RES;
        let clear = wgpu::Color {
            r: f64::from(self.clear[0]),
            g: f64::from(self.clear[1]),
            b: f64::from(self.clear[2]),
            a: 1.0,
        };
        // 1. The sky's six faces at the probe size.
        {
            let view = pipes.sky_atlas.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::ProbeSky)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &pipes.sky_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            for face in 0..6 {
                let x = face as u32 * res;
                pass.set_viewport(x as f32, 0.0, res as f32, res as f32, 0.0, 1.0);
                pass.set_scissor_rect(x, 0, res, res);
                self.draw_capture_sky(&mut pass, &capture.scene, capture.sky_bind(face), 0, face);
            }
        }
        // 2. Every probe's faces: the sky under them, then the scene.
        for row in 0..capture.probes {
            encoder.copy_texture_to_texture(
                pipes.sky_atlas.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas.colour,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: row * res,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: 6 * res,
                    height: res,
                    depth_or_array_layers: 1,
                },
            );
        }
        {
            let view = atlas.colour.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::ProbeCapture)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &atlas.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_bind_group(2, &pipes.no_shadow, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            for (row, faces) in capture.faces.iter().enumerate() {
                for (face, list) in faces.iter().enumerate() {
                    let (x, y) = (face as u32 * res, row as u32 * res);
                    pass.set_viewport(x as f32, y as f32, res as f32, res as f32, 0.0, 1.0);
                    pass.set_scissor_rect(x, y, res, res);
                    pass.set_bind_group(0, &capture.binds[row * 6 + face], &[]);
                    self.draw_face(&mut pass, &capture.scene, list, face);
                }
            }
        }
        // 3. The projection into the SH the forward pass reads.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::ProbeProjection)),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipes.project);
            pass.set_bind_group(0, &capture.project, &[]);
            pass.dispatch_workgroups(capture.probes, 1, 1);
        }
        if !capture.env {
            log::debug!(
                "[modern] probes: capture frame encoded in {:.1} ms",
                started.elapsed().as_secs_f32() * 1000.0
            );
            return;
        }
        // 4. The environment cube's faces at its size.
        let env_slot = capture.faces.len() * 6;
        for face in 0..6 {
            let view = face_view(&pipes.env_source, face as u32, 0);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::EnvironmentCapture)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &pipes.env_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            self.draw_capture_sky(&mut pass, &capture.scene, capture.sky_bind(face), 1, face);
            pass.set_bind_group(0, &capture.binds[env_slot + face], &[]);
            pass.set_bind_group(2, &pipes.no_shadow, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            self.draw_face(&mut pass, &capture.scene, &capture.env_faces[face], face);
        }
        // 5. Its mip chain, then the GGX prefilter into the bound cube
        // (level 0 copied: roughness 0).
        let mips = crate::lighting::probes::ENV_MIPS;
        let size = crate::lighting::probes::ENV_RES;
        let mut slot = 0;
        for level in 1..mips {
            for face in 0..6 {
                let target = face_view(&pipes.env_source, face, level);
                FilterGpu::pass(
                    encoder,
                    &pipes.filters.downsample,
                    &capture.downsample[level as usize - 1],
                    slot,
                    &target,
                );
                slot += 1;
            }
        }
        encoder.copy_texture_to_texture(
            pipes.env_source.as_image_copy(),
            self.lights.probes.env.as_image_copy(),
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
        );
        for level in 1..mips {
            for face in 0..6 {
                let target = face_view(&self.lights.probes.env, face, level);
                FilterGpu::pass(
                    encoder,
                    &pipes.filters.prefilter,
                    &capture.prefilter,
                    slot,
                    &target,
                );
                slot += 1;
            }
        }
        log::debug!(
            "[modern] probes: capture frame encoded in {:.1} ms",
            started.elapsed().as_secs_f32() * 1000.0
        );
    }

    /// One capture face's draws (`list`), the pipeline by the face's
    /// winding and each draw's depth writes.
    pub(crate) fn draw_face<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'p>,
        capture: &'p SceneCapture,
        list: &[u32],
        face: usize,
    ) {
        let pipes = self.probes.pipes.as_ref().expect("capture pipelines");
        let mut current = None;
        let mut bound = crate::frame::submit::Bound::default();
        for &i in list {
            let d = &capture.draws[i as usize];
            let write = match d {
                CaptureDraw::Loc(d) => usize::from(d.pass == Pass::Opaque),
                CaptureDraw::Floor { .. } => 1,
            };
            if current != Some(write) {
                pass.set_pipeline(&pipes.forward[capture.winding[face]][write]);
                current = Some(write);
            }
            self.draw_capture(pass, d, &mut bound);
        }
    }

    /// The sky of one capture face: its layers, then the sky models.
    pub(crate) fn draw_capture_sky<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'p>,
        capture: &'p SceneCapture,
        sky_bind: &'p wgpu::BindGroup,
        target: usize,
        face: usize,
    ) {
        let pipes = self.probes.pipes.as_ref().expect("capture pipelines");
        if let Some((_, layers)) = capture.sky_layers.as_ref() {
            pass.set_pipeline(&pipes.sky_layer);
            for &(slot, texture) in &capture.sky_draws[target][face] {
                pass.set_bind_group(
                    0,
                    layers,
                    &[slot * std::mem::size_of::<SkyLayerUniforms>() as u32],
                );
                let texture = texture
                    .and_then(|k| self.sky_textures.get(&k))
                    .map_or(self.sky_white, |t| &t.bind_group);
                pass.set_bind_group(1, texture, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if !capture.sky_models.is_empty() {
            pass.set_pipeline(&pipes.sky_model[capture.winding[face]]);
            pass.set_bind_group(0, sky_bind, &[]);
            pass.set_bind_group(2, &pipes.no_shadow, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            self.submit_all(pass, &capture.sky_models);
        }
    }
}
