//! Renderer state grouped by resource and producer ownership. Each owner
//! keeps its allocations across frames; encoding receives immutable views.

use crate::frame::*;

const COLOUR_CHANNELS: usize = 3;
const LUT_SLOTS: usize = 3;
const MATRIX_VALUES: usize = 16;

pub(crate) struct DeviceResources {
    pub(crate) samples: u32,
    pub(crate) output_format: wgpu::TextureFormat,
    pub(crate) textures: Textures,
    pub(crate) frame_buffer: wgpu::Buffer,
    pub(crate) frame_bind: wgpu::BindGroup,
    /// The forward-target pipelines per sample count (`frame::pipelines`).
    pub(crate) pipelines:
        crate::frame::pipelines::Variants<u32, crate::frame::pipelines::SamplePipelines>,
    /// What they are built from (a sample change builds the new count's).
    pub(crate) pipeline_inputs: crate::frame::pipelines::PipelineInputs,
    pub(crate) sky_layer_layout: wgpu::BindGroupLayout,
    pub(crate) sky_texture_layout: wgpu::BindGroupLayout,
    pub(crate) sky_white: wgpu::BindGroup,
    pub(crate) sky_sampler: wgpu::Sampler,
    /// The grading block of the post chain's composite (`post::grading`).
    pub(crate) post_buffer: wgpu::Buffer,
    /// The grading LUTs of the three remap slots (`post::grading`), stacked
    /// `256 x 48`, and the sprite each slot holds.
    pub(crate) lut_view: wgpu::TextureView,
    pub(crate) lut_texture: wgpu::Texture,
    pub(crate) lut_slots: [i32; LUT_SLOTS],
    pub(crate) luts: crate::post::grading::LutCache,
    /// The compiled shader modules (`crate::shaders`).
    pub(crate) shaders: crate::shaders::Library,
}

pub(crate) struct SceneResources {
    /// The static point lights (M4).
    pub(crate) lights: LightGpu,
    pub(crate) floors: Vec<Option<FloorGpu>>,
    /// The loc meshes, one per scene slot (`resources`).
    pub(crate) statics: crate::fast_hash::FastMap<LocSlot, StaticModel>,
    /// The seabed's selection and the underwater locs' meshes (`underwater`).
    pub(crate) underwater: crate::frame::underwater::UnderwaterState,
    /// Their geometry: the shared loc pages (`arenas`).
    pub(crate) loc_arena: crate::frame::arenas::LocArena,
    /// The installed scene's identity (level 0's floor token address, the
    /// static slot count): a change drops the loc model cache.
    pub(crate) scene_token: Option<(usize, usize)>,
    /// GPU buffers created for loc meshes so far (`loc_mesh_cache`).
    pub(crate) loc_buffers_created: u64,
    pub(crate) sky_textures: HashMap<SkyTextureKey, SkyTextureGpu>,
    /// The sky's cubes and their cross-fade (`gpu::sky_cube`).
    pub(crate) sky_cubes: crate::frame::gpu::sky_cube::SkyCubes,
    /// NXT terrain (M10, `terrain`, [`crate::terrain`]).
    pub(crate) terrain: crate::frame::gpu::terrain::TerrainGpu,
    /// The far scene (`far`, `rs910_far_scene`).
    pub(crate) far: crate::frame::gpu::far::FarGpu,
    /// The roof-hidden casters (`interior`,
    /// [`crate::shadows::interior`]), drawn in the cascades only.
    pub(crate) interior: crate::frame::gpu::interior::InteriorGpu,
}

