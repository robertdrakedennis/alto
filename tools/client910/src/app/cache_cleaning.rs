//! The caches' redraw schedule (`rs910_core::cache_schedule`) on the app's
//! owners: the interface caches, the scene's model caches and the toolkit's
//! textures are aged once per full redraw after loading, and their idle
//! entries are given back when the process is short of memory or when a
//! client cheat asks for it.
use super::*;
use rs910_core::cache_schedule::{Frame, Schedule, MODEL_AGE};

impl ViewerApp {
    /// One full redraw's cache work. Nothing happens while loading: the
    /// caches stay as the loaders filled them.
    pub(super) fn clean_caches(&mut self) {
        if self.lifecycle.loading.is_some() || self.core.session.is_none() {
            return;
        }
        let ram_mb = self
            .core
            .session
            .game()
            .map_or(0, |g| g.ui_variables.queries.preferences.hardware.ram_mb);
        let now = crate::logic_clock::monotonic_millis();
        let schedule = self
            .cache_schedule
            .get_or_insert_with(|| Schedule::new(u64::from(ram_mb)));
        let frame = schedule.frame(now, || {
            crate::debug_overlay::process_memory_kb()
                .and_then(|(used, _)| u64::try_from(used).ok())
                .map(|kb| kb * 1024)
        });
        let dropped = self.apply_cache_frame(frame);
        if frame.clear_soft {
            log::info!("[client910] memory is short: dropped {dropped} idle cache entries");
        }
    }

    /// Run `frame` on every cache the app owns; how many entries went.
    pub(super) fn apply_cache_frame(&mut self, frame: Frame) -> usize {
        let mut dropped = 0;
        if let Some(ui) = self.core.session.ui_mut() {
            dropped += ui.clean_caches(frame);
        }
        let entities = &mut self.entities;
        if frame.clean {
            entities.loc_models.clean(MODEL_AGE);
            entities.effect_models.clean(MODEL_AGE);
            entities.hint_models.clean(MODEL_AGE);
            entities.ground_models.clean(MODEL_AGE);
            entities.npcs.clean(MODEL_AGE);
            entities.npc_definitions.clean(MODEL_AGE);
        }
        if frame.clear_soft {
            dropped += entities.loc_models.clear_soft()
                + entities.effect_models.clear_soft()
                + entities.hint_models.clear_soft()
                + entities.ground_models.clear_soft()
                + entities.npcs.clear_soft()
                + entities.npc_definitions.clear_soft();
        }
        if let Some(renderer) = self.renderer.as_mut() {
            dropped += renderer.clean_caches(frame);
        }
        dropped
    }

    /// Give back every idle cache entry now (the client cheats that drop the
    /// soft-reference caches); how many went.
    pub(super) fn remove_soft_references(&mut self) -> usize {
        self.apply_cache_frame(Frame {
            clean: false,
            clear_soft: true,
        })
    }
}
