//! The client session state machine (title, lobby, game, reconnect).
//!
//! This is the CPU half of the session lifecycle: the app owns sockets,
//! workers and the retained interface runtime, and executes the [`Effect`]s
//! returned here in order. The transitions are:
//!
//! - setting the state ([`Machine::set_state`]),
//! - logging out ([`Machine::logout`]),
//! - reconnecting after a connection failure ([`Machine::try_reconnect`]),
//! - the state reached after a failed or cancelled login
//!   ([`Machine::update_login_state`]),
//! - the login-ready gate ([`Machine::login_ready`]),
//! - the state-14/19 arm of the logic loop ([`Machine::game_login_failed`]).
//!
//! State predicates are `is_title`/`is_lobby`/`is_game`/`is_rebuild`
//! (`rs910_core::client_state`) and the private lobby-enter-game predicate.
//!
//! The loading states 5/11/1 are driven by `crate::loading::Loading`, which
//! enters [`LOGIN`] from its last stage.
//!
//! The rebuild states 3/6/8/10/16 are entered when a map rebuild starts and
//! left when the rebuilt scene completes; [`RebuildProgress`] is the rebuild
//! stage and progress counters the redraw draws.

// The client-state predicates live in rs910-core (Phase 2.3) so the
// JS5 client can read them without an upward edge.
pub use rs910_core::client_state::{is_game, is_lobby, is_rebuild, is_title};

/// State 0: title account creation on a connected lobby stream.
pub const ACCOUNT_CREATION_CONNECTED: i32 = 0;
/// State 5: loading before the first cache loading screen is ready (the
/// initial state).
pub const LOADING: i32 = 5;
/// State 11: loading with the cache loading screens shown.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "state constant; read by tests only")
)]
pub const LOADING_SCREENS: i32 = 11;
/// State 1: loading after the audio defaults.
pub const LOADING_AUDIO: i32 = 1;
/// State 4: title screen showing the graphics defaults' login interface.
pub const LOGIN: i32 = 4;
/// State 7: title-screen game login in progress.
pub const LOGIN_GAME: i32 = 7;
/// State 9: a logout to the lobby, relogging into it.
pub const LOBBY_RELOGIN: i32 = 9;
/// State 12: title-screen account creation.
pub const ACCOUNT_CREATION: i32 = 12;
/// State 13: lobby.
pub const LOBBY: i32 = 13;
/// State 14: connection lost, reconnecting to the same world.
pub const RECONNECT: i32 = 14;
/// State 15: `lobby_entergame` world login in progress.
pub const LOBBY_ENTER_GAME: i32 = 15;
/// State 17: `lobby_enterlobby` lobby login in progress.
pub const LOBBY_LOGIN: i32 = 17;
/// State 18: in game.
pub const GAME: i32 = 18;
/// State 19: `LOGOUT_TRANSFER` world hop in progress.
pub const TRANSFER: i32 = 19;
/// State 3: in-game map rebuild (from a normal, region or cutscene rebuild,
/// or from state 18).
pub const REBUILD_GAME: i32 = 3;
/// State 6: lobby rebuild (from 13).
pub const REBUILD_LOBBY: i32 = 6;
/// State 8: account-creation rebuild (from 0).
pub const REBUILD_ACCOUNT_CREATION: i32 = 8;
/// State 10: title rebuild (from 4).
pub const REBUILD_TITLE: i32 = 10;
/// State 16: lobby enter-game rebuild (from 15).
pub const REBUILD_LOBBY_ENTER_GAME: i32 = 16;

/// The rebuild state entered from `state` when a rebuild starts (or the
/// toolkit changes); `None` where the state is left alone.
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "rebuild-start state table; exercised by tests only"
    )
)]
pub fn rebuild_state_for(state: i32) -> Option<i32> {
    match state {
        LOGIN => Some(REBUILD_TITLE),
        LOBBY => Some(REBUILD_LOBBY),
        LOBBY_ENTER_GAME => Some(REBUILD_LOBBY_ENTER_GAME),
        GAME => Some(REBUILD_GAME),
        ACCOUNT_CREATION_CONNECTED => Some(REBUILD_ACCOUNT_CREATION),
        _ => None,
    }
}

