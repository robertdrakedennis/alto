//! `rs910-toolkit`: the toolkit boundary of the 910 client port (`docs/architecture.md`).
//! The 2D toolkit and sprite/font draw API as the Rust client
//! uses them: the retained UI records its toolkit calls once, and a backend
//! executes them. No wgpu: the GPU
//! backend (`ui_paint_gpu`, `render`) lives above this crate.
//!
//! - [`ui_paint`]: the display lists. [`ui_paint::Op`] is one draw call
//!   (a filled rectangle, a sprite, a font glyph, ...),
//!   [`ui_paint::Recording`] the calls of a frame in painter order, and
//!   [`ui_paint::Painter`] the recorder the UI draws through, which also
//!   computes the GPU backend's batch quads ([`ui_paint::Plan`],
//!   batch geometry) as it records.
//! - [`sprite_draw`] (GPU sprite batch geometry) and
//!   [`text_render`] (the batch vertex and glyph quads):
//!   the CPU half of the GPU painter.
//! - [`font`]: the CPU font resource every toolkit creates its font from.
//! - [`toolkit`]: the [`Toolkit`] trait (the 2D API plus the
//!   sprite/font draws the UI makes), the one [`toolkit::draw`]
//!   dispatcher from an `Op` to a backend, and [`NullToolkit`], the headless
//!   backend.
//! - [`ui_output`]: the retained UI's output for a frame as the renderers
//!   take it, and [`game_canvas`]: the game canvas geometry the presenter
//!   composes (Phase 3.2).
//! - [`performance_metric`]: the performance-metric benchmark model and
//!   [`compressed_texture_format`]: the GL codes of the compressed formats
//!   (the CPU halves of two capability queries; Phase 3.2).
//! - [`capability`]: the hardware toolkit's capability answers as a
//!   `Profile` (renderer plan A3), and `Answers` with the software toolkit 0's.
//! - [`console_draw`]: the developer console's draw plan and `ConsoleView`,
//!   what the toolkits read of the console (Phase 3.2).
//! - [`toolkit_debug_flags`]: the diagnostics both the UI and the GPU
//!   renderer read (Phase 3.2).
//! - [`frame_plan`]: [`FramePlan`], one frame as the backends execute it:
//!   the recording with the 3D scene and the interface models interleaved at
//!   op indices, walked in painter order ([`FramePlan::walk`]).
//!
//! # Backends and extension points (programme §0.1)
//!
//! Two backends exist today: the faithful GPU toolkit (client910
//! `ui_paint_gpu` + `render`, consuming [`ui_paint::Plan`]) and
//! [`NullToolkit`] (tests, headless, replaying the [`ui_paint::Recording`]
//! through [`Toolkit`]). The software toolkit that also replayed the
//! recording was removed (lane DROP-SW). A modern 2D backend is another
//! implementation and must be able to live beside them without changing
//! this crate:
//!
//! - **Input is draw calls, not pixels or GPU geometry.** [`ui_paint::Op`]
//!   carries the call arguments (canvas-space integers, the CPU
//!   [`sprite::Sprite`]/[`font::Font`] resource, blend/tint colours), not
//!   device state. A backend is free to rasterise them at any resolution,
//!   with any filtering or colour pipeline. The GPU batch quads in
//!   [`ui_paint::Plan`] are one backend's precomputed geometry; nothing
//!   else needs to read them.
//! - **Resources are the CPU objects, identified by `Rc` identity.** Every
//!   backend keeps its own cache keyed by that identity (the GPU texture
//!   map), so a backend can build its own
//!   representation (mipmapped/sRGB textures, SDF glyphs) without the UI
//!   knowing. Toolkit-built images (framebuffers, the minimap base) are
//!   named by id ([`ui_paint::Image::External`]).
//! - **3D is a segment, not a call.** [`FramePlan`] marks where the scene
//!   viewport and each interface model draw; how a backend lights and
//!   shades them (fixed-function-like model shading, or PBR
//!   materials and dynamic lights) is outside this contract. The scene's
//!   own inputs (draw lists, lights, environment) stay in rs910-scene.
//! - **Revision facts stay out.** Nothing here names a 910 opcode, cache
//!   archive or CS2 command; `Op` is the 2D toolkit API, which is the
//!   same across builds, and a revision without it (NXT) supplies
//!   its own recorder that still emits these calls or a superset.
//! - **Headless.** [`NullToolkit`] implements the trait with no device;
//!   tests hash its op stream (the "FramePlan 2D op stream hash" of the
//!   replay gate, target-architecture.md §5).
//!
//! Capability queries (renderer plan A3): [`capability::Profile`], the
//! device answers (`supports_bloom`, anti-aliasing, scene sample levels, GL
//! texture formats) as data; every backend that draws a hardware toolkit's
//! frames answers with the faithful GPU toolkit's profile. Not yet behind
//! the trait: the resource factories with generation-checked handles.
//! Backend selection is the shell's since Phase 3.3/3.4
//! (`client910::active_toolkit::ActiveToolkit`: the GPU `Renderer` of
//! rs910-render-gpu, `--renderer` and `Backend::{Gpu, Null, Modern}`),
//! which answers the queries toolkit 0 changes
//! ([`capability::Answers`]); since Phase 4.1 the
//! shell also owns the `rs910_gpu_device` `Device` and lends it to the
//! backend per call. The full backend contract (inputs, call order,
//! answers, caching) is in `rs910_scene::scene_snapshot`'s module docs.
//!
//! Modules moved from client910 keep their names, so the facade in
//! `client910/src/lib.rs` keeps `crate::ui_paint::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod capability;
pub mod compressed_texture_format;
pub mod console_draw;
pub mod font;
pub mod frame_plan;
pub mod game_canvas;
pub mod performance_metric;
pub mod sprite_draw;
pub mod text_render;
pub mod toolkit;
pub mod toolkit_debug_flags;
pub mod ui_output;
pub mod ui_paint;

pub use frame_plan::{FramePlan, Segment};
pub use toolkit::{NullToolkit, Toolkit};

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{billboard, texture};
use rs910_config::{font_atlas, font_metrics, sprite_sheet, ui_component_fields};
use rs910_js5::cache;
use rs910_model::{font_layout, gpumodel, line_dashes, modelunlit, particle, sprite};
