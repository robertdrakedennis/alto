//! One frame's display list for a toolkit (target-architecture.md §2.4:
//! "execution of a recorded `FramePlan`"). Phase 3.1.
//!
//! The redraw draws the retained UI's interface, which calls into the
//! toolkit in painter order and, at the scene component, draws the 3D scene
//! and, at each type-6 component, an interface model. A
//! [`FramePlan`] is that sequence recorded: the 2D calls
//! ([`Recording`]), the op index where the scene viewport draws and the op
//! index of every interface model. The GPU renderer consumes the same
//! boundaries as quad indices of its [`crate::ui_paint::Plan`]; a toolkit
//! that executes the calls walks the plan with [`FramePlan::walk`].
//!
//! The model type `M` is the backend's model draw (today client910's
//! `ui_models::Draw`, which carries both the GPU and the canvas-space inputs);
//! this crate does not interpret it. A modern backend can supply its own `M`
//! (materials, lights) without changing the 2D contract.
use crate::ui_paint::{Op, Recording};

/// The scene viewport of a frame: `rect` is the canvas `[x, y, w, h]` of
/// the 3D scene's viewport, `clip` the `[x0, y0, x1, y1]` bounds in
/// force when the scene component drew.
pub struct Scene {
    pub rect: [i32; 4],
    pub clip: [i32; 4],
}

/// One frame for a toolkit: the retained UI's 2D calls in painter
/// order with the 3D segments interleaved at op indices.
pub struct FramePlan<M> {
    pub recording: Recording,
    /// Op index where the scene viewport draws (`Output::scene_quad` mapped
    /// through [`Recording::op_index`]).
    pub scene_op: usize,
    pub scene: Option<Scene>,
    /// Component models (type-6 components) and their op index.
    pub models: Vec<(usize, M)>,
}

/// One step of [`FramePlan::walk`], in painter order.
pub enum Segment<'a, M> {
    /// A run of 2D calls to execute.
    Ops(&'a [Op]),
    /// Draw the 3D scene into this viewport.
    Scene(&'a Scene),
    /// Draw one interface model.
    Model(&'a M),
}

impl<M> FramePlan<M> {
    /// A plan from a painter's output, whose scene and model positions are
    /// GPU quad boundaries: each maps onto an op index through
    /// [`Recording::op_index`].
    pub fn from_quads(
        recording: Recording,
        scene_quad: usize,
        scene: Option<Scene>,
        models: Vec<M>,
        quad: impl Fn(&M) -> usize,
    ) -> Self {
        let scene_op = recording.op_index(scene_quad);
        let models = models
            .into_iter()
            .map(|m| (recording.op_index(quad(&m)), m))
            .collect();
        Self {
            recording,
            scene_op,
            scene,
            models,
        }
    }

    /// Visit the frame in painter order: the ops up to each boundary, the
    /// scene at its op index (when there is a viewport), then the models at
    /// that index, and the remaining ops. This is the removed software
    /// toolkit's frame loop, kept exactly: boundaries are the
    /// model indices plus the scene index, sorted and deduplicated, each
    /// clamped to the op count as it is reached.
    pub fn walk(&self, mut visit: impl FnMut(Segment<'_, M>)) {
        let ops = &self.recording.ops;
        let split = self.scene_op.min(ops.len());
        let mut done = 0;
        let mut stops: Vec<usize> = self.models.iter().map(|(i, _)| *i).collect();
        stops.push(split);
        stops.sort_unstable();
        stops.dedup();
        for stop in stops {
            let stop = stop.min(ops.len());
            visit(Segment::Ops(&ops[done..stop]));
            done = stop;
            if stop == split {
                if let Some(viewport) = self.scene.as_ref() {
                    visit(Segment::Scene(viewport));
                }
            }
            for (_, model) in self.models.iter().filter(|(i, _)| *i == stop) {
                visit(Segment::Model(model));
            }
        }
        visit(Segment::Ops(&ops[done..]));
    }
}

#[cfg(test)]
#[path = "frame_plan_tests.rs"]
mod tests;
