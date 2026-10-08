//! `read` for the retained UI: [`Runtime::packet_event`] routes
//! each server event to its family applier (packets/*.rs) or, for the
//! interface packets, through the canvas gate to `Runner::packet`
//! (code-quality programme Phase 4.2).
use super::{Provider, Runner, Runtime};
use crate::{ui_hook_host::Domains, ui_vars::Variables};
use anyhow::Result;

mod camera;
mod chat;
mod menu;
mod platform;
mod scene;
mod session;
mod social;
mod vars;

impl Runtime {
    pub(super) fn packet_event(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        self.sync_active_clan_channel();
        let local_player_id = vars.scene.local_player.map(|player| player.index as usize);
        self.engine.social.local_player_name = local_player_id
            .and_then(|id| {
                vars.scene
                    .players
                    .and_then(|players| players.players.get(id))
            })
            .and_then(Option::as_ref)
            .and_then(|player| player.appearance.name.clone())
            // Title/lobby: localPlayerEntity is the placeholder entity
            // named by the lobby login reply.
            .unwrap_or_else(|| self.engine.login.lobby_player_name.clone());
        self.engine.social.local_player_title = local_player_id
            .and_then(|id| {
                vars.scene
                    .players
                    .and_then(|players| players.players.get(id))
            })
            .and_then(Option::as_ref)
            .and_then(|player| player.appearance.title.clone());
        // The packets owned outside the interface path, by family
        // (packets/*.rs); the rest take the canvas gate and `Runner::packet`.
        use crate::server_prot::UiEvent as E;
        match event {
            E::PointLightColour { .. }
            | E::PointLightIntensity { .. }
            | E::PlayerSnapshot { .. }
            | E::ClearPlayerSnapshot { .. }
            | E::LobbyAppearance { .. }
            | E::Cutscene { .. }
            | E::OverrideEnvironment(..)
            | E::SetTarget { .. }
            | E::SetDrawOrder { .. }
            | E::MinimapToggle { .. }
            | E::HintArrow { .. }
            | E::HintTrail { .. } => return self.apply_scene_packet(vars, event),
            E::StockmarketSlot { .. }
            | E::Inventory { .. }
            | E::Stat { .. }
            | E::VarClanEnable
            | E::VarClanDisable
            | E::VarClan { .. }
            | E::RunEnergy { .. }
            | E::RunWeight { .. }
            | E::VarcAck => return self.apply_vars_packet(vars, event),
            E::Audio { .. }
            | E::UrlOpen { .. }
            | E::SocialNetworkLogout { .. }
            | E::Telemetry { .. } => return self.apply_platform_packet(vars, event),
            E::SiteSettings { .. }
            | E::Uid192 { value: Some(_), .. }
            | E::Uid192 { value: None, .. }
            | E::LastLoginInfo { .. }
            | E::WorldList { .. }
            | E::RebootTimer { .. }
            | E::UpdateDob { .. }
            | E::LoyaltyUpdate { .. }
            | E::JCoinsUpdate { .. }
            | E::CreateEmailReply { .. }
            | E::AccountCreationResult { .. }
            | E::CreateNameReply { .. }
            | E::CreateSuggestNameError { .. }
            | E::CreateSuggestName { .. }
            | E::Logout { .. }
            | E::ChangeLobby { .. }
            | E::LogoutTransfer { .. } => return self.apply_session_packet(vars, event),
            E::CameraReset
            | E::CameraSmoothReset
            | E::CameraForceAngle { .. }
            | E::CameraShake { .. }
            | E::CameraMoveTo { .. }
            | E::CameraLookAt { .. }
            | E::CameraRemoveRoof { .. }
            | E::Camera { .. } => return self.apply_camera_packet(vars, event),
            E::ChatFilters { .. }
            | E::ChatPrivateFilter { .. }
            | E::FriendChannelMessage { .. }
            | E::ClanChannelMessage { .. }
            | E::ClanChannelSystemMessage { .. }
            | E::PlayerGroupMessage { .. }
            | E::QuickChat { .. }
            | E::PublicMessage { .. }
            | E::GameMessage { .. }
            | E::PrivateMessageEcho { .. }
            | E::PrivateMessage { .. } => return self.apply_chat_packet(vars, event),
            E::FriendList { .. }
            | E::FriendListLoaded
            | E::IgnoreList { .. }
            | E::FriendChatFull { .. }
            | E::FriendChatSingle { .. }
            | E::ClanChannelFull { .. }
            | E::ClanRosterDelta { .. }
            | E::ClanSettingsFull { .. }
            | E::ClanSettingsUpdate { .. }
            | E::PlayerGroupFull { .. }
            | E::GroupRosterDelta { .. }
            | E::PlayerGroupVars { .. } => return self.apply_social_packet(vars, event),
            E::SetMoveAction { .. }
            | E::ShowFaceHere { .. }
            | E::SetPlayerOp { .. }
            | E::PlayerAttackPriority { .. }
            | E::NpcAttackPriority { .. } => return self.apply_menu_packet(vars, event),
            _ => {}
        }
        anyhow::ensure!(self.canvas_ready, "UI packet before canvas initialization");
        // currentPlayerUid is read by IF_SETPLAYERHEAD/SELF model
        // packets at dispatch time; keep the lifecycle cache in sync even
        // when no script has run since the scene was installed.
        self.state.local_player_uid = vars.scene.local_player.map_or(-1, |p| p.index);
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        if matches!(event, crate::server_prot::UiEvent::TriggerDialogAbort) {
            // read calls runHookImmediate on the opened
            // top interface before accepting the next server packet.
            let top = self.state.life.top;
            crate::ui_hooks::immediate(&mut self.store, &mut self.state, top, 0, &mut runner)?;
            self.diagnostics.record(runner.executions, runner.missing);
            self.drain_services();
            for rect in std::mem::take(&mut self.state.menu.redraw) {
                self.frame.request_at(&mut self.state, rect);
            }
            return Ok(());
        }
        let result = runner.packet(&mut self.store, &mut self.state, event);
        self.diagnostics.record(runner.executions, runner.missing);
        self.drain_services();
        for rect in std::mem::take(&mut self.state.menu.redraw) {
            self.frame.request_at(&mut self.state, rect);
        }
        result
    }
}
