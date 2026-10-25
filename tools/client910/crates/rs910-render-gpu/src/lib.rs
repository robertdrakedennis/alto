//! `rs910-render-gpu`: the client's 2D/UI toolkit over wgpu
//! (`docs/architecture.md`). It draws over the shell's
//! `rs910_gpu_device::gpu_device::Device`, lent per call; the 3D scene is
//! the modern renderer's (`rs910-render-modern`), composed into this
//! toolkit's frame by the shell (`client910::active_toolkit`).
//!
//! - [`render`]: `Renderer` (the frame composition, the retained UI and
//!   console, the minimap base sprites, screenshots, the capability
//!   answers) and `OrbitCamera`.
//! - 2D: [`ui_paint_gpu`], [`text_render_gpu`], [`sprite_draw_gpu`] (the wgpu
//!   halves of rs910-toolkit's display lists), [`ui_model_gpu`] (interface
//!   models, with their [`floor_render`] meshes, [`quad_render`] particle
//!   and billboard pass and [`scene_target`] MSAA target),
//!   [`console_render`], [`window_canvas`] (the canvas scale-up).
//! - [`postprocess`]: the post-process chain's game state (the capture
//!   answer the UI reads).
//! - [`pipelines`]: the pipeline cache the painters take their pipelines
//!   from, keyed by (samples, format, variant).
//! - [`frame_profile`] and [`render_debug_flags`]: profiling and the
//!   diagnostic variables this toolkit reads.
pub mod billow;
pub mod console_render;
pub mod floor_render;
pub mod frame_profile;
pub mod pipelines;
pub mod postprocess;
pub mod quad_render;
pub mod render;
pub mod render_debug_flags;
pub mod scene_target;
pub mod sprite_draw_gpu;
pub mod text_render_gpu;
pub mod ui_model_gpu;
pub mod ui_paint_gpu;
pub mod window_canvas;

// The modules name their dependencies through `crate::`.
use rs910_config::{billboard, font_atlas, font_metrics, sprite_data, sprite_sheet, texture};
use rs910_core::{actor_matrix, logic_clock, trig};
use rs910_game::protocol910;
#[cfg(any(test, feature = "test-hooks"))]
use rs910_gpu_device::png_out;
use rs910_gpu_device::{gpu_device, uploads};
use rs910_js5::{cache, js5_fetch};
#[cfg(test)]
use rs910_model::sprite;
use rs910_model::{floor, font_layout, gpumodel, material, mesh_billboards, water};
use rs910_scene::{camera, env, interface_model, minimap};
use rs910_toolkit::{
    console_draw, frame_plan, game_canvas, sprite_draw, text_render, toolkit_debug_flags,
    ui_output, ui_paint,
};
