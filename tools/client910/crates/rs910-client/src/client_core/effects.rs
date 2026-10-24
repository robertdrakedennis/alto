//! Typed cross-owner requests (code-quality programme §0.5 "typed event
//! queues", Phase 4.4; moved from `client910::app::effects`, Phase 5):
//! what the session owners (the retained interface engine's `Effects` and
//! login queues, the game's cutscene map request) ask of the shell's
//! owners (scene, environment, minimap, browser, console, login flow).
//! They replace the ad-hoc `take_*` forwarders and the `SessionRequests`
//! fields.
//!
//! Each effect is produced and drained inside one logic phase, at the
//! points and in the order the old code applied them:
//!
//! | produced by | effects, in drain order | drained |
//! |---|---|---|
//! | an interface packet batch: the live read (P8, [`packet_effects`] with the console lines), the canvas install (`resumed`) and a world login (`install_reconnected_world`) | `PointLights`, `EnvironmentOverrides`, `HintArrows`, `BrowserUrls`, (`ConsoleMessages`) | right after the batch |
//! | the logic update (P9, `update_session_logic`) | `ScriptUrl`*, `CutsceneRebuild`, `Script`*, `WorldSwitch`, `LoginCancel`, `LoginContinue`, `LoginPacket`, `Logout`, `LoginRequest`, `LobbyEnterGame` | P10; a `Script(Quit)` stops the drain and the event loop |
//! | the logic update (P9) | `CreateConnect` | P11, after the title world (the account-creation update follows the title-screen update) |
//!
//! A packet batch always yields its four (five) effects, empty or not: the
//! appliers run once per batch as before (e.g. the hint-arrow rebase).

/// One cross-owner request.
pub enum ClientEffect {
    /// `LIGHT_*` packets for the live scene's point lights (`apply_point_light_updates`).
    PointLights(Vec<crate::session::UiEvent>),
    /// `ENVIRONMENT_OVERRIDE` packets for `EnvironmentManager`.
    EnvironmentOverrides(Vec<crate::session::EnvironmentOverrideEvent>),
    /// `HINT_ARROW` packets for the minimap (held until a map base exists).
    HintArrows(Vec<Vec<u8>>),
    /// `URL_OPEN`s, and whether one asked to leave fullscreen first.
    BrowserUrls {
        urls: Vec<crate::session::UiEvent>,
        exit_fullscreen: bool,
    },
    /// Lines for the developer console.
    ConsoleMessages(Vec<String>),
    /// A script's `openurl`.
    ScriptUrl(crate::session::UiEvent),
    /// A cutscene map rebuild: the ordinary map transaction.
    CutsceneRebuild(anyhow::Result<crate::session::RebuildEvent>),
    /// A CS2 request from `ui_runtime::host_builtins` (quit, console entry,
    /// cheat).
    Script(crate::ui_runtime::host_builtins::Request),
    /// A world hop.
    WorldSwitch(crate::ui_runtime::WorldSwitchRequest),
    /// Cancel the login in progress.
    LoginCancel,
    /// `login_continue`: resume a login parked on reply 1.
    LoginContinue,
    /// A framed client packet for the connection a login waits on.
    LoginPacket(Vec<u8>),
    /// `lobby_leavelobby`: log out to the title.
    Logout,
    /// A login request from the login interface.
    LoginRequest(crate::ui_runtime::LoginRequest),
    /// The lobby's enter-game request.
    LobbyEnterGame(crate::ui_runtime::LobbyEnterGameRequest),
    /// The account creation connect request.
    CreateConnect,
}

impl ClientEffect {
    /// Drained in P11 after the title world, not with the P10 requests.
    pub fn after_title_screen(&self) -> bool {
        matches!(self, Self::CreateConnect)
    }
}

/// The browser URLs the engine queued and whether one of them asked to
/// leave fullscreen first ([`crate::ui_runtime::Engine`] `effects`).
pub fn take_browser_urls(
    ui: &mut crate::ui_runtime::Runtime,
) -> (Vec<crate::session::UiEvent>, bool) {
    (
        std::mem::take(&mut ui.engine.effects.browser_urls),
        std::mem::take(&mut ui.engine.effects.browser_fullscreen_exit),
    )
}

/// The effects of one interface packet batch, in drain order: the engine's
/// point-light, environment-override, hint-arrow and browser queues, and
/// with `console` its console lines. Without a runtime the batch's effects
/// are empty.
pub fn packet_effects(
    ui: Option<&mut crate::ui_runtime::Runtime>,
    console: bool,
) -> Vec<ClientEffect> {
    let (point_lights, overrides, hints, (urls, exit_fullscreen), lines) = match ui {
        Some(ui) => (
            std::mem::take(&mut ui.engine.effects.point_light_updates),
            std::mem::take(&mut ui.engine.effects.environment_overrides),
            std::mem::take(&mut ui.engine.effects.hint_arrow_updates),
            take_browser_urls(ui),
            console.then(|| std::mem::take(&mut ui.engine.effects.console_messages)),
        ),
        None => (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            (Vec::new(), false),
            console.then(Vec::new),
        ),
    };
    let mut effects = vec![
        ClientEffect::PointLights(point_lights),
        ClientEffect::EnvironmentOverrides(overrides),
        ClientEffect::HintArrows(hints),
        ClientEffect::BrowserUrls {
            urls,
            exit_fullscreen,
        },
    ];
    effects.extend(lines.map(ClientEffect::ConsoleMessages));
    effects
}
