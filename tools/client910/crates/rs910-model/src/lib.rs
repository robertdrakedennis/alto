//! `rs910-model`: the CPU model layer of the 910 client port (`docs/architecture.md`).
//! The CPU half of the models, floors and particles: no wgpu (the
//! GPU upload stays in the render modules).
//!
//! - [`modelunlit`] (`ModelUnlit` decode), [`model`] (`Model`/`MeshRaw`),
//!   [`gpumodel`] (`GpuModel` lighting, animation transforms, billboards,
//!   particle anchors), [`animation_skeletal`] (skeletal poses) and
//!   [`animation_random`] (the animation/particle random stream).
//! - [`particle`]: particle types and the particle system runtime.
//! - [`floor`] (floor model build), [`hardshadow`] (floor and entity hard
//!   shadows), [`water`] (water noise/normal volumes), [`material`] (GLX
//!   material state dispatch).
//! - [`font_layout`] (Font text layout) and [`icon_raster`] (the CPU
//!   triangle rasteriser behind inventory item icons).
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::gpumodel::...` etc. compiling
//! (tools/README.md "Crate conventions").

// The 48-bit LCG random moved to rs910-core (shared with rs910-game).
pub use rs910_core::animation_random;
pub mod animation_skeletal;
pub mod floor;
pub mod font_layout;
pub mod gpumodel;
pub mod hardshadow;
pub mod icon_raster;
pub mod line_dashes;
pub mod material;
pub mod mesh_billboards;
pub mod model;
pub mod modelunlit;
pub mod particle;
pub mod sprite;
pub mod water;

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{anim, animation_curve, billboard, font_metrics, sprite_data, texture};
use rs910_core::{actor_matrix, animation_matrix, colour, trig};
use rs910_js5::cache;

#[cfg(test)]
mod corpus;

/// The shared test helpers (rs910-js5 `test-hooks`).
#[cfg(test)]
mod test_support {
    pub use rs910_js5::test_support::require_pack;
}
