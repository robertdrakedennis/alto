//! `ViewerApp::environment` (code-quality programme Phase 4.4): the GPU half
//! of `EnvironmentManager` (the skybox owner and the colour remappers). The
//! CPU half (map, override, fade, teleport count) is the core's
//! (`rs910_client::client_core::EnvironmentManager`, lane E-A1), updated at
//! P9 and at R3 ([`ViewerApp::environment_frame`]).
use super::*;

/// `EnvironmentManager`'s toolkit side: the skybox cache and the colour
/// remappers.
pub(super) struct EnvironmentController {
    /// The skybox cache + the model/texture stores the skybox build reads.
    pub(super) sky: Option<SkyResources>,
    /// The colour remapping cache.
    pub(super) colour_remappers: crate::postprocess::RemapperCache,
}

/// The skybox and sky-decor type lists + the skybox cache and the stores
/// the skybox builds its toolkit model from.
pub(super) struct SkyResources {
    pub(super) owner: crate::skybox::SkyboxOwner,
    /// Last reported `(current skybox, model drawn)` (diagnostic log).
    pub(super) reported: Option<(Option<crate::skybox::SkyboxKey>, bool)>,
    pub(super) pack: Pack,
    pub(super) billboards: crate::billboard::BillboardStore,
    pub(super) emitters: crate::particle::EmitterStore,
    /// The skybox models and textures loaded so far and this frame's layers
    /// (renderer-neutral; every backend draws its frame).
    pub(super) cache: rs910_scene::sky_frame::SkyCache,
}

/// The skybox key of an environment's skybox.
pub(super) fn sky_key(s: crate::env::SkyboxRef) -> crate::skybox::SkyboxKey {
    (s.kind, s.a, s.b, s.c)
}

impl ViewerApp {
    /// `ENVIRONMENT_OVERRIDE` -> the environment override in the core, for the environment's target tile.
    pub(super) fn apply_environment_overrides(
        &mut self,
        updates: Vec<crate::session::EnvironmentOverrideEvent>,
    ) {
        let tile = self.core.environment_tile(self.camera_window_tile());
        for update in updates {
            self.core.environment.set_override(update, tile);
        }
    }

    /// The camera's window tile: `computeTargetEnvironment`'s position in
    /// the title/lobby states and offline (the legacy camera x/z).
    pub(super) fn camera_window_tile(&self) -> [i32; 2] {
        [
            self.view.camera.target.x.floor() as i32 - self.scene.floor_base.0,
            self.view.camera.target.z.floor() as i32 - self.scene.floor_base.1,
        ]
    }

    /// The skybox half of an environment fade, for the fades the core
    /// started since the last frame, in order: the current skybox starts its
    /// cross-fade towards the target's.
    fn apply_sky_fades(&mut self) {
        let fades = self.core.environment.take_sky_fades();
        let Some(sky) = self.environment.sky.as_mut() else {
            return;
        };
        for fade in fades {
            let key = |s: Option<crate::env::SkyboxRef>| s.map(sky_key);
            sky.owner.current = key(fade.from).map(|k| sky.owner.create(k));
            if let Some(k) = key(fade.to) {
                sky.owner.create(k);
            }
            sky.owner.begin_fade(key(fade.to));
        }
    }

