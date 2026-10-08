//! The modern scene renderer (`--renderer modern`): draws one
//! [`SceneSnapshot`] into the scene viewport of a frame the faithful toolkit
//! composes (the UI below and over it stays the faithful painter's,
//! `Renderer::frame_composite`).
//!
//! # Frame
//!
//! [`passes`] declares the frame's passes in order, with their resolution,
//! sample count, inputs, outputs and resolves; [`ModernRenderer::prepare_frame`]
//! prepares the frame's data before drawable acquisition, then
//! [`ModernRenderer::record_frame`] records ordered units for the caller's
//! submission with the post chain. [`ModernRenderer::draw`] combines those
//! steps for callers with an eager scene submission. The passes are:
//!
//! 1. **Probes**: when the scene or the settled environment changes, a
//!    light-probe capture (the probes' cubes, their SH, the environment cube
//!    and its GGX prefilter; [`crate::lighting::probes`]).
//! 2. **Sky**, the frame's background, before the shadows and the geometry:
//!    the sky decor sprites (if any) into the forward target, then the sky
//!    shading ([`crate::atmosphere::sky`]): the environment's cube, fogged.
//! 3. **Shadows**: the sun's cascades into the shadow atlas
//!    ([`crate::shadows`]: visible, off-screen and roof-hidden casters,
//!    terrain), then the point-light shadow faces.
//! 4. **Caustics**: the water's caustic rays and their light on the river
//!    bed ([`crate::water_body::caustics`]).
//! 5. **Depth pre-pass**: the opaque entities' depth.
//! 6. **Ambient occlusion**: the geometry pass, SSAO or HBAO and its blur
//!    ([`crate::post::ao`]); the forward pass's ambient reads the map.
//! 7. **Forward lighting** in the faithful
//!    draw order ([`crate::models::draw_list`]): terrain, opaque entities,
//!    floors, transparent entities, billboards and particles at their
//!    places; with water, split around the water's reflection and surface
//!    passes ([`crate::water_body`]). HDR (`Rgba16Float`), multisampled at
//!    the client's anti-aliasing level, resolved at the pass's end.
//! 8. **Atmosphere**: volumetric scattering and depth of field over the
//!    resolved frame ([`crate::atmosphere::volumetrics`],
//!    [`crate::post::dof`]).
//! 9. **Post**: scene
//!    luminance and eye adaptation, bloom, the composite (exposure, tonemap,
//!    grading) and FXAA into the frame's colour target, inside the scene
//!    viewport and its clip ([`crate::post`]).
//!
//! The passes record in encode units ([`units`]), each into its own command
//! encoder on the renderer's threads (`jobs`), submitted in this order; the
//! models posed each frame are posed on the same threads (`posing`).
//!
//! Lighting comes from the environment as the modern client derives it, the
//! sun turned per map square by map file 6 ([`crate::lighting::environment`]).
//!
//! # Resources
//!
//! As the backend contract allows (`rs910_scene::scene_snapshot`): floors
//! per level until the scene is reinstalled (identified by the floor's
//! retained construction calls, an `Arc` the cache holds), their batch
//! indices re-selected when the plan's selection changes; loc models by
//! [`EntityKey`] (a dynamic loc's key carries its model revision) while the
//! scene is installed; materials by id ([`crate::models::materials`]); the
//! skybox textures by box. Models posed this frame (NPC bodies,
//! projectiles, spot anims, players with their shadows and hint arrows) and
//! the sky models (their fade changes their alphas) go through a per-frame
//! arena. Every pipeline is created with the renderer (`frame::pipelines`);
//! nothing borrows the snapshot or the device past the call.

pub(crate) use std::collections::HashMap;

pub(crate) use wgpu::util::DeviceExt;

pub(crate) use crate::models::draw_list::{DrawList, EntityDraw, Kind};
pub(crate) use crate::models::materials::Textures;
pub(crate) use crate::models::mesh::{Colour, ModelStreams, Vertex};
pub(crate) use crate::scene_snapshot::{EntityKey, SceneSnapshot};
pub(crate) use crate::shadows::{
    CasterUniforms, Settings as ShadowSettings, ShadowFrame, ShadowUniforms,
};

/// The HDR forward target's format (`ForwardLighting`).
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The depth format (the faithful toolkit's, so the device's MSAA answer
/// for the HDR target holds).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Instance flag: unlit (the colour is used as is).
pub(crate) const FLAG_UNLIT: u32 = 4;
/// Instance flag: a sky model.
pub(crate) const FLAG_SKY: u32 = 8;
/// Instance flag: the cutout tests the texture's coverage only.
pub(crate) const FLAG_TEXTURE_CUTOUT: u32 = 16;
/// Instance flag: a floor (its point lights from the tile grid at level
/// `p2[0]`, M4).
pub(crate) const FLAG_FLOOR: u32 = 32;

/// The frame's colour target and the scene viewport in it (what the shell
/// hands over from the faithful composition).
pub struct Target<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a dyn rs910_gpu_device::uploads::Uploader,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub view: &'a wgpu::TextureView,
    /// `view`'s format (the tonemap encodes sRGB unless it is an sRGB
    /// format).
    pub format: wgpu::TextureFormat,
    /// `view`'s size in pixels.
    pub size: [u32; 2],
    /// The scene viewport `[x, y, w, h]` in `view` pixels.
    pub rect: [i32; 4],
    /// Its scissor `[l, t, r, b]`.
    pub clip: [i32; 4],
}

/// The scene viewport's inputs before the shell acquires a drawable.
#[derive(Clone, Copy)]
pub struct PrepareTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a dyn rs910_gpu_device::uploads::Uploader,
    pub size: [u32; 2],
    pub rect: [i32; 4],
    pub clip: [i32; 4],
}

/// Prepared resources for one scene frame. Recording consumes its viewport;
/// the renderer owns every resource and does not retain the snapshot.
/// Consume it with `record_frame`, or `frame_skipped` after a failed acquire.
#[must_use = "record this frame or restore its producer progress with frame_skipped"]
pub struct PreparedFrame {
    rect: [i32; 4],
    clip: [i32; 4],
    checkpoint: lifecycle::FrameCheckpoint,
}

/// The `Frame` uniform block of [`crate::shaders::FORWARD_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_colour: [f32; 4],
    pub sky_ambient: [f32; 4],
    pub ground_ambient: [f32; 4],
    pub fog_colour: [f32; 4],
    pub fog_range: [f32; 4],
    pub params: [f32; 4],
}

/// One draw's instance record (vertex stream 2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Instance {
    pub(crate) model: [f32; 16],
    /// UV scale, alpha reference, flags.
    pub(crate) p0: [f32; 4],
    /// UV scroll, specular power and strength.
    pub(crate) p1: [f32; 4],
    /// Point lights (M4): a model's four light slots (ids plus one, 0
    /// none), or a floor's level.
    pub(crate) p2: [f32; 4],
}

/// The camera-local frame of `snapshot` (view, projection, eye) and its
/// lighting ([`crate::lighting::environment::Lighting`] of the environment, the
/// sun towards the environment's direction), the fog range and the fog
/// colour through the inverse tonemap, and the time the material scroll
/// samples (the injected clock, `logic_clock`, so fixed-clock frames
/// repeat).
#[must_use]
pub fn frame_uniforms(snapshot: &SceneSnapshot<'_>, viewport: (i32, i32)) -> FrameUniforms {
    let dir = glam::Vec3::from(snapshot.env.sun.dir).normalize_or(glam::Vec3::new(0.0, -1.0, 0.0));
    let lighting = crate::lighting::environment::Lighting::from_env(snapshot.env, dir.to_array());
    frame_uniforms_lit(snapshot, viewport, &lighting)
}

/// [`frame_uniforms`] with the frame's `lighting` (the renderer's, whose
/// sun direction follows map file 6, M5).
#[must_use]
pub fn frame_uniforms_lit(
    snapshot: &SceneSnapshot<'_>,
    viewport: (i32, i32),
    lighting: &crate::lighting::environment::Lighting,
) -> FrameUniforms {
    let mut local = snapshot.camera.clone();
    local.viewport = viewport;
    local.target = [0; 3];
    let view = local.view_entries();
    let vp = crate::camera::multiply(&view, &local.projection());
    let view_proj = crate::camera::gl_to_wgpu_depth() * crate::camera::to_glam(&vp);
    let eye = local.eye();
    let env = snapshot.env;
    let (fog_colour, fog_range) = match env.fog.range {
        Some((start, end)) if end > start => {
            // The modern decode, not the clear's inverse tonemap.
            let c = crate::atmosphere::fog::fog_colour(env.fog.distance_colour);
            (
                [c[0], c[1], c[2], 1.0],
                [start, 1.0 / (end - start), 0.0, 0.0],
            )
        }
        _ => ([0.0; 4], [0.0; 4]),
    };
    let millis = snapshot
        .time_ms
        .unwrap_or_else(crate::logic_clock::monotonic_millis);
    let v4 = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
    FrameUniforms {
        view_proj: view_proj.to_cols_array_2d(),
        view: crate::camera::to_glam(&view).to_cols_array_2d(),
        eye: [
            eye[0] as f32,
            eye[1] as f32,
            eye[2] as f32,
            (millis.rem_euclid(128_000)) as f32 / 1000.0,
        ],
        sun_dir: v4(lighting.sun_dir),
        sun_colour: v4(lighting.sun_colour),
        sky_ambient: v4(lighting.sky_ambient),
        ground_ambient: v4(lighting.ground_ambient),
        fog_colour,
        fog_range,
        params: [crate::post::tonemap::EXPOSURE, 0.0, 0.0, 0.0],
    }
}