/// The state a finished rebuild leaves for:
/// 10 -> 4, 6 -> 13, 16 -> 15, 8 -> 0, anything else -> 18 (which queues
/// `MAP_BUILD_COMPLETE` on a live game connection).
#[must_use]
pub fn rebuilt_state(state: i32) -> i32 {
    match state {
        REBUILD_TITLE => LOGIN,
        REBUILD_LOBBY => LOBBY,
        REBUILD_LOBBY_ENTER_GAME => LOBBY_ENTER_GAME,
        REBUILD_ACCOUNT_CREATION => ACCOUNT_CREATION_CONNECTED,
        _ => GAME,
    }
}

/// The rebuild stage: idle, waiting for maps, waiting for locs, building.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RebuildStage {
    #[default]
    Idle,
    LoadMaps,
    LoadLocs,
    Build,
}

/// The locs a map build still waits for the models of, and the last one it
/// waited on (both -1 when nothing waits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocsWaiting {
    pub count: i32,
    pub loc: i32,
    pub model: i32,
}

impl Default for LocsWaiting {
    fn default() -> Self {
        Self {
            count: 0,
            loc: -1,
            model: -1,
        }
    }
}

/// How many cycles the same non-zero number of locs may keep the map build
/// waiting before the client reports it stuck.
pub const MAP_BUILD_STUCK_CYCLES: i32 = 1000;

/// The rebuild stage plus the pending/total landscape and loc counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebuildProgress {
    pub stage: RebuildStage,
    pub landscape_progress: i32,
    pub landscape_total: i32,
    pub locs_count: i32,
    pub locs_total: i32,
    /// The logic cycle the number of locs waited on last changed (0: not yet).
    pub locs_changed_at: i32,
}

impl Default for RebuildProgress {
    fn default() -> Self {
        Self {
            stage: RebuildStage::Idle,
            landscape_progress: 0,
            landscape_total: 1,
            locs_count: 0,
            locs_total: 1,
            locs_changed_at: 0,
        }
    }
}

impl RebuildProgress {
    /// Finish a rebuild: reset the stage and counters (run when any rebuild
    /// state is entered).
    pub fn complete(&mut self) {
        *self = Self::default();
    }

    /// `pending` map squares (plus the world-map area group) not yet
    /// downloaded. False while the maps stage waits.
    pub fn load_maps(&mut self, pending: i32) -> bool {
        self.landscape_progress = pending;
        if pending > 0 {
            if self.landscape_total < pending {
                self.landscape_total = pending;
            }
            self.stage = RebuildStage::LoadMaps;
            return false;
        }
        true
    }

    /// `count` locs whose models are not yet downloaded. False while the
    /// locs stage waits.
    pub fn load_locs(&mut self, count: i32) -> bool {
        self.locs_count = count;
        if count > 0 {
            if self.locs_total < count {
                self.locs_total = count;
            }
            self.stage = RebuildStage::LoadLocs;
            return false;
        }
        true
    }

    /// [`RebuildProgress::load_locs`] at logic cycle `cycle` for the locs in
    /// `waiting`; returns them when the same non-zero number has waited
    /// exactly [`MAP_BUILD_STUCK_CYCLES`] cycles, once.
    pub fn watch_locs(&mut self, waiting: LocsWaiting, cycle: i32) -> Option<LocsWaiting> {
        let before = self.locs_count;
        if self.load_locs(waiting.count) {
            return None;
        }
        if waiting.count != before {
            self.locs_changed_at = cycle;
            None
        } else if self.locs_changed_at != 0
            && cycle.wrapping_sub(self.locs_changed_at) == MAP_BUILD_STUCK_CYCLES
        {
            Some(waiting)
        } else {
            None
        }
    }

    /// Enter the build stage; true when the `(100%)` message box is drawn
    /// first (some wait stage ran).
    pub fn begin_build(&mut self) -> bool {
        let waited = self.stage != RebuildStage::Idle;
        self.stage = RebuildStage::Build;
        waited
    }

