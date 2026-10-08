//! The environment manager's CPU half in the client core (lane E-A1): the
//! environment map, the server override, the fade between environments and
//! the local player's teleport count that resets it. The partial update runs
//! every logic cycle of state 18 **and** in the scene draw, so the core
//! calls [`EnvironmentManager::update_partial`] at P9
//! (`update_session_logic`) and the shell's R3 calls it again through
//! [`ClientCore::environment_frame`]. The GPU half (the skybox owner, the
//! colour remappers, the frame's sun/fog uniforms) stays the shell's; the
//! skybox cross-fade a fade starts reaches it as a [`SkyFade`].
use super::*;
use rs910_scene::env::{EnvState, Environment, SkyboxRef, CHUNK_FADE_MS, DEFAULT_SUN_DIR};
use std::time::{Duration, Instant};

/// `EnvironmentManager`: the map, the override,
/// the fade and the teleport count.
#[derive(Default)]
pub struct EnvironmentManager {
    /// Environment map of the window,
    /// installed with each scene build.
    pub env: Option<EnvState>,
    /// Current environment override, if one was installed by the server.
    pub override_event: Option<crate::session::EnvironmentOverrideEvent>,
    /// The fade between the current environment and the fade target.
    pub fade: Option<EnvironmentFade>,
    /// Set by a fade reset: the first override after a world
    /// rebuild is installed without an inherited transition.
    pub fade_reset: bool,
    /// The fade target (with its sun direction, which environment equality
    /// ignores): the last target a fade took.
    pub fade_target: Option<(Environment, [f32; 3])>,
    /// Last seen local-player `ActorState::teleports`.
    pub local_teleports_seen: Option<u32>,
    /// The skybox cross-fades started since the shell's skybox owner last
    /// took them ([`EnvironmentManager::take_sky_fades`]), in order.
    pub sky_fades: Vec<SkyFade>,
}

/// The skybox half of a fade: the
/// current skybox starts its cross-fade towards the target's. Applied by
/// the shell's skybox owner at its next R3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyFade {
    /// The current environment's skybox.
    pub from: Option<SkyboxRef>,
    /// The target's skybox.
    pub to: Option<SkyboxRef>,
}

#[derive(Clone)]
pub struct EnvironmentFade {
    pub from: Environment,
    pub to: Environment,
    pub from_sun_direction: [f32; 3],
    pub to_sun_direction: [f32; 3],
    pub started: Instant,
    pub duration: Duration,
}

impl EnvironmentFade {
    /// Elapsed fraction of the fade: `(duration - remaining) / duration`.
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32()).max(0.0)
    }

    pub fn sample(&self, now: Instant) -> (Environment, [f32; 3], bool) {
        if self.duration.is_zero() {
            return (self.to.clone(), self.to_sun_direction, true);
        }
        let t = now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1.0 {
            return (self.to.clone(), self.to_sun_direction, true);
        }
        let t = t.max(0.0);
        let mut env = self.from.clone();
        env.sun_colour = interpolate_rgb(self.from.sun_colour, self.to.sun_colour, t);
        env.sun_ambient = lerp(self.from.sun_ambient, self.to.sun_ambient, t);
        env.sun_diffuse = lerp(self.from.sun_diffuse, self.to.sun_diffuse, t);
        env.sun_shadow = lerp(self.from.sun_shadow, self.to.sun_shadow, t);
        env.fog_colour = interpolate_rgb(self.from.fog_colour, self.to.fog_colour, t);
        env.fog_depth = lerp_i32(self.from.fog_depth, self.to.fog_depth, t);
        env.sun_dir = [
            lerp(self.from_sun_direction[0], self.to_sun_direction[0], t),
            lerp(self.from_sun_direction[1], self.to_sun_direction[1], t),
            lerp(self.from_sun_direction[2], self.to_sun_direction[2], t),
        ];
        let sun_direction = [
            lerp(self.from_sun_direction[0], self.to_sun_direction[0], t),
            lerp(self.from_sun_direction[1], self.to_sun_direction[1], t),
            lerp(self.from_sun_direction[2], self.to_sun_direction[2], t),
        ];
        // Environment.setToInterpolation: bloom, levels and colour
        // remapping.
        rs910_scene::env::interpolate(&mut env, &self.from, &self.to, t);
        (env, sun_direction, false)
    }
}

pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

pub fn lerp_i32(from: i32, to: i32, t: f32) -> i32 {
    (from as f32 + (to - from) as f32 * t) as i32
}

