//! The app side of the loading sequence: the stage bodies that create client owners (pack archives, defaults, audio,
//! config decoders, sprites/fonts, world map, client variables, the login
//! session and the toolkit) while `crate::loading` draws the loading screens.
//! Startup runs every stage before the session exists; `JS5_RELOAD`
//! re-runs them against the live session.
use super::*;

/// The JS5 archives `OPEN_JS5_ARCHIVES` opens:
/// `(archive, discardPacked, prefetchAll)`, each with `discardUnpacked` 1.
const GAME_ARCHIVES: [(u32, bool, bool); 31] = [
    (8, false, true),  // SPRITES
    (0, false, true),  // ANIMS
    (56, false, true), // ANIMS_KEYFRAMES
    (1, false, true),  // BASES
    (2, false, true),  // CONFIG
    (49, false, true), // DBTABLEINDEX
    (3, false, true),  // INTERFACES
    (5, true, true),   // MAPS
    (7, false, true),  // MODELS
    (53, true, true),  // TEXTURES_PNG
    (10, false, true), // BINARY
    (12, false, true), // CLIENTSCRIPTS
    (14, true, false), // VORBIS
    (40, true, false), // AUDIOSTREAMS
    (16, false, true), // CONFIG_LOC
    (17, false, true), // CONFIG_ENUM
    (18, false, true), // CONFIG_NPC
    (19, false, true), // CONFIG_OBJ
    (20, false, true), // CONFIG_SEQ
    (21, false, true), // CONFIG_SPOT
    (22, false, true), // CONFIG_STRUCT
    (23, true, false), // WORLDMAP
    (41, true, true),  // WORLDMAPAREADATA
    (24, false, true), // QUICKCHAT
    (25, false, true), // QUICKCHAT_GLOBAL
    (26, true, true),  // MATERIALS
    (27, false, true), // CONFIG_PARTICLE
    (29, false, true), // CONFIG_BILLBOARD
    (35, true, true),  // CUTSCENES
    (30, true, true),  // DLLS
    (31, true, true),  // SHADERS
];

/// `Js5Archive` ids the loading stages name.
const LOADING_SCREENS: u32 = 33;
const FONTMETRICS: u32 = 13;
const DEFAULTS: u32 = 28;
const MODELS: u32 = 7;
const SPRITES: u32 = 8;

/// The raw loading sprites when the raw flag is set, the ordinary ones otherwise.
fn loading_sprites_id() -> u32 {
    if crate::loading::loading_sprites_raw() {
        34
    } else {
        32
    }
}

/// Safe mode, chosen with 's'.
const SAFE_MODE_KEY: i32 = 8;

/// The message box text of a rebuild state, from the rebuild stage and its
/// progress.
pub(super) fn rebuild_message_text(progress: &crate::login_state::RebuildProgress) -> String {
    let language = crate::loading::client_language();
    progress.message(crate::loading::Text::Loading.display(language))
}

/// The message box text of states 14/19.
pub(super) fn message_box_text(state: i32) -> Option<String> {
    let language = crate::loading::client_language();
    match state {
        crate::login_state::RECONNECT => Some(format!(
            "{}{}{}",
            crate::loading::Text::ConnectionLost.display(language),
            crate::message_box::BR,
            crate::loading::Text::AttemptToReestablish.display(language)
        )),
        crate::login_state::TRANSFER => Some(
            crate::loading::Text::PleaseWait
                .display(language)
                .to_owned(),
        ),
        _ => None,
    }
}

/// The loading sequence plus the owners its stages create before they join
/// the session.
pub(super) struct LoadingOwner {
    pub loading: crate::loading::Loading,
    /// This loop's keyboard event characters.
    pub keys: Vec<char>,
    startup: Startup,
    /// Loading frames drawn (`CLIENT910_LOADING_SCREENSHOTS`).
    frames: u32,
    shot_taken: bool,
    /// Screenshot label: `loading` at startup, `reload` for `JS5_RELOAD`.
    label: &'static str,
}