    /// Percentage of map squares downloaded.
    #[must_use]
    pub fn maps_percent(&self) -> i32 {
        100 - self.landscape_progress * 100 / self.landscape_total
    }

    /// Percentage of loc models downloaded.
    #[must_use]
    pub fn locs_percent(&self) -> i32 {
        100 - self.locs_count * 100 / self.locs_total
    }

    /// The message-box text drawn in a rebuild state; `loading` is the
    /// localised "Loading" text.
    #[must_use]
    pub fn message(&self, loading: &str) -> String {
        match self.stage {
            RebuildStage::LoadMaps => format!("{loading}<br>({}%)", self.maps_percent() / 2),
            RebuildStage::LoadLocs => {
                format!("{loading}<br>({}%)", self.locs_percent() / 2 + 50)
            }
            RebuildStage::Idle | RebuildStage::Build => loading.to_owned(),
        }
    }
}

/// Whether `state` is one of the lobby enter-game states (15, 16).
#[must_use]
fn is_lobby_enter_game(state: i32) -> bool {
    matches!(state, 15 | 16)
}

/// Side effect requested by a transition. The app applies them in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Show the login screen: when `reopen`, unload the top interface, close
    /// every sub-interface, drop loaded interfaces and open the graphics
    /// defaults' login interface; always reset the retained credentials and
    /// the local player/world presentation.
    ShowLogin { reopen: bool },
    /// Show the lobby: when `reopen`, replace the interface tree with the
    /// graphics defaults' lobby interface (`-1` in the 910 cache: the lobby
    /// server's `IF_OPENTOP` supplies it).
    ShowLobby { reopen: bool },
    /// Request a lobby login against the current lobby.
    RequestLobbyLogin,
    /// Request a game login against the current world. `reconnect` is the
    /// GAMELOGIN header byte (1 in state 14).
    RequestGameLogin { reconnect: bool },
    /// Reject the login with reply 3 and take the failed-login transition
    /// when the credentials are empty; `lobby` selects the lobby reply over
    /// the game reply.
    RejectEmptyCredentials { lobby: bool },
    /// Log out: flush and close every server connection, reset login state,
    /// release the scene and reset the world/actor/camera owners.
    CloseConnections,
    /// State-19 cancellable transfer failure: switch back to the previous
    /// world and force-close the game connection.
    RestorePreviousWorld,
    /// Entering a rebuild state: complete the rebuild and clear its start
    /// time. The front-to-back copy is the renderer's retained frame
    /// (`Renderer::frame_message_box`).
    CompleteRebuild,
}

/// Session inputs the transitions read from other owners.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The opened top interface.
    pub top: i32,
    /// The graphics defaults' login interface (defaults opcode 5).
    pub login_interface: i32,
    /// The graphics defaults' lobby interface (defaults opcode 6).
    pub lobby_interface: i32,
    /// Both username and password are non-empty (the non-SSO path).
    pub credentials: bool,
    /// The lobby connection has an open stream.
    pub lobby_connected: bool,
}

/// The retained client state plus the logged-in flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub state: i32,
    /// Set by showing the lobby, cleared by showing the login screen.
    pub logged_in: bool,
    /// Scene draws since the state last changed. The connection's silence is
    /// only counted once the world has been drawn a few times.
    pub state_ticks: i32,
}

impl Default for Machine {
    /// State 5: loading.
    fn default() -> Self {
        Self {
            state: LOADING,
            logged_in: false,
            state_ticks: 0,
        }
    }
}

