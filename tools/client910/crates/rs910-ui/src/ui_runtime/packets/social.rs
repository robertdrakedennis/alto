//! Friends, ignores, friend chat, clan channel/settings and player-group
//! state packets.
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_social_packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::FriendList { bytes } => {
                self.engine
                    .social
                    .apply_friend_update(bytes, self.engine.login.world)?;
                Ok(())
            }
            crate::server_prot::UiEvent::FriendListLoaded => {
                self.engine.social.mark_friend_list_loaded();
                self.engine.social.stamps.friend = true;
                Ok(())
            }
            crate::server_prot::UiEvent::IgnoreList { bytes } => {
                self.engine.social.apply_ignore_update(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::FriendChatFull { bytes } => {
                self.engine.social.apply_friend_chat_full(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::FriendChatSingle { bytes } => {
                self.engine.social.apply_friend_chat_single(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::ClanChannelFull { bytes } => {
                self.engine.social.apply_clan_channel_full(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::ClanRosterDelta { bytes } => {
                self.engine.social.apply_clan_channel_delta(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::ClanSettingsFull { bytes } => {
                self.engine.social.apply_clan_settings_full(bytes)?;
                // Link the script state's CLAN_SETTING domain to the social
                // owner's activeClanSettings.
                vars.state.clan_settings = Some(self.engine.social.active_settings_domain.clone());
                Ok(())
            }
            crate::server_prot::UiEvent::ClanSettingsUpdate { bytes } => {
                self.engine.social.apply_clan_settings_delta(bytes)?;
                Ok(())
            }
            crate::server_prot::UiEvent::PlayerGroupFull { bytes } => {
                let group_type = |id: i32| {
                    vars.definitions
                        .binding(9, id)
                        .ok()
                        .flatten()
                        .and_then(|binding| binding.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                };
                let member_type = |id: i32| {
                    vars.definitions
                        .binding(0, id)
                        .ok()
                        .flatten()
                        .and_then(|binding| binding.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                };
                self.engine
                    .social
                    .apply_player_group_full(bytes, group_type, member_type)?;
                vars.state.player_group = self
                    .engine
                    .social
                    .player_group
                    .as_ref()
                    .map(|group| group.vars.clone());
                self.state.life.cycles.player_group = self.state.life.cycles.redraw;
                Ok(())
            }
            crate::server_prot::UiEvent::GroupRosterDelta { bytes } => {
                let group_type = |id: i32| {
                    vars.definitions
                        .binding(9, id)
                        .ok()
                        .flatten()
                        .and_then(|binding| binding.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                };
                let member_type = |id: i32| {
                    vars.definitions
                        .binding(0, id)
                        .ok()
                        .flatten()
                        .and_then(|binding| binding.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                };
                let varbit_type = |id: i32| {
                    vars.definitions
                        .get(id, false)
                        .ok()
                        .and_then(|bit| bit.binding.map(|binding| (binding.id, bit.start, bit.end)))
                };
                let changed = self.engine.social.apply_player_group_delta(
                    bytes,
                    group_type,
                    member_type,
                    varbit_type,
                )?;
                vars.state.player_group = self
                    .engine
                    .social
                    .player_group
                    .as_ref()
                    .map(|group| group.vars.clone());
                self.state.life.cycles.player_group = self.state.life.cycles.redraw;
                if !changed.is_empty() {
                    self.state.life.cycles.player_group_varp = self.state.life.cycles.redraw;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::PlayerGroupVars { bytes } => {
                let member_type = |id: i32| {
                    vars.definitions
                        .binding(0, id)
                        .ok()
                        .flatten()
                        .and_then(|binding| binding.data_type)
                        .and_then(crate::protocol910::script_types::script_type)
                        .map(|(base, _)| base)
                };
                self.engine
                    .social
                    .apply_player_group_vars(bytes, member_type)?;
                self.state.life.cycles.player_group_varp = self.state.life.cycles.redraw;
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a social packet"),
        }
    }
}
