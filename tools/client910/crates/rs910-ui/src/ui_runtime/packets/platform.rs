//! Platform packets (audio, URL open, social-network logout, telemetry).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use super::super::SoundRequest;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_platform_packet(
        &mut self,
        _vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::Audio { command, args } => {
                self.engine.effects.sounds.push(SoundRequest {
                    command: command.clone(),
                    args: args.clone(),
                });
                Ok(())
            }
            crate::server_prot::UiEvent::UrlOpen { .. } => {
                self.engine.effects.browser_fullscreen_exit |=
                    self.engine.platform.fullscreen_allowed;
                self.engine.effects.browser_urls.push(event.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::SocialNetworkLogout { .. } => {
                self.engine.effects.browser_urls.push(event.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::Telemetry { opcode, bytes } => {
                self.engine.telemetry_packet(*opcode, bytes)
            }
            other => anyhow::bail!("packet router: {other:?} is not a platform packet"),
        }
    }
}