/// The camera-local model matrix of scene-local classic entries `m` (column
/// vectors, glam): the scene-local camera target `origin` subtracted.
pub(crate) fn local_matrix(m: &[f32; 16], origin: [f32; 3]) -> [f32; 16] {
    let mut out = *m;
    out[12] -= origin[0];
    out[13] -= origin[1];
    out[14] -= origin[2];
    out
}

/// Where a draw's geometry lives (`frame::submit`): a loc page and the
/// mesh's base vertex (`frame::arenas`), the per-frame arena, a floor batch.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Geometry {
    Loc {
        page: u16,
        base_vertex: i32,
    },
    /// A far-scene merged mesh chunk (`frame::gpu::far`): a page of the far
    /// scene's own loc arena and the chunk's base vertex.
    Far {
        page: u16,
        base_vertex: i32,
    },
    Arena {
        base_vertex: i32,
    },
    Floor {
        level: usize,
        batch: usize,
    },
}

/// Which pipeline a forward draw uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    Opaque,
    NoDepthWrite,
}

/// One recorded draw: a self-contained draw packet (`frame::submit`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Draw {
    pub(crate) geometry: Geometry,
    pub(crate) material: i32,
    pub(crate) first_index: u32,
    pub(crate) count: u32,
    pub(crate) instance: u32,
    pub(crate) pass: Pass,
    /// Drawn into the shadow cascades (M3).
    pub(crate) casts: bool,
    /// Index in the ordered far indirect argument stream (ordinary draws: none).
    pub(crate) indirect: Option<u32>,
}

/// The size-dependent targets.
pub(crate) struct Targets {
    pub(crate) size: [u32; 2],
    pub(crate) samples: u32,
    /// The multisampled colour target (`None` at one sample).
    pub(crate) msaa: Option<wgpu::TextureView>,
    pub(crate) resolved: wgpu::Texture,
    pub(crate) resolved_view: wgpu::TextureView,
    /// The resolve's twin: a full-screen pass over the resolved frame
    /// (volumetrics, the depth of field's composite) writes the other one,
    /// and the post chain reads the last written (lane P4-GPU: no copy
    /// back).
    pub(crate) scratch: wgpu::Texture,
    pub(crate) scratch_view: wgpu::TextureView,
    pub(crate) depth: wgpu::TextureView,
}

impl Targets {
    /// The frame's two HDR resolves: `[resolved, scratch]`.
    pub(crate) fn hdr_views(&self) -> [&wgpu::TextureView; 2] {
        [&self.resolved_view, &self.scratch_view]
    }
}

/// Diagnostics of the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub opaque: usize,
    pub transparent: usize,
    pub floor_batches: usize,
    pub draws: usize,
    /// Sky layers the frame draws (the decor sprites; the cube carries the rest).
    pub sky_layers: usize,
    pub statics: usize,
    pub materials: usize,
    /// RT7 materials cached and their maps that did not load (M2).
    pub rt7_materials: usize,
    pub missing_maps: usize,
    /// Sun shadow cascades rendered and the draws cast into each (M3).
    pub shadow_cascades: usize,
    pub shadow_casters: usize,
    /// The caster draws that meet a cascade, over every cascade (entity
    /// draws per cascade they meet; the terrain not counted).
    pub shadow_cascade_casters: usize,
    /// The caster draw calls this frame encodes (the cascade cache redraws
    /// only stale cascades' static casters and the dynamic ones,
    /// `shadows::cache`).
    pub shadow_draws: usize,
    /// Static point lights (M4): the scene's lights, the grid's tiles
    /// with a light and at the cap of four, and the entity draws with at
    /// least one light of their own.
    pub point_lights: usize,
    pub lit_tiles: usize,
    pub full_tiles: usize,
    pub lit_draws: usize,
    /// Model billboard quads (M9) and the models they follow.
    pub billboards: usize,
    pub billboard_owners: usize,
    /// Particle quads and batches drawn (M9).
    pub particles: usize,
    pub particle_batches: usize,
    /// Sprite quads whose material has an HDR scale (an aux map, M9).
    pub hdr_sprites: usize,
    /// The shadowed point lights and the caster draw
    /// calls this frame redrew into their cube faces.
    pub point_shadow_lights: usize,
    pub point_shadow_draws: usize,
}

/// What one sprite segment of the last frame drew (M9; the shell's
/// `CLIENT910_MODERN_CHECK` compares these with the faithful backend's):
/// `quads` quads after the first `at` entity draws of `list` (0 opaque, 1
/// transparent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteSegment {
    pub particles: bool,
    pub list: u8,
    pub at: usize,
    pub quads: usize,
    /// The quads drawn (lane Q-FX: particles of size <= 10 skipped,
    /// `crate::sprites::particles::draw_order`; otherwise `quads`).
    pub drawn: usize,
}

/// See the module docs.
pub struct ModernRenderer {
    /// The last frame's grading.
    pub grading: crate::post::grading::Grading,
    /// The per-square environment (M5, `lighting::environment`).
    pub environment: crate::lighting::environment::EnvironmentState,
    pub stats: Stats,
    /// RT7 models (M10, [`crate::models::rt7`]).
    pub rt7: crate::models::rt7::Rt7Cache,
    /// The threads the frame's jobs run on (`frame::jobs`: the encode
    /// units, the posed models).
    pub(crate) jobs: crate::frame::jobs::Jobs,
    /// The look's values ([`crate::lighting::look`], the settings' look mode).
    pub look: crate::lighting::look::Look,
    /// The environment record's remap slots and angle fog
    /// ([`crate::lighting::environment_record`]).
    pub environment_record: crate::lighting::environment_record::EnvironmentRecord,
    pub(crate) device_resources: DeviceResources,
    pub(crate) scene_resources: SceneResources,
    pub(crate) frame_resources: FrameResources,
    pub(crate) history: FrameHistory,
    pub(crate) preparation: PreparationState,
}

/// The sun shadows' GPU state (M3): the settings, the atlas, the receive
/// bindings of the forward pass and the caster pass.
pub(crate) struct ShadowGpu {
    pub(crate) settings: ShadowSettings,
    pub(crate) receive_layout: wgpu::BindGroupLayout,
    pub(crate) uniforms: wgpu::Buffer,
    pub(crate) casters: wgpu::Buffer,
    pub(crate) caster_bind: wgpu::BindGroup,
    pub(crate) sampler: wgpu::Sampler,
    pub(crate) pipeline: wgpu::RenderPipeline,
    /// The atlas's size in texels (1 while shadows are off), its view and
    /// the receive bind group over it.
    pub(crate) atlas: (u32, wgpu::TextureView, wgpu::BindGroup),
    /// The point-light shadow casters (`point_shadow`).
    pub(crate) point: crate::frame::gpu::point_shadows::PointCasterGpu,
    /// The sun cascades' cache (`shadows::cache`).
    pub(crate) sun: crate::frame::gpu::shadow_cache::SunCacheGpu,
}

pub(crate) fn vertex_layouts() -> [Option<wgpu::VertexBufferLayout<'static>>; 3] {
    const V: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 10 => Float32x4
    ];
    const C: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Unorm8x4];
    const I: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
        8 => Float32x4, 9 => Float32x4, 11 => Float32x4
    ];
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &V,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: 4,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &C,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &I,
        }),
    ]
}

/// `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` (the blend the classic scene draws
/// everything with).
pub(crate) const ALPHA_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

impl ModernRenderer {
    pub(crate) fn frame_millis(&self) -> i64 {
        self.frame_resources
            .frame_time
            .unwrap_or_else(crate::logic_clock::monotonic_millis)
    }