pub(crate) struct FrameResources {
    /// The frame water draws, reflection target and reusable resources.
    pub(crate) water: crate::frame::gpu::water::WaterGpu,
    pub(crate) targets: Option<Targets>,
    /// This frame's scene viewport at the render scale (`None`: at 100%).
    pub(crate) scaled: Option<crate::frame::scale::Scaled>,
    /// The frame's cascades (`None`: shadows off).
    pub(crate) shadow_frame: Option<ShadowFrame>,
    /// The off-screen locs' shadow-only draws
    /// (`shadows::casters`), drawn in the cascades only.
    pub(crate) shadow_only: Vec<(Draw, u8)>,
    /// Posed camera-local entity bounds, aligned with shadow-only packets.
    pub(crate) shadow_only_bounds: Vec<Option<crate::models::bounds::Bounds>>,
    /// Per draw, the cascades it casts into (bit `k`).
    pub(crate) cascade_masks: Vec<u8>,
    /// The depth-only passes' sorted draw lists of this frame (`submit`).
    pub(crate) packets: crate::frame::submit::FramePackets,
    /// `prepare_entity`'s batch list, reused each call.
    pub(crate) batch_scratch: Vec<(i32, u32, u32)>,
    /// Per-frame lists kept for their capacity: the visible locs' draw
    /// ranges (caster culling) and the transparent entities' ranges.
    pub(crate) visible_scratch: Vec<(usize, usize, usize)>,
    pub(crate) transparent_scratch: Vec<(std::ops::Range<usize>, [f32; MATRIX_VALUES])>,
    /// The visible entities' draw ranges and their camera-local boxes
    /// (`models::bounds`; the water reflection culls by them).
    pub(crate) draw_bounds: Vec<(u32, u32, crate::models::bounds::Bounds)>,
    pub(crate) arena: Arena,
    pub(crate) instances: Vec<Instance>,
    pub(crate) instance_buffer: Option<(wgpu::Buffer, u64)>,
    pub(crate) draws: Vec<Draw>,
    pub(crate) sky: Vec<SkyDraw>,
    pub(crate) sky_layer_buffer: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    /// The billboard and particle quads (M9).
    pub(crate) sprites: SpriteGpu,
    /// The particle frame the shell handed over last (M9,
    /// [`ModernRenderer::set_particles`]).
    pub(crate) particles: crate::sprites::particles::ParticleFrame,
    /// The last frame's billboards and sprite segments (M9).
    pub(crate) billboards: crate::sprites::billboards::Billboards,
    pub(crate) sprite_segments: Vec<SpriteSegment>,
    pub(crate) clear: [f32; COLOUR_CHANNELS],
    pub(crate) frame_time: Option<i64>,
    /// This frame's posed models (`posing`).
    pub(crate) posing: crate::frame::posing::Posing,
    /// The atmosphere layers (`atmos`).
    pub(crate) atmos: crate::frame::gpu::atmosphere::AtmosGpu,
}

pub(crate) struct FrameHistory {
    pub(crate) shadow: ShadowGpu,
    /// The post chain (M8, `post`, [`crate::post`]).
    pub(crate) post: PostGpu,
    /// Light probes and IBL (M6, `probes`).
    pub(crate) probes: ProbeGpu,
    /// The per-square ambient capture (`gpu::ambient`).
    pub(crate) ambient: crate::frame::gpu::ambient::AmbientGpu,
    pub(crate) frame: u64,
}

pub(crate) struct PreparationState {
    pub(crate) sky_sources: HashMap<SkyTextureKey, std::sync::Arc<crate::sky_frame::SkyTexture>>,
    /// The loc meshes built on the threads ahead of their draws this frame
    /// (`prebuild`).
    pub(crate) prebuilds: crate::frame::prebuild::Prebuilds,
    /// The quality settings ([`crate::settings`]).
    pub(crate) settings: crate::settings::ModernSettings,
    /// The display's scale factor and the saved render scale choice
    /// ([`ModernRenderer::set_display`]).
    pub(crate) display: crate::frame::scale::Display,
    /// The frame's pass order so far (`frame::passes`).
    pub(crate) pass_order: crate::frame::passes::Order,
    /// The faithful toolkit's bloom state (M9: billboards whose type hides
    /// under bloom are skipped, [`ModernRenderer::set_faithful_bloom`]).
    pub(crate) faithful_bloom: bool,
    /// Extra opaque models the tests add to the next frames (streams and a
    /// scene-local model matrix, column-major).
    #[cfg(test)]
    pub(crate) test_models: Vec<(ModelStreams, [f32; MATRIX_VALUES])>,
    /// The performance metric's model and its placements, drawn every frame
    /// (`benchmark`); `None` in the game's renderer.
    pub(crate) bench_models: Option<(ModelStreams, Vec<[f32; MATRIX_VALUES]>)>,
    /// Tests: the point lights and their tile grid of a snapshot without a
    /// live scene (scene-local lights; the grid's tiles are the floor path's).
    #[cfg(test)]
    pub(crate) test_lights: Option<(
        Vec<crate::lighting::point_lights::Light>,
        crate::lighting::point_lights::Grid,
    )>,
    /// Tests: every visible caster into every cascade.
    #[cfg(test)]
    pub(crate) test_no_cascade_cull: bool,
    /// Tests: the depth-only passes in the draw order (`submit`).
    #[cfg(test)]
    pub(crate) test_unsorted_packets: bool,
    /// Tests: every loc mesh and material built at its draw, nothing ahead
    /// on the threads (`prebuild`).
    #[cfg(test)]
    pub(crate) test_inline_builds: bool,
}
