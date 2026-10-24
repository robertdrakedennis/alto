//! The client state the UI works on: the rs910-game [`Game`] (entities,
//! packet appliers, cutscene, follow camera) and, beside it, the UI variable
//! state [`crate::ui_vars::State`] (client variables and their persistence,
//! preferences, stats, social, the transmit rings). `ui_variables` lives
//! outside `Game` so the game layer names no UI type; the app owns the two
//! side by side here.
//!
//! `ClientGame` dereferences to [`Game`], so `game.runtime`, `game.cutscene`
//! and the `Game` methods keep compiling at every call site. Code that
//! borrows `ui_variables` and a `Game` field at the same time names the half
//! (`game.game.runtime`), because a `Deref` borrow covers the whole value.
//! Phase 4 replaces the `Deref` with phases that borrow the halves
//! disjointly. [`with_game`]/[`with_game_clock`] (moved from `ui_runtime`)
//! join the halves into the CS2 host's `Variables` view for one call, so the
//! UI runtime itself names neither.
//!
//! The one UI value the game layer reads is the `textures` preference (it
//! decides whether NPC_INFO and cutscene NPCs randomise their recolours). The
//! methods below read it and pass it to the `Game` call that needs it. Only
//! game-layer code runs between this entry and the point that uses it, and it
//! cannot reach the preferences, so the value is the same.
use crate::{
    cache::Pack,
    game_runtime::Game,
    protocol910::{
        self,
        live::{Applied, Feed},
    },
    ui_vars::Variables,
};
use anyhow::Result;

pub struct ClientGame {
    /// Declared first: fields drop in order, so the UI state drops before
    /// the game it observed.
    pub ui_variables: crate::ui_vars::State,
    pub game: Game,
}

impl std::ops::Deref for ClientGame {
    type Target = Game;
    fn deref(&self) -> &Game {
        &self.game
    }
}

impl std::ops::DerefMut for ClientGame {
    fn deref_mut(&mut self) -> &mut Game {
        &mut self.game
    }
}

impl ClientGame {
    /// [`Game::login`], then the UI variable state it used to build last.
    pub fn login(
        pack: &Pack,
        local: usize,
        feed: Feed,
        seed: u64,
        logged_in_members: bool,
    ) -> anyhow::Result<Self> {
        let configs = rs910_config::login_configs::LoginConfigs::read(pack);
        Self::login_with(pack, &configs, local, feed, seed, logged_in_members)
    }

    /// [`Self::login`] decoding the config archives `configs` has read, so
    /// the interface engine can decode its stores from the same read
    /// ([`crate::ui_runtime::CacheConfigs::decode_with`]).
    pub fn login_with(
        pack: &Pack,
        configs: &rs910_config::login_configs::LoginConfigs,
        local: usize,
        feed: Feed,
        seed: u64,
        logged_in_members: bool,
    ) -> anyhow::Result<Self> {
        let game = Game::login_with(pack, configs, local, feed, seed, logged_in_members)?;
        // The fresh skill stats are installed before the first packets are
        // read.
        let mut ui_variables = crate::ui_vars::State::with_client(pack)?;
        let stats = ui_variables.stats.as_mut().unwrap();
        stats.logged_in_members = logged_in_members;
        stats.reset_session()?;
        Ok(Self { ui_variables, game })
    }

    /// The `textures` preference, which `Game::apply_with` and
    /// `Game::cutscene_move_to_facing` need.
    fn textures(&self) -> bool {
        self.ui_variables
            .queries
            .preferences
            .options
            .get("textures")
            == Some(1)
    }

    /// [`Game::apply_next`] with the current `textures` preference.
    pub fn apply_next(&mut self, now_ms: i64) -> Result<Option<Applied>, protocol910::Error> {
        let textures = self.textures();
        self.game.apply_next(now_ms, textures)
    }

    /// [`Game::apply_with_random`] with the current `textures` preference.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn apply_with_random(
        &mut self,
        now_ms: i64,
        random: &[[f64; 4]],
    ) -> Result<Option<Applied>, protocol910::Error> {
        let textures = self.textures();
        self.game.apply_with_random(now_ms, random, textures)
    }

    /// [`Game::update_scene_state`] with the current `textures` preference.
    pub fn update_scene_state(&mut self, pack: &Pack) -> anyhow::Result<bool> {
        let textures = self.textures();
        self.game.update_scene_state(pack, textures)
    }
}

/// No copied varps, client variables, delayed queue, or variable definitions.
pub fn with_game<T>(
    game: &mut crate::client_game::ClientGame,
    f: impl FnOnce(&mut Variables<'_>) -> Result<T>,
) -> Result<T> {
    with_game_probed(game, None, f)
}

/// [`with_game`] with the active renderer lent to the profiling commands for
/// the call (`Variables::probe`).
pub fn with_game_probed<T>(
    game: &mut crate::client_game::ClientGame,
    probe: Option<&mut dyn rs910_toolkit::performance_metric::RendererProbe>,
    f: impl FnOnce(&mut Variables<'_>) -> Result<T>,
) -> Result<T> {
    with_game_clock_probed(game, &mut crate::logic_clock::monotonic_millis, probe, f)
}

/// [`with_game`] with an explicit monotonic clock. Replays that run logic
/// cycles faster than the 20 ms client tick pass a clock that advances 20 ms
/// per cycle, so delayed state-change deadlines (500 ms) see the same
/// elapsed time a live client would.
pub fn with_game_clock<T>(
    game: &mut crate::client_game::ClientGame,
    now: &mut dyn FnMut() -> i64,
    f: impl FnOnce(&mut Variables<'_>) -> Result<T>,
) -> Result<T> {
    with_game_clock_probed(game, now, None, f)
}

/// [`with_game_clock`] with the active renderer lent to the profiling
/// commands for the call.
pub fn with_game_clock_probed<T>(
    game: &mut crate::client_game::ClientGame,
    now: &mut dyn FnMut() -> i64,
    probe: Option<&mut dyn rs910_toolkit::performance_metric::RendererProbe>,
    f: impl FnOnce(&mut Variables<'_>) -> Result<T>,
) -> Result<T> {
    let mut scene = crate::ui_cam2::SceneInput::new_with_objects(
        &game.game.runtime.map,
        &game.game.runtime.feed.state.players,
        Some(&game.game.runtime.feed.state.zones.objects),
        game.game.runtime.terrain.as_ref(),
        game.game.runtime.terrain_generation,
    );
    scene.npcs = Some(&game.game.runtime.feed.state.npcs);
    scene.locations = Some(&game.game.scene_locs);
    f(&mut Variables {
        cycle: game.game.cycle,
        definitions: &game.game.inputs.bits,
        state: &mut game.ui_variables,
        player: game.game.runtime.feed.state.varps.as_mut(),
        active_player: None,
        active_npc: None,
        now,
        // (Reborrowed so the trait object's bound shortens with the borrow.)
        probe: probe.map(|p| -> &mut dyn rs910_toolkit::performance_metric::RendererProbe { p }),
        varp_transmit: crate::ui_loop::Counter {
            num: game.game.runtime.varp_transmit_num,
            ids: game.game.runtime.varp_transmitted,
        },
        scene,
    })
}
