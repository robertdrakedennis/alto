//! The far scene's CPU half in this renderer (`docs/renderer/modern-renderer.md`):
//! what turns the
//! renderer-neutral far scene (`rs910_far_scene`: the ring, the private loc
//! placement, the worker pool) into this renderer's vertex streams. It
//! lives here, not in `rs910_far_scene`, because it builds this renderer's
//! vertex format and reuses its RT7 correspondence
//! ([`crate::models::rt7`]); the GPU half is `frame::gpu::far`.
//!
//! - [`lod`]: RT7 geometry with every LOD index list.
//! - [`batch`]: merged meshes (loc containers), their batches and per-frame
//!   draw runs.
//! - [`jobs`]: the build jobs the workers (or, in sync mode, the render
//!   thread) run: a square's terrain, a square's far locs, a near extension
//!   container.

pub(crate) mod batch;
pub(crate) mod jobs;
pub(crate) mod lod;
