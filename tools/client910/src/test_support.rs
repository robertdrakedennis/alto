//! Shared fixtures for unit tests.
//!
//! The fast suite (`cargo test --lib`) requires the revision-910
//! cache at `server/data/pack`. A test that reads it asserts the fixture is
//! present, so a missing pack fails the test with a message naming the file.
//! It never reports `ok` without asserting anything.
//!
//! Behaviour recorded once from the original client is committed under
//! `fixtures/recorded` and compared through
//! `rs910_core::test_support::frozen`.

use std::path::PathBuf;

/// Heavy test state (a started replay's client core, scene and minimap: about
/// 1.2 GB of maps, shared UI nodes and caches) whose teardown a one-test
/// process skips. nextest runs every test in its own process, which exits as
/// soon as the test returns, so the operating system takes the memory back at
/// once; freeing it value by value took a replay test several seconds. Where
/// tests share a process (`cargo test`) the value is dropped as usual.
/// Nothing a test asserts runs after its state is dropped.
pub struct ReclaimedAtExit<T>(Option<T>);

impl<T> ReclaimedAtExit<T> {
    pub fn new(value: T) -> Self {
        Self(Some(value))
    }

    pub fn into_inner(mut self) -> T {
        self.0.take().expect("a held value")
    }
}

impl<T> std::ops::Deref for ReclaimedAtExit<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_ref().expect("a held value")
    }
}

impl<T> std::ops::DerefMut for ReclaimedAtExit<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("a held value")
    }
}

impl<T> Drop for ReclaimedAtExit<T> {
    fn drop(&mut self) {
        if one_test_per_process() {
            std::mem::forget(self.0.take());
        }
    }
}

/// Whether this process runs a single test and exits after it (nextest's
/// process-per-test mode, the gate's).
fn one_test_per_process() -> bool {
    static ONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONE.get_or_init(|| {
        std::env::var_os("NEXTEST_EXECUTION_MODE").is_some_and(|mode| mode == "process-per-test")
    })
}

// `server/data/pack` fixtures (Phase 2.4: moved to rs910-js5 so the
// config crate's tests share them).
pub use rs910_js5::test_support::require_pack;

/// `fixtures/replays/<scenario>/<file>`: a checked-in dev-server corpus
/// written by `cd server && npm run export-fixtures` (see
/// `fixtures/replays/SERVER-EXPORT.md`). Panics naming the regeneration command when
/// the file is missing.
#[track_caller]
pub fn replay_fixture(scenario: &str, file: &str) -> PathBuf {
    let path = rs910_core::test_support::client_dir()
        .join("fixtures/replays")
        .join(scenario)
        .join(file);
    assert!(
        path.is_file(),
        "{} is missing: regenerate it with `cd server && npm run export-fixtures`",
        path.display()
    );
    path
}

/// Parsed JSON of [`replay_fixture`].
#[track_caller]
pub fn replay_json(scenario: &str, file: &str) -> serde_json::Value {
    let path = replay_fixture(scenario, file);
    serde_json::from_slice(&std::fs::read(&path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Scratch directory for the diagnostic proof files a replay writes next to
/// its assertions (never the checked-in fixture directory).
pub fn proof_dir(scenario: &str) -> PathBuf {
    // Per process: concurrent `cargo test` runs (other worktrees, CI shards)
    // must not reload each other's proof files.
    let dir = std::env::temp_dir()
        .join("alto-replay-proof")
        .join(std::process::id().to_string())
        .join(scenario);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A monotonic clock source for replays: starts at the real clock and advances
/// one logic interval (20 ms, `logic_clock::INTERVAL_NS`) per
/// [`SimClock::tick`], independent of how fast the test machine runs cycles.
pub struct SimClock(pub i64);
impl Default for SimClock {
    fn default() -> Self {
        Self(crate::logic_clock::monotonic_millis())
    }
}
impl SimClock {
    /// One logic cycle: advance 20 ms, then run `ui.tick` against `game`.
    pub fn tick(
        &mut self,
        game: &mut crate::client_game::ClientGame,
        ui: &mut crate::ui_runtime::Runtime,
    ) -> anyhow::Result<()> {
        self.tick_probed(game, ui, None)
    }
    /// [`Self::tick`] with a renderer lent to the profiling commands.
    pub fn tick_probed(
        &mut self,
        game: &mut crate::client_game::ClientGame,
        ui: &mut crate::ui_runtime::Runtime,
        probe: Option<&mut dyn crate::ui_preferences::metric::RendererProbe>,
    ) -> anyhow::Result<()> {
        self.0 += crate::logic_clock::INTERVAL_NS / 1_000_000;
        let now = self.0;
        crate::client_game::with_game_clock_probed(game, &mut move || now, probe, |vars| {
            ui.tick(vars)
        })
    }
    /// Run `f` at the current simulated time.
    pub fn with<T>(
        &self,
        game: &mut crate::client_game::ClientGame,
        f: impl FnOnce(&mut crate::ui_vars::Variables<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let now = self.0;
        crate::client_game::with_game_clock(game, &mut move || now, f)
    }
}
