//! Login/lobby/session packets (site settings, uid192, last login, world
//! list, logout/transfer, reboot timer, lobby profile, account creation
//! replies).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_session_packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::SiteSettings { value } => {
                self.engine.login.site_settings = value.clone();
                Ok(())
            }
            crate::server_prot::UiEvent::Uid192 { value: Some(value) } => {
                self.engine.login.uid192 = *value;
                Ok(())
            }
            crate::server_prot::UiEvent::Uid192 { value: None } => Ok(()),
            crate::server_prot::UiEvent::LastLoginInfo { address } => {
                // The name is looked up in the background.
                self.engine.login.last_login = Some(crate::host_name::HostName::lookup(
                    *address,
                    crate::logic_clock::monotonic_millis(),
                ));
                Ok(())
            }
            crate::server_prot::UiEvent::WorldList { bytes } => {
                self.engine.world_list.apply_reply(bytes, (vars.now)())
            }
            crate::server_prot::UiEvent::RebootTimer { ticks } => {
                // read uses 2.5 seconds per lobby tick and
                // 30 game cycles per world tick. The retained top id is the
                // same state discriminator used by the live interface owner.
                self.engine.reboot_timer =
                    if crate::client_state::is_lobby(self.engine.login.client_state) {
                        (i32::from(*ticks) * 5) / 2
                    } else {
                        i32::from(*ticks) * 30
                    };
                self.state.life.cycles.misc = self.state.life.cycles.redraw;
                Ok(())
            }
            crate::server_prot::UiEvent::UpdateDob { dob, verified } => {
                self.engine.account.dob = *dob;
                self.engine.account.dob_verified = *verified;
                Ok(())
            }
            crate::server_prot::UiEvent::LoyaltyUpdate { value } => {
                let changed = self.engine.lobby.loyalty_balance != *value;
                self.engine.lobby.loyalty_balance = *value;
                if changed {
                    self.run_global_trigger(vars, 21)?;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::JCoinsUpdate { value } => {
                let changed = self.engine.lobby.jcoins_balance != *value;
                self.engine.lobby.jcoins_balance = *value;
                if changed {
                    self.run_global_trigger(vars, 19)?;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::CreateEmailReply { value } => {
                self.engine.creation.email_reply = *value;
                Ok(())
            }
            crate::server_prot::UiEvent::AccountCreationResult { value } => {
                self.engine.creation.account_reply = *value;
                Ok(())
            }
            crate::server_prot::UiEvent::CreateNameReply { value } => {
                self.engine.creation.name_reply = *value;
                Ok(())
            }
            crate::server_prot::UiEvent::CreateSuggestNameError { value } => {
                self.engine.creation.suggest_reply = *value;
                self.engine.creation.suggested_name = None;
                Ok(())
            }
            crate::server_prot::UiEvent::CreateSuggestName { value } => {
                self.engine.creation.suggest_reply = 2;
                self.engine.creation.suggested_name = Some(value.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::Logout { reason, .. } => {
                // The app session owner closes the transport after this event.
                // The reason is recorded and the telemetry error latched. No UI
                // state is mutated here.
                self.engine.builtins.logout(*reason);
                self.logout_camera_and_cutscene();
                Ok(())
            }
            crate::server_prot::UiEvent::ChangeLobby { .. }
            | crate::server_prot::UiEvent::LogoutTransfer { .. } => {
                // The world switcher owns endpoint/transfer state; the retained UI
                // remains unchanged until the app session owner reopens lobby 906.
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a session packet"),
        }
    }
}
