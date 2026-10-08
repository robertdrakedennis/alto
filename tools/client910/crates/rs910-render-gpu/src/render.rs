//! client910 GPU renderer (wgpu + glam): the hardware toolkit's frame.
//!
//! [`Renderer`] draws the faithful scene
//! (`floor_render`/`floorpass` floors, lights and hard shadows, scene-graph
//! models in draw-plan order, particles, skybox/underwater),
//! the post-process chain, the retained UI, the developer console and the
//! canvas scale-up, and owns the minimap base sprites.
//! [`OrbitCamera`] holds the orbit/`cam2`/legacy camera inputs converted to
//! engine units per frame. It draws every toolkit's frames, toolkit 0
//! included; the shell's `ActiveToolkit` chooses the backend.
//!
//! Phase 4.1: the device layer (`gpu_device::Device`: surface, device,
//! queue, screenshot readback) is the shell's, not the renderer's. Every
//! method that creates, writes, acquires or presents takes it as its `gpu`
//! argument (`&Device`, or `&mut Device` to present or read back), in the
//! same order of device calls as when the renderer owned it, so a second
//! backend can share the one device. [`Faithful`] is the renderer and the
//! lent device together, for the scene-mesh owners.
//!
//! Renderer plan M1: [`Renderer::frame_composite`] is the faithful frame's
//! composition (the retained UI below and over the scene, the console, the
//! retained copy, the canvas scale-up, screenshots) around a scene another
//! backend encodes (the NXT renderer); [`Renderer::frame_with_levels`] uses
//! the same steps ([`Renderer::begin_frame`], [`Renderer::encode_ui_under`],
//! [`Renderer::encode_ui_over`]) around the faithful scene passes.
//!
//! Layout (programme §5 Phase 4.5, target-architecture §4):
//!
//! | Module | Concern |
//! |---|---|
//! | `resources` | [`Renderer`]'s state and the per-lifetime ownership table; [`Faithful`] |
//! | `device` | construction over the lent device, depth/canvas targets, resize, toolkit changes, MSAA/bloom, capability answers |
//! | `upload` | floor/light/shadow/model/underwater/sky/particle/billboard uploads and per-frame selections |
//! | `scene_pass` | the faithful scene passes (sky, underwater, opaque, floors, transparent, post) |
//! | `frame` | frame orchestration ([`Frame`], [`SceneTarget`], acquire/UI/scene/present, toolkit 0's presentation) |
//! | `ui` | the retained UI, framebuffer sprites, the message-box frame, the console |
//! | `minimap` | minimap base sprites |
//! | `post` | post-process chain inputs and capture bounds |
//! | `camera` | [`OrbitCamera`] |
//!
//! Every pass takes its pipelines from one cache keyed by (samples,
//! format, variant), [`crate::pipelines`] (`Renderer.pipelines`, shared with
//! the owners the renderer creates).

mod camera;
mod composition;
mod device;
mod frame;
mod minimap;
mod post;
mod resources;
mod scene_pass;
mod ui;
mod upload;

pub use camera::OrbitCamera;
pub use composition::Composition;
pub use frame::{Frame, SceneTarget, RETAINED_FRAME_ID};
pub use minimap::MinimapWorld;
pub use resources::{Faithful, FaithfulRef, Renderer};
use ui::RetainedUi;

pub(crate) use crate::pipelines::DEPTH_FORMAT;

#[cfg(test)]
mod tests;