impl Machine {
    /// Move to state `next`, queueing the effects the change requires.
    pub fn set_state(&mut self, next: i32, ctx: &Context, out: &mut Vec<Effect>) {
        if self.state == next {
            return;
        }
        self.state_ticks = 0;
        if next == RECONNECT || next == TRANSFER {
            out.push(Effect::RequestGameLogin {
                reconnect: next == RECONNECT,
            });
        }
        if next == LOGIN {
            // Reopen when coming from account creation or when the login
            // interface is not already the top interface.
            let reopen =
                self.state == 0 || self.state == ACCOUNT_CREATION || ctx.top != ctx.login_interface;
            out.push(Effect::ShowLogin { reopen });
            self.logged_in = false;
        }
        if next == LOBBY {
            let reopen = if ctx.lobby_interface == -1 {
                self.state == LOBBY_LOGIN || self.state == LOBBY_RELOGIN
            } else {
                ctx.top != ctx.lobby_interface
            };
            out.push(Effect::ShowLobby { reopen });
            self.logged_in = true;
        }
        if next == LOBBY_LOGIN || next == LOBBY_RELOGIN {
            if !ctx.credentials {
                out.push(Effect::RejectEmptyCredentials { lobby: true });
                self.update_login_state(ctx, out);
                return;
            }
            out.push(Effect::RequestLobbyLogin);
        } else if next == LOGIN_GAME || (next == LOBBY_ENTER_GAME && self.state != 16) {
            if !ctx.credentials {
                out.push(Effect::RejectEmptyCredentials { lobby: false });
                self.update_login_state(ctx, out);
                return;
            }
            out.push(Effect::RequestGameLogin { reconnect: false });
        }
        if is_rebuild(next) {
            out.push(Effect::CompleteRebuild);
        }
        self.state = next;
    }

    /// The state reached after a failed or cancelled login.
    pub fn update_login_state(&mut self, ctx: &Context, out: &mut Vec<Effect>) {
        if is_lobby_enter_game(self.state) {
            let next = if ctx.lobby_connected {
                LOBBY
            } else {
                LOBBY_LOGIN
            };
            self.set_state(next, ctx, out);
        } else if matches!(self.state, LOBBY_LOGIN | LOGIN_GAME | LOBBY_RELOGIN) {
            self.set_state(LOGIN, ctx, out);
        }
    }

    /// Log out, to the lobby relogin (`to_lobby`) or to the title.
    pub fn logout(&mut self, to_lobby: bool, ctx: &Context, out: &mut Vec<Effect>) {
        out.push(Effect::CloseConnections);
        let ctx = Context {
            lobby_connected: false,
            ..*ctx
        };
        if to_lobby {
            self.set_state(LOBBY_RELOGIN, &ctx, out);
        } else {
            self.set_state(LOGIN, &ctx, out);
        }
    }

    /// A read/write failure on the active connection. Title/lobby sockets log out to the title;
    /// the game socket enters state 14 and retries the same world.
    pub fn try_reconnect(&mut self, ctx: &Context, out: &mut Vec<Effect>) {
        if is_lobby(self.state) || is_title(self.state) {
            self.logout(false, ctx, out);
        } else {
            self.set_state(RECONNECT, ctx, out);
        }
    }

    /// Terminal game-login reply while in state 14 or 19:
    /// anything other than -3 (in progress), 2
    /// (ok) or 15 (reconnect ok) abandons the connection.
    pub fn game_login_failed(
        &mut self,
        reply: i32,
        transfer_cancellable: bool,
        ctx: &Context,
        out: &mut Vec<Effect>,
    ) {
        // -3 is still in progress, 2 and 15 are success; only a game login in
        // states 14 and 19 has those. Any other login that ends with one of
        // them (a lobby answered 15) is over.
        if matches!(self.state, RECONNECT | TRANSFER) && matches!(reply, -3 | 2 | 15) {
            return;
        }
        match self.state {
            TRANSFER if transfer_cancellable => {
                out.push(Effect::RestorePreviousWorld);
                self.set_state(RECONNECT, ctx, out);
            }
            TRANSFER => self.logout(self.logged_in, ctx, out),
            RECONNECT => self.logout(false, ctx, out),
            _ => self.update_login_state(ctx, out),
        }
    }

