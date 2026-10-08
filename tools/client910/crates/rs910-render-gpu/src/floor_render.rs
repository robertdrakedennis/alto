//! GPU side of the faithful floor: draws [`crate::floor::FloorGeometry`] with
//! the standard model shader ([`FLOOR_SHADER`]).
//!
//! # Behaviour
//! - Shader: the standard model program without point lights, lit shading
//!   mode, alpha ignore off; it is the program a lit floor batch without
//!   point lights uses. [`FLOOR_SHADER`] holds its maths (see the comments
//!   inside).
//! - Uniforms: the world-view-projection matrix, a texture-coordinate matrix
//!   `scale(1/materialScale)`, the sun direction, sun colour (`sunRGB *`
//!   sun intensity), anti-sun colour (`-anti-sun intensity * sunRGB`),
//!   ambient colour (`sunRGB * ambient intensity`), and the fog planes and
//!   colours (zero when fog is off).
//! - Streams: stream 0 = `VERTEX(3f) TEX_COORD_2(2f) [TEX_COORD_1(1f)]
//!   NORMAL(3f)`, stream 1 = the batch's `COLOR(u8x4)` buffer; one indexed
//!   draw per batch over the batch's owned triangles.
//! - Render state: alpha blend `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`, depth test
//!   `LEQUAL`, depth write on. The blend is what makes the per-corner alpha
//!   0/255 batch feathering work.
//! - Textures: PNG → ARGB ints → gamma `pow(c/255, 0.7)` per channel → for
//!   opaque non-reflective materials alpha is forced to 255 unless RGB == 0
//!   → mip chain by exact 2x2 integer average → trilinear
//!   (`LINEAR_MIPMAP_LINEAR` / `LINEAR`), wrap `REPEAT` per axis when
//!   `repeatS/T == 1` else `CLAMP_TO_EDGE`. Material `-1` samples the 1x1
//!   white texture.
//!
//! # Material profile
//! Models share this shader with floors: standard, specular, environmental,
//! unlit/alpha-ignore and up to four model-local point lights. Effect 5 is
//! the waterfall program, sampling the billow volume ([`crate::billow`]).
//! Back faces are culled, depth is `LEQUAL`, and alpha testing uses strict
//! `GREATER` (including ref zero). Nonzero model cutouts disable blending and
//! replace RGBA. Camera-relative matrices preserve the one-unit hard-shadow
//! separation; live tile selection and environment fog come from phases D/E4.
//! High-detail water: floors built with water detail 2 route effects
//! 2/4/8/9 to the environment-mapped water program (programs 7/8) with the
//! water normal volume; the underwater floor uses the underwater-ground
//! programs (9/10) with per-batch water fog.

#[cfg(test)]
use mesh::{batch_bind_group, batch_bind_group_full, index_buffer, next_render_id};
#[cfg(test)]
use textures::{gamma_lut, upload_argb, white_texture};
#[cfg(test)]
use wgpu::util::DeviceExt;

mod shader;
pub use shader::FLOOR_SHADER;
mod pipeline;
use pipeline::BatchUniforms;
pub use pipeline::{FloorPipeline, FloorVertex};
mod textures;
use textures::box_mip;
use textures::make_sampler;
pub use textures::{FloorTexture, FloorTextureCache};
mod mesh;
pub use mesh::{FloorGpuBatch, FloorMesh, MeshUpload, ModelBundle};
mod material_bindings;
use material_bindings::empty_cube;
use material_bindings::material_frame_bindings;
use material_bindings::upload_environment_cube;
use material_bindings::upload_volume_mips;
use material_bindings::FrameViews;

// `FloorUniforms` and `GAME_TO_WORLD` moved to rs910-scene (Phase 3.2).
pub use rs910_scene::floor_uniforms::*;

#[cfg(test)]
mod tests;
