//! The faithful toolkit's state ([`Renderer`]) and the
//! lent-device pair ([`Faithful`]).
//!
//! Resource ownership by lifetime (programme §6 "GPU resources by
//! lifetime"; target-architecture §4). The renderer frees nothing of its
//! own on a map rebuild except what the rebuild re-uploads through it: the
//! region's floors, lights, shadows and scene models are owned by
//! `scene_meshes::SceneMeshes` (and the player models by
//! `player_renderer::PlayersRenderer`) and only built through the upload
//! methods here.
//!
//! | Lifetime | Created / replaced when | `Renderer` fields |
//! |---|---|---|
//! | Device (the shell's `gpu_device::Device`, never replaced) | window creation | none: surface, device, queue and screenshot readback are lent per call; `msaa_samples`/`hdr_samples` are its answers, read once in `new` |
//! | Toolkit | a toolkit change (`recreate_toolkit_targets`), an MSAA/bloom change (`set_scene_effects`), a resize or game canvas change (`resize`, `set_game_canvas`), a lost surface (`begin_frame`) | `floor` (its scene variants), `passes`, `particles`, `billboards`, `scene_samples`, `bloom_enabled`, `scene_target`, `depth_view`, `canvas_target`, `game_canvas`, `size`, `ui_scale`; the scene pipelines they hold come from `pipelines` |
//! | Session (first use, then until the renderer drops) | first use | `pipelines` (the pipeline cache), `floor` (base pipelines, frame uniforms, environment cube cache, noise and water volumes), `floor_textures` (material textures by id), `post` (effect pipelines by entry and format; its targets follow the size and format), `frame_profile`, `console` (fonts), `ui` (the retained painter's and interface models' caches), `overlay`, `layer_paint`, `layers` (framebuffer sprites by id, resized in place), `retained_frame` (resized on a size/format change), `env_passes` (sky models and sprites by key), `next_external_id` |
//! | Region | a map rebuild | `minimap_textures` (base sprites by id: `render_minimap_base`, freed by `release_minimap_base` on a minimap rebuild), `underwater_models` (`set_underwater_models`), `env_passes.underwater` |
//! | Scene | dynamic/transient entity changes | none: transient and dynamic models are the mesh owners', keyed by their mesh-id maps (`build_model_mesh`, `update_model_mesh`; a transient slot whose model keeps its structure rewrites last frame's mesh, `reuse_model_mesh`) |
//! | Frame | every redraw | `scene_opaque_bundle`/`scene_transparent_bundle` (kept while the mesh ids match; cleared with the scene effects or the environment), `underwater_order`, `scene_blackout`, the particle and billboard frames (vertex buffers grown by powers of two), the sky layers (`env_passes`' flat-quad buffer grown by powers of two), the retained UI's quads (each painter's `QuadBuffer`, grown by powers of two; mask bind groups kept while their sprite lives), the post-process draw slots (`post`: uniforms and bind group per draw position, rebuilt when their views change) |
//!
//! Buffer writes of every lifetime go through the device's staging belt
//! (`gpu_device::Device` as `uploads::Uploader`), submitted ahead of the
//! next `Device::submit` (programme Phase 6; `Queue::write_buffer`
//! semantics).

use super::*;
use std::collections::HashMap;

/// The hardware toolkit's state: depth and scene targets and
/// the faithful floor/model/particle/sky/post passes, over the device the
/// shell lends each call (module docs).
pub struct Renderer {
    /// Scene multisample counts the device supports.
    pub(super) msaa_samples: std::collections::HashSet<u32>,
    pub(super) scene_target: Option<crate::scene_target::Target>,
    pub(super) scene_samples: u32,
    /// Sample counts the HDR (bloom) scene target supports.
    pub(super) hdr_samples: std::collections::HashSet<u32>,
    /// The post-process chain with its effects.
    pub(super) post: crate::postprocess::PostProcessor,
    pub(super) bloom_enabled: bool,