    /// The per-cycle skybox update (toolkit, viewport height, skybox
    /// preference) and the draw with the frame's camera angles and the
    /// current fog colour.
    pub(super) fn prepare_skybox(&mut self, env_frame: &crate::env::EnvFrame) {
        if self.scene.pack_root.is_none() {
            return;
        }
        if self.environment.sky.is_none() {
            let pack = self.pack.clone();
            let loaded = crate::skybox::SkyTypes::load(&pack).and_then(|mut types| {
                let decors: Vec<_> = crate::debug_flags::flags()
                    .sky_decor
                    .iter()
                    .map(|v| crate::skybox::SkyDecorType {
                        kind: v[0],
                        texture: v[1],
                        position: [v[2], v[3], v[4]],
                        size: v[5],
                        colour: v.get(6).copied().unwrap_or(16_777_216),
                        fixed: true,
                        ..Default::default()
                    })
                    .collect();
                if !decors.is_empty() {
                    types.attach_decors_to_all(decors);
                }
                Ok(SkyResources {
                    owner: crate::skybox::SkyboxOwner::new(types),
                    reported: None,
                    billboards: crate::billboard::BillboardStore::load(&pack)?,
                    emitters: crate::particle::EmitterStore::load(&pack)?,
                    pack,
                    cache: Default::default(),
                })
            });
            match loaded {
                Ok(sky) => self.environment.sky = Some(sky),
                Err(error) => {
                    static REPORTED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::logging::warn_repeated!("[client910] skybox resources: {error:#}");
                    }
                    return;
                }
            }
        }
        // The skybox of the current environment: the target
        // chunk's, replaced by an override's.
        let tile = self.core.environment_tile(self.camera_window_tile());
        let mut target = self.core.environment.base_environment(tile).0.skybox;
        let mut skybox_yaw_offset = self
            .core
            .environment
            .env
            .as_ref()
            .map_or(0, |e| e.skybox_yaw_offset);
        if let Some(sky) = self
            .core
            .environment
            .override_event
            .as_ref()
            .filter(|o| !o.clear)
            .and_then(|o| o.skybox)
        {
            target = Some(sky);
            skybox_yaw_offset = sky.yaw_offset;
        }
        let pref = self.core.session.game().map_or(1, |g| {
            g.ui_variables.queries.preferences.options.live().sky_detail
        });
        // The viewport component's height is passed in canvas pixels (not
        // the HiDPI framebuffer's).
        let component_height = self
            .core
            .session
            .ui()
            .and_then(|ui| ui.state.viewport_component_height);
        let (Some(sky), Some(renderer), Some(materials)) = (
            self.environment.sky.as_mut(),
            self.renderer.as_mut(),
            self.scene.material_store.as_ref(),
        ) else {
            return;
        };
        let owner = &mut sky.owner;
        match self.core.environment.fade.as_ref() {
            Some(fade) => {
                let from = fade.from.skybox.map(sky_key);
                let to = fade.to.skybox.map(sky_key);
                for key in [from, to].into_iter().flatten() {
                    owner.create(key);
                }
                let t = fade.progress(crate::logic_clock::now());
                if t >= 1.0 {
                    owner.end_fade(to);
                } else {
                    owner.fade_sample(from, to, t);
                }
            }
            None => {
                owner.select(target);
                // Completion of the fade.
                let current = owner.current;
                owner.end_fade(current);
            }
        }
        let viewport = renderer.scene_size();
        for b in owner.boxes.values_mut() {
            b.model_missing = sky.cache.model_failed(b.key);
        }
        let canvas_height = {
            let (_, canvas_h) = renderer.canvas_size();
            let (_, target_h) = renderer.size();
            (viewport.1 as i64 * i64::from(canvas_h) / i64::from(target_h.max(1))) as i32
        };
        owner.update(component_height.unwrap_or(canvas_height), pref);
        let state = (
            owner.current,
            owner.current.is_some_and(|k| owner.boxes[&k].model_wanted),
        );
        if sky.reported != Some(state) {
            sky.reported = Some(state);
            if let Some(key) = state.0 {
                let b = &sky.owner.boxes[&key];
                log::info!(
                    "[client910] skybox {key:?}: material {} model {} fill {:?} ({})",
                    b.material,
                    b.model_id,
                    b.fill,
                    if state.1 { "3D model" } else { "2D material" }
                );
            } else {
                log::info!("[client910] skybox: none (clear to fog colour)");
            }
        }
        let owner = &mut sky.owner;
        let camera = self.view.camera.scene_camera(viewport);
        let fog = env_frame
            .clear
            .iter()
            .fold(0, |acc, c| (acc << 8) | (c * 255.0).round() as i32);
        let layers = owner.frame(
            skybox_yaw_offset,
            camera.pitch_int(),
            camera.yaw_int(),
            0,
            fog,
        );
        let assets = crate::skybox_render::SkyAssets {
            pack: &sky.pack,
            materials,
            billboards: &sky.billboards,
            emitters: &sky.emitters,
        };
        // The neutral resolution (models, textures, fades), then the faithful
        // toolkit's uploads; a model it cannot create is a
        // swallowed model-creation failure.
        sky.cache.set_fade_override(
            self.core
                .environment
                .override_event
                .as_ref()
                .filter(|o| !o.clear)
                .map(|o| u32::from(o.duration_ms)),
        );
        sky.cache.resolve(&assets, &sky.owner, layers);
        if renderer.kind() == crate::active_toolkit::RendererKind::Modern
            && !renderer.toolkit0()
            && !rs910_render_modern::modern_debug_flags::flags().check
        {
            return;
        }
        let failed = renderer.prepare_skybox(&assets, sky.cache.frame(), &camera, env_frame);
        for key in failed {
            sky.cache.model_upload_failed(key);
        }
    }

    /// The loading frame's environment: the partial environment update
    /// in the core (`ClientCore::environment_frame`), then
    /// [`ViewerApp::build_env_frame`].
    pub(super) fn environment_frame(&mut self) -> crate::env::EnvFrame {
        let current = self.core.environment_frame(self.camera_window_tile());
        self.build_env_frame(current)
    }

    /// R3's shell half: the skybox fades the core started, then
    /// `updateSun`/`updateFog` for `currentEnv` (`current`, with its sun
    /// direction) as this frame's uniforms.
    pub(super) fn build_env_frame(
        &mut self,
        current: (crate::env::Environment, [f32; 3]),
    ) -> crate::env::EnvFrame {
        let scene_camera = self.view.camera.scene_camera(self.view.camera.viewport);
        let (far, near_min) = scene_camera.fog_reference();
        let view = scene_camera.view_entries();
        let live = self
            .core
            .session
            .game()
            .map(|g| g.ui_variables.queries.preferences.options.live())
            .unwrap_or_default();
        let (brightness, fog) = (live.brightness, live.fog);
        let (env, sun_direction) = current;
        self.apply_sky_fades();
        crate::env::EnvFrame::build(
            &env,
            crate::env::SunSettings {
                direction: sun_direction,
                brightness_pref: brightness,
                anti_macro: 0.0,
            },
            fog,
            crate::env::FogReference {
                far,
                near_min,
                view: &view,
            },
        )
    }
}