    /// The pipelines and the material cache for frames of `output_format`,
    /// with `samples` (1 or a count the device supports for
    /// [`HDR_FORMAT`] with [`DEPTH_FORMAT`]) for the forward target.
    pub fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        output_format: wgpu::TextureFormat,
        samples: u32,
        settings: crate::settings::ModernSettings,
    ) -> Self {
        debug_assert!(
            crate::frame::units::declared_in_frame_order(),
            "the encode units keep the frame's pass order"
        );
        let mut textures = Textures::new(device, queue);
        textures.env_reflections = settings.env_reflections;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern frame"),
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
                // M8: the ambient occlusion map (`post`).
                crate::frame::gpu::post::AO_ENTRY,
            ],
        });
        let uniform = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_size = std::mem::size_of::<FrameUniforms>() as u64;
        let frame_buffer = uniform("modern frame", frame_size);
        let ao_white = crate::frame::gpu::post::white_view(device, queue);
        let bind = |buffer: &wgpu::Buffer| {
            crate::frame::gpu::post::frame_bind(device, &frame_layout, buffer, &ao_white)
        };
        let frame_bind = bind(&frame_buffer);

        // The shader modules, each composed and compiled once (`shaders`):
        // the forward module is shared by the forward, sprite, shadow, water
        // reflection and probe capture pipelines (`samples`).
        let shaders = crate::shaders::Library::default();
        let forward_module = shaders.get(device, crate::shaders::Module::Forward);
        let receive_layout = shadow_receive_layout(device);
        let point_layout = point_light_layout(device);
        let forward_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern forward"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&textures.layout),
                Some(&receive_layout),
                Some(&point_layout),
                Some(&textures.arrays.layout),
            ],
            immediate_size: 0,
        });
        let layouts = vertex_layouts();

        let sky_module = shaders.get(device, crate::shaders::Module::SkyLayers);
        let sky_layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern sky layer"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<SkyLayerUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let sky_texture_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("modern sky texture"),
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
                ],
            });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern sky"),
            bind_group_layouts: &[Some(&sky_layer_layout), Some(&sky_texture_layout)],
            immediate_size: 0,
        });
        let inputs = crate::frame::pipelines::PipelineInputs {
            forward_module,
            forward_layout,
            sky_module,
            sky_layout,
        };
        // The shadow casters, the forward-target pipelines and the post chain
        // are created at once (`frame::compile`).
        let shadow_fill = shaders.get(device, crate::shaders::Module::ShadowFill);
        let post_modules = (
            shaders.get(device, crate::shaders::Module::AoGeometry),
            shaders.get(
                device,
                crate::shaders::Module::Post {
                    look: settings.look,
                },
            ),
        );
        let (shadow, (sample_set, post)) = crate::frame::compile::join(
            || {
                ShadowGpu::new(
                    device,
                    &inputs.forward_module,
                    &frame_layout,
                    &textures,
                    receive_layout,
                    &layouts,
                    &shadow_fill,
                )
            },
            || {
                crate::frame::compile::join(
                    || crate::frame::pipelines::SamplePipelines::new(device, &inputs, samples),
                    || {
                        PostGpu::new(
                            device,
                            frame_layout.clone(),
                            &inputs.forward_layout,
                            &layouts,
                            output_format,
                            ao_white,
                            post_modules,
                        )
                    },
                )
            },
        );
        let mut pipelines = crate::frame::pipelines::Variants::default();
        pipelines.select(samples, || sample_set);
        let sky_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern sky"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = device.create_texture_with_data(
            queue.queue(),
            &wgpu::TextureDescriptor {
                label: Some("modern sky white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255; 4],
        );
        let sky_white = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern sky white"),
            layout: &sky_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &white.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sky_sampler),
                },
            ],
        });

        let post_buffer = uniform(
            "modern post",
            std::mem::size_of::<crate::post::grading::PostUniforms>() as u64,
        );
        let lut_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("modern grading LUTs"),
            size: wgpu::Extent3d {
                width: crate::post::grading::LUT_WIDTH,
                height: 3 * crate::post::grading::LUT_SIZE as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lut_view = lut_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let lights = LightGpu::new(device, queue, point_layout, &shaders);
        let mut renderer = Self {
            grading: crate::post::grading::Grading::default(),
            environment: crate::lighting::environment::EnvironmentState::default(),
            stats: Stats::default(),
            rt7: crate::models::rt7::Rt7Cache::default(),
            jobs: crate::frame::jobs::Jobs::new(crate::frame::jobs::default_threads()),
            look: crate::lighting::look::Look::for_mode(settings.look),
            environment_record: crate::lighting::environment_record::EnvironmentRecord::for_mode(
                settings.look,
            ),
            device_resources: DeviceResources {
                samples,
                output_format,
                textures,
                frame_buffer,
                frame_bind,
                pipelines,
                pipeline_inputs: inputs,
                sky_layer_layout,
                sky_texture_layout,
                sky_white,
                sky_sampler,
                post_buffer,
                lut_view,
                lut_texture,
                lut_slots: [-1; 3],
                luts: crate::post::grading::LutCache::default(),
                shaders,
            },
            scene_resources: SceneResources {
                lights,
                floors: Vec::new(),
                statics: Default::default(),
                underwater: Default::default(),
                loc_arena: crate::frame::arenas::LocArena::default(),
                scene_token: None,
                loc_buffers_created: 0,
                sky_textures: HashMap::new(),
                sky_cubes: Default::default(),
                terrain: crate::frame::gpu::terrain::TerrainGpu::default(),
                far: crate::frame::gpu::far::FarGpu::default(),
                interior: crate::frame::gpu::interior::InteriorGpu::default(),
            },
            frame_resources: FrameResources {
                water: crate::frame::gpu::water::WaterGpu::default(),
                targets: None,
                scaled: None,
                shadow_frame: None,
                shadow_only: Vec::new(),
                shadow_only_bounds: Vec::new(),
                cascade_masks: Vec::new(),
                packets: crate::frame::submit::FramePackets::default(),
                batch_scratch: Vec::new(),
                visible_scratch: Vec::new(),
                transparent_scratch: Vec::new(),
                draw_bounds: Vec::new(),
                arena: Arena::default(),
                instances: Vec::new(),
                instance_buffer: None,
                draws: Vec::new(),
                sky: Vec::new(),
                sky_layer_buffer: None,
                sprites: SpriteGpu::default(),
                particles: crate::sprites::particles::ParticleFrame::default(),
                billboards: crate::sprites::billboards::Billboards::default(),
                sprite_segments: Vec::new(),
                clear: [0.0; 3],
                frame_time: None,
                posing: crate::frame::posing::Posing::default(),
                atmos: crate::frame::gpu::atmosphere::AtmosGpu::default(),
            },
            history: FrameHistory {
                shadow,
                post,
                probes: ProbeGpu::default(),
                ambient: Default::default(),
                frame: 0,
            },
            preparation: PreparationState {
                sky_sources: HashMap::new(),
                prebuilds: crate::frame::prebuild::Prebuilds::default(),
                settings,
                display: crate::frame::scale::Display::default(),
                pass_order: crate::frame::passes::Order,
                faithful_bloom: false,
                #[cfg(test)]
                test_models: Vec::new(),
                bench_models: None,
                #[cfg(test)]
                test_lights: None,
                #[cfg(test)]
                test_no_cascade_cull: false,
                #[cfg(test)]
                test_unsorted_packets: false,
                #[cfg(test)]
                test_inline_builds: false,
            },
        };
        // Every pipeline the frame draws with, created here rather than in
        // the first frame (`frame::pipelines`): the probe capture, the
        // caustics, the terrain, water and atmosphere sets at this sample
        // count, created at once.
        renderer.select_count_sets(device, queue);
        renderer
    }

    /// Whether the visible casters are culled per cascade (the
    /// tests draw every caster into every cascade to check the culling
    /// leaves the frame unchanged).
    pub(crate) fn cull_visible_casters(&self) -> bool {
        #[cfg(test)]
        if self.preparation.test_no_cascade_cull {
            return false;
        }
        true
    }

    /// The threads the frame's jobs use, the render thread included
    /// (`CLIENT910_MODERN_THREADS`; 1: synchronous).
    #[must_use]
    pub fn threads(&self) -> usize {
        self.jobs.threads()
    }

    /// Use `threads` threads from the next frame on (tests and the bench:
    /// threaded and synchronous frames of one process).
    #[cfg(test)]
    pub(crate) fn set_threads(&mut self, threads: usize) {
        if threads != self.jobs.threads() {
            self.jobs = crate::frame::jobs::Jobs::new(threads);
        }
    }

    /// The draws from `start` on have the camera-local box `bounds`.
    pub(crate) fn note_bounds(
        &mut self,
        start: usize,
        bounds: Option<crate::models::bounds::Bounds>,
    ) {
        if let Some(b) = bounds {
            self.frame_resources.draw_bounds.push((
                start as u32,
                self.frame_resources.draws.len() as u32,
                b,
            ));
        }
    }

    /// The frames' colour format this renderer was built for.
    #[must_use]
    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.device_resources.output_format
    }

    /// The forward target's sample count.
    #[must_use]
    pub fn samples(&self) -> u32 {
        self.device_resources.samples
    }

    /// The display's scale factor and the player's saved render scale (`None`:
    /// automatic, [`crate::settings::RenderScale::auto`]); the shell sets them
    /// when the window and the saved choice are known and when either changes.
    /// The setting's own value (`CLIENT910_MODERN_RENDER_SCALE`) still wins.
    pub fn set_display(&mut self, scale_factor: f64, saved: Option<crate::settings::RenderScale>) {
        self.preparation.display.scale_factor = scale_factor;
        self.preparation.display.saved = saved;
    }

    /// Apply local quality choices between frames. Target extents, occlusion,
    /// reflection buffers and far-ring builds are keyed by the next frame's
    /// settings; native shadow, sample-count and bloom owners remain intact.
    pub fn set_quality(&mut self, settings: crate::settings::ModernSettings) {
        let current = &mut self.preparation.settings;
        current.ao = settings.ao;
        current.ao_resolution = settings.ao_resolution;
        current.far = settings.far;
        current.volumetrics = settings.volumetrics;
        current.reflections = settings.reflections;
        current.dof = settings.dof;
        current.render_scale = settings.render_scale;
    }

    /// The sun shadow settings (M3; the shell maps them from the faithful
    /// `ClientOptions`, [`ShadowSettings::from_options`]).
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) {
        self.history.shadow.settings = settings;
    }

    /// The shadow settings in effect: the shell's options, with the
    /// quality of [`crate::settings::ModernSettings::shadows`] when fixed.
    #[must_use]
    pub fn shadow_settings(&self) -> ShadowSettings {
        self.encoding_inputs().shadow_settings()
    }

    /// This frame's particle quads (M9, [`crate::sprites::particles`]): the
    /// faithful toolkit's CPU particle frame of the snapshot's runtime, which
    /// the shell hands over when it prepares the faithful one; kept until
    /// the next (the faithful particle pass keeps its last batches the same
    /// way).
    pub fn set_particles(&mut self, frame: crate::sprites::particles::ParticleFrame) {
        self.frame_resources.particles = frame;
    }

    /// The particle frame the next frames draw (M9; the shell's check).
    #[must_use]
    pub fn particles(&self) -> &crate::sprites::particles::ParticleFrame {
        &self.frame_resources.particles
    }

    /// The faithful toolkit's bloom state (M9): billboards whose type hides
    /// under bloom are skipped while it is on, so the billboard set
    /// stays the faithful one.
    pub fn set_faithful_bloom(&mut self, bloom: bool) {
        self.preparation.faithful_bloom = bloom;
    }

    /// Frames drawn so far (the shell's check skips a call that drew none).
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.history.frame
    }

    /// The last frame's billboards (M9; the shell's check).
    #[must_use]
    pub fn billboards(&self) -> &crate::sprites::billboards::Billboards {
        &self.frame_resources.billboards
    }

    /// What the last frame's billboard and particle segments drew, in draw
    /// order (M9; the shell's check).
    #[must_use]
    pub fn sprite_segments(&self) -> &[SpriteSegment] {
        &self.frame_resources.sprite_segments
    }

    /// The last frame's cascades (`None`: shadows off).
    #[must_use]
    pub fn shadow_frame(&self) -> Option<&ShadowFrame> {
        self.frame_resources.shadow_frame.as_ref()
    }

    /// Draw RT7 materials from `source` (`None`: through the M1 path),
    /// re-uploading the floors that hold material bindings
    /// ([`Textures::set_rt7_source`]; verification only).
    pub fn set_rt7_textures(&mut self, source: Option<crate::models::materials::TextureSource>) {
        self.device_resources.textures.set_rt7_source(source);
        self.scene_resources.floors.clear();
    }

    /// The material cache (what each material resolved to; M2 tests).
    #[must_use]
    pub fn textures(&self) -> &Textures {
        &self.device_resources.textures
    }

    /// The last frame's lit HDR frame, the post chain's source (the
    /// forward target's resolve after the atmosphere's passes; tests read
    /// it back to check for non-finite values).
    #[must_use]
    pub fn hdr_target(&self) -> Option<&wgpu::Texture> {
        self.frame_resources
            .targets
            .as_ref()
            .map(|t| match self.hdr_source() {
                0 => &t.resolved,
                _ => &t.scratch,
            })
    }

    pub(crate) fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self
            .frame_resources
            .targets
            .as_ref()
            .is_some_and(|t| t.size == size && t.samples == self.device_resources.samples)
        {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        };
        let texture = |label: &str, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        // A sample change at the same size keeps the resolved target (the
        // post chain binds it by size).
        let ((resolved, resolved_view), (scratch, scratch_view)) = match self
            .frame_resources
            .targets
            .take()
            .filter(|t| t.size == size)
        {
            Some(t) => ((t.resolved, t.resolved_view), (t.scratch, t.scratch_view)),
            None => {
                let hdr = |label| {
                    let t = texture(
                        label,
                        HDR_FORMAT,
                        1,
                        wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_SRC
                                // M8's post tests write synthetic HDR images into it.
                                | wgpu::TextureUsages::COPY_DST,
                    );
                    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                    (t, v)
                };
                (
                    hdr("modern forward lighting"),
                    hdr("modern forward lighting (twin)"),
                )
            }
        };
        let msaa = (self.device_resources.samples > 1).then(|| {
            texture(
                "modern forward lighting (msaa)",
                HDR_FORMAT,
                self.device_resources.samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let depth = texture(
            "modern depth",
            DEPTH_FORMAT,
            self.device_resources.samples,
            // Sampled by the water pass (M7) as the scene depth.
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        self.frame_resources.targets = Some(Targets {
            size,
            samples: self.device_resources.samples,
            msaa,
            resolved,
            resolved_view,
            scratch,
            scratch_view,
            depth,
        });
    }

    /// Draw and submit scene units for callers that own their final encoder.
    /// The shell uses `prepare_frame` and `record_frame` to submit the post
    /// chain and units together after acquiring its drawable.
    pub fn draw(&mut self, target: Target<'_>, snapshot: &SceneSnapshot<'_>) {
        let Target {
            device,
            queue,
            encoder,
            view,
            format: _,
            size,
            rect,
            clip,
        } = target;
        let Some(prepared) = self.prepare_frame(
            PrepareTarget {
                device,
                queue,
                size,
                rect,
                clip,
            },
            snapshot,
        ) else {
            return;
        };
        let commands = self.record_frame(device, encoder, view, prepared);
        queue.submit_uploads(commands);
        self.frame_submitted();
    }

    /// Prepare scene resources without holding a surface drawable. Scene unit
    /// commands wait for recording; independent sky bakes submit when due.
    pub fn prepare_frame(
        &mut self,
        target: PrepareTarget<'_>,
        snapshot: &SceneSnapshot<'_>,
    ) -> Option<PreparedFrame> {
        let PrepareTarget {
            device,
            queue,
            size,
            rect,
            clip,
        } = target;
        let [_, _, w, h] = rect;
        if w <= 0 || h <= 0 || snapshot.blackout {
            return None;
        }
        // The render scale (`scale`): the scene's targets, viewport and
        // scissor; the camera keeps the viewport's size `(w, h)`.
        let viewport_pixels = u64::from(w.unsigned_abs()) * u64::from(h.unsigned_abs());
        let percent = self
            .preparation
            .display
            .resolve(self.preparation.settings.render_scale, viewport_pixels)
            .as_percent();
        self.frame_resources.scaled = crate::frame::scale::Scaled::of(percent, size, rect, clip);
        let (size, rect, clip) = self
            .frame_resources
            .scaled
            .map_or((size, rect, clip), |s| (s.size, s.rect, s.clip));
        let [target_width, target_height] = size.map(|v| v as i32);
        let [left, top, right, bottom] = clip;
        let left = left.clamp(0, target_width);
        let top = top.clamp(0, target_height);
        if right.clamp(left, target_width) <= left || bottom.clamp(top, target_height) <= top {
            return None;
        }
        self.frame_resources.frame_time = snapshot.time_ms;
        let previous_frame = self.history.frame;
        self.history.frame += 1;
        self.preparation.pass_order.start();
        self.ensure_targets(device, size);
        self.check_scene(snapshot);
        let checkpoint = lifecycle::FrameCheckpoint::capture(self, snapshot, previous_frame);
        self.preparation.prebuilds.clear();
        self.device_resources.textures.clear_prefetched();
        self.scene_resources.loc_arena.begin_frame();
        self.frame_resources.arena.clear();
        self.frame_resources.instances.clear();
        self.frame_resources.draws.clear();
        self.frame_resources.draw_bounds.clear();
        self.frame_resources.shadow_only.clear();
        self.frame_resources.shadow_only_bounds.clear();
        self.frame_resources.water.draws.clear();
        self.frame_resources.sky.clear();

        // The camera-local frame: the scene-local camera target.
        let origin = [
            (snapshot.camera.target[0] - snapshot.floor_base[0] * 512) as f32,
            snapshot.camera.target[1] as f32,
            (snapshot.camera.target[2] - snapshot.floor_base[1] * 512) as f32,
        ];
        let prep = PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        };
        let now = self.frame_millis();
        let sun_dir = self.environment.sun_dir(snapshot, now);
        // The camera square's global environment cube.
        self.prepare_global_env(device, queue, snapshot, now);
        // The camera square's tone-map block (lighting::look).
        if self.preparation.settings.look == crate::settings::LookMode::Verified {
            self.look = self
                .look
                .with_tone_map(&self.environment.tone_map(snapshot, now));
        }
        // The camera square's remaps and angle fog (lighting::environment_record).
        let record = self.environment.camera_record(snapshot);
        self.environment_record.update(record, now);
        let lighting = self.look.lighting(snapshot.env, sun_dir);
        let mut uniforms = frame_uniforms_lit(snapshot, (w, h), &lighting);
        // The NXT far plane and fog (`far`; off: unchanged).
        self.far_uniforms(snapshot, (w, h), &mut uniforms);
        // The modern fog colour and probe ambient.
        if uniforms.fog_colour[3] > 0.0 {
            let c = self.look.fog_colour(snapshot.env.fog.distance_colour);
            uniforms.fog_colour = [c[0], c[1], c[2], 1.0];
        }
        uniforms.params[3] = crate::lighting::look::FRAME_FLAG;
        // Sky models drawn in the forward pass (flag 8) likewise.
        uniforms.params[0] = self.look.sky_exposure();
        // M8: the post effects, and whether the forward pass applies SSAO.
        let ssao = self.prepare_post(device, queue, &uniforms, size, rect, clip);
        uniforms.params[1] = if ssao {
            self.preparation.settings.ao_resolution.divisor() as f32
        } else {
            0.0
        };
        uniforms.params[2] = 1.0;
        // The scattering in the geometry passes' block.
        let frame_uniforms = self.atmosphere_uniforms(snapshot, &uniforms);
        queue.write_buffer(
            &self.device_resources.frame_buffer,
            0,
            bytemuck::bytes_of(&frame_uniforms),
        );
        self.prepare_shadows(device, queue, snapshot, (w, h), origin, sun_dir);
        self.prepare_lights(device, queue, snapshot, origin);
        // The point-light shadows' candidates, slots and levels
        // (`point_shadow`; the faces and casters follow the frame's draws).
        let eye = [uniforms.eye[0], uniforms.eye[1], uniforms.eye[2]];
        self.select_point_shadows(&uniforms.view, &uniforms.view_proj, eye, origin);
        self.prepare_grading(queue, snapshot);
        self.frame_resources.clear = crate::post::tonemap::display_to_hdr(snapshot.env.clear);

        // The sky's cubes: the environment's cube through its fade, the bakes that are due.
        self.plan_sky_cubes(device, queue, snapshot, &uniforms.view_proj, now);
        let sky_layers = self.prepare_sky(device, queue, snapshot, rect);

        let list = DrawList::build(snapshot);
        // M10: the modern terrain (`terrain`), which replaces the classic floor's
        // non-water batches below.
        self.prepare_terrain(device, queue, snapshot, &list, origin);
        // The far squares and the near extension (`far`).
        self.prepare_far(device, queue, snapshot, &list, origin);
        // The models posed this frame, on the threads (`posing`).
        self.pose_models(snapshot, &list);
        let mut stats = Stats {
            opaque: list.opaque.len(),
            transparent: list.transparent.len(),
            ..Stats::default()
        };
        // The visible locs' draw ranges, each culled per cascade
        // below (`crate::shadows::casters::visible_cascades`): `(first, end, id)`.
        let cull = self.frame_resources.shadow_frame.is_some() && self.cull_visible_casters();
        let mut visible = std::mem::take(&mut self.frame_resources.visible_scratch);
        visible.clear();
        // The entities' loc meshes not cached yet, built on the threads
        // (`prebuild`).
        self.prebuild_locs(snapshot, list.opaque.iter().chain(&list.transparent));
        let ranges = self.prepare_entities(device, queue, snapshot, &list.opaque, origin);
        for (entity, (start, end, bounds)) in list.opaque.iter().zip(ranges) {
            if let Some(bounds) = bounds {
                self.frame_resources
                    .draw_bounds
                    .push((start as u32, end as u32, bounds));
            }
            if self.frame_resources.shadow_frame.is_some() {
                visible.push((start, end, entity.id));
            }
        }
        let floor_matrix = local_matrix(
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            origin,
        );
        #[cfg(test)]
        for (streams, matrix) in std::mem::take(&mut self.preparation.test_models) {
            let start = self.frame_resources.draws.len();
            let model_bounds = crate::models::bounds::Bounds::of(&streams.vertices)
                .map(|b| b.transformed(&local_matrix(&matrix, origin)));
            let (base_vertex, first) = self.frame_resources.arena.push(&streams);
            for &(material, start, count) in &streams.batches {
                // The snapshot's materials (None in most tests).
                self.device_resources.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    snapshot.materials,
                    material,
                );
                let instance = self.frame_resources.instances.len() as u32;
                let mut record =
                    self.instance(local_matrix(&matrix, origin), material, 1.0, FLAG_FLOOR);
                record.p2 = [0.0; 4];
                self.frame_resources.instances.push(record);
                self.frame_resources.draws.push(Draw {
                    geometry: Geometry::Arena { base_vertex },
                    material,
                    first_index: start + first,
                    count,
                    instance,
                    pass: Pass::Opaque,
                    casts: self.frame_resources.shadow_frame.is_some(),
                    indirect: None,
                });
            }
            self.note_bounds(start, model_bounds);
            self.preparation.test_models.push((streams, matrix));
        }
        self.prepare_benchmark_models(device, queue, snapshot, origin);
        self.prepare_far_locs(device, queue, snapshot, origin, None);
        // The underwater locs stand below everything drawn here.
        self.prepare_underwater_locs(device, queue, snapshot, origin, false, None);
        let opaque_draws = self.frame_resources.draws.len();
        let floor_casts =
            self.frame_resources.shadow_frame.is_some() && self.shadow_settings().scenery;
        for floor in &list.floors {
            self.prepare_floor(device, queue, snapshot, floor);
            let n = self.scene_resources.floors[floor.level]
                .as_ref()
                .map_or(0, |f| f.batches.len());
            for batch in 0..n {
                let (material, uv_scale, count) = {
                    let b = &self.scene_resources.floors[floor.level]
                        .as_ref()
                        .expect("floor")
                        .batches[batch];
                    (b.material, b.uv_scale, b.count)
                };
                if count == 0 || self.terrain_replaces(snapshot, floor.level, material) {
                    continue;
                }
                stats.floor_batches += 1;
                let instance = self.frame_resources.instances.len() as u32;
                // Floors keep their ownership alpha for the batch blending;
                // only the texture's own cutout applies. Their point lights
                // come from the tile grid at their level (M4).
                let mut record = self.instance(floor_matrix, material, uv_scale, FLAG_FLOOR);
                record.p2 = [floor.level as f32, 0.0, 0.0, 0.0];
                self.frame_resources.instances.push(record);
                // Water batches draw in the water pass (M7, `water`).
                if self.frame_resources.water.take(
                    snapshot,
                    floor.level,
                    batch,
                    material,
                    instance,
                    count,
                ) {
                    continue;
                }
                self.frame_resources.draws.push(Draw {
                    geometry: Geometry::Floor {
                        level: floor.level,
                        batch,
                    },
                    material,
                    first_index: 0,
                    count,
                    instance,
                    pass: Pass::Opaque,
                    casts: floor_casts,
                    indirect: None,
                });
            }
        }
        // The seabed, after the floors.
        self.prepare_underwater_bed(device, queue, snapshot, origin);
        // The transparent entities, each with its model-draw range and
        // camera-local matrix (M9: the blended sprites join them by depth).
        let group0_end = self.frame_resources.draws.len();
        self.frame_resources.water.split = group0_end;
        self.history.post.geometry_draws = group0_end;
        let mut transparent = std::mem::take(&mut self.frame_resources.transparent_scratch);
        transparent.clear();
        self.prepare_far_locs(device, queue, snapshot, origin, Some(&mut transparent));
        self.prepare_underwater_locs(
            device,
            queue,
            snapshot,
            origin,
            true,
            Some(&mut transparent),
        );
        let ranges = self.prepare_entities(device, queue, snapshot, &list.transparent, origin);
        for (entity, (start, end, bounds)) in list.transparent.iter().zip(ranges) {
            if let Some(bounds) = bounds {
                self.frame_resources
                    .draw_bounds
                    .push((start as u32, end as u32, bounds));
            }
            if self.frame_resources.shadow_frame.is_some() {
                visible.push((start, end, entity.id));
            }
            transparent.push((start..end, local_matrix(&entity.matrix, origin)));
        }
        // Model billboards and particles (M9).
        self.prepare_sprites(&prep, &list, group0_end, &transparent);
        self.frame_resources.transparent_scratch = transparent;
        drop(list);
        // What the roof removal hid this frame (its storeys and
        // roofs cast; `interior`).
        let hidden = self.roof_hidden(snapshot);
        // The off-screen casters, drawn into the cascades only;
        // With the roof-hidden entities among them, each once
        // and culled per cascade (`shadows::casters`); the static ones a
        // frame keeping its maps does not need are deferred
        // (`shadows::cache`).
        let deferred = self.gather_shadow_casters(device, queue, snapshot, hidden.as_ref(), origin);
        // The roof-hidden terrain tiles, casters only.
        self.prepare_interior(device, queue, snapshot, hidden.as_ref());
        stats.billboards = self.stats.billboards;
        stats.billboard_owners = self.stats.billboard_owners;
        stats.particles = self.stats.particles;
        stats.particle_batches = self.stats.particle_batches;
        stats.hdr_sprites = self.stats.hdr_sprites;
        stats.draws = self.frame_resources.draws.len();
        stats.sky_layers = sky_layers.len();
        stats.statics = self.scene_resources.statics.len();
        stats.materials = self.device_resources.textures.len();
        stats.rt7_materials = self.device_resources.textures.stats.rt7;
        stats.missing_maps = self.device_resources.textures.stats.missing;
        stats.shadow_cascades = self
            .frame_resources
            .shadow_frame
            .as_ref()
            .map_or(0, |f| f.profile.cascades);
        stats.shadow_casters = self
            .frame_resources
            .draws
            .iter()
            .filter(|d| d.casts)
            .count()
            + self.frame_resources.shadow_only.len();
        stats.point_lights = self.scene_resources.lights.frame.len();
        stats.lit_tiles = self.scene_resources.lights.grid_stats.lit_tiles;
        stats.full_tiles = self.scene_resources.lights.grid_stats.full_tiles;
        stats.lit_draws = self
            .frame_resources
            .draws
            .iter()
            .filter(|d| {
                !matches!(d.geometry, Geometry::Floor { .. })
                    && self.frame_resources.instances[d.instance as usize].p2[0] > 0.0
                    && self.frame_resources.instances[d.instance as usize].p0[3] as u32 & FLAG_FLOOR
                        == 0
            })
            .count();

        // M6: the probes (a capture's draws and instances join the frame's
        // before the uploads).
        self.prepare_ambient(&prep, &uniforms);
        self.prepare_probes(&prep, &uniforms);
        // Each caster draw's cascades (the modern client gathers every
        // cascade's casters from its own volume): the visible locs' from
        // their footprints, every cascade for the rest (floors, models no
        // loc owns); the caster draw calls over the cascades.
        self.frame_resources.cascade_masks.clear();
        self.frame_resources
            .cascade_masks
            .resize(self.frame_resources.draws.len(), u8::MAX);
        if let Some(frame) = self.frame_resources.shadow_frame.as_ref().filter(|_| cull) {
            let entities: Vec<_> = visible
                .iter()
                .map(|&(_, _, id)| snapshot.live_frame().and_then(|live| live.entities.get(id)))
                .collect();
            let masks = self.jobs.map(entities.len(), |i| {
                crate::shadows::casters::entity_cascades(frame, entities[i])
            });
            for (&(start, end, _), mask) in visible.iter().zip(masks) {
                let end = end.min(self.frame_resources.cascade_masks.len());
                for m in &mut self.frame_resources.cascade_masks[start.min(end)..end] {
                    *m = mask;
                }
            }
        }
        // The depth pre-pass's sorted list (`submit`).
        #[cfg(test)]
        let sorted = !self.preparation.test_unsorted_packets;
        #[cfg(not(test))]
        let sorted = true;
        self.prepare_far_indirect(device, queue);
        self.frame_resources
            .packets
            .build(&self.frame_resources.draws, opaque_draws, sorted);
        // Each cascade's casters, whether its maps are kept and its sorted
        // caster packets (`shadows::cache`).
        (stats.shadow_cascade_casters, stats.shadow_draws) =
            self.plan_shadow_casters(device, queue, snapshot, origin, &visible, deferred);
        self.frame_resources.visible_scratch = visible;
        // Uploads: the arena, the instances, the sky layer slots.
        self.frame_resources.arena.upload(device, queue);
        if !self.frame_resources.instances.is_empty() {
            let need =
                (self.frame_resources.instances.len() * std::mem::size_of::<Instance>()) as u64;
            if self
                .frame_resources
                .instance_buffer
                .as_ref()
                .is_none_or(|(_, cap)| *cap < need)
            {
                let cap = need.next_power_of_two();
                self.frame_resources.instance_buffer = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("modern instances"),
                        size: cap,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            let (buffer, _) = self
                .frame_resources
                .instance_buffer
                .as_ref()
                .expect("instances");
            queue.write_buffer(
                buffer,
                0,
                bytemuck::cast_slice(&self.frame_resources.instances),
            );
        }
        if !sky_layers.is_empty() {
            let need = (sky_layers.len() * std::mem::size_of::<SkyLayerUniforms>()) as u64;
            if self
                .frame_resources
                .sky_layer_buffer
                .as_ref()
                .is_none_or(|(_, _, cap)| *cap < need)
            {
                let cap = need.next_power_of_two();
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern sky layers"),
                    size: cap,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("modern sky layers"),
                    layout: &self.device_resources.sky_layer_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(
                                std::mem::size_of::<SkyLayerUniforms>() as u64
                            ),
                        }),
                    }],
                });
                self.frame_resources.sky_layer_buffer = Some((buffer, bind, cap));
            }
            let (buffer, _, _) = self
                .frame_resources
                .sky_layer_buffer
                .as_ref()
                .expect("sky layers");
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&sky_layers));
        }
        // The cubes planned above, drawn now that the arena and the instances are uploaded.
        self.bake_sky_cubes(device, queue);
        if self.history.frame == 1 || self.history.frame.is_multiple_of(600) {
            log::info!(
                "[modern] frame {}: {} opaque + {} transparent entity draws, {} floor batches, {} draws, {} sky decor layers; {} loc models, {} materials cached ({} white; RT7 {}: {} normal + {} compound maps, {} fallbacks, {} missing, {:?} preferred); sun shadows {:?}: {} cascades x {} casters; point lights {}: {} lit tiles ({} full), {} lit entity draws; {} billboards on {} models, {} particles in {} batches ({} HDR-scaled quads)",
                self.history.frame,
                stats.opaque,
                stats.transparent,
                stats.floor_batches,
                stats.draws,
                stats.sky_layers,
                stats.statics,
                stats.materials,
                self.device_resources.textures.failed,
                self.device_resources.textures.stats.rt7,
                self.device_resources.textures.stats.normal_maps,
                self.device_resources.textures.stats.compound_maps,
                self.device_resources.textures.stats.fallbacks,
                self.device_resources.textures.stats.missing,
                self.device_resources.textures.source(),
                self.shadow_settings(),
                stats.shadow_cascades,
                stats.shadow_casters,
                stats.point_lights,
                stats.lit_tiles,
                stats.full_tiles,
                stats.lit_draws,
                stats.billboards,
                stats.billboard_owners,
                stats.particles,
                stats.particle_batches,
                stats.hdr_sprites
            );
            let (vertices, indices) = self.scene_resources.loc_arena.used();
            log::info!(
                "[modern] loc pages: {} ({vertices} vertices, {indices} indices in use); depth pre-pass {} draws, cascades {:?} draws",
                self.scene_resources.loc_arena.pages.len(),
                self.frame_resources.packets.prepass.len(),
                self.history.shadow
                    .sun
                    .static_lists
                    .iter()
                    .zip(&self.history.shadow.sun.dynamic_lists)
                    .map(|(s, d)| s.len() + d.len())
                    .collect::<Vec<_>>()
            );
        }
        self.stats = stats;
        // The point-light shadows (`point_shadow`).
        self.prepare_point_shadows(device, queue, origin);
        self.prepare_water(&prep, rect, size, &frame_uniforms);
        // The caustics on the drawn bed (`crate::frame::gpu::caustics`).
        self.prepare_caustics(device, queue, origin);
        self.prepare_atmos(device, queue, &uniforms, size, rect, clip);
        // The loc pages' staged writes (`arenas`), before the submission.
        self.scene_resources.loc_arena.flush(queue);
        self.scene_resources.far.arena.flush(queue);
        Some(PreparedFrame {
            rect,
            clip,
            checkpoint,
        })
    }

    /// Record ordered scene units and the post chain. Unit commands precede
    /// the caller's final encoder; every buffer can share one submission.
    pub fn record_frame(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        prepared: PreparedFrame,
    ) -> Vec<wgpu::CommandBuffer> {
        self.encode(device, encoder, view, prepared.rect, prepared.clip)
    }

    /// Discard unsent producer progress after a skipped drawable. Queued
    /// uploads remain ordered and installed until the next actual submission.
    /// The caller must retain them when using an explicit staging encoder.
    pub fn frame_skipped(&mut self, prepared: PreparedFrame) {
        prepared.checkpoint.restore(self);
    }

    /// Schedule face readbacks after the caller submits this frame's copies.
    pub fn frame_submitted(&mut self) {
        self.map_ambient_readback();
    }

    /// This frame's grading (`post::grading`): the environment's levels and
    /// remap slots, the slots' LUT rows uploaded when their sprite changes,
    /// the composite's uniforms. `CLIENT910_MODERN_GRADING=off` shows the
    /// ungraded tonemap (verification).
    pub(crate) fn prepare_grading(
        &mut self,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
    ) {
        let pack = snapshot.pack;
        let luts = &mut self.device_resources.luts;
        // The record's remap slots (lighting::environment_record).
        let record_grading = self.environment_record.remap;
        let mut grading = if record_grading {
            crate::lighting::environment_record::grading(&self.environment_record.remaps, |id| {
                luts.get(pack, id).is_some()
            })
        } else {
            crate::post::grading::Grading::from_env(snapshot.env, |id| luts.get(pack, id).is_some())
        };
        if crate::modern_debug_flags::flags().grading_off {
            grading = crate::post::grading::Grading::default();
        }
        for (slot, &id) in grading.luts.iter().enumerate() {
            if id < 0 || self.device_resources.lut_slots[slot] == id {
                continue;
            }
            let Some(lut) = self.device_resources.luts.get(pack, id) else {
                continue;
            };
            let size = crate::post::grading::LUT_SIZE as u32;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.device_resources.lut_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: slot as u32 * size,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &crate::post::grading::lut_rows(&lut),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(crate::post::grading::LUT_WIDTH * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: crate::post::grading::LUT_WIDTH,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            self.device_resources.lut_slots[slot] = id;
        }
        let mut post = crate::post::grading::PostUniforms::new(
            &grading,
            !self.device_resources.output_format.is_srgb(),
        );
        if record_grading {
            post = crate::lighting::environment_record::post_uniforms(
                &grading,
                !self.device_resources.output_format.is_srgb(),
            );
        } else {
            // The modern colour correction has no levels; the remap
            // weights at the look's strength.
            post.params[3] = 0.0;
            self.look.grade_weights(&mut post.weights);
        }
        queue.write_buffer(
            &self.device_resources.post_buffer,
            0,
            bytemuck::bytes_of(&post),
        );
        if self.grading != grading {
            log::info!("[modern] grading {grading:?}");
        }
        self.grading = grading;
    }

    /// Record the frame (`frame::units`): every unit into its own command
    /// encoder on the renderer's threads (`frame::jobs`), submitted in
    /// frame order; the post chain, which writes `view`, into `encoder`.
    pub(crate) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rect: [i32; 4],
        clip: [i32; 4],
    ) -> Vec<wgpu::CommandBuffer> {
        self.encoding_inputs()
            .encode(device, encoder, view, rect, clip)
    }
}

