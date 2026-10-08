//! `rs910-render-gpu`: the faithful GPU toolkit of the 910 client port
//! (the hardware toolkit over wgpu;
//! `docs/architecture.md`). It draws
//! over the shell's `rs910_gpu_device::gpu_device::Device`, lent per call
//! (Phase 4.1); the shell (`client910::active_toolkit`) chooses the
//! backend.
//!
//! - [`render`]: `Renderer` (the toolkit state: the frame, the retained UI
//!   and console composition, minimap base sprites, screenshots) and
//!   `OrbitCamera`.
//! - Scene passes: [`floor_render`], [`floorpass`], [`particle_render`],
//!   [`billboard_render`], [`skybox_render`], [`postprocess`],
//!   [`scene_target`], [`actor_render`]; [`player_renderer`] (the player
//!   model owner and its GPU meshes); [`scene_meshes`] (the scene's uploaded
//!   floors, floor passes and models, drawn from the renderer-neutral
//!   `rs910_scene::scene_snapshot::SceneSnapshot`; renderer plan A1).
//!   [`render::Faithful`] is the renderer with the lent device, what the
//!   mesh owners upload through.
//! - 2D: [`ui_paint_gpu`], [`text_render_gpu`], [`sprite_draw_gpu`] (the wgpu
//!   halves of rs910-toolkit's display lists), [`ui_model_gpu`] (interface
//!   models), [`console_render`], [`window_canvas`] (the canvas scale-up).
//! - [`pipelines`]: the one pipeline cache every pass takes its pipelines
//!   from, keyed by (samples, format, variant), with the census of the
//!   duplicate pipelines it replaced.
//! - [`frame_profile`] and [`render_debug_flags`]: profiling and the
//!   diagnostic variables this renderer reads.
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::render::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod actor_render;
pub mod billboard_render;
pub mod billow;
pub mod console_render;
pub mod floor_render;
pub mod floorpass;
pub mod frame_profile;
pub mod particle_render;
pub mod pipelines;
pub mod player_renderer;
pub mod postprocess;
pub mod render;
pub mod render_debug_flags;
pub mod scene_meshes;
pub mod scene_target;
pub mod skybox_render;
pub mod sprite_draw_gpu;
pub mod text_render_gpu;
pub mod ui_model_gpu;
pub mod ui_paint_gpu;
pub mod window_canvas;

// `warn_repeated!` as `crate::logging::warn_repeated!`, as in client910.
use rs910_core::log_repeat as logging;

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{
    billboard, client_options, config, font_atlas, font_metrics, sprite_data, sprite_sheet, texture,
};
#[cfg(any(test, feature = "test-hooks"))]
use rs910_core::png_out;
use rs910_core::{actor_matrix, animation_matrix, logic_clock, trig};
use rs910_game::{game_runtime, protocol910};
use rs910_gpu_device::{gpu_device, uploads};
use rs910_js5::{cache, js5_fetch};
#[cfg(test)]
use rs910_model::sprite;
use rs910_model::{
    floor, font_layout, gpumodel, hardshadow, material, mesh_billboards, modelunlit, particle,
    water,
};
#[cfg(test)]
use rs910_protocol::server_prot;
use rs910_scene::{
    animation_assets, camera, draw, dynamic_scene, env, floorlight, interface_model, live_scene,
    minimap, player_body, player_draw, player_model, player_picking, player_shadow, rebuild, scene,
    scene_player_pick, scene_snapshot, sky_frame, skybox,
};
use rs910_toolkit::{
    console_draw, frame_plan, game_canvas, sprite_draw, text_render, toolkit_debug_flags,
    ui_output, ui_paint,
};

/// The shared test helpers (rs910-core / rs910-js5 / rs910-gpu-device
/// `test-hooks`), under the path the moved tests use
/// (`crate::test_support::...`).
#[cfg(test)]
mod test_support {
    pub use rs910_gpu_device::test_support::require_gpu;
}
