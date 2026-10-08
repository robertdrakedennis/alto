//! `rs910-far-scene`: the render-only far scene of the 910 client port
//! (design in `docs/renderer/modern-renderer.md`).
//! The modern client does not draw the
//! server's build area: it builds map squares from the cache in rings around
//! the camera focus out to one of five draw distances. This crate is the CPU half of that for
//! `--renderer modern`:
//!
//! - [`far_level`]: the five draw-distance levels, the far plane, the ring
//!   radius, the distance classes, the fog law and the projection with the
//!   modern far plane.
//! - [`far_debug_flags`]: `CLIENT910_MODERN_FAR_SYNC=1` (the level is the
//!   renderer's setting, never stored in `ClientOptions`).
//! - [`far_ring`]: map squares, the ring around the focus (nearest first), the
//!   classic window exclusion and the squares' distance classes.
//! - [`far_terrain`]: map file 5 of one square, decoded with the modern heights
//!   (moved here from `rs910_render_modern::terrain`, whose near terrain
//!   reads it the same way, so the two meet with equal heights), read
//!   quietly (`Pack::read_group_resident`).
//! - [`far_world`]: the resident squares: the ring around the focus, the
//!   squares missing from it named nearest first, one renderer value per
//!   started square with a ticket (stale results recognised), eviction
//!   outside the ring with one ring of hysteresis.
//! - [`far_jobs`] (F5): the worker threads that build squares off the render
//!   thread (per-thread contexts, priority order, cancellation), and the
//!   synchronous mode.
//! - [`far_locs`] (F3): a square's static locs outside the classic window,
//!   placed by the classic placement on a private scene (private model cache,
//!   resident reads only), and the 16 x 16-tile loc containers.
//!
//! **Inert by construction.** It reads only the cache, through the resident
//! read (no JS5 request, no read log), keeps private caches, and takes the
//! scene as values (a focus, a level); it never sees or writes game state.
//! The renderer that draws it (`rs910_render_modern::frame::far`) borrows
//! the snapshot immutably like the rest of that backend.

pub mod far_debug_flags;
pub mod far_jobs;
pub mod far_level;
pub mod far_locs;
pub mod far_ring;
pub mod far_terrain;
pub mod far_world;

/// The shared test helpers under the path the tests use.
#[cfg(test)]
mod test_support {
    pub use rs910_js5::test_support::require_pack;
}