    /// Gates login requests and entering the lobby from the title screen.
    #[must_use]
    pub fn login_ready(&self, login_in_progress: bool, creation_in_progress: bool) -> bool {
        self.state == LOGIN && !login_in_progress && !creation_in_progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs910_core::client_state::is_loading;

    /// Every client state against the predicate sets: loading {5,11,1},
    /// rebuild {10,6,3,16,8}, title {4,10,17,7,0,12,8}, lobby {13,6,15,16},
    /// game {18,3,9}, and login-ready: state 4 with no login or account
    /// creation in progress.
    #[test]
    fn client_state_predicates_hold() {
        let in_set = |set: &[i32], state| set.contains(&state);
        for state in 0..=20 {
            assert_eq!(
                is_loading(state),
                in_set(&[5, 11, 1], state),
                "loading {state}"
            );
            assert_eq!(
                is_rebuild(state),
                in_set(&[10, 6, 3, 16, 8], state),
                "rebuild {state}"
            );
            assert_eq!(
                is_title(state),
                in_set(&[4, 10, 17, 7, 0, 12, 8], state),
                "title {state}"
            );
            assert_eq!(
                is_lobby(state),
                in_set(&[13, 6, 15, 16], state),
                "lobby {state}"
            );
            assert_eq!(is_game(state), in_set(&[18, 3, 9], state), "game {state}");
            let m = Machine {
                state,
                logged_in: state == LOBBY,
                ..Machine::default()
            };
            assert_eq!(
                m.login_ready(false, false),
                state == 4,
                "login ready {state}"
            );
            assert!(!m.login_ready(true, false) && !m.login_ready(false, true));
        }
        assert_eq!(
            [LOADING, LOADING_SCREENS, LOADING_AUDIO, LOGIN, LOBBY, GAME],
            [5, 11, 1, 4, 13, 18]
        );
        // The client starts loading.
        assert_eq!(Machine::default().state, LOADING);
    }

    fn ctx(top: i32) -> Context {
        Context {
            top,
            login_interface: 744,
            lobby_interface: -1,
            credentials: true,
            lobby_connected: false,
        }
    }

    fn run(machine: &mut Machine, f: impl FnOnce(&mut Machine, &mut Vec<Effect>)) -> Vec<Effect> {
        let mut out = Vec::new();
        f(machine, &mut out);
        out
    }

    #[test]
    fn startup_title_opens_login_interface() {
        let mut m = Machine::default();
        let out = run(&mut m, |m, out| m.set_state(LOGIN, &ctx(-1), out));
        assert_eq!(out, vec![Effect::ShowLogin { reopen: true }]);
        assert_eq!(m.state, LOGIN);
        assert!(!m.logged_in);
    }

    #[test]
    fn lobby_login_round_trip_visits_the_expected_states() {
        let mut m = Machine {
            state: LOGIN,
            logged_in: false,
            ..Machine::default()
        };
        // lobby_enterlobby -> enter the lobby -> state 17.
        let out = run(&mut m, |m, out| m.set_state(LOBBY_LOGIN, &ctx(744), out));
        assert_eq!(out, vec![Effect::RequestLobbyLogin]);
        // Lobby reply 2 -> setState(13); lobby_interface -1 closes the title.
        let out = run(&mut m, |m, out| m.set_state(LOBBY, &ctx(744), out));
        assert_eq!(out, vec![Effect::ShowLobby { reopen: true }]);
        assert!(m.logged_in);
        // lobby_entergame -> setState(15) -> requestGameLogin.
        let out = run(&mut m, |m, out| {
            m.set_state(LOBBY_ENTER_GAME, &ctx(906), out)
        });
        assert_eq!(out, vec![Effect::RequestGameLogin { reconnect: false }]);
        m.set_state(GAME, &ctx(906), &mut Vec::new());
        // LOGOUT_FULL -> logout(loggedIn) -> state 9 lobby relogin.
        let out = run(&mut m, |m, out| m.logout(m.logged_in, &ctx(1477), out));
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::RequestLobbyLogin]
        );
        assert_eq!(m.state, LOBBY_RELOGIN);
        // The relogin reopens the lobby tree from state 9.
        let out = run(&mut m, |m, out| m.set_state(LOBBY, &ctx(1477), out));
        assert_eq!(out, vec![Effect::ShowLobby { reopen: true }]);
        // lobby_leavelobby -> logout(false) -> title.
        let out = run(&mut m, |m, out| m.logout(false, &ctx(906), out));
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::ShowLogin { reopen: true }]
        );
        assert_eq!(m.state, LOGIN);
        assert!(!m.logged_in);
        // A plain logout straight from the game also returns to the title,
        // even though the session came through the lobby.
        let mut m = Machine {
            state: GAME,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| m.logout(false, &ctx(1477), out));
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::ShowLogin { reopen: true }]
        );
        assert_eq!(m.state, LOGIN);
    }

    #[test]
    fn failed_logins_follow_update_login_state() {
        // A rejected lobby login returns to the still-open title screen.
        let mut m = Machine {
            state: LOBBY_LOGIN,
            logged_in: false,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| {
            m.game_login_failed(3, false, &ctx(744), out)
        });
        assert_eq!(out, vec![Effect::ShowLogin { reopen: false }]);
        assert_eq!(m.state, LOGIN);
        // A rejected lobby_entergame stays in the connected lobby.
        let mut m = Machine {
            state: LOBBY_ENTER_GAME,
            logged_in: true,
            ..Machine::default()
        };
        let connected = Context {
            lobby_connected: true,
            ..ctx(906)
        };
        let out = run(&mut m, |m, out| {
            m.game_login_failed(5, false, &connected, out)
        });
        assert_eq!(out, vec![Effect::ShowLobby { reopen: false }]);
        assert_eq!(m.state, LOBBY);
        // Empty credentials never leave the title.
        let mut m = Machine {
            state: LOGIN,
            logged_in: false,
            ..Machine::default()
        };
        let empty = Context {
            credentials: false,
            ..ctx(744)
        };
        let out = run(&mut m, |m, out| m.set_state(LOBBY_LOGIN, &empty, out));
        assert_eq!(out, vec![Effect::RejectEmptyCredentials { lobby: true }]);
        assert_eq!(m.state, LOGIN);
    }

    #[test]
    fn dropped_sockets_follow_try_reconnect() {
        // Game socket: state 14 retries the same world with the reconnect flag.
        let mut m = Machine {
            state: GAME,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| m.try_reconnect(&ctx(1477), out));
        assert_eq!(out, vec![Effect::RequestGameLogin { reconnect: true }]);
        assert_eq!(m.state, RECONNECT);
        // Retries exhausted (-4) -> logout(false) -> title.
        let out = run(&mut m, |m, out| {
            m.game_login_failed(-4, false, &ctx(1477), out)
        });
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::ShowLogin { reopen: true }]
        );
        // Lobby socket: straight back to the title.
        let mut m = Machine {
            state: LOBBY,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| m.try_reconnect(&ctx(906), out));
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::ShowLogin { reopen: true }]
        );
    }

    #[test]
    fn transfer_failure_restores_cancellable_world() {
        let mut m = Machine {
            state: GAME,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| m.set_state(TRANSFER, &ctx(1477), out));
        assert_eq!(out, vec![Effect::RequestGameLogin { reconnect: false }]);
        let out = run(&mut m, |m, out| {
            m.game_login_failed(5, true, &ctx(1477), out)
        });
        assert_eq!(
            out,
            vec![
                Effect::RestorePreviousWorld,
                Effect::RequestGameLogin { reconnect: true }
            ]
        );
        assert_eq!(m.state, RECONNECT);
        let mut m = Machine {
            state: TRANSFER,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| {
            m.game_login_failed(5, false, &ctx(1477), out)
        });
        assert_eq!(
            out,
            vec![Effect::CloseConnections, Effect::RequestLobbyLogin]
        );
    }

    /// A reconnect or transfer answered 2 or 15 is a success and stays put; a
    /// lobby login that ends with 15 (there is no world to resume) is over and
    /// returns to the title like any failed login.
    #[test]
    fn reply_15_is_a_success_only_for_a_game_reconnect() {
        for state in [RECONNECT, TRANSFER] {
            let mut m = Machine {
                state,
                logged_in: true,
                ..Machine::default()
            };
            let out = run(&mut m, |m, out| {
                m.game_login_failed(15, false, &ctx(1477), out)
            });
            assert!(out.is_empty());
            assert_eq!(m.state, state);
        }
        let mut m = Machine {
            state: LOBBY_LOGIN,
            logged_in: false,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| {
            m.game_login_failed(15, false, &ctx(744), out)
        });
        assert_eq!(out, vec![Effect::ShowLogin { reopen: false }]);
        assert_eq!(m.state, LOGIN);
    }

    /// Rebuild start/finish state round trips, and the rebuild completion
    /// for every rebuild state.
    #[test]
    fn rebuild_states_round_trip_like_world() {
        for (from, rebuild) in [(4, 10), (13, 6), (15, 16), (18, 3), (0, 8)] {
            assert_eq!(rebuild_state_for(from), Some(rebuild));
            assert!(is_rebuild(rebuild));
            assert_eq!(rebuilt_state(rebuild), from);
        }
        assert_eq!(rebuild_state_for(14), None);
        assert!(!is_rebuild(18) && !is_rebuild(4));
        // Rebuild states keep the title/lobby/game predicates.
        assert!(is_game(3) && is_title(10) && is_title(8) && is_lobby(6) && is_lobby(16));
        let mut m = Machine {
            state: GAME,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| m.set_state(REBUILD_GAME, &ctx(1477), out));
        assert_eq!(out, vec![Effect::CompleteRebuild]);
        let out = run(&mut m, |m, out| {
            m.set_state(rebuilt_state(m.state), &ctx(1477), out)
        });
        assert!(out.is_empty());
        assert_eq!(m.state, GAME);
        // 16 -> 15 does not re-request the game login.
        let mut m = Machine {
            state: REBUILD_LOBBY_ENTER_GAME,
            logged_in: true,
            ..Machine::default()
        };
        let out = run(&mut m, |m, out| {
            m.set_state(LOBBY_ENTER_GAME, &ctx(906), out)
        });
        assert!(out.is_empty());
    }

    /// The map build reports itself stuck once, on the 1000th cycle the same
    /// non-zero number of locs has kept it waiting; a change of the number or
    /// no wait at all starts the count again.
    #[test]
    fn map_build_is_stuck_after_a_thousand_unchanged_cycles() {
        let waiting = |count| LocsWaiting {
            count,
            loc: 38760,
            model: 4096,
        };
        let mut p = RebuildProgress::default();
        assert_eq!(p.watch_locs(waiting(5), 100), None, "the count appears");
        for cycle in 101..1100 {
            assert_eq!(p.watch_locs(waiting(5), cycle), None);
        }
        assert_eq!(p.watch_locs(waiting(5), 1100), Some(waiting(5)));
        assert_eq!(p.watch_locs(waiting(5), 1101), None, "reported once");
        // A change starts the thousand cycles again.
        assert_eq!(p.watch_locs(waiting(4), 1102), None);
        assert_eq!(p.watch_locs(waiting(4), 2101), None);
        assert_eq!(p.watch_locs(waiting(4), 2102), Some(waiting(4)));
        // Nothing waiting never reports.
        let mut p = RebuildProgress::default();
        for cycle in 1..3000 {
            assert_eq!(p.watch_locs(LocsWaiting::default(), cycle), None);
        }
    }

    /// Rebuild progress counters and the message-box text.
    #[test]
    fn rebuild_progress_matches_world_counters() {
        let mut p = RebuildProgress::default();
        assert_eq!(
            p.message("Loading - please wait."),
            "Loading - please wait."
        );
        // 9 squares pending, then 3: total keeps the maximum.
        assert!(!p.load_maps(9));
        assert_eq!(p.message("L"), "L<br>(0%)");
        assert!(!p.load_maps(3));
        assert_eq!(p.maps_percent(), 100 - 3 * 100 / 9);
        assert_eq!(p.message("L"), format!("L<br>({}%)", (100 - 300 / 9) / 2));
        assert!(p.load_maps(0));
        assert!(!p.load_locs(40));
        assert!(!p.load_locs(10));
        assert_eq!(
            p.message("L"),
            format!("L<br>({}%)", (100 - 1000 / 40) / 2 + 50)
        );
        assert!(p.load_locs(0));
        assert!(p.begin_build());
        assert_eq!(p.stage, RebuildStage::Build);
        assert_eq!(p.message("L"), "L");
        p.complete();
        assert_eq!(p, RebuildProgress::default());
        // No wait stage: no "(100%)" box.
        assert!(!p.begin_build());
    }
}