struct Startup {
    pack: Pack,
    /// The client preferences loaded at startup,
    /// until the game owner (stage 10) takes it. `None` on a reload: the
    /// live game owns it.
    preferences: Option<crate::ui_preferences::Preferences>,
    /// `GraphicsDefaults`/wear defaults (stage 1).
    defaults: Option<crate::protocol910::defaults::EntityDefaults>,
    audio: Option<crate::audio_runtime::AudioRuntime>,
    game: Option<crate::client_game::ClientGame>,
    engine: Option<crate::ui_runtime::Engine>,
    runtime: Option<crate::ui_runtime::Runtime>,
    /// The client state before the session exists.
    client_state: i32,
    /// SETUP_CONFIG_DECODERS' decodes, running off the UI thread.
    decoders: Option<std::thread::JoinHandle<anyhow::Result<Decoded>>>,
}

/// The original stage runs on the client's main thread; macOS schedules a
/// default-QoS worker like background work, so the decoders ask for the
/// user-initiated class the main thread's work has.
fn decoder_thread_priority() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        /// `QOS_CLASS_USER_INITIATED` (<sys/qos.h>).
        const QOS_CLASS_USER_INITIATED: u32 = 0x19;
        // SAFETY: sets the calling thread's own QoS class.
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0);
        }
    }
}

/// What SETUP_CONFIG_DECODERS decodes: the game
/// owner's type lists and the interface engine's config tables.
type Decoded = (
    crate::client_game::ClientGame,
    crate::ui_runtime::CacheConfigs,
);

impl LoadingOwner {
    /// The client state before the session exists.
    pub(super) fn client_state(&self) -> i32 {
        self.startup.client_state
    }

    fn new(startup: Startup, label: &'static str) -> Self {
        Self {
            loading: crate::loading::Loading::new(crate::loading::client_language()),
            keys: Vec::new(),
            startup,
            frames: 0,
            shot_taken: false,
            label,
        }
    }

    /// Process start: the preferences load before the first loading update.
    pub(super) fn startup(cli: &Cli, pack: Pack) -> Self {
        let mut preferences = crate::ui_preferences::Preferences::default();
        preferences.apply_hardware(rs910_core::hardware::probe());
        preferences.install(preferences_path(&cli.pack_root));
        Self::new(
            Startup {
                pack,
                preferences: Some(preferences),
                defaults: None,
                audio: None,
                game: None,
                engine: None,
                runtime: None,
                client_state: crate::login_state::LOADING,
                decoders: None,
            },
            "loading",
        )
    }

    /// `Loading.reload`: the stages rebuild the caches of the live session.
    pub(super) fn reload(app: &ViewerApp) -> Self {
        Self::new(
            Startup {
                pack: app.pack.clone(),
                preferences: None,
                defaults: None,
                audio: None,
                game: None,
                engine: None,
                runtime: None,
                client_state: crate::login_state::LOADING,
                decoders: None,
            },
            "reload",
        )
    }
}

struct Host<'a> {
    app: &'a mut ViewerApp,
    startup: &'a mut Startup,
    keys: &'a mut Vec<char>,
}

impl Host<'_> {
    fn preferences(&self) -> Option<&crate::ui_preferences::Preferences> {
        if let Some(game) = self.app.core.session.game() {
            return Some(&game.ui_variables.queries.preferences);
        }
        if let Some(game) = self.startup.game.as_ref() {
            return Some(&game.ui_variables.queries.preferences);
        }
        self.startup.preferences.as_ref()
    }

    fn preferences_mut(&mut self) -> Option<&mut crate::ui_preferences::Preferences> {
        if let Some(game) = self.app.core.session.game_mut() {
            return Some(&mut game.ui_variables.queries.preferences);
        }
        if let Some(game) = self.startup.game.as_mut() {
            return Some(&mut game.ui_variables.queries.preferences);
        }
        self.startup.preferences.as_mut()
    }

    fn reloading(&self) -> bool {
        self.app.core.session.is_some()
    }

    fn runtime_mut(&mut self) -> Option<&mut crate::ui_runtime::Runtime> {
        match self.app.core.session.ui_mut() {
            Some(ui) => Some(ui),
            _ => self.startup.runtime.as_mut(),
        }
    }

    fn graphics(&self) -> anyhow::Result<&crate::protocol910::defaults::Scalars> {
        Ok(&self
            .startup
            .defaults
            .as_ref()
            .context("graphics defaults not loaded")?
            .graphics
            .scalars)
    }
}