pub fn interpolate_rgb(from: i32, to: i32, t: f32) -> i32 {
    let channel = |shift: u32| {
        let a = ((from >> shift) & 0xff) as f32;
        let b = ((to >> shift) & 0xff) as f32;
        (a + (b - a) * t).round() as i32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

impl EnvironmentManager {
    /// The target environment for the window tile `tile` (see
    /// [`ClientCore::environment_tile`]), with the map's sun direction.
    pub fn base_environment(&self, tile: [i32; 2]) -> (Environment, [f32; 3]) {
        match &self.env {
            Some(state) => (
                state.target_environment(tile[0], tile[1]),
                state.sun_direction,
            ),
            None => (Environment::default(), DEFAULT_SUN_DIR),
        }
    }

    /// The current environment: the running fade's sample, else the
    /// last target.
    pub fn current_environment(&self, tile: [i32; 2]) -> (Environment, [f32; 3]) {
        if let Some(fade) = self.fade.as_ref() {
            let (sample, direction, _) = fade.sample(crate::logic_clock::now());
            return (sample, direction);
        }
        self.fade_target
            .clone()
            .unwrap_or_else(|| self.base_environment(tile))
    }

    /// Fade to a target environment: a target that is
    /// not equal (ignoring the sun direction) to the fade's end environment starts a fade from the current
    /// environment (instant after `resetFade` or for a zero duration), and
    /// the current skybox starts its cross-fade towards the target's
    /// ([`SkyFade`]).
    pub fn fade_environment(
        &mut self,
        to: Environment,
        to_sun_direction: [f32; 3],
        duration_ms: u32,
        tile: [i32; 2],
    ) {
        let mut duration_ms = duration_ms;
        if self.fade_reset {
            self.fade_reset = false;
            duration_ms = 0;
        }
        if self
            .fade_target
            .as_ref()
            .is_some_and(|(b, _)| b.equal_ignoring_sun_direction(&to))
        {
            return;
        }
        let (from, from_sun_direction) = self.current_environment(tile);
        self.fade_target = Some((to.clone(), to_sun_direction));
        if duration_ms == 0 {
            // A zero duration installs the target at once.
            self.fade = None;
            return;
        }
        self.sky_fades.push(SkyFade {
            from: from.skybox,
            to: to.skybox,
        });
        self.fade = Some(EnvironmentFade {
            from,
            to,
            from_sun_direction,
            to_sun_direction,
            started: crate::logic_clock::now(),
            duration: Duration::from_millis(u64::from(duration_ms)),
        });
    }

    /// `ENVIRONMENT_OVERRIDE` -> set the override:
    /// install (or clear) the override, then fade to the overridden target
    /// over the packet's duration.
    pub fn set_override(
        &mut self,
        update: crate::session::EnvironmentOverrideEvent,
        tile: [i32; 2],
    ) {
        // Setting the override: the new target
        // fades in over the packet's duration.
        let (mut to, mut to_sun_direction) = self.base_environment(tile);
        apply_override_values(&update, &mut to, &mut to_sun_direction);
        let duration = u32::from(update.duration_ms);
        self.override_event = Some(update);
        self.fade_environment(to, to_sun_direction, duration, tile);
    }

    /// The partial update's CPU half for the target tile `tile`: the local
    /// player's teleport (`teleports`) resets the fade first, then the tile's
    /// chunk environment (with the override applied) fades in over the chunk
    /// fade time or the override's duration, and the fade samples it. Returns
    /// the current environment and its sun direction.
    pub fn update_partial(
        &mut self,
        tile: [i32; 2],
        teleports: Option<u32>,
    ) -> (Environment, [f32; 3]) {
        if let Some(teleports) = teleports {
            if self
                .local_teleports_seen
                .is_some_and(|seen| seen != teleports)
            {
                self.fade_reset = true;
            }
            self.local_teleports_seen = Some(teleports);
        }
        let (mut target, mut target_sun) = self.base_environment(tile);
        let mut duration = CHUNK_FADE_MS;
        if let Some(override_state) = self.override_event.as_ref().filter(|o| !o.clear) {
            apply_override_values(override_state, &mut target, &mut target_sun);
            duration = u32::from(override_state.duration_ms);
        }
        self.fade_environment(target, target_sun, duration, tile);
        // updateFade.
        let (env, sun_direction) = self.current_environment(tile);
        if self
            .fade
            .as_ref()
            .is_some_and(|fade| fade.sample(crate::logic_clock::now()).2)
        {
            self.fade = None;
        }
        (env, sun_direction)
    }

    /// The skybox cross-fades started since the last call (the shell's
    /// skybox owner applies them in order at R3).
    pub fn take_sky_fades(&mut self) -> Vec<SkyFade> {
        std::mem::take(&mut self.sky_fades)
    }
}

pub fn apply_override_values(
    override_state: &crate::session::EnvironmentOverrideEvent,
    env: &mut Environment,
    sun_direction: &mut [f32; 3],
) {
    if override_state.clear {
        return;
    }
    if let Some(value) = override_state.sun_colour {
        env.sun_colour = value;
    }
    if let Some(value) = override_state.sun_ambient {
        env.sun_ambient = value;
    }
    if let Some(value) = override_state.sun_diffuse {
        env.sun_diffuse = value;
    }
    if let Some(value) = override_state.sun_shadow {
        env.sun_shadow = value;
    }
    if let Some(value) = override_state.sun_dir {
        *sun_direction = value;
    }
    if let Some(value) = override_state.fog_colour {
        env.fog_colour = value;
    }
    if let Some(value) = override_state.fog_depth {
        env.fog_depth = value;
    }
    // Environment.applyOverride.
    if let Some(value) = override_state.bloom_intensity {
        env.bloom[1] = value;
    }
    if let Some(value) = override_state.bloom_threshold {
        env.bloom[2] = value;
    }
    if let Some(value) = override_state.bloom_white_point_sq {
        env.bloom[0] = value;
    }
    //: `createEnvironmentSampler(getSampler())`.
    if let Some(value) = override_state.sampler {
        env.sampler = value;
    }
    // Environment.applyOverride: `createSkybox(...)`.
    if let Some(value) = override_state.skybox {
        env.skybox = Some(value);
    }
    // Each overridden colour remapping slot.
    for (slot, value) in override_state.colour_remap.iter().enumerate() {
        if let Some(value) = value {
            env.colour_remap[slot] = *value;
        }
    }
}

/// The environment target tile outside the title and lobby states: the local
/// player's first route waypoint (window tiles; [`EnvState::target_environment`]
/// takes `>> 3`), or an out-of-range tile (the map's centre chunk) without
/// one.
pub fn local_player_tile(players: &crate::protocol910::Players, local: usize) -> [i32; 2] {
    players
        .players
        .get(local)
        .and_then(Option::as_ref)
        .map_or([-8, -8], |p| [p.x[0], p.z[0]])
}

/// P9 (`updateGame`): the partial environment update for the
/// local player of `game` (state 18; the logic's actors).
pub fn update_game_environment(
    environment: &mut EnvironmentManager,
    game: &crate::client_game::ClientGame,
) {
    let state = &game.runtime.feed.state;
    let local = game.runtime.map.local;
    let tile = local_player_tile(&state.players, local);
    let teleports = state
        .players
        .players
        .get(local)
        .and_then(Option::as_ref)
        .map(|p| p.actor.teleports);
    environment.update_partial(tile, teleports);
}

impl ClientCore {
    /// The environment target tile:
    /// in the title and lobby states (and offline, without a session) the
    /// camera's (`camera_tile`, window tiles), otherwise the local player's
    /// route tile ([`local_player_tile`]; the logic's actors while a
    /// cutscene's are drawn).
    pub fn environment_tile(&self, camera_tile: [i32; 2]) -> [i32; 2] {
        let Some(session) = self.session.as_ref() else {
            return camera_tile;
        };
        let state = session.machine.state;
        if crate::login_state::is_title(state) || crate::login_state::is_lobby(state) {
            return camera_tile;
        }
        let Some(game) = session.game.as_ref() else {
            return [-8, -8];
        };
        let players = if self.drawing_cutscene {
            &game.cutscene.players
        } else {
            &game.runtime.feed.state.players
        };
        local_player_tile(players, game.runtime.map.local)
    }

    /// R3's core half: the partial environment update from the scene draw
    /// with [`ClientCore::environment_tile`]; returns
    /// the current environment and its sun direction for the shell's frame uniforms.
    pub fn environment_frame(&mut self, camera_tile: [i32; 2]) -> (Environment, [f32; 3]) {
        let tile = self.environment_tile(camera_tile);
        let teleports = self.session.as_ref().and_then(|s| {
            let game = s.game.as_ref()?;
            let players = if self.drawing_cutscene {
                &game.cutscene.players
            } else {
                &game.runtime.feed.state.players
            };
            players
                .players
                .get(game.runtime.map.local)?
                .as_ref()
                .map(|p| p.actor.teleports)
        });
        self.environment.update_partial(tile, teleports)
    }
}
