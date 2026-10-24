//! The shell's drain of the core's typed cross-owner requests
//! (`rs910_client::client_core::effects`, whose module docs hold the drain
//! table): `ViewerApp::apply_effects` applies each [`ClientEffect`] to its
//! owner at the point the old code did.
use super::*;

impl ViewerApp {
    /// Applies `effects` in order; true when a script asked to quit (the
    /// remaining effects are dropped, as the old early return did).
    pub(super) fn apply_effects(&mut self, effects: Vec<ClientEffect>) -> bool {
        for effect in effects {
            if self.apply_effect(effect) {
                return true;
            }
        }
        false
    }

    fn apply_effect(&mut self, effect: ClientEffect) -> bool {
        match effect {
            ClientEffect::PointLights(updates) => self.apply_point_light_updates(updates),
            ClientEffect::EnvironmentOverrides(overrides) => {
                self.apply_environment_overrides(overrides)
            }
            ClientEffect::HintArrows(updates) => self.apply_hint_arrow_updates(updates),
            ClientEffect::BrowserUrls {
                urls,
                exit_fullscreen,
            } => open_browser_urls((urls, exit_fullscreen), self.core.session.game_mut()),
            ClientEffect::ConsoleMessages(lines) => self.console.pending_messages.extend(lines),
            ClientEffect::ScriptUrl(url) => open_browser_url(&url),
            ClientEffect::CutsceneRebuild(Ok(event)) => self.reload_for_event(event),
            ClientEffect::CutsceneRebuild(Err(error)) => {
                log::warn!("[client910] cutscene map request: {error:#}")
            }
            ClientEffect::Script(request) => return self.apply_script_request(request),
            ClientEffect::WorldSwitch(request) => self.start_world_switch(request),
            ClientEffect::LoginCancel => self.cancel_login_request(),
            ClientEffect::LoginContinue => {
                if let Some(session) = self.core.session.as_ref() {
                    session.login_progress.request_resume();
                }
            }
            ClientEffect::LoginPacket(packet) => {
                if let Some(session) = self.core.session.as_ref() {
                    session.login_progress.queue_packet(packet);
                }
            }
            // Leaving the lobby through a script logs out without the reconnect delay.
            ClientEffect::Logout => self.client_logout(false),
            ClientEffect::LoginRequest(request) => self.start_login_request(request),
            ClientEffect::LobbyEnterGame(request) => self.start_game_from_lobby(request),
            ClientEffect::CreateConnect => self.start_create_connect(),
        }
        false
    }
}