/// What a frame's prepare steps share (`draw`): the device and queue they
/// upload through, the scene, and the camera-local origin.
#[derive(Clone, Copy)]
pub(crate) struct PrepareFrame<'a, 's> {
    pub(crate) device: &'a wgpu::Device,
    pub(crate) queue: &'a dyn rs910_gpu_device::uploads::Uploader,
    pub(crate) snapshot: &'a SceneSnapshot<'s>,
    pub(crate) origin: [f32; 3],
}

/// What the encode units share (`encode`): the frame's targets and views,
/// the scene viewport and its clamped scissor.
pub(crate) struct EncodeFrame<'a> {
    pub(crate) targets: &'a Targets,
    /// The forward colour target (multisampled, or the resolve at one
    /// sample).
    pub(crate) colour: &'a wgpu::TextureView,
    /// The frame's colour target (the post chain's output).
    pub(crate) view: &'a wgpu::TextureView,
    pub(crate) rect: [i32; 4],
    pub(crate) clip: [i32; 4],
    /// The clip clamped to the targets: `[x, y, w, h]`.
    pub(crate) scissor: [u32; 4],
}

impl EncodeFrame<'_> {
    /// The scene viewport and scissor.
    pub(crate) fn set_view(&self, pass: &mut wgpu::RenderPass<'_>) {
        let [x, y, w, h] = self.rect;
        let [l, t, sw, sh] = self.scissor;
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_scissor_rect(l, t, sw, sh);
    }

    /// The scene depth, loaded with `load` and stored.
    pub(crate) fn depth_attachment(
        &self,
        load: wgpu::LoadOp<f32>,
    ) -> Option<wgpu::RenderPassDepthStencilAttachment<'_>> {
        Some(wgpu::RenderPassDepthStencilAttachment {
            view: &self.targets.depth,
            depth_ops: Some(wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        })
    }
}

