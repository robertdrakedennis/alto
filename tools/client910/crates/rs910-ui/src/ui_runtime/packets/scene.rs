//! Scene-owner packets retained for the renderer/minimap owners (point
//! lights, player snapshots, environment overrides, hint arrows/trails,
//! minimap toggle, target/draw order, cutscene).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::HintTrail;
use super::super::Runtime;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_scene_packet(
        &mut self,
        _vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::PointLightColour { .. }
            | crate::server_prot::UiEvent::PointLightIntensity { .. } => {
                self.engine.effects.point_light_updates.push(event.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::PlayerSnapshot { id, gender, bytes } => {
                if let Some(models) = &mut self.target.models {
                    models.apply_snapshot(*id, *gender, bytes)?;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::ClearPlayerSnapshot { id } => {
                if let Some(models) = &mut self.target.models {
                    models.clear_snapshot(*id);
                }
                Ok(())
            }
            crate::server_prot::UiEvent::LobbyAppearance { gender, bytes } => {
                if let Some(models) = &mut self.target.models {
                    models.apply_lobby_appearance(*gender, bytes)?;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::Cutscene { .. } => {
                // The app session owner installs the cutscene state on Game
                // (`Game::begin_cutscene`); its MiniMenu close/reset request is
                // drained with the other cutscene requests on the next tick.
                Ok(())
            }
            crate::server_prot::UiEvent::OverrideEnvironment(override_event) => {
                self.engine
                    .effects
                    .environment_overrides
                    .push(override_event.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::SetTarget { value } => {
                self.engine.scene.active_target = i32::from(*value);
                Ok(())
            }
            crate::server_prot::UiEvent::SetDrawOrder { value } => {
                self.engine.scene.draw_order = i32::from(*value);
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::MinimapToggle { toggle } => {
                self.engine.minimap.toggle = *toggle;
                Ok(())
            }
            crate::server_prot::UiEvent::HintArrow { bytes } => {
                self.engine.effects.hint_arrow_updates.push(bytes.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::HintTrail {
                slot,
                model,
                points,
            } => {
                self.engine.scene.hint_trails[*slot] = (*model >= 0).then(|| HintTrail {
                    model: *model,
                    points: points.clone(),
                });
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a scene packet"),
        }
    }
}
