//! The retained UI's output for one frame, as the renderers take it
//! (the interface's toolkit calls with the scene and the interface
//! models placed in them). Split out of client910's `ui_backend` in Phase
//! 3.2 so the GPU renderer does not name the UI crate; generic over the
//! interface-model draw `M` like [`FramePlan`] (the UI's is
//! `rs910_scene::interface_model::Draw`).
use crate::frame_plan::{FramePlan, Scene};
use crate::ui_paint::Plan;
pub struct Output<M> {
    pub models: Vec<M>,
    pub paint: Plan,
    pub scene: Option<Scene>,
    /// Paint before and after this index surrounds the real scene pass.
    pub scene_quad: usize,
    /// The toolkit calls behind `paint` (the null backend's digest).
    pub recording: crate::ui_paint::Recording,
    /// The `FULLSCREEN_ENV_LAYER` capture resolved this frame, if any.
    pub postprocess: Option<PostRegion>,
    /// Offscreen `FrameBuffer` sprites drawn before `paint`, which names each
    /// as `Image::External(id)` ([`crate::ui_paint::Painter::framebuffer_sprite`]).
    pub layers: Vec<(u64, Plan)>,
}
impl<M> Output<M> {
    /// This frame for a toolkit that executes the recorded calls: the
    /// recording, with the scene and interface-model
    /// quad boundaries mapped to op indices ([`FramePlan::from_quads`]);
    /// `quad` is a model draw's quad index.
    pub fn into_frame_plan(self, quad: impl Fn(&M) -> usize) -> FramePlan<M> {
        FramePlan::from_quads(
            self.recording,
            self.scene_quad,
            self.scene,
            self.models,
            quad,
        )
    }
}
/// One post-process capture: the layer origin and size of the region drawn
/// into the capture framebuffer, plus the painter quads emitted while the
/// framebuffer was bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostRegion {
    /// Canvas-unit `[x, y, width, height]` of the final pass bounds.
    pub rect: [i32; 4],
    pub start_quad: usize,
    pub end_quad: usize,
}