pub(crate) use crate::frame::gpu::point_lights::{point_light_layout, LightGpu};
pub(crate) use crate::frame::gpu::post::PostGpu;
pub(crate) use crate::frame::gpu::probes::ProbeGpu;
pub use crate::frame::gpu::probes::ProbeStats;
pub(crate) use crate::frame::gpu::sky_layers::{
    SkyDraw, SkyLayerUniforms, SkyTextureGpu, SkyTextureKey,
};
pub(crate) use crate::frame::gpu::sprites::SpriteGpu;
pub(crate) use crate::frame::gpu::sun_shadows::shadow_receive_layout;
pub(crate) use crate::frame::gpu::terrain::TerrainPass;
pub(crate) use crate::frame::gpu::water::WaterForward;
pub use crate::frame::gpu::water::WaterStats;
pub(crate) use crate::frame::resources::{Arena, FloorGpu, LocSlot, StaticModel};

mod state;
use state::{DeviceResources, FrameHistory, FrameResources, PreparationState, SceneResources};
pub(crate) mod encoding;
use encoding::EncodeInputs;
pub(crate) mod arenas;
pub mod benchmark;
pub(crate) mod compile;
pub(crate) mod gpu;
pub(crate) mod jobs;
mod lifecycle;
pub mod passes;
pub(crate) mod pipelines;
pub(crate) mod posing;
pub(crate) mod prebuild;
pub(crate) mod resources;
pub(crate) mod scale;
pub mod startup;
pub(crate) mod submit;
#[cfg(test)]
mod tests;
pub(crate) mod underwater;
pub mod units;

