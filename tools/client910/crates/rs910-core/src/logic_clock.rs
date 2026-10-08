//! The client's logic timer with explicit monotonic samples and a nonblocking sleep.
//! Presentation may run between begin/finish; it must not sample the timer again.
//!
//! # The injected clock (programme §6/§8, Phase 0 clock injection)
//!
//! This module is the client's one time source. Every logic and
//! render-animation read goes through it:
//!
//! - [`monotonic_millis`] — the client's monotonic time:
//!   wall-clock milliseconds with backwards jumps compensated.
//! - [`now`] — an [`Instant`] for the Rust owners that measure elapsed
//!   durations (`Instant::now()` in live mode, the same value as before).
//! - [`system_time`] — a [`SystemTime`] for clock-derived seeds
//!   (`SystemTime::now()` in live mode).
//! - [`Clock`] — the app-owned nanosecond timer
//!   that schedules the 20 ms logic cycles; [`Clock::poll`] is the one place
//!   the fixed clock advances (top of `ViewerApp::about_to_wait`, before the
//!   logic cycle).
//!
//! Live mode (the default) reads the operating-system clocks exactly as the
//! call sites did before routing: same call, same moment, same units and
//! truncation. `--fixed-clock <start_ms>[:step_ms]` ([`install_fixed`])
//! replaces them with a clock that starts at `start_ms` and advances by
//! `step_ms` (default 20, one logic interval) per logic cycle: each
//! `about_to_wait` runs exactly one logic cycle, and every read in between
//! (logic and the redraw after it) sees that cycle's time. Tests fix the
//! clock per thread with [`set_test_now`] (the headless session replays hand
//! out each recorded cycle's time); it drives all three reads above.
//!
//! In fixed mode the three reads agree: `monotonic_millis() == t`,
//! `now() == base + t ms` and `system_time() == UNIX_EPOCH + t ms`, so any
//! difference between two samples is the fixed-clock difference.
//!
//! ## Inventory of wall-clock reads in client910 (2026-09-26)
//!
//! native910 reads no clock outside its tests. Classes: **L** logic (must use
//! this clock), **R** render animation (reads this clock at the redraw; never
//! advances it), **IO** socket/worker/device timeouts and pacing (real time,
//! intentionally), **D** diagnostics/profiling (real time, never feeds state).
//! Test-only reads (`#[cfg(test)]` modules, `*_tests.rs`, replays, benches)
//! are not listed; they may use real time for their own watchdogs.
//!
//! | Site | Class | Source |
//! |---|---|---|
//! | `app.rs` `about_to_wait` `logic_clock.poll()` | L | [`Clock::poll`] |
//! | `app.rs` input/key/focus timestamps, `poll_vars`, `apply_next`, rebuild timer, `MAP_BUILD_COMPLETE`, lobby membership, `session_record::cycle` | L | [`monotonic_millis`] |
//! | `app.rs` `record_redraw` (fps ring) | L | [`monotonic_millis`] |
//! | `app.rs` `EnvironmentFade::{started, sample, progress}` | R | [`now`] |
//! | `app.rs` follow camera `camera_now` | R | [`Clock::sample`] |
//! | `app.rs` `about_to_wait` `last_frame`/`dt` (free-camera keys) | R | [`now`] |
//! | `app_loading.rs` loading timers | L | [`monotonic_millis`] |
//! | `ui_runtime.rs`, `ui_social.rs`, `ui_chat.rs`, `ui_host_builtins.rs` (`date_*`), `ui_icons.rs`, `ui_preferences_{autosetup,metric}.rs` | L | [`monotonic_millis`] |
//! | `ui_backend.rs` HTTP image `?a=` cache-buster | L | [`monotonic_millis`] |
//! | `ui_cam2.rs` random seed | L | [`system_time`] |
//! | `js5net.rs` `process`/`connect`/HTTP backoff | L | [`monotonic_millis`] |
//! | `js5net.rs` `error` xorcode | L | [`system_time`] |
//! | `audio_runtime.rs` `AudioApi::update` time | L | [`monotonic_millis`] |
//! | `world_map.rs` `decode` 5 ms slice | L | [`now`] |
//! | `live_scene.rs` `animation_cycle` / dynamic-loc seed (offline scene) | R | [`now`] / [`system_time`] |
//! | `floor_render.rs` `begin_material_frame` (water/waterfall/scroll uniforms, skybox) | R | [`now`] |
//! | `net.rs` `LoginClock`, `session.rs` drain deadline, `app.rs` startup JS5 wait | IO | real `Instant` |
//! | `audio_backend.rs` `ClockLine`, device thread sleeps | IO | real `Instant` |
//! | `graphics_runtime.rs` `finish_frame` | IO | `thread::sleep` |
//! | `app.rs` `last_title`/`last_render` (window-title fps), `update_particles` timing, `app_loading.rs` blocked-stage log, `rebuild.rs` `elapsed_ms`, `ui_hook_host.rs` hook trace, `frame_profile.rs`, `render.rs` profile, the engine profiler (`profile.rs`) | D | real `Instant` |
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One logic update per 20,000,000 nanoseconds.
pub const INTERVAL_NS: i64 = 20_000_000;
pub const RATE: i32 = (1_000_000_000 / INTERVAL_NS) as i32;

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    /// A fixed clock for the calling test thread (the headless session
    /// replay hands out each recorded cycle's time).
    static TEST_NOW: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Fix [`monotonic_millis`], [`now`] and [`system_time`] on this test
