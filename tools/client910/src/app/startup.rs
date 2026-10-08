//! Startup and world assets: the `--direct-login` world login, session
//! construction, the JS5 system, the client persistence files
//! (`uid192`, preferences, client varcs) and `WorldAssets` (the
//! scene rebuild the scene host installs).
use super::*;

/// Wait-then-load for the startup block: every group through the JS5
/// client (`mapsJs5.isGroupReady`, fetched into the JS5 disk store when the
/// pack lacks it), then `map::load_groups` over `Pack::open`. A JS5 failure
/// only warns — the disk-store load is attempted regardless.
pub(super) fn ensure_and_load_world(
    cli: &Cli,
    pack: &Pack,
    groups: &[u16],
) -> anyhow::Result<crate::map::World> {
    // The map squares wait on the maps JS5 groups being ready before the
    // scene builds. Startup runs before
    // the event loop, so a short-lived JS5 owner pumps here; its writes
    // reach the disk store before it quits.
    let mut js5 = new_js5_system(cli);
    let started = std::time::Instant::now();
    loop {
        js5.mainloop(crate::login_state::GAME);
        if let Some(fatal) = js5.fatal.take() {
            log::info!("[client910] startup js5 error {fatal}; trying the disk store as is");
            break;
        }
        if js5.tcp.error_count >= 2 && !js5.master_loaded() {
            log::info!(
                "[client910] startup: content server unreachable (js5State {}); trying the disk store as is",
                js5.tcp.js5_state
            );
            break;
        }
        match js5.map_groups_missing(groups) {
            Some(0) => break,
            missing => {
                if started.elapsed() > std::time::Duration::from_secs(60) {
                    log::warn!(
                        "[client910] startup map groups still missing ({missing:?}) after 60 s; trying the disk store as is"
                    );
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    while js5.disk.pending_requests() > 0 && started.elapsed() < std::time::Duration::from_secs(70)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    js5.quit();
    crate::map::load_groups(pack, groups).map_err(|err| {
        anyhow::anyhow!(
            "load groups {groups:?} from {}: {err:#}",
            pack.root().display()
        )
    })
}

/// Online path: real world login plus the live `REBUILD_NORMAL`/`PLAYER_INFO`
/// drain (see `session::login_and_drain`), then the initial world is built
/// via ensure+load over the LIVE groups (not `Pack::open`-only), with the
/// cached Lumbridge / synthetic fallbacks from the offline path.
///
/// Startup runtime note: a short-lived Tokio current-thread runtime drives
/// `login_and_drain` + the initial `ensure_and_load_world` here, BEFORE the
/// winit event loop exists, and is dropped before `ViewerApp` runs. The winit
/// thread itself never `block_on`s (per-frame network uses `try_read` /
/// `try_write` + `drain_pending_sync`; region loads run on the worker's own
/// runtime in [`spawn_prefetch_worker`]). Live interface frames from the
/// drain (the lobby 906 or world 1477 tree that the server's `LoginLayout.ts`
/// opens) are applied to a fresh `OpenInterfaces` + `VmAdapter` (real VM)
/// before the first frame.
pub(super) fn prepare_game_scene(
    game: &mut crate::client_game::ClientGame,
    pack: &Pack,
    assets: &mut WorldAssets,
) -> anyhow::Result<crate::entity_runtime::PreparedMap> {
    rs910_core::profile::scope!("scene prepare game");
    game.rebuild_started_ms
        .get_or_insert_with(crate::logic_clock::monotonic_millis);
    let prepared = rs910_core::profile::scope!("prepare map", game.runtime.prepare_map(pack))?;
    let world = &game
        .runtime
        .map_request
        .as_ref()
        .context("no requested world")?
        .world;
    let flo = assets
        .flo
        .as_ref()
        .context("live scene requires floor definitions")?;
    let materials = assets
        .material_store
        .as_ref()
        .context("live scene requires materials")?;
    let locs = config::LocStore::load(pack)?;
    // The asynchronously rebuilt world's loc type list.
    locs.allow_members.set(game.allow_members);
    let tables = crate::maploader::FloTables::from_store(flo);
    let prefs =
        crate::rebuild::BuildPrefs::from_options(&game.ui_variables.queries.preferences.options);
    let mut result =
        if let Some(layout) = &game.runtime.map_request.as_ref().unwrap().effects.region {
            crate::rebuild::rebuild_region_world(
                pack, &tables, materials, &locs, world, layout, &prefs,
            )?
        } else {
            crate::rebuild::rebuild_world(pack, &tables, materials, &locs, world, &prefs)?
        };
    rs910_core::profile::scope!("verify scene", game.verify_scene(&prepared, &result))?;
    game.capture_scene(
        result
            .scene_graph
            .as_ref()
            .context("missing normal scene graph")?,
    );
    let mut live = result
        .scene_graph
        .as_mut()
        .and_then(|scene| {
            crate::live_scene::LiveScene::new(
                scene,
                &result.scene.normal,
                result.flags.clone(),
                &result.env.lights,
            )
        })
        .context("live scene installation requires all floors and the graph")?;
    live.enable_dynamic_with_preferences(
        pack,
        &locs,
        result.model_cache.clone(),
        game.runtime.feed.state.varps.as_ref(),
        &prefs,
    )?;
    if let Some((level, x, _, z)) = game.local_position() {
        assets.spawn_x = (x as i32) >> 9;
        assets.spawn_z = (z as i32) >> 9;
        assets.focus_level = u8::try_from(level)?;
    }
    assets.floor_base = (result.base_x, result.base_z);
    assets.loc_sounds = Some(std::mem::take(&mut result.loc_sounds));
    assets.underwater_floor = result.scene.underwater.first().cloned().flatten();
    assets.underwater_models = result.underwater_models;
    assets.floors = result.scene.normal;
    assets.scene_graph = result.scene_graph;
    assets.live_scene = Some(live);
    assets.env = Some(result.env);
    assets.lights = result.lights;
    Ok(prepared)
}

/// The logged-out client owners: no local player, no scene, varps at their
/// defaults. Title and lobby CS2 run against this runtime exactly as the
/// client's statics exist before any login.
pub(super) fn title_game(pack: &Pack) -> anyhow::Result<crate::client_game::ClientGame> {
    title_game_with(pack, &rs910_config::login_configs::LoginConfigs::read(pack))
}

/// [`title_game`] decoding the config archives `configs` has read.
pub(super) fn title_game_with(
    pack: &Pack,
    configs: &rs910_config::login_configs::LoginConfigs,
) -> anyhow::Result<crate::client_game::ClientGame> {
    crate::client_game::ClientGame::login_with(
        pack,
        configs,
        0,
        crate::protocol910::live::Feed::default(),
        0,
        false,
    )
}

/// The runtime logout leaves behind: a fresh logged-out entity runtime that
/// keeps the process-lifetime client variables, after the transmit counters
/// reset. The new entity runtime restarts `varpTransmitNum`; the retained varc/varcstr/inv/stat/
/// varclan counters are reset explicitly.
pub(super) fn logged_out_game(
    pack: &Pack,
    previous: Option<crate::client_game::ClientGame>,
) -> anyhow::Result<crate::client_game::ClientGame> {
    let mut game = title_game(pack)?;
    if let Some(previous) = previous {
        game.ui_variables = previous.ui_variables;
    }
    game.ui_variables.reset_transmit_nums();
    Ok(game)
}

/// The defaults' `login_interface`/`lobby_interface` (defaults opcodes 5/6),
/// read when the login and lobby screens open.
pub(super) fn title_interfaces(pack: &Pack) -> anyhow::Result<(i32, i32)> {
    let scalars = crate::protocol910::pack_defaults::load(pack)?
        .graphics
        .scalars;
    Ok((scalars.login_interface, scalars.lobby_interface))
}

/// The JS5 content server: applet parameters 23 (host) and 12 (port). The
/// applet viewer gives them the lobby's host and port, so they follow `--host`/`--lobby-port` unless overridden with `--applet-param`.
pub(super) fn content_server(cli: &Cli) -> (String, u16) {
    let params = crate::applet_params::get();
    (
        params.content_host().unwrap_or_else(|| cli.host.clone()),
        params.content_port().unwrap_or(cli.lobby_port),
    )
}

/// The JS5 disk store directory (`--cache-dir`).
pub(super) fn js5_cache_dir(cli: &Cli) -> PathBuf {
    cli.cache_dir
        .clone()
        .or_else(|| crate::debug_flags::flags().cache_dir.clone())
        .unwrap_or_else(|| {
            cli.pack_root
                .parent()
                .unwrap_or(&cli.pack_root)
                .join("cache/client910")
        })
}

/// The JS5 owners for this launch: the content server (applet parameters
/// 23/12/30) and the HTTP content server (14/44).
pub(super) fn new_js5_system(cli: &Cli) -> crate::js5net::Js5System {
    let (host, port) = content_server(cli);
    let params = crate::applet_params::get();
    let overlay = crate::cache::disk_overlay()
        .unwrap_or_else(|| crate::cache::DiskOverlay::new(js5_cache_dir(cli)));
    let http = (
        params
            .http_content_host()
            .unwrap_or_else(|| cli.host.clone()),
        params.http_content_port().unwrap_or(80),
    );
    crate::js5net::Js5System::new(
        &cli.pack_root,
        overlay,
        crate::js5net::ContentAddress::new(host, port, params.content_port2().unwrap_or(port)),
        http,
        params.mode_game_id(),
        params.gamepack().unwrap_or_default(),
        params.language().ok().flatten().unwrap_or(0) as u8,
    )
}

/// What a new session owner starts from (the title/loading path, the
/// direct login and the headless replay).
pub(super) struct SessionStart {
    /// The world connection (`None` on the title screen).
    pub(super) stream: Option<crate::wire_stream::WireStream<std::net::TcpStream>>,
    pub(super) startup_connection: Option<crate::loading_connection::LoadingConnection>,
    pub(super) entities: crate::protocol910::live::Feed,
    pub(super) game: Option<crate::client_game::ClientGame>,
    pub(super) prepared_map: Option<crate::entity_runtime::PreparedMap>,
    pub(super) ui: crate::ui_runtime::Runtime,
    /// World bytes the login drain left unread.
    pub(super) pending: Vec<u8>,
    pub(super) initial_ui: Vec<crate::session::UiEvent>,
    /// The defaults' `login_interface` / `lobby_interface`.
    pub(super) login_interface: i32,
    pub(super) lobby_interface: i32,
}

/// Shared session owner construction for the title and direct-login paths.
pub(super) fn new_session(cli: &Cli, pack: &Pack, start: SessionStart) -> Session {
    let SessionStart {
        stream,
        startup_connection,
        entities,
        game,
        prepared_map,
        ui,
        pending,
        initial_ui,
        login_interface,
        lobby_interface,
    } = start;
    let (req_tx, req_rx) = mpsc::channel::<PrefetchRequest>();
    let (resp_tx, resp_rx) = mpsc::channel::<PrefetchResponse>();
    let (reconnect_resp_tx, reconnect_resp_rx) = mpsc::channel::<ReconnectResponse>();
    let (host_resolve_req_tx, host_resolve_req_rx) = mpsc::channel::<HostResolveRequest>();
    let (host_resolve_resp_tx, host_resolve_resp_rx) = mpsc::channel::<HostResolveResponse>();
    spawn_prefetch_worker(pack.clone(), req_rx, resp_tx);
    spawn_host_resolver(host_resolve_req_rx, host_resolve_resp_tx);
    // Applet parameters: the lobby / the startup world.
    let default_world = ServerAddress {
        node: 1,
        host: cli.host.clone(),
        port: cli.world_port,
        port2: cli.world_port,
        use_secondary_port: true,
        use_proxy: false,
    };
    Session {
        entities,
        strict_entities: cli.entity_state,
        game,
        prepared_map,
        io: SessionIo {
            startup_connection,
            idle_connection: Default::default(),
            lobby_idle_connection: Default::default(),
            incoming_idle: Default::default(),
            ping: Default::default(),
            world: Connection {
                stream,
                pending,
                pending_writes: Vec::new(),
                resync: Default::default(),
            },
            lobby: Connection::default(),
            net_stats: Default::default(),
        },
        machine: crate::login_state::Machine::default(),
        login_interface,
        lobby_interface,
        current_lobby: ServerAddress {
            node: 1,
            host: cli.host.clone(),
            port: cli.lobby_port,
            port2: cli.lobby_port,
            use_secondary_port: true,
            use_proxy: false,
        },
        current_world: default_world.clone(),
        default_world,
        previous_world: None,
        target_world: None,
        transfer_cancellable: false,
        world_port_base: cli.world_port.saturating_sub(1),
        auth: crate::net::AuthOptions::default(),
        server_commands: cli.server_commands.clone().into(),
        rebuild: crate::login_state::RebuildProgress::default(),
        rebuild_timer: None,
        scene_debug_mode: 0,
        scene_models_released: false,
        next_command_cycle: 100,
        groups: Vec::new(),
        username: cli.username.clone(),
        password: cli.password.clone(),
        pack_root: cli.pack_root.clone(),
        ui,
        initial_ui,
        prefetch_req_tx: req_tx,
        prefetch_resp_rx: resp_rx,
        prefetch_loading: None,
        polling_dead: false,
        prefetch_dead: false,
        reconnect_resp_tx,
        reconnect_resp_rx,
        reconnect_started: false,
        instance_id: allocate_session_instance(),
        reconnect_generation: 0,
        reconnect_cancel: None,
        login_progress: crate::net::LoginProgress::new(),
        login_crypto: cli.login_crypto(),
        sso: SsoState::default(),
        host_resolve_req_tx,
        host_resolve_resp_rx,
    }
}

pub(super) fn online_login(cli: &Cli, pack: &Pack) -> anyhow::Result<(WorldAssets, Session)> {
    crate::session_record::start_from_flags();
    let uid_path = uid192_path(&cli.pack_root);
    let uid192 = load_uid192(&uid_path);
    let runtime = current_thread_runtime().context("build tokio runtime for world login")?;
    let login_crypto = cli.login_crypto();
    let mut live = runtime
        // `--direct-login` honours `--world-port` like the lobby path does
        // (default world 1 = `43594 + 1`).
        .block_on(crate::session::login_world_and_drain(
            &crate::net::LoginParams {
                uid192,
                // The login reports an all-unknown machine: the probe stays local.
                hardware: rs910_core::hardware::Hardware::default(),
                launcher: launcher_report_with(
                    crate::applet_params::get().user_flow(),
                    crate::applet_params::get().automated_test_flags(),
                ),
                crypto: login_crypto.clone(),
                ..crate::net::LoginParams::new(
                    &cli.host,
                    cli.world_port,
                    &cli.username,
                    &cli.password,
                )
            },
            cli.entity_state,
        ))
        .map_err(|err| anyhow::anyhow!("world login failed: {err:#}"))?;
    record_session_header(cli, &live, &uid192);
    let profile = live.login.profile.clone();
    let stream = detach_world_socket(live.stream)?;
    let startup_connection = crate::loading_connection::LoadingConnection::start(&stream)?;
    match &live.drain.rebuild {
        Some(rebuild) => {
            log::info!(
                "[client910] live REBUILD_NORMAL: zone=({},{}) npc_bits={} count={} build_area={} force={} high_res_block={}",
                rebuild.zone_x,
                rebuild.zone_z,
                rebuild.npc_bits,
                rebuild.map_count,
                rebuild.build_area_id,
                rebuild.force,
                rebuild.has_high_res_block,
            );
            let groups = crate::session::rebuild_to_groups(rebuild);
            if crate::session::is_cached_lumbridge_block(&groups) {
                log::info!(
                    "[client910] live groups match cached 3x3 Lumbridge block (base {},{}); rendering LIVE-validated coords",
                    map::LUMBRIDGE_BASE.0,
                    map::LUMBRIDGE_BASE.1,
                );
            } else {
                log::info!(
                    "[client910] live groups {groups:?} fall outside the cached 3x3 block; ensure+load will fetch them"
                );
            }
        }
        None => {
            log::info!(
                "[client910] no live REBUILD_NORMAL within the drain timeout; rendering cached terrain as fallback"
            );
        }
    }
    if live.drain.player_seen {
        log::info!(
            "[client910] live PLAYER_INFO seen ({} bytes retained in ordered CPU feed; context adapter pending)",
            live.drain.player_info_bytes
        );
    } else {
        log::info!("[client910] no live PLAYER_INFO within the drain timeout");
    }
    log::info!(
        "[client910] world login ok: pid={:?} server_token={}",
        live.login.pid,
        live.login.server_token
    );
    if !live.drain.ui_events.is_empty() {
        log::info!(
            "[client910] live drain collected {} interface/script frames",
            live.drain.ui_events.len()
        );
    }
    // Initial world: live groups when the server sent a rebuild, else the
    // cached Lumbridge block (same fallback the old code rendered).
    let want_groups: Vec<u16> = live
        .drain
        .rebuild
        .as_ref()
        .map(crate::session::rebuild_to_groups)
        .unwrap_or_else(lumbridge_groups_u16);
    let (spawn_x, spawn_z) = live
        .drain
        .rebuild
        .as_ref()
        .map(spawn_for_rebuild)
        .unwrap_or((SPAWN_X, SPAWN_Z));
    let (mut assets, mut rendered_groups) = match ensure_and_load_world(cli, pack, &want_groups) {
        Ok(world) => {
            let rendered = world.groups.clone();
            match build_assets_from_world(pack, spawn_x, spawn_z) {
                Ok(assets) => {
                    log::info!(
                        "[client910] live world: base {},{} extent {} spawn {spawn_x},{spawn_z}",
                        world.base_x,
                        world.base_z,
                        world.extent,
                    );
                    (assets, rendered)
                }
                Err(err) => {
                    log::warn!(
                        "[client910] live asset build failed ({err:#}); falling back to cached Lumbridge"
                    );
                    match load_world_assets(pack) {
                        Ok(assets) => (assets, lumbridge_groups_u16()),
                        Err(err) => {
                            log::warn!(
                                "[client910] {err:#}; falling back to synthetic heightfield"
                            );
                            (WorldAssets::synthetic()?, want_groups)
                        }
                    }
                }
            }
        }
        Err(err) => {
            log::warn!("[client910] {err:#}; falling back to cached Lumbridge");
            match load_world_assets(pack) {
                Ok(assets) => (assets, lumbridge_groups_u16()),
                Err(err) => {
                    log::warn!("[client910] {err:#}; falling back to synthetic heightfield");
                    (WorldAssets::synthetic()?, want_groups)
                }
            }
        }
    };
    // One read of the config archives for the game owner and the retained
    // interface.
    let configs = rs910_config::login_configs::LoginConfigs::read(pack);
    let mut game = if cli.entity_state {
        let local = usize::from(
            live.login
                .pid
                .context("world login omitted local player index")?,
        );
        Some(crate::client_game::ClientGame::login_with(
            pack,
            &configs,
            local,
            std::mem::take(&mut live.drain.entities),
            live.login.server_token as u64,
            live.login.profile.logged_in_members,
        )?)
    } else {
        None
    };
    if let Some(game) = &mut game {
        game.ui_variables
            .queries
            .preferences
            .apply_hardware(rs910_core::hardware::probe());
        install_client_persistence(game, &cli.pack_root)?;
        restore_server_varcs(game, &live.login.server_varcs)?;
    }
    let prepared_map = if let Some(game) = &mut game {
        game.apply_next(crate::logic_clock::monotonic_millis())
            .map_err(|e| anyhow::anyhow!("initial entity packet: {e:?}"))?;
        let prepared = prepare_game_scene(game, pack, &mut assets)?;
        rendered_groups = game
            .runtime
            .map_request
            .as_ref()
            .context("missing initial map request")?
            .world
            .groups
            .iter()
            .take(game.runtime.map_request.as_ref().unwrap().world.group_count)
            .map(|&id| u16::try_from(id))
            .collect::<Result<Vec<_>, _>>()?;
        Some(prepared)
    } else {
        None
    };
    anyhow::ensure!(
        game.is_some(),
        "the retained interface runtime requires --entity-state=true"
    );
    let ui = retained_session_ui(pack, configs, &profile, uid192, game.as_mut().unwrap())?;
    // Deregister from Tokio while its reactor still exists. Keeping a Tokio
    // stream after dropping the startup runtime leaves try_read/try_write's
    // readiness flags stale after the first WouldBlock. Std nonblocking I/O
    // polls the OS socket directly on the winit logic cadence.
    drop(runtime);
    let (login_interface, lobby_interface) = title_interfaces(pack)?;
    let mut session = new_session(
        cli,
        pack,
        SessionStart {
            stream: Some(stream),
            startup_connection: Some(startup_connection),
            entities: live.drain.entities,
            game,
            prepared_map,
            ui,
            pending: live.drain.pending,
            initial_ui: live.drain.ui_events,
            login_interface,
            lobby_interface,
        },
    );
    session.groups = rendered_groups;
    // The seeds this login drew are the ones a reconnect repeats.
    session.login_crypto = login_crypto;
    // `--direct-login` is the title-screen `login_request` path
    // (state 7 -> 18) with the credentials supplied on the command line.
    session.machine.state = crate::login_state::GAME;
    Ok((assets, session))
}

/// Render-ready world: the faithful scene rebuild output (floors,
/// scene graph, lights, environment) around the spawn/rebuild tile, uploaded
/// after the GPU context exists.
pub(super) struct WorldAssets {
    /// Camera spawn tile this build is centred on (hardcoded `SPAWN_X/Z` for
    /// the cached world, the rebuild zone centre for live worlds).
    pub(super) spawn_x: i32,
    /// Camera spawn Z (see [`WorldAssets::spawn_x`]).
    pub(super) spawn_z: i32,
    /// Player/camera-target level (the current player level).
    pub(super) focus_level: u8,
    /// Floor configs (`None` without a pack); preference rebuilds reuse them.
    pub(super) flo: Option<FloStore>,
    /// Faithful floors per level (`rebuild::rebuild_normal`).
    pub(super) floors: Vec<Option<crate::floor::FloorGeometry>>,
    /// The underwater level height map 0 (`waterDetail == 2` rebuilds).
    pub(super) underwater_floor: Option<crate::floor::FloorGeometry>,
    /// The underwater scene's loc models (the underwater loc map file).
    pub(super) underwater_models: Vec<crate::rebuild::UnderwaterModel>,
    /// Materials for floor batch textures (`None` when the archive is missing).
    pub(super) material_store: Option<crate::texture::MaterialStore>,
    /// Pack root the floor textures load from (`None` without a pack).
    pub(super) pack_root: Option<PathBuf>,
    /// `sceneBaseTile` of `floors` (absolute tile of scene-local 0,0).
    pub(super) floor_base: (i32, i32),
    /// The placed scene graph.
    pub(super) scene_graph: Option<crate::scene::Scene>,
    pub(super) live_scene: Option<crate::live_scene::LiveScene>,
    /// The environment map / lights of the window.
    pub(super) env: Option<crate::env::EnvState>,
    /// Baked static lights per level (the static lighting build).
    pub(super) lights: Vec<Vec<crate::floorlight::BakedLight>>,
    /// The map load's positional sound registrations, handed to the audio owner on install.
    pub(super) loc_sounds: Option<crate::positioned_sound::LocSoundScene>,
}

impl WorldAssets {
    /// No scene: the title/lobby placeholder (never presented) and the
    /// pack-free fallback, which draws the sky/background only.
    pub(super) fn synthetic() -> anyhow::Result<Self> {
        Ok(Self {
            spawn_x: SPAWN_X,
            spawn_z: SPAWN_Z,
            focus_level: SPAWN_LEVEL as u8,
            flo: None,
            floors: (0..NUM_LEVELS).map(|_| None).collect(),
            underwater_floor: None,
            underwater_models: Vec::new(),
            material_store: None,
            pack_root: None,
            floor_base: (0, 0),
            scene_graph: None,
            live_scene: None,
            env: None,
            lights: Vec::new(),
            loc_sounds: None,
        })
    }
    /// Camera target height (tiles, y up) at the spawn tile over the
    /// faithful floor of the focus level (the heightmap lookup).
    pub(super) fn spawn_ground(&self) -> f32 {
        let level = usize::from(self.focus_level).min(3);
        self.floors
            .get(level)
            .and_then(Option::as_ref)
            .map_or(0.0, |floor| {
                let x = (self.spawn_x - self.floor_base.0) * 512 + 256;
                let z = (self.spawn_z - self.floor_base.1) * 512 + 256;
                -(floor.heights.get_fine_height(x, z) as f32) / 512.0
            })
    }
}

/// Cached world load around the Lumbridge spawn.
///
/// `--offline` comes through here; the online path uses
/// [`ensure_and_load_world`] + [`build_assets_from_world`] for the live
/// groups and only falls back here when that fails. The Lumbridge map groups
/// must be in the pack; use [`WorldAssets::synthetic`] when they are not.
pub(super) fn load_world_assets(pack: &Pack) -> anyhow::Result<WorldAssets> {
    map::load_lumbridge(pack)
        .map_err(|err| anyhow::anyhow!("load Lumbridge from {}: {err:#}", pack.root().display()))?;
    build_assets_from_world(pack, SPAWN_X, SPAWN_Z)
}

/// The faithful scene for the map around (`spawn_x`, `spawn_z`): loc/flo/
/// material configs, then `rebuild::rebuild_normal` (floors, scene graph,
/// lights, environment) and the live scene owner over it. Shared by the
/// cached Lumbridge load and every live reload; a missing config archive or a
/// failed build leaves the scene empty rather than blocking the frame.
pub(super) fn build_assets_from_world(
    pack: &Pack,
    spawn_x: i32,
    spawn_z: i32,
) -> anyhow::Result<WorldAssets> {
    rs910_core::profile::scope!("scene build assets");
    let loc_store = match config::LocStore::load(pack) {
        Ok(store) => {
            log::info!("[client910] loc configs: {}", store.len());
            Some(store)
        }
        Err(err) => {
            log::warn!("[client910] loc configs unavailable ({err:#}); scene built without locs");
            None
        }
    };
    // Floor configs for the faithful floor build (`FloTables`).
    let flo: Option<FloStore> = match FloStore::load(pack) {
        Ok(store) => {
            log::info!(
                "[client910] flo: {} overlays / {} underlays",
                store.overlay_count(),
                store.underlay_count(),
            );
            Some(store)
        }
        Err(err) => {
            log::warn!("[client910] flo unavailable ({err:#}); no floors built");
            None
        }
    };
    // Materials for the floor/model batches.
    let material_store: Option<crate::texture::MaterialStore> =
        match crate::texture::MaterialStore::load(pack) {
            Ok(store) => {
                let present = store.iter().count();
                log::info!(
                    "[client910] materials: {present} present / {} slots",
                    store.len()
                );
                Some(store)
            }
            Err(err) => {
                log::warn!("[client910] materials unavailable ({err:#}); texture cache empty");
                None
            }
        };
    // Faithful floors (phase B): the scene rebuild floor slice over
    // the standard 104x104 window centred on the spawn/rebuild tile.
    let (floors, floor_base, scene_graph, live_scene, env, lights) = match (&flo, &material_store) {
        (Some(flo_store), Some(materials)) => {
            let tables = crate::maploader::FloTables::from_store(flo_store);
            let prefs = crate::rebuild::BuildPrefs::default();
            match crate::rebuild::rebuild_normal(
                pack,
                &tables,
                materials,
                loc_store.as_ref(),
                spawn_x,
                spawn_z,
                &prefs,
            ) {
                Ok(mut result) => {
                    if let Err(err) = crate::rebuild::report(&result, None) {
                        log::warn!("[client910] floor report failed: {err:#}");
                    }
                    let base = (result.base_x, result.base_z);
                    let mut live = result.scene_graph.as_mut().and_then(|s| {
                        crate::live_scene::LiveScene::new(
                            s,
                            &result.scene.normal,
                            result.flags,
                            &result.env.lights,
                        )
                    });
                    if let (Some(live), Some(locs)) = (&mut live, &loc_store) {
                        live.enable_dynamic(pack, locs, result.model_cache.clone())?;
                    }
                    (
                        result.scene.normal,
                        base,
                        result.scene_graph,
                        live,
                        Some(result.env),
                        result.lights,
                    )
                }
                Err(err) => {
                    log::warn!("[client910] faithful floor build failed ({err:#}); no scene drawn");
                    (
                        (0..NUM_LEVELS).map(|_| None).collect(),
                        (0, 0),
                        None,
                        None,
                        None,
                        Vec::new(),
                    )
                }
            }
        }
        _ => (
            (0..NUM_LEVELS).map(|_| None).collect(),
            (0, 0),
            None,
            None,
            None,
            Vec::new(),
        ),
    };

    Ok(WorldAssets {
        spawn_x,
        spawn_z,
        focus_level: SPAWN_LEVEL as u8,
        flo,
        floors,
        underwater_floor: None,
        underwater_models: Vec::new(),
        material_store,
        pack_root: Some(pack.root().to_path_buf()),
        floor_base,
        scene_graph,
        live_scene,
        env,
        lights,
        loc_sounds: None,
    })
}
