//! Answering a GPU device fault (a lost device, a failed allocation): the
//! way the original client answers a renderer exception, by falling back to
//! a safer toolkit and drawing again.
//!
//! Each fault within [`WINDOW_MS`] of the first takes the next step of a
//! ladder: first a new device and resources for the same renderer; if that
//! fails again, the faithful renderer in place of the modern one; then the
//! software toolkit's answers (toolkit 0, which is what the original falls
//! back to, reported to the server as a toolkit change); beyond that the
//! client gives up rather than loop.
use super::*;
use crate::active_toolkit::Recovery as Toolkit;

/// The time a ladder lasts after its first fault, in milliseconds. A fault
/// after a quiet minute starts the ladder again.
pub(super) const WINDOW_MS: i64 = 60_000;

/// One step of the ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// A new device and resources for the same renderer.
    Recreate,
    /// A new device, with the faithful renderer in place of the modern one.
    SaferRenderer,
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
    /// `modern` is whether the modern renderer is drawing and `hardware`
    /// whether the active toolkit is a hardware one: the steps that change
    /// neither are left out.
    pub(super) fn next(&mut self, now: i64, modern: bool, hardware: bool) -> Option<Step> {
        if self.steps.is_empty() || now - self.started > WINDOW_MS {
            self.steps = std::iter::once(Step::Recreate)
                .chain(modern.then_some(Step::SaferRenderer))
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
        let modern =
            renderer.kind() == crate::active_toolkit::RendererKind::Modern && !renderer.toolkit0();
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
        let now = crate::logic_clock::monotonic_millis();
        let Some(step) = self.device_ladder.next(now, modern, hardware) else {
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
    /// installed scene to the new one: the scene's meshes (made again at once,
    /// the next dynamic loc refresh reads them), the players' bodies and the
    /// sky (made again when next used) and the minimap's base.
    pub(super) fn reset_gpu_owners(&mut self) {
        self.scene.meshes = Default::default();
        self.entities.players = None;
        self.environment.sky = None;
        self.minimap.cached_level = -1;
        self.upload_floors();
    }

    /// Run `step`: a new device (and renderer), then everything that was
    /// uploaded to the old one is forgotten and the scene is built again, as
    /// after a toolkit change.
    pub(super) fn recover_device(&mut self, step: Step) -> anyhow::Result<()> {
        let renderer = self.renderer.as_mut().context("no renderer")?;
        renderer.recover(match step {
            Step::SaferRenderer => Toolkit::SaferRenderer,
            Step::Recreate | Step::SaferToolkit => Toolkit::Recreate,
        })?;
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

#[cfg(test)]
mod tests {
    use super::super::console_tests::app_in_game;
    use super::*;
    use crate::active_toolkit::{ActiveToolkit, RendererKind};
    use crate::login_state::GAME;

    /// A fault after a quiet minute starts the ladder again; steps the
    /// toolkit cannot take are left out; a spent ladder gives none.
    #[test]
    fn the_ladder_steps_down_once_per_fault_in_a_minute() {
        let mut ladder = Ladder::default();
        let modern = (true, true);
        assert_eq!(ladder.next(1_000, modern.0, modern.1), Some(Step::Recreate));
        assert_eq!(ladder.next(2_000, false, true), Some(Step::SaferRenderer));
        assert_eq!(ladder.next(3_000, false, true), Some(Step::SaferToolkit));
        assert_eq!(ladder.next(4_000, false, false), Some(Step::Recreate));
        assert_eq!(ladder.next(5_000, false, false), Some(Step::Recreate));
        assert_eq!(ladder.next(6_000, false, false), None, "spent");
        // Quiet for a minute: it starts over, and the faithful renderer on
        // a software toolkit has only recreation to offer.
        assert_eq!(
            ladder.next(6_000 + WINDOW_MS + 1, false, false),
            Some(Step::Recreate)
        );
        assert_eq!(ladder.next(70_000, false, false), Some(Step::Recreate));
        assert_eq!(ladder.next(71_000, false, false), Some(Step::Recreate));
        assert_eq!(ladder.next(72_000, false, false), None);
    }

    /// The client's answer to a lost device through the app: the renderer
    /// loses its device, the next redraw's service recovers it and queues the
    /// toolkit rebuild; losses in a row step down to the faithful renderer
    /// and then to toolkit 0, reported as a toolkit change; and a device that
    /// will not stay up closes the client.
    #[test]
    #[ignore = "needs a GPU adapter and the cache"]
    fn a_lost_device_is_answered_by_the_ladder() -> anyhow::Result<()> {
        use crate::ui_preferences::PreferenceEffect;
        let (mut app, _keep) = app_in_game()?;
        app.renderer = Some(pollster::block_on(ActiveToolkit::headless(
            (64, 48),
            RendererKind::Modern,
        ))?);
        assert_eq!(app.core.session.as_ref().unwrap().machine.state, GAME);
        let toolkit_before = |app: &mut ViewerApp| {
            app.core
                .session
                .game()
                .unwrap()
                .ui_variables
                .queries
                .preferences
                .options
                .get("displayMode")
        };
        let generation = |app: &ViewerApp| app.renderer.as_ref().unwrap().device_generation();
        let effects = |app: &mut ViewerApp| {
            app.core
                .session
                .game_mut()
                .unwrap()
                .ui_variables
                .queries
                .preferences
                .drain_effects()
        };
        app.service_gpu_device();
        assert_eq!(generation(&app), 0, "nothing wrong, nothing done");
        // Uploads that belong to the old device.
        app.scene.meshes.scene_upload_failed = true;
        app.minimap.cached_level = 3;
        effects(&mut app);

        // 1. Recreate: the same renderer on a new device; the scene rebuild
        // is queued like after a toolkit change.
        app.renderer.as_ref().unwrap().lose_device();
        app.service_gpu_device();
        assert_eq!(generation(&app), 1);
        assert!(!app.scene.meshes.scene_upload_failed && app.minimap.cached_level == -1);
        let queued = effects(&mut app);
        assert!(
            queued.contains(&PreferenceEffect::ResetModelCaches),
            "{queued:?}"
        );
        assert!(
            !queued.contains(&PreferenceEffect::RecreateToolkit),
            "the device is already new"
        );
        let hardware = toolkit_before(&mut app);
        assert_ne!(hardware, Some(0));
        assert_eq!(app.renderer.as_ref().unwrap().kind(), RendererKind::Modern);

        // 2. The modern renderer gives way to the faithful one.
        app.renderer.as_ref().unwrap().lose_device();
        app.service_gpu_device();
        assert_eq!(generation(&app), 2);
        assert_eq!(
            app.renderer.as_ref().unwrap().kind(),
            RendererKind::FaithfulGpu
        );
        assert_eq!(toolkit_before(&mut app), hardware);
        effects(&mut app);

        // 3. Toolkit 0, as the original falls back, and the server is told.
        app.renderer.as_ref().unwrap().lose_device();
        app.service_gpu_device();
        assert_eq!(generation(&app), 3);
        assert_eq!(toolkit_before(&mut app), Some(0));
        let game = app.core.session.game_mut().unwrap();
        let reported = game
            .ui_variables
            .queries
            .preferences
            .flush_graphics_packets(GAME, true);
        assert_eq!(reported.len(), 0, "a fall back is not a failure report");
        assert!(!game.ui_variables.queries.preferences.change_notified);
        effects(&mut app);

        // 4. A device that keeps failing: recreations, then the client closes.
        for expected in [4, 5] {
            app.renderer.as_ref().unwrap().lose_device();
            app.service_gpu_device();
            assert_eq!(generation(&app), expected);
            assert!(!app.lifecycle.shutdown_requested);
        }
        app.renderer.as_ref().unwrap().lose_device();
        app.service_gpu_device();
        assert!(app.lifecycle.shutdown_requested);
        Ok(())
    }

    /// A toolkit change makes a new native device, not only new targets: the
    /// queued change is applied by the next cycle, which recreates the
    /// device, forgets the uploads and applies the toolkit's own answers.
    #[test]
    #[ignore = "needs a GPU adapter and the cache"]
    fn a_toolkit_change_recreates_the_native_device() -> anyhow::Result<()> {
        let (mut app, _keep) = app_in_game()?;
        app.renderer = Some(pollster::block_on(ActiveToolkit::headless(
            (64, 48),
            RendererKind::FaithfulGpu,
        ))?);
        app.scene.meshes.scene_upload_failed = true;
        app.minimap.cached_level = 2;
        let preferences = &mut app
            .core
            .session
            .game_mut()
            .unwrap()
            .ui_variables
            .queries
            .preferences;
        preferences.drain_effects();
        preferences.set_toolkit(5, false);
        app.apply_toolkit_preferences()?;
        let renderer = app.renderer.as_ref().unwrap();
        assert_eq!(renderer.device_generation(), 1, "a new device");
        assert!(!renderer.toolkit0());
        assert!(!app.scene.meshes.scene_upload_failed);
        assert_eq!(app.minimap.cached_level, -1);
        // The scene is rebuilt from the cached model reset.
        let queued = app
            .core
            .session
            .game_mut()
            .unwrap()
            .ui_variables
            .queries
            .preferences
            .drain_effects();
        assert!(queued.contains(&crate::ui_preferences::PreferenceEffect::ResetModelCaches));
        // Toolkit 0 follows the saved display mode.
        let preferences = &mut app
            .core
            .session
            .game_mut()
            .unwrap()
            .ui_variables
            .queries
            .preferences;
        preferences.set_toolkit(0, false);
        app.apply_toolkit_preferences()?;
        let renderer = app.renderer.as_ref().unwrap();
        assert_eq!(renderer.device_generation(), 2);
        assert!(renderer.toolkit0());
        Ok(())
    }
}