/// thread (`None` restores the clock).
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_now(now: Option<i64>) {
    TEST_NOW.with(|cell| cell.set(now));
}

/// A bounded tape of actual monotonic reads within one fixture phase.
/// Other clock APIs and the production clock are unchanged.
#[cfg(any(test, feature = "test-hooks"))]
pub const MAX_MONOTONIC_SAMPLES: usize = 16_384;

#[cfg(any(test, feature = "test-hooks"))]
enum MonotonicSampleState {
    Recording(Vec<i64>),
    Replaying { samples: Vec<i64>, consumed: usize },
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static MONOTONIC_SAMPLES: std::cell::RefCell<Option<MonotonicSampleState>> = const { std::cell::RefCell::new(None) };
}

/// Calling-thread scope; dropping it also clears a failed or unwinding phase.
#[cfg(any(test, feature = "test-hooks"))]
pub struct MonotonicSamples {
    active: bool,
    _calling_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl MonotonicSamples {
    pub fn record() -> crate::Result<Self> {
        Self::start(MonotonicSampleState::Recording(Vec::new()))
    }
    pub fn replay(samples: Vec<i64>) -> crate::Result<Self> {
        crate::ensure!(
            samples.len() <= MAX_MONOTONIC_SAMPLES,
            "monotonic phase exceeds sample bound"
        );
        Self::start(MonotonicSampleState::Replaying {
            samples,
            consumed: 0,
        })
    }
    fn start(state: MonotonicSampleState) -> crate::Result<Self> {
        MONOTONIC_SAMPLES.with(|cell| {
            let mut current = cell.borrow_mut();
            crate::ensure!(current.is_none(), "monotonic sample phases cannot nest");
            *current = Some(state);
            Ok(Self {
                active: true,
                _calling_thread: std::marker::PhantomData,
            })
        })
    }
    pub fn finish(mut self) -> crate::Result<Vec<i64>> {
        let state = MONOTONIC_SAMPLES.with(|cell| cell.borrow_mut().take());
        self.active = false;
        match state {
            Some(MonotonicSampleState::Recording(samples)) => Ok(samples),
            Some(MonotonicSampleState::Replaying { samples, consumed }) => {
                crate::ensure!(
                    consumed == samples.len(),
                    "monotonic phase has {} unused samples",
                    samples.len() - consumed
                );
                Ok(samples)
            }
            None => Err(crate::err!("monotonic sample phase is absent")),
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for MonotonicSamples {
    fn drop(&mut self) {
        if self.active {
            MONOTONIC_SAMPLES.with(|cell| *cell.borrow_mut() = None);
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn replaying_monotonic_samples() -> bool {
    MONOTONIC_SAMPLES
        .with(|cell| matches!(*cell.borrow(), Some(MonotonicSampleState::Replaying { .. })))
}

#[cfg(any(test, feature = "test-hooks"))]
fn replayed_monotonic_sample() -> Option<i64> {
    MONOTONIC_SAMPLES.with(|cell| match cell.borrow_mut().as_mut() {
        Some(MonotonicSampleState::Replaying { samples, consumed }) => {
            let value = *samples
                .get(*consumed)
                .expect("monotonic phase exhausted by an extra read");
            *consumed += 1;
            Some(value)
        }
        _ => None,
    })
}

fn captured_monotonic_sample(value: i64) -> i64 {
    #[cfg(any(test, feature = "test-hooks"))]
    MONOTONIC_SAMPLES.with(|cell| {
        if let Some(MonotonicSampleState::Recording(samples)) = cell.borrow_mut().as_mut() {
            assert!(
                samples.len() < MAX_MONOTONIC_SAMPLES,
                "monotonic phase exceeds sample bound"
            );
            samples.push(value);
        }
    });
    value
}

/// `--fixed-clock`: the process clock replaced by `start + ticks * step`.
struct Fixed {
    start: i64,
    step: i64,
    /// Logic cycles begun ([`Clock::poll`] calls).
    ticks: AtomicI64,
    now: AtomicI64,
}

static FIXED: OnceLock<Fixed> = OnceLock::new();

/// Parse `--fixed-clock <start_ms>[:step_ms]` (step defaults to one
/// 20 ms logic interval).
pub fn parse_fixed(spec: &str) -> crate::Result<(i64, i64)> {
    let (start, step) = match spec.split_once(':') {
        Some((start, step)) => (start, Some(step)),
        None => (spec, None),
    };
    let start: i64 = start
        .trim()
        .parse()
        .map_err(|e| crate::err!("--fixed-clock start {start:?}: {e}"))?;
    let step: i64 = match step {
        Some(step) => step
            .trim()
            .parse()
            .map_err(|e| crate::err!("--fixed-clock step {step:?}: {e}"))?,
        None => INTERVAL_NS / 1_000_000,
    };
    crate::ensure!(step >= 0, "--fixed-clock step must not be negative");
    Ok((start, step))
}

/// Replace the process clock (before the app and its [`Clock`] exist).
pub fn install_fixed(start_ms: i64, step_ms: i64) -> crate::Result<()> {
    FIXED
        .set(Fixed {
            start: start_ms,
            step: step_ms,
            ticks: AtomicI64::new(0),
            now: AtomicI64::new(start_ms),
        })
        .map_err(|_| crate::err!("the fixed clock is already installed"))
}

/// The fixed time of this read, if the clock is not the operating system's.
fn fixed_now() -> Option<i64> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(now) = TEST_NOW.with(std::cell::Cell::get) {
        return Some(now);
    }
    FIXED.get().map(|f| f.now.load(Ordering::Acquire))
}

/// The [`Instant`] a fixed millisecond value maps to (a process-wide base
/// plus `ms`), so fixed-clock differences are exact in both representations.
fn instant_at(ms: i64) -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = *BASE.get_or_init(Instant::now);
    let offset = Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 {
        base.checked_add(offset)
    } else {
        base.checked_sub(offset)
    }
    .unwrap_or(base)
}

/// Retain forward wall-clock jumps and compensate
/// backwards jumps. Shared by packet timestamps and local/server varp deadlines.
pub fn monotonic_millis() -> i64 {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(now) = replayed_monotonic_sample() {
        return now;
    }
    if let Some(now) = fixed_now() {
        return captured_monotonic_sample(now);
    }
    static STATE: std::sync::Mutex<(i64, i64)> = std::sync::Mutex::new((0, 0));
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(t) => t.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    };
    let mut s = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if now < s.0 {
        s.1 = s.1.wrapping_add(s.0.wrapping_sub(now));
    }
    s.0 = now;
    captured_monotonic_sample(s.1.wrapping_add(now))
}

/// `Instant::now()` for logic and render-animation reads (see the module
/// inventory): the operating-system instant in live mode.
pub fn now() -> Instant {
    match fixed_now() {
        Some(ms) => instant_at(ms),
        None => Instant::now(),
    }
}

/// `SystemTime::now()` for clock-derived seeds: the operating-system time in
/// live mode.
pub fn system_time() -> SystemTime {
    match fixed_now() {
        Some(ms) if ms >= 0 => UNIX_EPOCH + Duration::from_millis(ms as u64),
        Some(ms) => UNIX_EPOCH - Duration::from_millis(ms.unsigned_abs()),
        None => SystemTime::now(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timer {
    pub time: i64,
    pub next: i64,
    pub previous: i64,
    pub samples: [i64; 10],
    pub index: usize,
    pub count: usize,
}
impl Timer {
    pub fn new(first: i64, second: i64) -> Self {
        Self {
            time: first,
            next: second,
            previous: 0,
            samples: [0; 10],
            index: 0,
            count: 1,
        }
    }
    /// Loading resets the sample origin, not the history.
    pub fn reset(&mut self) {
        self.previous = 0;
        if self.next > self.time {
            self.time = self.next;
        }
    }
    /// Returns whole milliseconds to sleep.
    pub fn begin(&mut self, now: i64) -> i64 {
        let delta = now.wrapping_sub(self.previous);
        self.previous = now;
        if delta > -5_000_000_000 && delta < 5_000_000_000 {
            self.samples[self.index] = delta;
            self.index = (self.index + 1) % 10;
            // The count stays at 1; this is not a ten-sample moving average.
            if self.count < 1 {
                self.count += 1;
            }
        }
        let mut sum = 0i64;
        for n in 1..=self.count {
            sum = sum.wrapping_add(self.samples[(self.index + 10 - n) % 10]);
        }
        self.time = self.time.wrapping_add(sum / self.count as i64);
        if self.next > self.time {
            self.next.wrapping_sub(self.time) / 1_000_000
        } else {
            0
        }
    }
    /// At least one update, at most ten catch-up updates.
    /// Called after the requested sleep, without taking another clock sample.
    pub fn finish(&mut self, interval: i64) -> i32 {
        assert!(interval > 0);
        if self.next > self.time {
            self.previous = self
                .previous
                .wrapping_add(self.next.wrapping_sub(self.time));
            self.time = self.next;
            self.next = self.next.wrapping_add(interval);
            return 1;
        }
        let mut ticks = 0;
        loop {
            ticks += 1;
            self.next = self.next.wrapping_add(interval);
            if ticks == 10 || self.next >= self.time {
                break;
            }
        }
        if self.next < self.time {
            self.next = self.time;
        }
        ticks
    }
}

#[derive(Clone, Debug)]
pub struct Schedule {
    pub timer: Timer,
    wake: Option<i64>,
}
impl Schedule {
    pub fn new(first: i64, second: i64) -> Self {
        Self {
            timer: Timer::new(first, second),
            wake: None,
        }
    }
    /// Winit adaptation of the timer's sleep. Early redraws do not
    /// advance logic, resample the timer or shorten the pending sleep.
    pub fn poll(&mut self, now: i64) -> i32 {
        if let Some(wake) = self.wake {
            if now < wake {
                return 0;
            }
            self.wake = None;
        } else {
            let delay = self.timer.begin(now);
            if delay > 0 {
                self.wake = Some(now.wrapping_add(delay.wrapping_mul(1_000_000)));
                return 0;
            }
        }
        self.timer.finish(INTERVAL_NS)
    }
    /// The pending sleep's wake, or the next cycle before a sleep is begun.
    /// Reading it never samples or advances the timer.
    pub fn deadline(&self) -> i64 {
        self.wake.unwrap_or(self.timer.next)
    }

    pub fn reset(&mut self) {
        self.wake = None;
        self.timer.reset();
    }
}

impl Fixed {
    /// One logic cycle at `start + k * step` (k = cycles begun before it).
    fn tick(&self) -> i32 {
        let k = self.ticks.fetch_add(1, Ordering::AcqRel);
        self.now.store(
            self.start.wrapping_add(k.wrapping_mul(self.step)),
            Ordering::Release,
        );
        1
    }
}

pub struct Clock {
    origin: Instant,
    epoch: i64,
    pub schedule: Schedule,
}
impl Clock {
    #[allow(
        clippy::new_without_default,
        reason = "Clock::new samples the system clock; a Default impl would hide that"
    )]
    pub fn new() -> Self {
        // Only an origin: subsequent samples use Instant and cannot jump with
        // wall-clock adjustments. Timer differences are independent of epoch.
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i64;
        let origin = Instant::now();
        let second = epoch.wrapping_add(origin.elapsed().as_nanos() as i64);
        Self {
            origin,
            epoch,
            schedule: Schedule::new(epoch, second),
        }
    }
    /// `System.nanoTime()`; the fixed clock's
    /// milliseconds in nanoseconds.
    pub fn sample(&self) -> i64 {
        if let Some(ms) = fixed_now() {
            return ms.wrapping_mul(1_000_000);
        }
        self.epoch
            .wrapping_add(self.origin.elapsed().as_nanos() as i64)
    }
    /// Logic cycles due now. The fixed clock runs one
    /// cycle per call and advances before it.
    pub fn poll(&mut self) -> i32 {
        if let Some(fixed) = FIXED.get() {
            return fixed.tick();
        }
        self.schedule.poll(self.sample())
    }
    /// The real monotonic deadline for the event loop's next logic poll.
    /// Fixed-clock diagnostics stay unpaced: every callback grants one cycle.
    pub fn wake_deadline(&self) -> Option<Instant> {
        if fixed_now().is_some() {
            return None;
        }
        let offset = self.schedule.deadline().wrapping_sub(self.epoch).max(0) as u64;
        Some(self.origin + std::time::Duration::from_nanos(offset))
    }

    pub fn reset(&mut self) {
        self.schedule.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redraws_during_sleep_do_not_change_the_timer() {
        let start = 10_000_000_000;
        let mut s = Schedule::new(start, start);
        assert_eq!(s.poll(start), 1);
        assert_eq!(s.poll(start + 1_000_000), 0);
        let timer = s.timer.clone();
        let deadline = s.deadline();
        assert_eq!(deadline, timer.next);
        for delta in [2, 5, 8, 12, 19] {
            assert_eq!(s.poll(start + delta * 1_000_000), 0);
            assert_eq!(s.timer, timer);
            assert_eq!(s.deadline(), deadline);
        }
        assert_eq!(s.poll(deadline), 1);
    }
    #[test]
    fn stalled_presentation_is_bounded_and_reset_cancels_old_sleep() {
        let start = 10_000_000_000;
        let mut s = Schedule::new(start, start);
        assert_eq!(s.poll(start), 1);
        assert_eq!(s.poll(start + 500_000_000), 10);
        assert_eq!(s.timer.next, s.timer.time);
        assert_eq!(s.poll(start + 501_000_000), 1);
        assert_eq!(s.poll(start + 502_000_000), 0);
        s.reset();
        assert_eq!(s.poll(start + 502_000_000), 1);
    }

    #[test]
    fn wake_deadline_is_read_only_and_fixed_diagnostics_are_unpaced() {
        const TEST_EPOCH_NS: i64 = 10_000_000_000;
        let origin = Instant::now();
        let clock = Clock {
            origin,
            epoch: TEST_EPOCH_NS,
            schedule: Schedule::new(TEST_EPOCH_NS, TEST_EPOCH_NS + INTERVAL_NS),
        };
        let timer = clock.schedule.timer.clone();
        assert_eq!(
            clock.wake_deadline(),
            Some(origin + std::time::Duration::from_nanos(INTERVAL_NS as u64))
        );
        assert_eq!(clock.schedule.timer, timer);
        set_test_now(Some(TEST_EPOCH_NS));
        assert_eq!(clock.wake_deadline(), None);
        set_test_now(None);
        assert_eq!(clock.schedule.timer, timer);
    }

    #[test]
    fn fixed_clock_spec_parses_start_and_optional_step() {
        assert_eq!(parse_fixed("1000").unwrap(), (1000, 20));
        assert_eq!(parse_fixed("1000:0").unwrap(), (1000, 0));
        assert_eq!(parse_fixed("-5:7").unwrap(), (-5, 7));
        assert!(parse_fixed("x").is_err());
        assert!(parse_fixed("1:-1").is_err());
        assert!(parse_fixed("1:y").is_err());
    }

    /// The global fixed clock is process-wide, so its tick is checked on a
    /// private instance: cycle k runs at `start + k * step`.
    #[test]
    fn fixed_clock_runs_one_cycle_per_poll_at_start_plus_k_steps() {
        let fixed = Fixed {
            start: 1_000,
            step: 20,
            ticks: AtomicI64::new(0),
            now: AtomicI64::new(1_000),
        };
        let mut seen = Vec::new();
        for _ in 0..4 {
            assert_eq!(fixed.tick(), 1);
            seen.push(fixed.now.load(Ordering::Acquire));
        }
        assert_eq!(seen, [1_000, 1_020, 1_040, 1_060]);
    }

    /// A fixed read agrees in all three representations, and differences
    /// between two fixed instants are the exact millisecond difference.
    #[test]
    fn test_clock_drives_every_read() {
        set_test_now(Some(1_700_000_000_123));
        let (ms, instant, system) = (monotonic_millis(), now(), system_time());
        let clock = Clock::new();
        assert_eq!(ms, 1_700_000_000_123);
        assert_eq!(clock.sample(), 1_700_000_000_123 * 1_000_000);
        assert_eq!(
            system.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_millis(1_700_000_000_123)
        );
        set_test_now(Some(1_700_000_000_123 + 250));
        assert_eq!(now().duration_since(instant), Duration::from_millis(250));
        assert_eq!(now(), now());
        set_test_now(None);
        // Live mode: the operating-system clocks again.
        let before = Instant::now();
        let live = now();
        assert!(live >= before && live <= Instant::now());
    }
}

#[cfg(test)]
mod monotonic_sample_tests {
    use super::*;
    const FIRST_SAMPLE: i64 = 101;
    const SECOND_SAMPLE: i64 = 407;

    #[test]
    fn phase_replays_exact_reads_and_rejects_unused_samples() {
        let recorded = MonotonicSamples::record().unwrap();
        set_test_now(Some(FIRST_SAMPLE));
        assert_eq!(monotonic_millis(), FIRST_SAMPLE);
        set_test_now(Some(SECOND_SAMPLE));
        assert_eq!(monotonic_millis(), SECOND_SAMPLE);
        let samples = recorded.finish().unwrap();
        set_test_now(None);
        let replay = MonotonicSamples::replay(samples.clone()).unwrap();
        assert_eq!(monotonic_millis(), FIRST_SAMPLE);
        assert_eq!(monotonic_millis(), SECOND_SAMPLE);
        assert_eq!(replay.finish().unwrap(), samples);
        let incomplete = MonotonicSamples::replay(samples).unwrap();
        assert_eq!(monotonic_millis(), FIRST_SAMPLE);
        assert!(incomplete.finish().is_err());
        assert!(!replaying_monotonic_samples());
    }

    #[test]
    fn extra_read_fails_and_unwinding_restores_thread_clock() {
        let result = std::panic::catch_unwind(|| {
            let _phase = MonotonicSamples::replay(Vec::new()).unwrap();
            monotonic_millis();
        });
        assert!(result.is_err());
        assert!(!replaying_monotonic_samples());
        set_test_now(Some(FIRST_SAMPLE));
        assert_eq!(monotonic_millis(), FIRST_SAMPLE);
        set_test_now(None);
    }
}
