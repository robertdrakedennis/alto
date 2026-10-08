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

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000_000;

    fn input(target: Option<u32>, transition: bool) -> SkyInput<u32> {
        SkyInput {
            target,
            ready: true,
            transition,
            duration_override_ms: None,
        }
    }

    /// The first environment shows at once; the next one fades over the default 5000 ms, `t`
    /// growing linearly and the old cube gone at the end.
    #[test]
    fn a_change_fades_over_the_default_time_after_an_instant_first_apply() {
        let mut sky = SkyState::default();
        let first = sky.update(input(Some(1), false), T0);
        assert_eq!(
            (first.previous, first.t, first.mixing),
            (Some(1), 0.0, true)
        );
        let start = sky.update(input(Some(2), true), T0 + 1000);
        assert_eq!((start.current, start.t), (Some(2), 0.0));
        let half = sky.update(input(Some(2), true), T0 + 1000 + FADE_MS / 2);
        assert!((half.t - 0.5).abs() < 1e-6);
        let done = sky.update(input(Some(2), true), T0 + 1000 + FADE_MS);
        assert_eq!((done.current, done.t, done.mixing), (Some(2), 1.0, false));
    }

    /// An override's explicit duration replaces the default; no duration at all installs at once.
    #[test]
    fn the_override_sets_the_duration_and_a_zero_duration_is_instant() {
        let mut sky = SkyState::default();
        sky.update(input(Some(1), false), T0);
        sky.update(input(Some(2), true), T0);
        let mut ask = input(Some(3), true);
        ask.duration_override_ms = Some(1000);
        sky.update(ask, T0 + 10_000);
        let half = sky.update(ask, T0 + 10_500);
        assert!((half.t - 0.5).abs() < 1e-6, "{}", half.t);
        let done = sky.update(ask, T0 + 11_000);
        assert!(!done.mixing);
        // Instant: a zero duration shows the new cube with nothing fading.
        ask.duration_override_ms = Some(0);
        ask.target = Some(4);
        let at_once = sky.update(ask, T0 + 12_000);
        assert_eq!(
            (at_once.previous, at_once.current, at_once.t, at_once.mixing),
            (Some(4), None, 0.0, true)
        );
    }

    /// Going back to the previous cube swaps the roles and back-dates the clock, so the share of
    /// each cube in the picture is the same the instant before and after.
    #[test]
    fn going_back_does_not_jump() {
        let mut sky = SkyState::default();
        sky.update(input(Some(1), false), T0);
        sky.update(input(Some(2), true), T0);
        // Finish 1 -> 2, then fade 2 -> 3 a quarter of the way.
        sky.update(input(Some(3), true), T0 + 6000);
        let before = sky.update(input(Some(3), true), T0 + 6000 + FADE_MS / 4);
        assert_eq!((before.previous, before.current), (Some(2), Some(3)));
        let share_of_3_before = before.weight(3);
        let after = sky.update(input(Some(2), true), T0 + 6000 + FADE_MS / 4);
        assert_eq!((after.previous, after.current), (Some(3), Some(2)));
        assert!((after.weight(3) - share_of_3_before).abs() < 1e-5);
        assert!((after.weight(2) - before.weight(2)).abs() < 1e-5);
        // ... and it runs on to the cube it went back to.
        let done = sky.update(input(Some(2), true), T0 + 6000 + FADE_MS);
        assert_eq!((done.current, done.mixing), (Some(2), false));
    }

    /// Going back after a finished fade starts the new fade at 0; with no fade ever running it
    /// is finished at once.
    #[test]
    fn going_back_from_a_finished_fade_and_from_none() {
        let mut fade = SkyFade::default();
        fade.apply(Some(1), T0, FADE_MS);
        fade.apply(Some(2), T0 + 10_000, FADE_MS);
        assert!(!fade.blend(T0 + 20_000).mixing);
        fade.apply(Some(1), T0 + 20_000, FADE_MS);
        let b = fade.blend(T0 + 20_000);
        assert_eq!((b.previous, b.current, b.t), (Some(2), Some(1), 0.0));
        // No fade running (an instant apply): the old picture is kept, finished at once.
        let mut instant = SkyFade::default();
        instant.apply(Some(1), T0, 0);
        instant.apply(Some(1), T0 + 1, FADE_MS);
        let b = instant.blend(T0 + 1);
        assert_eq!(
            (b.previous, b.current, b.t, b.mixing),
            (None, Some(1), 1.0, false)
        );
    }

    /// Decor models of the cubes fade with the same clock: the old ones out, the new ones in,
    /// and only the current cube's when nothing fades.
    #[test]
    fn decor_weights_follow_the_clock() {
        let mut sky = SkyState::default();
        sky.update(input(Some(1), false), T0);
        let rest = sky.update(input(Some(1), false), T0 + 1);
        assert_eq!((rest.weight(1), rest.weight(2)), (1.0, 0.0));
        sky.update(input(Some(2), true), T0 + 100);
        let mid = sky.update(input(Some(2), true), T0 + 100 + FADE_MS / 2);
        // After an instant apply the old cube is not in the state's `previous`: only the new one
        // fades in.
        assert!((mid.weight(2) - 0.5).abs() < 1e-6);
        assert_eq!(mid.weight(9), 0.0);
        let end = sky.update(input(Some(2), true), T0 + 100 + FADE_MS);
        assert_eq!((end.weight(2), end.weight(1)), (1.0, 0.0));
    }

    /// A cube that is not ready waits: the old one stays until it is, and the wait starts the
    /// fade then.
    #[test]
    fn a_change_to_a_cube_that_is_not_ready_waits() {
        let mut sky = SkyState::default();
        sky.update(input(Some(1), false), T0);
        let mut waiting = input(Some(2), true);
        waiting.ready = false;
        let b = sky.update(waiting, T0 + 1000);
        assert_eq!((b.previous, b.current), (Some(1), None));
        waiting.ready = true;
        let b = sky.update(waiting, T0 + 3000);
        assert_eq!((b.current, b.t), (Some(2), 0.0));
        let b = sky.update(waiting, T0 + 3000 + FADE_MS / 2);
        assert!((b.t - 0.5).abs() < 1e-6);
    }
}
