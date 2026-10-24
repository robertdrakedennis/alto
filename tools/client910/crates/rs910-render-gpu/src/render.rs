//! The toolkit's frame: [`Renderer`] owns the retained UI (2D paint and
//! interface models), the developer console, the canvas scale-up, the
//! message-box and loading frames, the minimap base sprites, screenshots
//! and the capability answers. The 3D scene is the modern renderer's
//! (`rs910-render-modern`), encoded into [`Renderer::frame_composite`]'s
//! scene viewport. [`OrbitCamera`] holds the orbit/`cam2`/legacy camera
//! inputs converted to engine units per frame.
//!
//! The device layer (`gpu_device::Device`: surface, device, queue,
//! screenshot readback) is the shell's. Every method that creates, writes,
//! acquires or presents takes it as its `gpu` argument (`&Device`, or
//! `&mut Device` to present or read back).
//!
//! | Module | Concern |
//! |---|---|
//! | `resources` | [`Renderer`]'s state and the per-lifetime ownership table |
//! | `device` | construction over the lent device, depth/canvas targets, resize, toolkit changes, anti-aliasing/bloom, capability answers |
//! | `frame` | frame orchestration ([`Frame`], [`SceneTarget`], acquire/UI/scene/present) |
//! | `composition` | the frozen [`Composition`] the render thread draws and presents |
//! | `ui` | the retained UI, framebuffer sprites, the message-box frame, the console |
//! | `minimap` | minimap base sprites |
//! | `post` | the post-process chain's game state and the capture gate |
//! | `camera` | [`OrbitCamera`] |

mod camera;
mod composition;
mod device;
mod frame;
mod minimap;
mod post;
mod resources;
mod ui;

pub use camera::OrbitCamera;
pub use composition::Composition;
pub use frame::{Frame, SceneTarget, RETAINED_FRAME_ID};
pub use minimap::MinimapWorld;
pub use resources::Renderer;
use ui::RetainedUi;

pub(crate) use crate::pipelines::DEPTH_FORMAT;
