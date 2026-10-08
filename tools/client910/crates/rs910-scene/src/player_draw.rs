//! What the player draw reads of the player
//! owner this frame: each player's body draw matrix and visibility, its
//! spot shadow and the local player's hint arrows. The GPU renderer's
//! `PlayersRenderer` builds them beside its GPU meshes; the other backends
//! read them through [`PlayerDraws`] without naming that owner (Phase 3.3;
//! target-architecture §2.3: the player model prep as a toolkit-neutral
//! scene frame input). The body model itself is the player's scene-graph
//! entity (`scene.temporary[..].model`).
use crate::actor_matrix::Matrix;
use crate::gpumodel::GpuModel;

/// One player this frame (`PlayersRenderer.meshes[player]`).
pub struct PlayerDraw<'a> {
    /// The body's draw matrix (the actor matrices the owner built).
    pub matrix: &'a Matrix,
    /// Whether the body passed the draw-list visibility test.
    pub visible: bool,
    /// The spot shadow drawn before the body, if the player has one.
    pub shadow: Option<ShadowDraw<'a>>,
    /// The local player's hint-arrow models, drawn after the shadow without
    /// depth writes, under the shadow's matrix.
    pub hint_arrows: Vec<ShadowDraw<'a>>,
}

/// A player's spot-shadow or hint-arrow model this frame.
pub struct ShadowDraw<'a> {
    pub model: &'a GpuModel,
    pub matrix: &'a Matrix,
    pub visible: bool,
}

/// The player owner's frame, by player index.
pub trait PlayerDraws {
    fn player_draw(&self, player: usize) -> Option<PlayerDraw<'_>>;
}
