//! The post-process chain's per-frame inputs (the environment's full update)
//! and the capture gate.

use super::*;

impl Renderer {
    /// The full environment update's `updateBloom/updateLevels/
    /// updateColourRemapping` for this frame's environment.
    pub fn update_postprocess_environment(
        &mut self,
        env: &crate::env::EnvFrame,
        remappers: &mut crate::postprocess::RemapperCache,
    ) {
        let before = self.post.params.clone();
        crate::postprocess::update_full(&self.post.chain, &mut self.post.params, env, remappers);
        if before != self.post.params && crate::toolkit_debug_flags::flags().postfx_trace {
            let p = &self.post.params;
            log::info!(
                "[postfx] live {:?} remap {:?} weights {:?} base {} levels {:?} bloom {:?}",
                self.post.chain.live(p),
                p.remap
                    .remappers
                    .each_ref()
                    .map(|r| r.as_ref().map(|r| r.id)),
                p.remap.weights,
                p.remap.base,
                p.levels,
                p.bloom
            );
        }
    }

    /// The gate for the UI traversal on the hardware
    /// toolkits (`ActiveToolkit::postprocess_capture` answers for toolkit 0).
    pub fn postprocess_capture(&self) -> bool {
        self.post.capture()
    }
}
