//! The sky's cross-fade between cubes: which cube shows, which cube it came from and how far the
//! change has gone.
//!
//! # The rules (the modern client's)
//!
//! The state is a `previous` and a `current` cube (each may be absent) and a start and end time.
//! When the environment's cube changes to `new` at time `now` with a fade of `duration`:
//!
//! - `new` is the current cube: nothing changes.
//! - `duration` 0: `previous` becomes `new`, `current` becomes empty and `start = end`: the new
//!   cube shows at once.
//! - `new` is the previous cube (going back): the two swap roles and the clock is back-dated so
//!   the picture does not jump: `start = now - duration * f`, `end = start + duration`, with
//!   `f = 1` when no fade was running, `1 - t` in the middle of one and 0 when it had finished.
//! - Otherwise `previous` becomes `current`, `current` becomes `new`, `start = now`,
//!   `end = now + duration`.
//!
//! The blend fraction is `t = (now - start) / (end - start)`, 1 from `end` on. With `start ==
//! end` it is 0 (only the previous cube shows, or the current one when there is no previous).
//! While `t < 1` the sky is `mix(previous, current, t)`, a missing side taking the other one;
//! from `t = 1` on it is the current cube alone.
//!
//! The fade lasts 5000 ms ([`FADE_MS`]) unless the environment override asks for a duration, and
//! 0 for the first environment and for one installed without a transition (a teleport). One quirk
//! of the rules is kept: after a zero-duration apply `current` is empty, so the next ordinary
//! change fades from an empty `previous` and shows the new cube at once.
//!
//! The decor models the sky carries fade with the same clock ([`Blend::weight`]).

/// The default cross-fade time.
pub const FADE_MS: i64 = 5000;

/// The state of the sky's cubes, generic over how a cube is named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyFade<K> {
    previous: Option<K>,
    current: Option<K>,
    start_ms: i64,
    end_ms: i64,
}

impl<K> Default for SkyFade<K> {
    fn default() -> Self {
        Self {
            previous: None,
            current: None,
            start_ms: 0,
            end_ms: 0,
        }
    }
}

/// What the sky shows at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend<K> {
    pub previous: Option<K>,
    pub current: Option<K>,
    /// The share of `current` in the picture (0 when nothing fades).
    pub t: f32,
    /// The blend of the two cubes draws (`t < 1`); false: the current cube alone.
    pub mixing: bool,
}

impl<K: Copy + PartialEq> SkyFade<K> {
    /// The environment's cube changed to `new` (`None`: no sky) at `now_ms`, to be reached over
    /// `duration_ms`.
    pub fn apply(&mut self, new: Option<K>, now_ms: i64, duration_ms: i64) {
        if new == self.current {
            return;
        }
        if duration_ms == 0 {
            self.previous = new;
            self.current = None;
            self.start_ms = now_ms;
            self.end_ms = now_ms;
            return;
        }
        let back = new.is_some() && new == self.previous;
        let fraction = if !back {
            0.0
        } else if self.start_ms == self.end_ms {
            1.0
        } else if now_ms >= self.end_ms {
            0.0
        } else {
            1.0 - (now_ms - self.start_ms) as f32 / (self.end_ms - self.start_ms) as f32
        };
        self.previous = self.current;
        self.current = new;
        self.start_ms = now_ms - (duration_ms as f32 * fraction) as i64;
        self.end_ms = self.start_ms + duration_ms;
    }

    /// The picture at `now_ms`.
    #[must_use]
    pub fn blend(&self, now_ms: i64) -> Blend<K> {
        let (t, mixing) = if self.start_ms == self.end_ms {
            (0.0, true)
        } else if now_ms >= self.end_ms {
            (1.0, false)
        } else {
            let t = (now_ms - self.start_ms) as f32 / (self.end_ms - self.start_ms) as f32;
            (t.clamp(0.0, 1.0), true)
        };
        Blend {
            previous: self.previous,
            current: self.current,
            t,
            mixing,
        }
    }
}

impl<K: Copy + PartialEq> Blend<K> {
    /// How much of the decor models of `cube` show: the current cube's grow with `t`, the
    /// previous cube's shrink, anything else's are gone.
    #[must_use]
    pub fn weight(&self, cube: K) -> f32 {
        if !self.mixing {
            return f32::from(self.current == Some(cube));
        }
        let mut w = 0.0;
        if self.previous == Some(cube) {
            w += 1.0 - self.t;
        }
        if self.current == Some(cube) {
            w += self.t;
        }
        w
    }
}

/// The sky's cube follows the environment: the fade plus what has been applied to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyState<K> {
    fade: SkyFade<K>,
    /// The last environment cube the fade took (`None`: none yet).
    applied: Option<Option<K>>,
}

impl<K> Default for SkyState<K> {
    fn default() -> Self {
        Self {
            fade: SkyFade::default(),
            applied: None,
        }
    }
}

/// What one frame tells the sky about its environment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyInput<K> {
    /// The environment's cube (`None`: no sky).
    pub target: Option<K>,
    /// The cube can be drawn. A change to a cube that is not ready waits.
    pub ready: bool,
    /// The environment reached the cube through a transition (not installed at once).
    pub transition: bool,
    /// The environment override's fade duration in milliseconds.
    pub duration_override_ms: Option<u32>,
}

impl<K: Copy + PartialEq> SkyState<K> {
    /// This frame's picture: the fade takes a changed environment cube (the first one, and any
    /// installed without a transition, at once) and is read at `now_ms`.
    pub fn update(&mut self, input: SkyInput<K>, now_ms: i64) -> Blend<K> {
        let changed = self.applied != Some(input.target);
        if changed && (input.target.is_none() || input.ready) {
            let duration = if self.applied.is_none() || !input.transition {
                0
            } else {
                input.duration_override_ms.map_or(FADE_MS, i64::from)
            };
            self.fade.apply(input.target, now_ms, duration);
            self.applied = Some(input.target);
        }
        self.fade.blend(now_ms)
    }
}