    pub(super) console: Option<crate::console_render::Renderer>,
    pub(super) ui: Option<RetainedUi>,
    pub(super) ui_spare: Option<RetainedUi>,
    pub(super) threaded_composition: bool,
    pub(super) composition_pending: bool,
    /// With camera state 3 and `cam2` not yet ready the viewport is filled
    /// black and the scene is not drawn.
    pub(super) scene_blackout: bool,
    /// Minimap base sprites by external id.
    pub(super) minimap_textures: std::collections::HashMap<u64, wgpu::Texture>,
    pub(super) next_external_id: u64,
    pub(super) depth_view: wgpu::TextureView,
    /// Faithful floor pipeline (`Model.glsl` ShaderMode 0, see
    /// `floor_render.rs`).
    pub(super) floor: crate::floor_render::FloorPipeline,
    /// The pipeline cache every pass and painter of this renderer takes its
    /// pipelines from (`crate::pipelines`; the floor pipeline and the
    /// retained UI's interface models hold clones of the handle).
    pub(super) pipelines: crate::pipelines::PipelineCache,
    /// The static-light and hard-shadow floor passes.
    pub(super) passes: std::sync::Arc<crate::floorpass::FloorPasses>,
    /// Particle batches drawn after the transparent entities.
    pub(super) particles: crate::particle_render::ParticlePass,
    /// Billboard quads drawn after their model's batches.
    pub(super) billboards: crate::billboard_render::BillboardPass,
    /// Material textures for floor batches.
    pub(super) floor_textures: crate::floor_render::FloorTextureCache,
    pub(super) frame_profile: Option<crate::frame_profile::FrameProfile>,
    /// The engine profiler's per-pass GPU times (`rs910_core::profile`,
    /// lane E-A4), created on the first frame the profiler records.
    pub(super) pass_timer: Option<crate::frame_profile::PassTimer>,
    /// Opaque / transparent scene lists as bundles, one per run between
    /// particle owners (`ParticlePass::splits`).
    pub(super) scene_opaque_bundle: Vec<crate::floor_render::ModelBundle>,
    pub(super) scene_transparent_bundle: Vec<crate::floor_render::ModelBundle>,
    /// The underwater scene's loc models (`buildDrawLists(true)` entities)
    /// and whether each is transparent.
    pub(super) underwater_models: Vec<(crate::floor_render::FloorMesh, bool)>,
    /// This frame's `buildDrawLists(true)` order of `underwater_models`
    /// (opaque, transparent); `None` draws every model unsorted.
    pub(super) underwater_order: Option<(Vec<usize>, Vec<usize>)>,
    /// Physical surface pixels.
    pub(super) size: (u32, u32),
    /// Window scale factor (physical pixels per game canvas pixel). The UI
    /// canvas is `size / ui_scale`: the client's canvas size is in
    /// user-space units, and winit's matching unit is the logical pixel.
    /// The surface, depth
    /// buffer and 3D passes stay physical; only UI layout/mouse use canvas
    /// units, scaled back to physical at composition.
    pub(super) ui_scale: f32,
    pub(super) game_canvas: Option<crate::game_canvas::Canvas>,
    pub(super) canvas_target: Option<crate::window_canvas::CanvasTarget>,
    /// Skybox pre-pass and underwater floor (`skybox_render.rs`).
    pub(super) env_passes: crate::skybox_render::EnvPasses,
    /// The last presented game frame: a front-to-back buffer copy made on
    /// entering a rebuild state, so the message box paints over the frozen
    /// scene.
    pub(super) retained_frame: Option<wgpu::Texture>,
    /// The painter message-box frames draw with ([`Renderer::frame_message_box`]).
    pub(super) overlay: Option<crate::ui_paint_gpu::Renderer>,
    /// Canvas-sized render targets of this frame's [`crate::ui_backend::Output::layers`],
    /// by `Image::External` id.
    pub(super) layers: HashMap<u64, wgpu::Texture>,
    pub(super) layer_paint: Option<crate::ui_paint_gpu::Renderer>,
}

/// The faithful GPU toolkit with the shell's device lent to it for one call
/// (Phase 4.1: the shell owns the `gpu_device::Device` and every backend
/// borrows it). The scene-mesh owners (`scene_meshes::SceneMeshes`,
/// `player_renderer::PlayersRenderer`) upload through it at the client's points;
/// `client910::active_toolkit::ActiveToolkit::faithful` lends it.
pub type Faithful<'a> = (&'a mut Renderer, &'a crate::gpu_device::Device);

/// [`Faithful`] for the owners that only write existing buffers.
pub type FaithfulRef<'a> = (&'a Renderer, &'a crate::gpu_device::Device);
