//! Answering a GPU device fault (a lost device, a failed allocation): the
//! way the original client answers a renderer exception, by falling back to
//! a safer toolkit and drawing again.
//!
//! Each fault within [`WINDOW_MS`] of the first takes the next step of a
//! ladder: first a new device and resources; if that fails again, the
//! software toolkit's answers (toolkit 0, which is what the original falls
//! back to, reported to the server as a toolkit change); beyond that the
//! client gives up rather than loop.
use super::*;

/// The time a ladder lasts after its first fault, in milliseconds. A fault
/// after a quiet minute starts the ladder again.
pub(super) const WINDOW_MS: i64 = 60_000;

/// One step of the ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// A new device and resources.
    Recreate,
    /// A new device, with toolkit 0's answers: the safest the game offers.
    SaferToolkit,
}

/// The ladder and where a run of faults is on it.
#[derive(Debug, Default)]
pub(super) struct Ladder {
    steps: Vec<Step>,
    taken: usize,
    started: i64,
    /// `CLIENT910_LOSE_DEVICE` was carried out.
    injected: bool,
}

impl Ladder {
    /// The step for a fault at `now`, or none when the ladder is spent.
    /// `hardware` is whether the active toolkit is a hardware one (else the
    /// safer toolkit step is left out).
    pub(super) fn next(&mut self, now: i64, hardware: bool) -> Option<Step> {
        if self.steps.is_empty() || now - self.started > WINDOW_MS {
            self.steps = std::iter::once(Step::Recreate)
                .chain(hardware.then_some(Step::SaferToolkit))
                .chain([Step::Recreate, Step::Recreate])
                .collect();
            self.taken = 0;
            self.started = now;
        }
        let step = self.steps.get(self.taken).copied();
        self.taken += 1;
        step
    }
}

impl ViewerApp {
    /// Once per redraw: answer what the GPU device reported since the last
    /// one.
    pub(super) fn service_gpu_device(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        if let Some(at) = crate::debug_flags::flags().lose_device_at {
            if !self.device_ladder.injected && self.core.cycle >= at {
                self.device_ladder.injected = true;
                log::warn!("[client910] CLIENT910_LOSE_DEVICE: destroying the GPU device");
                renderer.lose_device();
            }
        }
        let Some(fault) = renderer.take_fault() else {
            return;
        };
        let hardware = self
            .core
            .session
            .game()
            .and_then(|g| {
                g.ui_variables
                    .queries
                    .preferences
                    .options
                    .get("displayMode")
            })
            .is_some_and(|toolkit| toolkit != 0);
        log::error!("[client910] {fault}{}", self.crash_context());
        let now = rs910_core::logic_clock::monotonic_millis();
        let Some(step) = self.device_ladder.next(now, hardware) else {
            log::error!("[client910] the GPU device keeps failing: closing");
            self.lifecycle.shutdown_requested = true;
            return;
        };
        log::warn!("[client910] answering the GPU fault with {step:?}");
        if let Err(error) = self.recover_device(step) {
            log::error!("[client910] no GPU device to draw with: {error:#}");
            self.lifecycle.shutdown_requested = true;
        }
    }

    /// Forget what the app uploaded to the toolkit's device and upload the
    /// installed scene to the new one: the players' bodies and the sky (made
    /// again when next used) and the minimap's base.
    pub(super) fn reset_gpu_owners(&mut self) {
        self.entities.players = None;
        self.environment.sky = None;
        self.minimap.cached_level = -1;
        self.install_scene_floors();
    }

    /// Run `step`: a new device (and renderer), then everything that was
    /// uploaded to the old one is forgotten and the scene is built again, as
    /// after a toolkit change.
    pub(super) fn recover_device(&mut self, step: Step) -> anyhow::Result<()> {
        let renderer = self.renderer.as_mut().context("no renderer")?;
        renderer.recover()?;
        log::info!(
            "[client910] recovered on GPU device {}",
            self.renderer.as_ref().map_or(0, |r| r.device_generation()) + 1
        );
        self.reset_gpu_owners();
        if let Some(ui) = self.core.session.ui_mut() {
            // The world map's area and sprites belong to the old device.
            ui.engine.world_map.borrow_mut().reload();
        }
        if let Some(game) = self.core.session.game_mut() {
            let preferences = &mut game.ui_variables.queries.preferences;
            if step == Step::SaferToolkit {
                // The original's answer to a renderer failure.
                preferences.set_toolkit(0, false);
            } else {
                preferences.device_recreated();
            }
        }
        Ok(())
    }
}
