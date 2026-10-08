//! `rs910-render-modern`: the default modern scene renderer of the 910
//! client port (`--renderer modern`; programme §0.1). It draws the
//! renderer-neutral `rs910_scene::scene_snapshot::SceneSnapshot` with its
//! own resources and lighting, after the NXT client's renderer; the shell
//! (`client910::active_toolkit`) composes the faithful UI around it
//! (`rs910_render_gpu::render::Renderer::frame_composite`), answers every
//! capability query with the faithful profile and keeps the faithful
//! toolkit's uploads running, so the client's behaviour does not depend on
//! it (the replay gate's `renderer_choice_is_observationally_inert`). Its
//! pixels are not bound by the pixel invariant; the faithful backends stay
//! the reference.
//!
//! The design and how to add a feature: `docs/renderer/modern-renderer.md`.
//!
//! Subsystems (their CPU halves; the GPU halves, the `impl ModernRenderer`
//! blocks and GPU state, are the renderer's, `frame::gpu`, so the module
//! graph stays acyclic):
//! - [`frame`]: `ModernRenderer`, the frame's passes, pipelines and
//!   resources; [`settings`] (the quality settings) and
//!   [`modern_debug_flags`] (diagnostic switches).
//! - [`shaders`]: every WGSL module, composed once from the subsystems'
//!   ordered snippets.
//! - [`models`]: the draw list and vertex streams, materials, the forward
//!   shading, RT7 static and animated models.
//! - [`lighting`]: the environment's sun and ambient, its record (colour
//!   remap), the calibrated look, point lights, light probes, RT5
//!   environment mapping.
//! - [`shadows`]: the sun's cascades and quality presets, off-screen and
//!   roof-hidden casters, point-light shadows.
//! - [`atmosphere`]: scattering, distance fog, the sky (a cube map sampled by
//!   view direction, its cross-fade), volumetrics, the classic skybox layers
//!   the cubes are baked from.
//! - [`post`]: ambient occlusion, eye adaptation, bloom, the tonemap and
//!   grading composite, depth of field, FXAA.
//! - [`water_body`]: water surfaces, their reflection, body effects and
//!   caustics.
//! - [`terrain`]: map file 5's terrain meshes and texture atlas.
//! - [`sprites`]: billboards and particles.
//! - The far scene around the camera focus: `rs910_far_scene` (the ring,
//!   private loc placement, workers); its batching here, `far` (RT7 LODs,
//!   merged loc containers, build jobs); its GPU half `frame::gpu::far`.

pub mod atmosphere;
pub mod device_features;
pub mod exclusive;
pub(crate) mod far;
pub(crate) mod fast_hash;
pub mod frame;
pub mod lighting;
pub mod models;
pub mod modern_debug_flags;
pub mod post;
pub mod settings;
pub mod shaders;
pub mod shadows;
pub mod sprites;
pub mod terrain;
pub mod water_body;

// The lower crates' modules under `crate::` (as the other renderers name
// them).
use rs910_config::{billboard, texture};
use rs910_core::{actor_matrix, logic_clock};
use rs910_js5::cache;
use rs910_model::{floor, gpumodel, mesh_billboards, modelunlit};
use rs910_scene::{camera, draw, scene, scene_snapshot, sky_frame, skybox};

#[cfg(test)]
use rs910_scene::{dynamic_scene, live_scene};

/// The shared test helpers under the path the tests use.
#[cfg(test)]
mod test_support {
    pub use rs910_gpu_device::test_support::require_gpu;
    pub use rs910_js5::test_support::require_pack;
}