impl<'a> EncodeInputs<'a> {
    /// The shadow settings in effect: the shell's options, with the
    /// quality of [`crate::settings::ModernSettings::shadows`] when fixed.
    #[must_use]
    pub fn shadow_settings(&self) -> ShadowSettings {
        match self.settings.shadows {
            crate::settings::Shadows::Fixed(quality) => ShadowSettings {
                quality,
                ..self.shadow.settings
            },
            crate::settings::Shadows::Options => self.shadow.settings,
        }
    }

    /// The depth pre-pass (GeometryPass): the opaque entities that write
    /// depth.
    fn encode_depth_prepass(&self, encoder: &mut wgpu::CommandEncoder, cx: &EncodeFrame<'_>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::DepthPrepass)),
            color_attachments: &[],
            depth_stencil_attachment: cx.depth_attachment(wgpu::LoadOp::Clear(1.0)),
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        cx.set_view(&mut pass);
        pass.set_pipeline(&self.pipes().depth_prepass);
        pass.set_bind_group(0, self.frame_bind, &[]);
        pass.set_bind_group(2, &self.shadow.atlas.2, &[]);
        pass.set_bind_group(3, &self.lights.bind, &[]);
        // Sorted (`submit`): depth only, so the order does not matter.
        self.submit_all(&mut pass, &self.packets.prepass);
    }

