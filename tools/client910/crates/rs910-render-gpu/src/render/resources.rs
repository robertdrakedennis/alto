//! The toolkit's state ([`Renderer`]).
//!
//! Resource ownership by lifetime:
//!
//! | Lifetime | Created / replaced when | `Renderer` fields |
//! |---|---|---|
//! | Device (the shell's `gpu_device::Device`) | window creation, device recovery | none: surface, device, queue and screenshot readback are lent per call; `msaa_samples`/`hdr_samples` are its answers, read once in `new` |
//! | Toolkit | a toolkit change (`recreate_toolkit_targets`), an MSAA/bloom change (`set_scene_effects`), a resize or game canvas change (`resize`, `set_game_canvas`), a lost surface (`begin_frame`) | `scene_samples`, `bloom_enabled`, `depth_view`, `canvas_target`, `game_canvas`, `size`, `ui_scale` |
//! | Session | first use | `pipelines`, `floor` (the minimap base's floor pipeline), `floor_textures`, `post` (the chain's game state), `console`, `ui`, `overlay`, `layer_paint`, `layers`, `retained_frame`, `next_external_id` |
//! | Region | a minimap rebuild | `minimap_textures` (base sprites by id) |
//!
//! Buffer writes go through the device's staging belt
//! (`gpu_device::Device` as `uploads::Uploader`), submitted ahead of the
//! next `Device::submit`.

use super::*;
use std::collections::HashMap;

/// The toolkit's state: the retained UI and its composition, the console,
/// the canvas scale-up, the minimap base sprites and the capability
/// answers, over the device the shell lends each call (module docs).
pub struct Renderer {
    /// Scene multisample counts the device supports.
    pub(super) msaa_samples: std::collections::HashSet<u32>,
    pub(super) scene_samples: u32,
    /// Sample counts the HDR (bloom) scene target supports.
    pub(super) hdr_samples: std::collections::HashSet<u32>,
    /// The post-process chain's game state (the capture answer).
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
    /// The floor pipeline the minimap base draws through.
    pub(super) floor: crate::floor_render::FloorPipeline,
    /// The pipeline cache every painter of this renderer takes its
    /// pipelines from (`crate::pipelines`).
    pub(super) pipelines: crate::pipelines::PipelineCache,
    /// Material textures for the minimap base's floors.
    pub(super) floor_textures: crate::floor_render::FloorTextureCache,
    /// The engine profiler's per-pass GPU times (`rs910_core::profile`),
    /// created on the first frame the profiler records.
    pub(super) pass_timer: Option<crate::frame_profile::PassTimer>,
    /// Physical surface pixels.
    pub(super) size: (u32, u32),
    /// Window scale factor (physical pixels per game canvas pixel). The UI
    /// canvas is `size / ui_scale`; the surface and depth buffer stay
    /// physical.
    pub(super) ui_scale: f32,
    pub(super) game_canvas: Option<crate::game_canvas::Canvas>,
    pub(super) canvas_target: Option<crate::window_canvas::CanvasTarget>,
    /// The last presented game frame: a front-to-back buffer copy made on
    /// entering a rebuild state, so the message box paints over the frozen
    /// scene.
    pub(super) retained_frame: Option<wgpu::Texture>,
    /// The painter message-box frames draw with ([`Renderer::frame_message_box`]).
    pub(super) overlay: Option<crate::ui_paint_gpu::Renderer>,
    /// Canvas-sized render targets of this frame's framebuffer sprites, by
    /// `Image::External` id.
    pub(super) layers: HashMap<u64, wgpu::Texture>,
    pub(super) layer_paint: Option<crate::ui_paint_gpu::Renderer>,
}