impl crate::loading::Host for Host<'_> {
    fn now(&self) -> i64 {
        crate::logic_clock::monotonic_millis()
    }

    fn client_state(&self) -> i32 {
        self.app
            .core
            .session
            .as_ref()
            .map_or(self.startup.client_state, |s| s.machine.state)
    }

    fn set_state(&mut self, state: i32) {
        if self.app.core.session.is_some() {
            self.app.set_client_state(state);
        } else {
            log::info!(
                "[client910] loading: client state {} -> {state}",
                self.startup.client_state
            );
            self.startup.client_state = state;
        }
    }

    fn request_login(&mut self, username: String, password: String) {
        self.app
            .start_login_request(crate::ui_runtime::LoginRequest {
                username,
                password,
                new_auth_preference: String::new(),
                auth_dont_trust: false,
                lobby: false,
                sso: None,
            });
    }

    fn safe_mode_key(&mut self) {
        let keys = std::mem::take(self.keys);
        let Some(preferences) = self.preferences_mut() else {
            return;
        };
        if preferences.options.get("safeMode") != Some(0) {
            return;
        }
        if keys.iter().any(|&c| c == 's' || c == 'S') {
            let _ = preferences.options.set_field("safeMode", 1);
            preferences.dirty = true;
            preferences.chose_safe_mode = true;
            preferences.queue_toolkit_change(SAFE_MODE_KEY);
        }
    }

    fn pack(&self) -> Pack {
        self.startup.pack.clone()
    }

    fn loading_screen_preference(&self) -> i32 {
        self.preferences()
            .map_or(0, |p| p.options.live().loading_screen)
    }

    fn world(&self) -> (String, i32) {
        self.app.core.session.as_ref().map_or_else(
            || (self.app.cli.host.clone(), 1),
            |s| (s.current_world.host.clone(), s.current_world.node),
        )
    }

    fn default_fonts(&self) -> Option<Vec<i32>> {
        let g = self.graphics().ok()?;
        // The default fonts: p11, p12, b12 full.
        Some(vec![g.p11_full, g.p12_full, g.b12_full])
    }

    fn stage(&mut self, stage: usize) -> anyhow::Result<crate::loading::Step> {
        use crate::loading::Step;
        let pack = self.startup.pack.clone();
        match stage {
            // Stage 0: the JS5 client and its master index, toolkit 0 for the
            // loading screens, then the loading archives.
            0 => {
                let js5 = &mut self.app.js5;
                js5.ensure_client();
                if !js5.load_master_index() {
                    return Ok(Step::Stay(0));
                }
                if let Some(preferences) = self.preferences_mut() {
                    // Create toolkit 0 for the loading screens.
                    preferences.create_toolkit(0, true);
                }
                let js5 = &mut self.app.js5;
                js5.create_js5(loading_sprites_id(), false, 1, true)?;
                js5.create_js5(LOADING_SCREENS, false, 1, true)?;
                js5.create_js5(FONTMETRICS, false, 1, true)?;
                js5.create_js5(DEFAULTS, true, 1, true)?;
                // The loading screens run in toolkit 0 (the software toolkit's
                // answers; the GPU renderer draws them) until the last stage
                // creates the saved toolkit.
                if let Some(renderer) = self.app.renderer.as_mut() {
                    renderer.set_toolkit0(true);
                }
            }
            // Stage 1: fetch all of the loading screens and the defaults, then
            // the graphics and customization defaults.
            1 => {
                let js5 = &mut self.app.js5;
                let screens = js5.fetch_all(LOADING_SCREENS);
                let defaults = js5.fetch_all(DEFAULTS);
                let mut total = js5.provider_percentage(LOADING_SCREENS);
                total += js5.provider_percentage(loading_sprites_id());
                total += js5.provider_percentage(FONTMETRICS);
                total += if defaults {
                    100
                } else {
                    js5.percentage(DEFAULTS)
                };
                total += if screens {
                    100
                } else {
                    js5.percentage(LOADING_SCREENS)
                };
                if total != 500 {
                    return Ok(Step::Stay(total / 5));
                }
                self.startup.defaults = Some(crate::protocol910::pack_defaults::load(&pack)?);
            }
            // OPEN_JS5_ARCHIVES.
            5 => {
                for (archive, discard_packed, prefetch_all) in GAME_ARCHIVES {
                    self.app
                        .js5
                        .create_js5(archive, discard_packed, 1, prefetch_all)?;
                }
            }
            // GET_JS5_INDEXES: the providers' average index
            // percentage.
            6 => {
                let average = self.app.js5.index_average();
                if average != 100 {
                    return Ok(Step::Stay(average));
                }
            }
            // Stage 7: the audio API and the title song from the audio defaults,
            // then state 1.
            7 => {
                self.startup.audio = Some(crate::audio_runtime::AudioRuntime::new(pack.clone()));
                crate::loading::Host::set_state(self, crate::login_state::LOADING_AUDIO);
            }
            // Stage 8: the hardware platform loader; it loads no native
            // libraries because this port links its native owners statically.
            8 => self.app.js5.create_hardware_platform_loader(),
            // DOWNLOAD_STUFF: the resource manager's progress.
            9 => {
                let reloading = self.reloading();
                let progress = self.app.js5.load_progress(reloading);
                if progress < 100 {
                    return Ok(Step::Stay(progress));
                }
                let g = self.graphics()?;
                if g.client_frame_width != -1 && g.client_frame_height != -1 {
                    self.app.lifecycle.client_frame = [g.client_frame_width, g.client_frame_height];
                }
                // Wearpos/skill/minimenu/cutscene/world-map defaults are
                // decoded by the owners SETUP_CONFIG_DECODERS creates.
            }
            // SETUP_CONFIG_DECODERS. The original decodes type lists on
            // demand, so its stage is short and the loading screen keeps
            // drawing on its own thread either way; this port decodes
            // every list up front, on a worker thread, and stays at the
            // stage's own 99 until it finishes so the loading
            // screen keeps animating.
            10 => {
                let model = self.graphics()?.performancemetricsmodel;
                if model != -1 && !self.app.js5.request_download(MODELS, model, 0) {
                    return Ok(Step::Stay(99));
                }
                if self.startup.decoders.is_none() {
                    let pack = pack.clone();
                    let started = crate::logic_clock::monotonic_millis();
                    self.startup.decoders = Some(
                        std::thread::Builder::new()
                            .name("client910-config-decoders".into())
                            .spawn(move || -> anyhow::Result<Decoded> {
                                decoder_thread_priority();
                                // One read of the config archives for the
                                // game owner and the interface engine.
                                let shared = rs910_config::login_configs::LoginConfigs::read(&pack);
                                let game = title_game_with(&pack, &shared)?;
                                let game_ms = crate::logic_clock::monotonic_millis() - started;
                                let configs =
                                    crate::ui_runtime::CacheConfigs::decode_with(&pack, shared);
                                log::info!(
                                    "[client910] loading: config decoders took {} ms (game owner {game_ms} ms)",
                                    crate::logic_clock::monotonic_millis() - started
                                );
                                Ok((game, configs))
                            })
                            .context("spawn the config decoders")?,
                    );
                }
                if !self.startup.decoders.as_ref().unwrap().is_finished() {
                    return Ok(Step::Stay(99));
                }
                let (mut game, configs) = self
                    .startup
                    .decoders
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| anyhow::anyhow!("config decoders panicked"))??;
                if self.reloading() {
                    let session = self.app.core.session.as_mut().unwrap();
                    if let Some(previous) = session.game.take() {
                        // The preferences and the variable owners persist.
                        game.ui_variables = previous.ui_variables;
                    }
                    session.game = Some(game);
                    {
                        let ui = &mut session.ui;
                        ui.engine.install_cache_configs(configs);
                    }
                } else {
                    if let Some(preferences) = self.startup.preferences.take() {
                        game.ui_variables.queries.preferences = preferences;
                    }
                    let mut engine = crate::ui_runtime::Engine::default();
                    engine.install_cache_configs(configs);
                    self.startup.game = Some(game);
                    self.startup.engine = Some(engine);
                }
            }
            // SETUP_STATIC_SPRITES: default sprites and fonts.
            11 => {
                if let Some(runtime) = self.runtime_mut() {
                    runtime.state.load(&pack)?;
                } else {
                    let state = crate::ui_runtime::Runtime::load_state(&pack)?;
                    let engine = self.startup.engine.take().context("config decoders")?;
                    let audio = self.startup.audio.take().context("audio api")?;
                    let mut runtime =
                        crate::ui_runtime::Runtime::assemble(pack.clone(), engine, state, audio)?;
                    runtime.engine.login.uid192 =
                        load_uid192(&uid192_path(&self.app.cli.pack_root));
                    self.startup.runtime = Some(runtime);
                }
                if self.reloading() {
                    if let (Some(audio), Some(ui)) =
                        (self.startup.audio.take(), self.app.core.session.ui_mut())
                    {
                        ui.audio = audio;
                    }
                }
            }
            // Stage 12: the world map.
            12 => {
                if let Some(runtime) = self.runtime_mut() {
                    runtime.engine.load_world_map(&pack);
                }
            }
            // SETUP_VARC_SYSTEM: the client var domain and the cursor setup.
            13 => {
                let root = self.app.cli.pack_root.clone();
                if self.reloading() {
                    let session = self.app.core.session.as_mut().unwrap();
                    if let Some(game) = session.game.as_mut() {
                        install_client_varcs(game, &session.pack_root)?;
                    }
                } else {
                    let game = self.startup.game.as_mut().context("game owner")?;
                    install_client_varcs(game, &root)?;
                    let runtime = self.startup.runtime.as_mut().context("runtime")?;
                    crate::client_game::with_game(game, |v| runtime.initialize_cursors(v))?;
                }
            }
            // Stage 14: the login interface and its graphics.
            14 => {
                let login = self.graphics()?.login_interface;
                if login != -1 {
                    let runtime = self.runtime_mut().context("runtime")?;
                    if !runtime.store.open(login, None)? {
                        return Ok(Step::Stay(0));
                    }
                    let graphics: Vec<i32> = runtime
                        .store
                        .interfaces
                        .get(&login)
                        .map(|i| {
                            i.borrow()
                                .components
                                .borrow()
                                .iter()
                                .flatten()
                                .filter_map(|c| {
                                    let f = &c.borrow().f;
                                    (f.r#type == 5 && f.graphic != -1).then_some(f.graphic)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut ready = true;
                    for graphic in graphics {
                        // Request the sprite group download.
                        if !self.app.js5.request_download(SPRITES, graphic, 0) {
                            ready = false;
                        }
                    }
                    if !ready {
                        return Ok(Step::Stay(0));
                    }
                }
            }
            // Stage 15: show the login screen.
            15 => {
                if !self.reloading() {
                    self.install_session()?;
                    let _ = self.app.install_session_canvas();
                }
                self.app.show_login(true);
            }
            // Stage 16 (after the loading sequence's own cleanup): safe mode,
            // the saved toolkit, the window mode and the toolkit fonts.
            16 => {
                // Remove the loading archives.
                for archive in [LOADING_SCREENS, 32, 34] {
                    self.app.js5.remove_archive(archive);
                }
                self.app.install_session_toolkit();
                if let Err(error) = self.app.sync_window_settings() {
                    log::warn!("[client910] loading window mode: {error:#}");
                }
            }
            _ => {}
        }
        Ok(Step::Next)
    }
}

impl Host<'_> {
    /// The login session the title state runs on: the logged-out game
    /// owner, the retained interface runtime and the startup lobby/world
    /// addresses (applet parameters).
    fn install_session(&mut self) -> anyhow::Result<()> {
        let g = self.graphics()?;
        let (login_interface, lobby_interface) = (g.login_interface, g.lobby_interface);
        let mut game = self.startup.game.take().context("game owner")?;
        let runtime = self.startup.runtime.take().context("interface runtime")?;
        let cli = self.app.cli.clone();
        game.camera.pitch = cli.cam_pitch;
        game.camera.yaw = cli.cam_yaw;
        self.app.scene.applied_preferences = Some(crate::rebuild::BuildPrefs::from_options(
            &game.ui_variables.queries.preferences.options,
        ));
        self.app.view.follow = !cli.free_camera && cli.cam_target.is_none();
        let mut session = new_session(
            &cli,
            &self.startup.pack,
            SessionStart {
                stream: None,
                startup_connection: None,
                entities: crate::protocol910::live::Feed::default(),
                game: Some(game),
                prepared_map: None,
                ui: runtime,
                pending: Vec::new(),
                initial_ui: Vec::new(),
                login_interface,
                lobby_interface,
            },
        );
        session.polling_dead = true;
        session.machine.state = self.startup.client_state;
        log::info!(
            "[client910] title session: login_interface={login_interface} lobby_interface={lobby_interface} lobby={}:{} world={}:{}",
            session.current_lobby.host,
            session.current_lobby.port,
            session.current_world.host,
            session.current_world.port
        );
        self.app.core.session = Some(session);
        Ok(())
    }
}

impl ViewerApp {
    /// One loading update per logic cycle.
    pub(super) fn update_loading(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_none() {
            return;
        }
        let Some(mut owner) = self.lifecycle.loading.take() else {
            return;
        };
        let before = owner.loading.stage;
        let started = std::time::Instant::now();
        let result = owner.loading.update(&mut Host {
            app: self,
            startup: &mut owner.startup,
            keys: &mut owner.keys,
        });
        // The loading screen only redraws between updates (the original draws
        // it on its own thread): report updates long enough to stall it.
        let blocked = started.elapsed().as_millis();
        if blocked >= 100 {
            log::info!(
                "[client910] loading stage {before} update blocked the loading screen {blocked} ms"
            );
        }
        if owner.loading.stage != before {
            log::info!(
                "[client910] loading stage {before} -> {} ({}) frame {}",
                owner.loading.stage,
                owner.loading.text_percent,
                owner.frames
            );
        }
        if let Err(error) = result {
            // The client stops on a failed stage.
            log::warn!(
                "[client910] loading stage {} failed: {error:#}",
                owner.loading.stage
            );
            event_loop.exit();
            return;
        }
        if owner.loading.stage == crate::loading::DONE_STAGE {
            log::info!("[client910] loading complete after {} frames", owner.frames);
            if owner.shot_taken {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.reset_screenshot_written();
                }
            }
            return;
        }
        self.lifecycle.loading = Some(owner);
    }

    /// The loading screen draw in the loading states,
    /// through the shared UI painter over a blacked-out scene.
    pub(super) fn render_loading_frame(&mut self) {
        let env = self.environment_frame();
        let (Some(owner), Some(renderer)) =
            (self.lifecycle.loading.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        let (w, h) = renderer.canvas_size();
        let mut painter = crate::ui_paint::Painter::new([w, h]);
        if let Err(error) = owner.loading.draw(
            &mut painter,
            [w as i32, h as i32],
            self.lifecycle.client_frame,
            crate::logic_clock::monotonic_millis(),
        ) {
            log::warn!("[client910] loading screen: {error:#}");
        }
        // The draw calls (the null backend's digest).
        let recording = std::mem::take(&mut painter.recording);
        let layers = std::mem::take(&mut painter.layers);
        let paint = painter.finish();
        let output = crate::ui_backend::Output {
            recording,
            layers,
            models: Vec::new(),
            scene_quad: paint.quads.len(),
            paint,
            scene: None,
            // No FULLSCREEN_ENV_LAYER (1407) on the loading screen.
            postprocess: None,
        };
        if let Err(error) = renderer.prepare_ui(output) {
            log::warn!("[client910] loading screen upload: {error:#}");
            return;
        }
        renderer.set_scene_blackout(true);
        owner.frames += 1;
        // Diagnostic PNGs of loading frames: `CLIENT910_LOADING_SCREENSHOTS=
        // n1,n2,..` writes `<stem>_loading_<n>.png` beside `--screenshot`.
        if let (Some((path, _)), Some(frames)) = (
            self.diag.screenshot.as_ref(),
            crate::debug_flags::flags().loading_screenshots.as_ref(),
        ) {
            if frames.contains(&owner.frames) {
                let stem = path
                    .file_stem()
                    .map_or("frame".into(), |s| s.to_string_lossy().into_owned());
                let shot =
                    path.with_file_name(format!("{stem}_{}_{:04}.png", owner.label, owner.frames));
                log::info!(
                    "[client910] loading screenshot {} stage {} ({}) state {}",
                    shot.display(),
                    owner.loading.stage,
                    owner.loading.text_percent,
                    self.core
                        .session
                        .as_ref()
                        .map_or(owner.startup.client_state, |s| s.machine.state)
                );
                renderer.request_screenshot(shot);
                owner.shot_taken = true;
            }
        }
        // The loading screen has no scene component; the active backend
        // draws the retained UI (in toolkit 0, the faithful GPU toolkit).
        let result = renderer.frame_loading(&self.view.camera, &env);
        if let Err(error) = result {
            log::warn!("[client910] loading frame failed: {error:#}");
        }
    }
}

impl ViewerApp {
    /// Title screen update: outside a login the camera
    /// statics (moved by the retained per-tick cutscene/spline camera owner)
    /// at the build-area edge rebuild the title world at the `buildArea`
    /// preference's size.
    pub(super) fn update_title_world(&mut self) {
        let Some(session) = self.core.session.as_ref() else {
            return;
        };
        let (ui, Some(game)) = (&session.ui, session.game.as_ref()) else {
            return;
        };
        let state = session.machine.state;
        let e = &ui.engine;
        // A login or account-creation attempt in progress.
        let login = session.reconnect_started;
        let ready = matches!(state, 4 | 15 | 13 | 0)
            && (!login
                || (state == 15 && e.login.reply == 42)
                || (state == 17 && matches!(e.login.lobby_reply, 49 | 52)))
            && !e.creation.connect_in_progress;
        if !ready {
            return;
        }
        let camera = [e.camera.cam2.legacy.pose.x, e.camera.cam2.legacy.pose.z];
        if !self.lifecycle.title_world.camera_at_edge(camera) {
            return;
        }
        let build_area = game
            .ui_variables
            .queries
            .preferences
            .options
            .live()
            .build_area;
        let pack = self.pack.clone();
        let mut index = None;
        // Whether the land group is valid in the maps archive.
        let mut land = |group: i32| -> bool {
            let index = index.get_or_insert_with(|| pack.read_archive_index("mapsv2").ok());
            let (Some(index), Ok(group)) = (index.as_ref(), u32::try_from(group)) else {
                return false;
            };
            let count = index.file_count_for_group(group).unwrap_or(0);
            (0..count).any(|i| {
                index.file_id_for_group_index(group, i).ok() == Some(crate::cache::LAND_FILE)
            })
        };
        match self
            .lifecycle
            .title_world
            .rebuild(camera, build_area, state, &mut land)
        {
            Ok(None) => {}
            // The title and lobby screens draw interfaces only and the scene
            // draw is black outside a world, so this world is never presented:
            // its size, region and squares are the observable result.
            Ok(Some(rebuild)) => {
                log::debug!(
                    "[client910] title world rebuilt for state {} at {:?}: {} map squares",
                    rebuild.state,
                    rebuild.region,
                    rebuild.squares.len()
                );
                self.update_title_npcs(&rebuild);
            }
            Err(error) => log::warn!("[client910] title world: {error:#}"),
        }
    }

    /// The NPC step of a title rebuild: the NPCs standing carry over the base change and the
    /// rebuild's squares stand up their spawn lists. The login clears them before the world.
    fn update_title_npcs(&mut self, rebuild: &rs910_scene::title_world::Rebuild) {
        let files = rs910_scene::title_world::read_squares(&self.pack, &rebuild.squares);
        let Some(client) = self.core.session.as_mut().and_then(|s| s.game.as_mut()) else {
            return;
        };
        let textures = client
            .ui_variables
            .queries
            .preferences
            .options
            .get("textures")
            == Some(1);
        let game = &mut client.game;
        let inputs = rs910_scene::title_world::NpcInputs {
            types: &game.inputs.npc_types,
            cycle: game.cycle,
            textures,
            random: &mut || game.random.next(),
        };
        match self.lifecycle.title_world.update_npcs(
            rebuild,
            &files,
            &mut game.runtime.feed.state.npcs,
            inputs,
        ) {
            Ok(changes) => log::info!(
                "[client910] title NPCs: {} placed, {} dropped",
                changes.placed,
                changes.removed.len()
            ),
            Err(error) => log::warn!("[client910] title NPCs: {error:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rebuild states show LOADING, LOADING (N%)
    /// while the map squares download and LOADING again once the build
    /// starts; CONLOST + BR + ATTEMPT_TO_REESTABLISH in state 14, PLEASEWAIT
    /// in state 19, nothing elsewhere. (Decoded's `Send` bound is enforced
    /// at compile time by the SETUP_CONFIG_DECODERS worker `spawn`.)
    #[test]
    fn message_box_texts_follow_the_redraw() {
        let mut progress = crate::login_state::RebuildProgress::default();
        assert_eq!(rebuild_message_text(&progress), "Loading - please wait.");
        progress.load_maps(4);
        progress.load_maps(1);
        assert_eq!(
            rebuild_message_text(&progress),
            "Loading - please wait.<br>(37%)"
        );
        progress.load_maps(0);
        progress.begin_build();
        assert_eq!(rebuild_message_text(&progress), "Loading - please wait.");
        assert_eq!(
            message_box_text(crate::login_state::RECONNECT).as_deref(),
            Some("Connection lost.<br>Please wait - attempting to reestablish.")
        );
        assert_eq!(
            message_box_text(crate::login_state::TRANSFER).as_deref(),
            Some("Please wait...")
        );
        for state in [4, 13, 18, 5, 11, 1] {
            assert_eq!(message_box_text(state), None);
        }
    }

    /// The JS5 content server follows the launcher's lobby address, so JS5 downloads honour `--lobby-port`.
    #[test]
    fn content_server_follows_the_lobby_address() {
        let cli = Cli::parse_from(["client910", "--host", "10.0.0.2", "--lobby-port", "46594"]);
        assert_eq!(content_server(&cli), ("10.0.0.2".to_owned(), 46594));
    }
}
