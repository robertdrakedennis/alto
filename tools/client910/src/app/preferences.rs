//! `ViewerApp`'s preference owners: the toolkit selection, graphics
//! device settings and the window/canvas mode, synced from
//! the client preferences each logic cycle (P11).
use super::*;

impl ViewerApp {
    pub(super) fn sync_graphics_settings(&mut self) -> anyhow::Result<()> {
        let (Some(renderer), Some(game)) = (self.renderer.as_mut(), self.core.session.game_mut())
        else {
            return Ok(());
        };
        let preferences = &mut game.ui_variables.queries.preferences;
        preferences.anti_aliasing = renderer.supports_antialiasing();
        // The device keeps the sample level of its last toolkit creation.
        let level = preferences.active_aa;
        let mut samples = if level == 0 { 1 } else { (level * 2) as u32 };
        if !renderer.supports_scene_samples(samples) {
            // A graphics-device failure recovery clears saved and active AA before retrying.
            // An unsupported native sample count takes the same AA fallback.
            preferences.options.set_field("antiAliasing", 0).unwrap();
            preferences.options.set_field("antiAliasing2", 0).unwrap();
            preferences.active_aa = 0;
            preferences.dirty = true;
            preferences.window.changed = true;
            samples = 1;
        }
        preferences.bloom = renderer.supports_bloom();
        renderer.set_scene_effects(samples, preferences.bloom_enabled)?;
        Ok(())
    }

    /// Toolkit lifecycle consumers for the preference owner: the queued
    /// toolkit device recreation and the environment fade reset, the
    /// mainloop safe-mode confirmation and the graphics packet queue flush.
    pub(super) fn apply_toolkit_preferences(&mut self) -> anyhow::Result<()> {
        let pack_root = self.scene.pack_root.clone();
        let Some(session) = self.core.session.as_mut() else {
            return Ok(());
        };
        let state = session.machine.state;
        let connected = session.io.world.stream.is_some();
        let Some(game) = session.game.as_mut() else {
            return Ok(());
        };
        // The world map size x (0 before the first rebuild).
        let map_size_x = game
            .runtime
            .feed
            .state
            .world
            .as_ref()
            .map_or(0, |w| w.width);
        let preferences = &mut game.ui_variables.queries.preferences;
        preferences.metric_context =
            pack_root.map(|pack_root| crate::ui_preferences::MetricContext {
                pack_root,
                map_size_x,
            });
        let ui = &mut session.ui;
        preferences.performance_metrics_model = ui.engine.builtins.performance_metrics_model;
        ui.engine.platform.safe_mode = preferences.is_safe_mode;
        ui.engine.platform.physical_memory_mb =
            i32::try_from(preferences.hardware.ram_mb).unwrap_or(i32::MAX);
        ui.engine.platform.chose_safe_mode = preferences.chose_safe_mode;
        let top_level_open = ui.state.life.top != -1;
        preferences.confirm_safe_mode(state, top_level_open);
        let packets = preferences.flush_graphics_packets(state, connected);
        session.io.world.pending_writes.extend(packets);
        let (mut recreate, mut fade) = (false, false);
        preferences.pending_effects.retain(|effect| match effect {
            crate::ui_preferences::PreferenceEffect::RecreateToolkit => {
                recreate = true;
                false
            }
            crate::ui_preferences::PreferenceEffect::ResetEnvironmentFade => {
                fade = true;
                false
            }
            _ => true,
        });
        if fade {
            self.core.environment.fade_reset = true;
        }
        if !recreate {
            return Ok(());
        }
        // A toolkit change resets the model caches: the scene consumer re-uploads.
        preferences
            .pending_effects
            .push(crate::ui_preferences::PreferenceEffect::ResetModelCaches);
        let level = preferences.active_aa;
        let bloom = preferences.bloom_enabled;
        // Toolkit 0 (the software toolkit) is game state only,
        // drawn by the GPU renderer.
        let toolkit0 = preferences.options.get("displayMode") == Some(0);
        if crate::toolkit_debug_flags::flags().settings_trace {
            log::info!(
                "[settings] changeToolkit displayMode={} aa={level} bloom={bloom}",
                preferences.options.get("displayMode").unwrap()
            );
        }
        // A new keyboard/mouse pair is bound to the canvas, current cursor -1.
        {
            let ui = &mut session.ui;
            ui.keyboard
                .focus_lost(crate::logic_clock::monotonic_millis());
            // The world map's area and sprites belong to the old toolkit:
            // the same map loads again for the new one.
            ui.engine.world_map.borrow_mut().reload();
        }
        self.input.cursor_state.current = i32::MIN;
        if let Some(renderer) = self.renderer.as_mut() {
            // The old device is disposed and a new one made, as the original
            // does for a toolkit change; what was uploaded to the old one
            // is forgotten and the model-cache reset above rebuilds it.
            renderer.recreate_device(toolkit0)?;
            self.reset_gpu_owners();
            let renderer = self.renderer.as_mut().expect("the renderer was just used");
            let samples = if level == 0 { 1 } else { (level * 2) as u32 };
            let samples = if renderer.supports_scene_samples(samples) {
                samples
            } else {
                1
            };
            // Bloom is reapplied only when the new toolkit supports it.
            renderer.recreate_toolkit_targets(samples, bloom && renderer.supports_bloom())?;
        }
        Ok(())
    }

    pub(super) fn sync_window_settings(&mut self) -> anyhow::Result<()> {
        let (Some(session), Some(renderer)) = (self.core.session.as_mut(), self.renderer.as_mut())
        else {
            return Ok(());
        };
        if session.game.is_none() {
            {
                let ui = &mut session.ui;
                let (w, h) = renderer.canvas_size();
                ui.resize([w as i32, h as i32])?;
            }
            return Ok(());
        }
        let pointer = self
            .input
            .last_cursor
            .map(|(x, y)| ([x, y], self.window.as_ref().unwrap().scale_factor()));
        sync_window_state(session, pointer, &mut |canvas| {
            renderer.set_game_canvas(canvas)
        })
    }
}
