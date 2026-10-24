//! MiniMenu state packets (`SET_MOVEACTION`, `SHOW_FACE_HERE`,
//! `SET_PLAYER_OP`, attack priority).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_menu_packet(
        &mut self,
        _vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::SetMoveAction { .. }
            | crate::server_prot::UiEvent::ShowFaceHere { .. }
            | crate::server_prot::UiEvent::SetPlayerOp { .. }
            | crate::server_prot::UiEvent::PlayerAttackPriority { .. }
            | crate::server_prot::UiEvent::NpcAttackPriority { .. } => {
                self.engine.apply_menu_state(event, &mut self.state.life);
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a menu packet"),
        }
    }
}