    /// Whether `unit` records anything this frame.
    fn unit_records(&self, unit: crate::frame::units::Unit) -> bool {
        use crate::frame::units::Unit;
        match unit {
            Unit::Probes => self.probes.capture.is_some() || self.ambient.capture.is_some(),
            Unit::SunShadows => self.shadow_frame.is_some(),
            Unit::PointShadows => self.point_shadows_record(),
            Unit::WaterReflection | Unit::ForwardAfterWater => !self.water.draws.is_empty(),
            _ => true,
        }
    }

    /// `part` of the forward pass with the water (`water`).
    fn encode_water_forward(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cx: &EncodeFrame<'_>,
        part: WaterForward,
    ) {
        self.encode_forward_with_water(encoder, cx, part);
    }

    /// The sky's decor sprites into the sky shading's source (transparent, blended over what is
    /// behind them by the shading): the cube carries the rest of the sky, so a frame without a
    /// decor draws nothing here.
    fn encode_sky(&self, encoder: &mut wgpu::CommandEncoder, cx: &EncodeFrame<'_>) {
        if !self.atmos.frame.sky_decor {
            return;
        }
        let clear = wgpu::Color::TRANSPARENT;
        let (sky_view, sky_resolve) = self.sky_shading_target().unwrap_or((cx.colour, None));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::Sky)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: sky_view,
                resolve_target: sky_resolve,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        cx.set_view(&mut pass);
        for draw in self.sky {
            match draw {
                SkyDraw::Layer {
                    slot,
                    texture: texture @ Some(SkyTextureKey::Decor(..)),
                } => {
                    let Some((_, layers, _)) = self.sky_layer_buffer.as_ref() else {
                        continue;
                    };
                    pass.set_pipeline(&self.pipes().sky_layer);
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
                // The cube carries the box's layers and dome model.
                SkyDraw::Layer { .. } => {}
            }
        }
    }

    /// The forward lighting in the faithful order (with water, its group 0:
    /// the water surfaces and group 2 are units of their own, `water`).
    fn encode_forward(&self, encoder: &mut wgpu::CommandEncoder, cx: &EncodeFrame<'_>) {
        let targets = cx.targets;
        let receive = &self.shadow.atlas.2;
        if !self.water.draws.is_empty() {
            self.encode_water_forward(encoder, cx, WaterForward::Group0);
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::Forward)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: cx.colour,
                resolve_target: targets.msaa.as_ref().map(|_| &targets.resolved_view),
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: cx.depth_attachment(wgpu::LoadOp::Load),
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        cx.set_view(&mut pass);
        pass.set_bind_group(0, self.frame_bind, &[]);
        pass.set_bind_group(2, receive, &[]);
        pass.set_bind_group(3, &self.lights.bind, &[]);
        // M10: the terrain first (opaque, depth tested and written).
        self.encode_terrain(&mut pass, TerrainPass::Forward);
        let mut current = None;
        let mut bound = crate::frame::submit::Bound::default();
        // The billboards and particles (M9) go between the model draws
        // at their places (`sprites`: the group 0 tail and group 2).
        let mut sprites = self.sprites.draws.iter().peekable();
        let mut i = 0;
        while i < self.draws.len() {
            let d = &self.draws[i];
            while let Some(sprite) = sprites.next_if(|s| s.after <= i) {
                self.sprites
                    .draw(&mut pass, &self.pipes().sprites, self.textures, sprite);
                current = None;
                bound.reset();
            }
            if current != Some(d.pass) {
                pass.set_pipeline(match d.pass {
                    Pass::Opaque => &self.pipes().forward,
                    Pass::NoDepthWrite => &self.pipes().forward_no_depth_write,
                });
                current = Some(d.pass);
            }
            let end = sprites
                .peek()
                .map_or(self.draws.len(), |s| s.after)
                .min(self.draws.len());
            i += self.submit_run(&mut pass, &self.draws[i..end], &mut bound);
        }
        for sprite in sprites {
            self.sprites
                .draw(&mut pass, &self.pipes().sprites, self.textures, sprite);
        }
    }

    /// Record `unit`'s passes into `encoder`.
    fn encode_unit(
        &self,
        unit: crate::frame::units::Unit,
        encoder: &mut wgpu::CommandEncoder,
        cx: &EncodeFrame<'_>,
    ) {
        use crate::frame::units::Unit;
        match unit {
            // 1. Probes: a light-probe capture this frame (`probes`).
            Unit::Probes => {
                self.encode_probes(encoder);
                self.encode_ambient(encoder);
            }
            // 2. Sky, the frame's background: the decor sprites into the sky
            // shading's source, then the sky shading over it (`atmos`).
            Unit::Sky => {
                self.encode_sky(encoder, cx);
                self.encode_sky_shading(encoder, cx.colour);
            }
            // 3. Sun shadows (pass type 0): each cascade's tile of the atlas.
            Unit::SunShadows => self.encode_sun_shadows(encoder),
            // 3b. The point-light shadow faces redrawn.
            Unit::PointShadows => self.encode_point_shadows(encoder),
            // 4. The caustic rays and their light (the terrain of step 7
            // reads it).
            Unit::Caustics => self.encode_caustics(encoder),
            // 5. Depth pre-pass.
            Unit::DepthPrepass => self.encode_depth_prepass(encoder, cx),
            // 6. Ambient occlusion (`post::ao`): the geometry pass, the
            // occlusion pass and the blur.
            Unit::AmbientOcclusion => self.encode_ssao(encoder, cx.rect, cx.clip),
            // 7a. The water's planar reflection (`water`).
            Unit::WaterReflection => self.encode_water_reflection(encoder),
            // 7. Forward lighting, in the faithful order; with water (M7)
            // split around the water pass (`water`): group 0, then the
            // water surfaces and group 2.
            Unit::Forward => self.encode_forward(encoder, cx),
            Unit::ForwardAfterWater => {
                self.encode_water_forward(encoder, cx, WaterForward::Surface);
                self.encode_water_forward(encoder, cx, WaterForward::Group2);
            }
            // 8. Volumetric scattering, depth of field. 9. The post chain
            // into the frame (`post`).
            Unit::Post => {
                self.encode_atmos_post(encoder);
                self.encode_post(encoder, cx.view, cx.rect);
            }
        }
    }

    /// Record the frame (`frame::units`): every unit into its own command
    /// encoder on the renderer's threads (`frame::jobs`), submitted in
    /// frame order; the post chain, which writes `view`, into `encoder`.
    pub(crate) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rect: [i32; 4],
        clip: [i32; 4],
    ) -> Vec<wgpu::CommandBuffer> {
        let targets = self.targets.as_ref().expect("targets");
        let [l, t, r, b] = clip;
        let (tw, th) = (targets.size[0] as i32, targets.size[1] as i32);
        let (l, t) = (l.clamp(0, tw), t.clamp(0, th));
        let (r, b) = (r.clamp(l, tw), b.clamp(t, th));
        if r <= l || b <= t {
            return Vec::new();
        }
        use crate::frame::units::Unit;
        let cx = EncodeFrame {
            targets,
            colour: targets.msaa.as_ref().unwrap_or(&targets.resolved_view),
            view,
            rect,
            clip,
            scissor: [l as u32, t as u32, (r - l) as u32, (b - t) as u32],
        };
        let units: Vec<Unit> = crate::frame::units::CLAIM_ORDER
            .into_iter()
            .filter(|&u| self.unit_records(u))
            .collect();
        // The post chain's encoder (the frame's): one unit takes it.
        let frame_encoder = std::sync::Mutex::new(encoder);
        let recorded = self.jobs.map(units.len(), |i| {
            let unit = units[i];
            self.pass_order.start_unit(unit);
            let decl = unit.decl();
            if decl.frame_encoder {
                let mut encoder = frame_encoder
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.encode_unit(unit, &mut encoder, &cx);
                return (unit.index(), None);
            }
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(decl.label),
            });
            self.encode_unit(unit, &mut encoder, &cx);
            (unit.index(), Some(encoder.finish()))
        });
        // Submission order (`frame::units::UNITS`), whatever order the
        // threads took and finished the units in.
        let mut buffers: Vec<(usize, wgpu::CommandBuffer)> = recorded
            .into_iter()
            .filter_map(|(k, b)| b.map(|b| (k, b)))
            .collect();
        buffers.sort_by_key(|&(k, _)| k);
        self.pass_order.start();
        buffers.into_iter().map(|(_, b)| b).collect()
    }
}
