//! The session owners' logic shared by the windowed shell (`ViewerApp`)
//! and the headless client: the app `Session` state, the live read and
//! packet drain, the logic update (game update and interface update), the per-cycle session inputs and the retained-UI
//! input halves (moved from `client910::app::session_core`, Phase 5).
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

const FIRST_SESSION_INSTANCE: u64 = 1;
const SESSION_INSTANCE_INCREMENT: u64 = 1;
static NEXT_SESSION_INSTANCE: AtomicU64 = AtomicU64::new(FIRST_SESSION_INSTANCE);

/// Process-local identity for observations; independent of packet and game state.
pub fn allocate_session_instance() -> u64 {
    NEXT_SESSION_INSTANCE.fetch_add(SESSION_INSTANCE_INCREMENT, Ordering::Relaxed)
}

/// Set the overhead chat line of the players named by `MESSAGE_PUBLIC`, with
/// the timeout `logic_rate * playerChatTimeout` (the graphics default).
pub fn apply_overhead_chat(
    game: &mut crate::client_game::ClientGame,
    requests: &mut Vec<crate::ui_runtime::OverheadChat>,
) {
    if requests.is_empty() {
        return;
    }
    let ttl = game
        .inputs
        .appearance
        .defaults
        .graphics
        .entity_limits(game.inputs.logic_rate)
        .player_chat_ticks;
    for request in requests.drain(..) {
        let Some(player) = game
            .runtime
            .feed
            .state
            .players
            .players
            .get_mut(request.player)
            .and_then(Option::as_mut)
        else {
            continue;
        };
        if crate::ui_debug_flags::flags().chat_trace {
            log::info!(
                "[chat-trace] overhead player={} text={:?} colour={} effect={} ttl={ttl}",
                request.player,
                request.text,
                request.colour,
                request.effect
            );
        }
        player.chat = Some(crate::entities910::chat::Chat {
            text: Some(request.text),
            colour: request.colour,
            effect: request.effect,
            total: ttl,
            time: ttl,
        });
    }
}

/// Live world session: kept open in [`ViewerApp`] so the render loop can poll
/// `REBUILD_NORMAL` frames on it once per frame (see
/// [`ViewerApp::poll_live_rebuild`]). `groups` is the currently rendered 3×3
/// block (dedup baseline for [`crate::session::should_reload`]).
/// No Tokio runtime lives here: the winit thread never `block_on`s. Startup
/// (`online_login`) uses a short-lived runtime before the event loop and drops
/// it; the prefetch worker owns its own current-thread runtime (see
/// [`spawn_prefetch_worker`]). Per-frame network uses non-blocking
/// `try_read`/`try_write` + [`crate::session::drain_pending_sync`], and
/// region map groups arrive through the app's JS5 client before the worker
/// loads them.
pub struct Session {
    /// Distinguishes a replacement session from a reconnect of this same owner.
    pub instance_id: u64,
    /// The world and lobby connections (`SessionIo`, app/session_io.rs).
    pub io: SessionIo,
    /// CPU state and original ordered entity packets; independent of graphics.
    pub entities: crate::protocol910::live::Feed,
    pub strict_entities: bool,
    pub game: Option<crate::client_game::ClientGame>,
    pub prepared_map: Option<crate::entity_runtime::PreparedMap>,
    /// The client state and logged-in owner.
    pub machine: crate::login_state::Machine,
    /// The login and lobby interfaces (graphics defaults
    /// opcodes 5/6) read by `showLogin`/`showLobby`.
    pub login_interface: i32,
    pub lobby_interface: i32,
    /// The current lobby: startup lobby, updated by `CHANGE_LOBBY`.
    pub current_lobby: ServerAddress,
    /// The world the game login connects to.
    pub current_world: ServerAddress,
    /// The startup world.
    pub default_world: ServerAddress,
    /// The previous world, saved by `LOGOUT_TRANSFER`.
    pub previous_world: Option<ServerAddress>,
    /// The target world from the lobby login profile.
    pub target_world: Option<ServerAddress>,
    /// Whether the transfer can be cancelled, from `LOGOUT_TRANSFER`.
    pub transfer_cancellable: bool,
    /// Development server ports: world `id` listens on
    /// `world_port_base + id` (43594 + id by default; `--world-port` moves it).
    pub world_port_base: u16,
    /// Auth options retained for the next game or lobby login (the new auth
    /// preference and the don't-trust flag).
    pub auth: crate::net::AuthOptions,
    pub server_commands: std::collections::VecDeque<String>,
    pub next_command_cycle: i32,
    /// The rebuild stage and its progress counters.
    pub rebuild: crate::login_state::RebuildProgress,
    /// Set by `EXECUTE_CLIENT_CHEAT` 17: time the next rebuild and print it.
    pub rebuild_timer: Option<i64>,
    /// The scene debug mode, set by `EXECUTE_CLIENT_CHEAT` 1/3/15 and handed
    /// to the scene's debug-mode and debug-font hooks on every rebuild, both
    /// of which are empty in the 910 client.
    pub scene_debug_mode: i32,
    /// Whether the scene models have been released.
    pub scene_models_released: bool,
    /// Currently rendered groups (dedup baseline).
    pub groups: Vec<u16>,
    /// Credentials retained by the session owner for the post-transfer
    /// login state. These are the same CLI credentials used for startup.
    pub username: String,
    pub password: String,
    /// Cache pack root (`Pack::open` loads through the JS5 disk store).
    pub pack_root: PathBuf,
    /// Exactly one interface owner receives packets in this session.
    pub ui: crate::ui_runtime::Runtime,
    /// Startup stops at the initial rebuild; hooks wait for the real canvas.
    pub initial_ui: Vec<crate::session::UiEvent>,
    /// Prefetch requests to the background worker (non-blocking `send`).
    pub prefetch_req_tx: Sender<PrefetchRequest>,
    /// Prefetch responses from the worker (non-blocking `try_recv` per frame).
    pub prefetch_resp_rx: Receiver<PrefetchResponse>,
    /// In-flight region load, if any (for the window title progress).
    pub prefetch_loading: Option<PrefetchLoading>,
    /// Set on IO error to stop polling (keeps rendering; avoids per-frame
    /// error spam after a disconnect).
    pub polling_dead: bool,
    /// Set when the worker channel disconnects (keeps rendering).
    pub prefetch_dead: bool,
    /// Result channel for the non-blocking world-transfer login worker.
    pub reconnect_resp_tx: Sender<ReconnectResponse>,
    pub reconnect_resp_rx: Receiver<ReconnectResponse>,
    /// Guard against starting more than one reconnect attempt for a transfer.
    pub reconnect_started: bool,
    /// Monotonic owner generation. A canceled worker may still finish its
    /// socket future; responses from older generations are discarded.
    pub reconnect_generation: u64,
    /// Cancellation flag shared with the current worker.
    pub reconnect_cancel: Option<Arc<AtomicBool>>,
    /// What the active login worker reports (queue position, interim reply,
    /// pages to open, a social sign-on's key) and is handed in return.
    pub login_progress: Arc<crate::net::LoginProgress>,
    /// How the session's login blocks are protected (and the seeds a
    /// reconnect repeats).
    pub login_crypto: crate::login_crypto::LoginCrypto,
    /// The social sign-on the session's logins present.
    pub sso: SsoState,
    pub host_resolve_req_tx: Sender<HostResolveRequest>,
    pub host_resolve_resp_rx: Receiver<HostResolveResponse>,
}

impl Session {
    /// The predicate inputs for [`crate::login_state::Machine`].
    pub fn login_context(&self) -> crate::login_state::Context {
        crate::login_state::Context {
            top: self.ui.state.life.top,
            login_interface: self.login_interface,
            lobby_interface: self.lobby_interface,
            credentials: self.sso.network.is_some()
                || (!self.username.is_empty() && !self.password.is_empty()),
            lobby_connected: self.io.lobby.stream.is_some(),
        }
    }

    /// Switch to a world, plus the retained
    /// `currentWorld` id the world-list CS2 queries read.
    pub fn set_world(&mut self, world: ServerAddress) {
        {
            let ui = &mut self.ui;
            ui.engine.login.world = world.node;
        }
        self.current_world = world;
    }
}

/// A server address (the current lobby, current world and target world).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    pub node: i32,
    pub host: String,
    pub port: u16,
    pub port2: u16,
    /// Use the secondary port (default true) and use a proxy (default false).
    pub use_secondary_port: bool,
    pub use_proxy: bool,
}

impl ServerAddress {
    /// The port the socket connects to.
    /// `useProxy` selects the proxy socket, which falls back
    /// to a direct socket without a system proxy; this port has no proxy
    /// discovery, so both connect directly.
    pub fn socket_port(&self) -> u16 {
        if self.use_secondary_port {
            self.port2
        } else {
            self.port
        }
    }
    /// Switch to the secondary port on the next connect.
    pub fn configure_socket_type(&mut self) {
        if !self.use_secondary_port {
            self.use_secondary_port = true;
            self.use_proxy = true;
        } else if self.use_proxy {
            self.use_proxy = false;
        } else {
            self.use_secondary_port = false;
        }
    }
}

/// The social sign-on a session's logins present: the network the player
/// picked, and the key and name word the server gave the sign-on. The key is
/// kept for the session's later logins (lobby, world, reconnect) and dropped
/// when the player logs in another way or picks another network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SsoState {
    pub network: Option<i32>,
    pub social_key: Option<i64>,
    pub social_name: i64,
}

impl SsoState {
    /// A login by username and password: no social sign-on.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The player picked `network`; a key from another network is dropped.
    pub fn choose(&mut self, network: i32) {
        if self.network != Some(network) {
            self.social_key = None;
        }
        self.network = Some(network);
    }

    /// The login parameters of the current sign-on, if there is one.
    #[must_use]
    pub fn login(&self) -> Option<crate::net::SsoLogin> {
        self.network.map(|network| crate::net::SsoLogin {
            network,
            social_key: self.social_key,
            social_name: self.social_name,
        })
    }

    /// Keep the key and name word a login negotiated.
    pub fn learn(&mut self, key: i64, name: i64) {
        self.social_key = Some(key);
        self.social_name = name;
    }
}

pub fn begin_reconnect_attempt(session: &mut Session) -> (u64, Arc<AtomicBool>) {
    session.reconnect_generation = session.reconnect_generation.wrapping_add(1);
    session.login_progress.reset();
    let cancel = Arc::new(AtomicBool::new(false));
    session.reconnect_cancel = Some(cancel.clone());
    (session.reconnect_generation, cancel)
}

/// One server packet to the session's retained interface runtime, with the
/// game-side work the read does around it: `CUTSCENE` starts the
/// cutscene, `LOGOUT` resets it, `MESSAGE_PUBLIC` sets overhead
/// chat and `UPDATE_UID192` is persisted.
pub fn ui_packet(
    ui: &mut crate::ui_runtime::Runtime,
    pack_root: &Path,
    mut game: Option<&mut crate::client_game::ClientGame>,
    event: &crate::session::UiEvent,
) -> anyhow::Result<()> {
    if let (
        Some(game),
        crate::session::UiEvent::Cutscene {
            id,
            parameter,
            appearance,
        },
    ) = (game.as_deref_mut(), event)
    {
        game.begin_cutscene(*id, *parameter, appearance);
    }
    // A logout resets the cutscene.
    if let (Some(game), crate::session::UiEvent::Logout { .. }) = (game.as_deref_mut(), event) {
        game.cutscene.reset_cutscene();
    }
    let result = {
        let game = game.context("retained UI needs game runtime")?;
        let result = crate::client_game::with_game(game, |v| ui.packet(v, event));
        apply_overhead_chat(game, &mut ui.engine.effects.overhead_chat);
        result
    };
    if result.is_ok() {
        if let crate::session::UiEvent::Uid192 { value: Some(value) } = event {
            store_uid192(&uid192_path(pack_root), value)?;
        }
    }
    result
}

/// Drain the player-info chat masks' chat-history requests (type 2) into the
/// retained chat history, using the name with extras / the plain name / `name`.
pub fn drain_entity_chat_history(
    game: &mut crate::client_game::ClientGame,
    chat: &mut crate::ui_chat::ChatHistory,
) -> bool {
    let mut changed = false;
    for player in game.runtime.feed.state.players.players.iter_mut().flatten() {
        if player.chat_history.is_empty() {
            continue;
        }
        let name = player.appearance.name.clone().unwrap_or_default();
        let titled = player
            .appearance
            .title
            .as_ref()
            .map_or_else(|| name.clone(), |title| title.replace("<name>", &name));
        for request in std::mem::take(&mut player.chat_history) {
            chat.add_message(rs910_ui::ui_chat::NewChatLine {
                flags: i32::from(request.flags),
                name: titled.clone(),
                name_unfiltered: name.clone(),
                name_simple: name.clone(),
                ..rs910_ui::ui_chat::NewChatLine::system(2, request.text)
            });
            changed = true;
        }
    }
    changed
}

/// Leave fullscreen when a queued URL asked for it (`URL_OPEN`,
/// `openurl_nologin`), then open each URL.
pub fn open_browser_urls(
    (urls, exit_fullscreen): (Vec<crate::session::UiEvent>, bool),
    game: Option<&mut crate::client_game::ClientGame>,
) {
    if exit_fullscreen {
        if let Some(game) = game {
            let p = &mut game.ui_variables.queries.preferences;
            match p.window.leave_fullscreen(&p.options) {
                Ok(true) => p.window.changed = true,
                Ok(false) => {}
                Err(error) => log::warn!("[client910] URL fullscreen exit: {error:?}"),
            }
        }
    }
    for url in &urls {
        open_browser_url(url);
    }
}

pub fn open_browser_url(event: &crate::session::UiEvent) {
    let target = match event {
        crate::session::UiEvent::UrlOpen {
            primary,
            fallback,
            javascript,
        } => {
            let valid = |url: &str| url.starts_with("http://") || url.starts_with("https://");
            let target = if *javascript {
                fallback
                    .as_deref()
                    .filter(|url| valid(url))
                    .unwrap_or(primary.as_str())
            } else {
                primary.as_str()
            };
            target
        }
        crate::session::UiEvent::SocialNetworkLogout { url } => url.as_str(),
        _ => return,
    };
    let valid = |url: &str| url.starts_with("http://") || url.starts_with("https://");
    if !valid(target) {
        log::warn!("[client910] URL_OPEN rejected non-http target");
        return;
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", target])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(error) = result {
        log::warn!("[client910] URL_OPEN launch failed: {error}");
    }
}

/// One background region load: the `REBUILD_NORMAL`-derived 3×3 block the
/// worker is ensuring + loading.
#[derive(Debug, Clone)]
pub struct PrefetchRequest {
    /// Full rebuild event (groups drive ensure/load, rebuild drives spawn).
    pub event: crate::session::RebuildEvent,
}

/// Progress of one [`PrefetchRequest`] (sent after each group ensure).
#[derive(Debug, Clone)]
pub struct PrefetchProgress {
    /// Requested groups (echoed for correlation).
    pub groups: Vec<u16>,
    /// Groups ensured so far (including failures, which fall back to cache).
    pub loaded: usize,
    /// Total groups requested.
    pub total: usize,
}

/// Worker → winit responses (all non-blocking via `try_recv`).
#[derive(Debug)]
pub enum PrefetchResponse {
    /// One group ensure finished (`loaded`/`total` for the title).
    Progress(PrefetchProgress),
    /// Ensure + `load_groups` finished: the merged [`crate::map::World`].
    /// `app.rs` builds `WorldAssets` on the winit thread (CPU-only, no
    /// network) and swaps via [`ViewerApp::apply_reloaded_assets`].
    Done {
        /// Echoed rebuild event (spawn + dedup baseline).
        event: crate::session::RebuildEvent,
        /// Merged world for the event groups.
        #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
        world: crate::map::World,
    },
    /// `load_groups` failed after ensures (cached pack decides success).
    Error {
        /// Requested groups.
        groups: Vec<u16>,
        /// Human-readable failure.
        message: String,
    },
}

/// In-flight prefetch for the title (`loading 3/9 …`).
#[derive(Debug, Clone)]
pub struct PrefetchLoading {
    /// Requested groups.
    pub groups: Vec<u16>,
    /// Latest progress (`loaded`/`total`).
    pub loaded: usize,
    /// See [`PrefetchLoading::loaded`].
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct HostResolveRequest {
    pub world_id: i32,
    pub hostname: String,
}

#[derive(Debug, Clone)]
pub struct HostResolveResponse {
    pub world_id: i32,
    pub hostpacked: i32,
}

/// Resolve one world hostname away from the UI thread, with a single
/// outstanding lookup owner. Failed lookups retain the `-1` value and are
/// retried by the world-list owner later.
pub fn spawn_host_resolver(
    req_rx: Receiver<HostResolveRequest>,
    resp_tx: Sender<HostResolveResponse>,
) {
    let spawned = thread::Builder::new()
        .name("client910-world-host-resolver".to_string())
        .spawn(move || {
            while let Ok(request) = req_rx.recv() {
                let hostpacked = (request.hostname.as_str(), 0)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|addresses| {
                        addresses.into_iter().find_map(|address| match address {
                            std::net::SocketAddr::V4(address) => {
                                Some(i32::from_be_bytes(address.ip().octets()))
                            }
                            std::net::SocketAddr::V6(_) => None,
                        })
                    })
                    .unwrap_or(-1);
                let _ = resp_tx.send(HostResolveResponse {
                    world_id: request.world_id,
                    hostpacked,
                });
            }
        });
    if let Err(error) = spawned {
        log::warn!("[client910] world host resolver spawn failed: {error}");
    }
}

/// Spawn the background map-load worker (P2).
///
/// Design (thread/channel/message types):
/// - One `std::thread` (named `client910-prefetch`) runs
///   `map::load_groups` off the winit thread. Group downloads are not its
///   job: the rebuild waits until every map square's group is ready first
///   ([`ViewerApp::poll_js5_map_wait`]), so the
///   groups are in the JS5 disk store when the request arrives.
/// - `req_rx: Receiver<PrefetchRequest>` (winit → worker).
/// - `resp_tx: Sender<PrefetchResponse>` (worker → winit): `Progress`, then
///   `Done {event,world}` or `Error`.
/// - The winit thread polls `try_recv` once per frame (never blocks; see
///   [`ViewerApp::poll_prefetch_responses`]). `--offline` never spawns this.
pub fn spawn_prefetch_worker(
    pack: Pack,
    req_rx: Receiver<PrefetchRequest>,
    resp_tx: Sender<PrefetchResponse>,
) {
    let spawned = thread::Builder::new()
        .name("client910-prefetch".to_string())
        .spawn(move || {
            while let Ok(request) = req_rx.recv() {
                let groups = request.event.groups.clone();
                let total = groups.len();
                let _ = resp_tx.send(PrefetchResponse::Progress(PrefetchProgress {
                    groups: groups.clone(),
                    loaded: total,
                    total,
                }));
                match crate::map::load_groups(&pack, &groups) {
                    Ok(world) => {
                        let _ = resp_tx.send(PrefetchResponse::Done {
                            event: request.event,
                            world,
                        });
                    }
                    Err(err) => {
                        let _ = resp_tx.send(PrefetchResponse::Error {
                            groups,
                            message: format!("{err:#}"),
                        });
                    }
                }
            }
        });
    if let Err(err) = spawned {
        // TODO(#gap-12): surface worker-spawn failure to the caller as
        // `anyhow::Result` instead of degrading to no background loads
        // (needs `online_login` signature work). No `unwrap`/`expect` here:
        // without a worker the render loop keeps the initial terrain and
        // every prefetch request logs on send (never silent).
        log::warn!(
            "[client910] prefetch worker spawn failed ({err}); live reload keeps cached terrain"
        );
    }
}

/// Spawn tile for a rebuild: the centre of the 8×8-tile zone
/// (`zone = abs >> 3`, so `abs = zone * 8 + sub`). The sub-zone offset is not
/// on the wire, so the zone centre keeps the camera inside the new 3×3 block.
/// Lumbridge spawn zone (402, 402) maps to (3220, 3222-adjacent): within two
/// tiles of the hardcoded spawn, i.e. the spawn path stays visually identical.
pub fn spawn_for_rebuild(rebuild: &crate::session::Rebuild) -> (i32, i32) {
    (
        i32::from(rebuild.zone_x) * 8 + 4,
        i32::from(rebuild.zone_z) * 8 + 4,
    )
}

/// Cached Lumbridge block as `u16` group ids (same set as
/// [`map::load_lumbridge`], which delegates to `load_groups` with these).
pub fn lumbridge_groups_u16() -> Vec<u16> {
    crate::map::LUMBRIDGE_GROUPS
        .iter()
        .map(|group| *group as u16)
        .collect()
}

/// `CLIENT910_OUT_TRACE=1`: print each client packet the retained UI queued
/// (opcode name, payload hex) as it joins the game connection's write queue.
pub fn trace_outgoing(bytes: &[u8]) {
    if bytes.is_empty() || !crate::client_debug_flags::flags().out_trace {
        return;
    }
    let now = crate::logic_clock::monotonic_millis();
    match crate::client_watch::frames(bytes) {
        Some(frames) => {
            for (opcode, payload) in frames {
                let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
                log::info!(
                    "[out] t={now} {} ({opcode}) len={} {hex}",
                    crate::proto::client::name(opcode),
                    payload.len()
                );
            }
        }
        None => log::info!("[out] t={now} unframed {} bytes", bytes.len()),
    }
}

/// One non-blocking live poll over `session` (winit thread, no runtime).
///
/// Flushes queued replies with `try_write`, fills `pending` with `try_read`
/// (both return immediately on `WouldBlock`), then drains complete frames
/// with [`crate::session::drain_pending_sync`]. Returns any `REBUILD_NORMAL`
/// plus all interface / script frames seen. `Err` means the connection failed
/// (caller stops polling); `Ok((None, []))` with no data ready is the normal
/// idle case and must return immediately (P2 no-block guarantee).
pub fn poll_live_once(session: &mut Session, io: &mut dyn Io) -> anyhow::Result<LiveRead> {
    publish_client_state(session);
    let outgoing = std::mem::take(&mut session.ui.engine.outgoing);
    trace_outgoing(&outgoing);
    session.io.world.pending_writes.extend(outgoing);
    session
        .io
        .idle_connection
        .tick(&mut session.io.world.pending_writes);
    if session.machine.state == crate::login_state::GAME {
        queue_verify_id(&mut session.ui, &mut session.io.world.pending_writes);
    }
    // TODO(#gap-G-live-context): install real rebuild/config/logic contexts.
    // Never supply oracle fixtures or renderer tile coordinates here.
    let contexts = crate::protocol910::live::Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: None,
    };
    while session.strict_entities && session.game.is_none() && session.entities.front().is_some() {
        let applied = session
            .entities
            .apply_next(&contexts)
            .map_err(|error| anyhow::anyhow!("entity stream paused: {error:?}"))?;
        if applied.is_some() {
            return Ok(LiveRead::default());
        }
    }
    let Some(stream) = session.io.world.stream.as_mut() else {
        return Ok(LiveRead::default());
    };
    io.flush(
        Conn::World,
        stream,
        &mut session.io.world.pending_writes,
        &mut |written| {
            session.io.idle_connection.wrote();
            session.io.net_stats[0].wrote(written.len());
        },
    )
    .map_err(|err| ConnectionLost(format!("live poll write: {err}")))?;
    // A map transaction owns the front of the stream until scene installation.
    // Keep bytes already buffered; do not read more deltas against the old map.
    if session
        .game
        .as_ref()
        .is_some_and(|g| g.runtime.map_request.is_some())
    {
        return Ok(LiveRead::default());
    }
    // Bytes that arrived before the end of stream are still read in order
    // (a LOGOUT is followed by the server closing the socket).
    let closed = io
        .read(
            Conn::World,
            stream,
            &mut session.io.world.pending,
            &mut |read| {
                session.io.net_stats[0].read(read.len());
                session.io.incoming_idle.heard();
            },
        )
        .map_err(|err| ConnectionLost(format!("live poll read: {err}")))?;
    let mut ui_out = Vec::new();
    let mut session_events = Vec::new();
    let rebuild = if let Some(game) = session.game.as_mut() {
        drain_game_frames(
            game,
            &mut session.ui,
            &session.pack_root,
            &mut session.io.world,
            &mut session_events,
        )?
    } else if session.strict_entities {
        crate::session::drain_pending_entities(
            &mut session.io.world.pending,
            &mut session.io.world.pending_writes,
            &mut ui_out,
            &mut session.entities,
            &contexts,
        )?
    } else {
        crate::session::drain_pending_sync(
            &mut session.io.world.pending,
            &mut session.io.world.resync,
            &mut session.io.world.pending_writes,
            &mut ui_out,
        )?
    };
    if closed
        && !session_events
            .iter()
            .chain(ui_out.iter())
            .any(ends_connection)
    {
        return Err(ConnectionLost("live poll: server closed the connection".into()).into());
    }
    let in_game = session.machine.state == crate::login_state::GAME;
    if in_game && session.io.incoming_idle.tick(session.machine.state_ticks) {
        return Err(ConnectionLost(format!(
            "live poll: {} cycles without a byte from the world",
            crate::connection_upkeep::WORLD_SILENCE_CYCLES
        ))
        .into());
    }
    answer_reflection_checks(
        &mut session_events,
        in_game,
        &mut session.io.world.pending_writes,
    );
    answer_reflection_checks(&mut ui_out, in_game, &mut session.io.world.pending_writes);
    if in_game {
        session.io.ping.cycle(
            crate::logic_clock::monotonic_millis(),
            &session.current_world.host,
            crate::ui_runtime::host_builtins::game_shell_fps(),
            &mut session.io.world.pending_writes,
        );
    }
    // Opportunistically flush replies queued by this drain (still
    // non-blocking; leftovers flush next frame).
    let outgoing = std::mem::take(&mut session.ui.engine.outgoing);
    trace_outgoing(&outgoing);
    session.io.world.pending_writes.extend(outgoing);
    if let Some(stream) = session.io.world.stream.as_mut().filter(|_| !closed) {
        io.flush(
            Conn::World,
            stream,
            &mut session.io.world.pending_writes,
            &mut |written| {
                session.io.idle_connection.wrote();
                session.io.net_stats[0].wrote(written.len());
            },
        )
        .map_err(|err| ConnectionLost(format!("live poll write: {err}")))?;
    }
    Ok(LiveRead {
        rebuild,
        ui_events: ui_out,
        session_events,
    })
}

/// Tell the interface runtime the client state, for the packets whose
/// reading depends on it.
pub fn publish_client_state(session: &mut Session) {
    session.ui.engine.login.client_state = session.machine.state;
}

/// Whether the lobby connection is the one the player is waiting on, which
/// is when its keepalive runs: in the lobby with no login running, in a
/// world login parked on its device check (reply 42), in a lobby login parked
/// on the device check or the advertisement (replies 49 and 52), and on the
/// title's account-creation connection.
pub fn lobby_connection_live(session: &Session) -> bool {
    use crate::login_state::{ACCOUNT_CREATION_CONNECTED, LOBBY, LOBBY_ENTER_GAME, LOBBY_LOGIN};
    let login = &session.ui.engine.login;
    match session.machine.state {
        LOBBY => !session.reconnect_started,
        LOBBY_ENTER_GAME => login.reply == 42,
        LOBBY_LOGIN => matches!(login.lobby_reply, 49 | 52),
        ACCOUNT_CREATION_CONNECTED => true,
        _ => false,
    }
}

/// `TRANSMITVAR_VERIFYID`: after a cycle in which a server packet changed
/// interface state, the running count of such packets goes to the server.
pub fn queue_verify_id(ui: &mut crate::ui_runtime::Runtime, writes: &mut Vec<u8>) {
    if std::mem::take(&mut ui.state.life.verify_changed) {
        writes.extend(crate::net::encode_transmitvar_verifyid(
            ui.state.life.verify,
        ));
    }
}

/// What one [`poll_live_once`] read.
#[derive(Default)]
pub struct LiveRead {
    /// The pending rebuild.
    pub rebuild: Option<crate::session::RebuildEvent>,
    /// The interface events still to apply (the pre-entity paths).
    pub ui_events: Vec<crate::session::UiEvent>,
    /// The session events the game reader already applied.
    pub session_events: Vec<crate::session::UiEvent>,
}

/// The I/O error arm for a connection read/write: read failures route to
/// the reconnect transition, unlike
/// decoder or VM failures which stop the session owner.
#[derive(Debug)]
pub struct ConnectionLost(pub String);

impl std::fmt::Display for ConnectionLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConnectionLost {}

/// Session packets (`LOGOUT`, `LOGOUT_FULL`,
/// `LOGOUT_TRANSFER`, `CHANGE_LOBBY`) are consumed by the app's
/// `login_state::Machine` owner after the retained UI has recorded them.
pub fn is_session_event(event: &crate::session::UiEvent) -> bool {
    matches!(
        event,
        crate::session::UiEvent::Logout { .. }
            | crate::session::UiEvent::LogoutTransfer { .. }
            | crate::session::UiEvent::ChangeLobby { .. }
    ) || is_client_debug_event(event)
}

/// `LOGOUT`/`LOGOUT_FULL`/`LOGOUT_TRANSFER`: the read returns false and
/// the session owner replaces the connection.
/// `JS5_RELOAD` also returns false; an unhandled
/// table entry (`UNHANDLED_51`/`UNHANDLED_170`) and a packet that does not
/// decode log out, closing the stream the next read would use.
pub fn ends_connection(event: &crate::session::UiEvent) -> bool {
    matches!(
        event,
        crate::session::UiEvent::Logout { .. }
            | crate::session::UiEvent::LogoutTransfer { .. }
            | crate::session::UiEvent::Js5Reload
            | crate::session::UiEvent::UnhandledPacket { .. }
            | crate::session::UiEvent::MalformedPacket { .. }
    )
}

/// Packet cases owned by the client shell rather than the interface
/// runtime: reflection checks, developer-console cheats, the loading reload,
/// the unhandled-packet branch and the report of a packet that does not
/// decode. They never bump the verify id.
pub fn is_client_debug_event(event: &crate::session::UiEvent) -> bool {
    matches!(
        event,
        crate::session::UiEvent::ReflectionProbe(_)
            | crate::session::UiEvent::Js5Reload
            | crate::session::UiEvent::ExecuteClientCheat { .. }
            | crate::session::UiEvent::DoCheat { .. }
            | crate::session::UiEvent::UnhandledPacket { .. }
            | crate::session::UiEvent::MalformedPacket { .. }
    )
}

/// After the read batch, while the client state is 18, every queued
/// reflection check is answered with one `REFLECTION_CHECK_REPLY`. Checks
/// read on any other connection stay queued until the map preparation
/// discards them, so they are dropped here.
pub fn answer_reflection_checks(
    events: &mut Vec<crate::session::UiEvent>,
    in_game: bool,
    pending_writes: &mut Vec<u8>,
) {
    events.retain(|event| {
        let crate::session::UiEvent::ReflectionProbe(check) = event else {
            return true;
        };
        if in_game {
            pending_writes.extend(crate::reflection_check::encode_reply(check));
        }
        false
    });
}

/// Each logic update is limited to 100 successful reads; SERVER_TICK_END
/// stops the current read batch only. The lobby and game connections share
/// this reader and dispatch into the same varp, interface and social owners.
/// `LOGOUT`/`LOGOUT_TRANSFER` return false from the read and end the batch. `connection`
/// supplies the unread bytes and takes the replies.
pub fn drain_game_frames(
    game: &mut crate::client_game::ClientGame,
    ui: &mut crate::ui_runtime::Runtime,
    pack_root: &Path,
    connection: &mut Connection,
    session_events: &mut Vec<crate::session::UiEvent>,
) -> anyhow::Result<Option<crate::session::RebuildEvent>> {
    for _ in 0..100 {
        if game.runtime.feed.front().is_none() {
            // An opcode outside the table is a broken stream: the connection
            // is dropped (and, in the game, reconnected).
            let Some((frame, used)) =
                crate::net::decode_frame_at(&connection.pending, &connection.resync)
                    .map_err(|error| ConnectionLost(format!("{error:#}")))?
            else {
                return Ok(None);
            };
            connection.resync.advance(used as u64, frame.opcode);
            if !game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                let mut events = Vec::new();
                // REBUILD_NORMAL is an entity-feed packet; nothing returned
                // here reaches the map owner.
                crate::session::handle_sync_frame(
                    frame,
                    &mut connection.pending_writes,
                    &mut events,
                )?;
                connection.pending.drain(..used);
                // An interface's onloads run before the
                // following packet, including intervening varp/entity packets.
                let mut stop = false;
                for event in &events {
                    if !is_client_debug_event(event) {
                        ui_packet(ui, pack_root, Some(&mut *game), event)?;
                    }
                    if is_session_event(event) {
                        stop |= ends_connection(event);
                        session_events.push(event.clone());
                    }
                }
                open_browser_urls(take_browser_urls(ui), Some(&mut *game));
                if stop {
                    return Ok(None);
                }
                continue;
            }
            connection.pending.drain(..used);
        }
        let front = game
            .runtime
            .feed
            .front()
            .map(|frame| (frame.opcode, frame.payload.len()));
        let a = match game.apply_next(crate::logic_clock::monotonic_millis()) {
            Ok(applied) => applied.context("entity queue disappeared")?,
            // A player or NPC update that does not decode logs the player out.
            Err(
                error @ (rs910_config::types910::Error::Truncated { .. }
                | rs910_config::types910::Error::Invalid(_)),
            ) => {
                let (opcode, size) = front.context("entity queue disappeared")?;
                log::warn!(
                    "[client910] {} (opcode {opcode}) malformed ({error:?}); logging out",
                    crate::proto::server::name(opcode)
                );
                session_events.push(crate::session::UiEvent::MalformedPacket {
                    opcode,
                    size,
                    reason: format!("{error:?}"),
                });
                return Ok(None);
            }
            // What the client does not support yet pauses the stream.
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "entity stream paused at {:?}: {error:?}",
                    front.map(|(opcode, _)| opcode)
                ));
            }
        };
        let listener = game
            .runtime
            .feed
            .state
            .players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
            .map(|player| (player.level, player.x[0], player.z[0]));
        // `world.getRebuildType()`, not the retained last rebuild kind.
        let rebuild_kind = if game.cutscene.rebuild_type_cutscene {
            crate::protocol910::rebuild_state::Kind::Cutscene
        } else {
            crate::protocol910::rebuild_state::Kind::Normal
        };
        for sound in game.take_sounds() {
            if !sound_area_audible(listener, rebuild_kind, &sound) {
                continue;
            }
            ui_packet(
                ui,
                pack_root,
                Some(&mut *game),
                &crate::session::UiEvent::Audio {
                    command: "sound_area".into(),
                    args: vec![
                        sound.sound,
                        sound.loops,
                        sound.delay,
                        sound.volume,
                        sound.rate,
                        sound.level,
                        sound.x,
                        sound.z,
                        sound.radius,
                        i32::from(sound.dialog),
                    ],
                },
            )?;
        }
        if let Some((w, e)) = &a.rebuild {
            if e.rebased {
                return rebuild_event(w).map(Some);
            }
        }
        if a.read_batch_end {
            break;
        }
    }
    Ok(None)
}

/// Shell-owned inputs to [`update_session_logic`]: the window, console,
/// renderer and cursor owners the logic update reads (`ViewerApp` in the
/// windowed client; recorded values in the headless session replay).
pub struct InputFrame<'a> {
    pub console_open: bool,
    pub focused: bool,
    /// `crate::client_watch::gl_compressed_texture_formats` of the active
    /// renderer (`None` without a renderer).
    pub gl_texture_formats: Option<Vec<i32>>,
    /// `ui_cursor::State::current`.
    pub cursor: i32,
    /// The renderer-owned scene picking refresh (player/NPC/loc picks under
    /// the mouse) before the interface tick.
    pub refresh_picks:
        &'a mut dyn FnMut(&mut crate::client_game::ClientGame, &mut crate::ui_runtime::Runtime),
    /// The active renderer, lent to the interface tick for the profiling
    /// commands (`None` without one: headless replays).
    pub probe: Option<&'a mut dyn rs910_ui::ui_preferences::metric::RendererProbe>,
}

/// One logic cycle of the session owners after the live read
/// (the logic cycle): title/lobby interfaces, or in game the game update
/// (vars, camera, cutscene input, actors, transients) and the interface
/// update. Shared by `ViewerApp::about_to_wait` and the headless
/// session replay (`session_replay`). `environment` is the core's
/// `EnvironmentManager` (`updateGame`'s `updatePartial`,). Returns the
/// requests for the shell's owners in their drain order (app/effects.rs).
pub fn update_session_logic(
    session: &mut Session,
    pack: &Pack,
    mut shell: InputFrame<'_>,
    environment: Option<&mut EnvironmentManager>,
) -> Vec<ClientEffect> {
    let mut cutscene_rebuild = None;
    let mut harvested = Vec::new();
    // Title and lobby states run the title-screen update (interfaces only);
    // state 18 runs the game update; states 9/14/19 only advance the login.
    let state = session.machine.state;
    let title_ui = (crate::login_state::is_title(state) || crate::login_state::is_lobby(state))
        && !crate::login_state::is_rebuild(state);
    let in_game = state == crate::login_state::GAME;
    if title_ui || (in_game && !session.polling_dead) {
        if let Some(game) = session
            .game
            .as_mut()
            .filter(|g| g.runtime.map_request.is_none())
        {
            if let Err(error) = game.poll_vars(crate::logic_clock::monotonic_millis) {
                crate::logging::warn_repeated!("[client910] variable effects paused: {error:?}");
                if in_game {
                    session.polling_dead = true;
                }
            }
            if in_game {
                // The telemetry send reads
                // the camera before this cycle's update.
                {
                    let ui = &mut session.ui;
                    ui.telemetry_camera =
                        Some(ui.camera_sample(game.camera.pitch, game.camera.yaw));
                }
                if game.runtime.feed.state.initialized {
                    game.camera.logic();
                }
                if !session.polling_dead {
                    // Game update (state 18): the partial environment update
                    // and the zone loc change requests, the zone loc
                    // changes the loc menus and CS2 position queries read.
                    if let Some(environment) = environment {
                        update_game_environment(environment, game);
                    }
                    game.apply_location_snapshots();
                    // The keyboard step precedes the game update, whose
                    // cancel-binding test runs before the action clock.
                    {
                        let ui = &mut session.ui;
                        crate::ui_runtime::host_game::sync_cutscene_input(&mut ui.engine, game);
                        if ui.poll_update_input() {
                            game.cutscene.cancel_requested = true;
                        }
                    }
                    // Actors, or the
                    // cutscene clock while sceneState != 3.
                    match game.update_scene_state(pack) {
                        Ok(true) => {
                            cutscene_rebuild = game
                                .runtime
                                .map_request
                                .as_ref()
                                .map(|r| rebuild_event(&r.world));
                        }
                        Ok(false) => {}
                        Err(error) => {
                            crate::logging::warn_repeated!(
                                "[client910] actor updates paused: {error:?}"
                            );
                            session.polling_dead = true;
                        }
                    }
                    if !session.polling_dead {
                        if let Err(error) = game.update_transients() {
                            crate::logging::warn_repeated!(
                                "[client910] transient updates paused: {error:?}"
                            );
                            session.polling_dead = true;
                        }
                    }
                }
            }
            if !session.polling_dead || title_ui {
                let result = {
                    let ui = &mut session.ui;
                    ui.input.single_mouse_button = game.single_mouse_button;
                    ui.input.console_open = shell.console_open;
                    ui.engine.platform.app_focused = shell.focused;
                    if let Some(formats) = shell.gl_texture_formats.take() {
                        ui.engine.platform.gl_texture_formats = formats;
                    }
                    // The camera yaw outside cam2.
                    ui.audio.set_camera_yaw(game.camera.yaw as i32 & 0x3FFF);
                    if drain_entity_chat_history(game, &mut ui.engine.messages.history) {
                        ui.engine.messages.mark_changed();
                    }
                    ui.engine
                        .effects
                        .sequence_sounds
                        .extend(game.take_actor_sounds());
                    if let Some(icons) = &ui.state.icons {
                        let palette = game.runtime.feed.state.players.players
                            [game.runtime.map.local]
                            .as_ref()
                            .and_then(|p| p.appearance.model.as_ref())
                            .map(|m| crate::avatar::AvatarPalette {
                                recolours: m.colours.map(|v| v as u8),
                                retextures: m.textures.map(|v| v as u8),
                            });
                        icons.borrow_mut().set_palette(palette);
                    }
                    (shell.refresh_picks)(game, ui);
                    ui.engine.login.world_list_game = crate::login_state::is_game(state);
                    ui.engine.login.lobby_login = state == crate::login_state::LOBBY;
                    ui.engine.login.account_creation_connected =
                        state == crate::login_state::ACCOUNT_CREATION_CONNECTED;
                    ui.engine.login.ready = session.machine.login_ready(
                        session.reconnect_started,
                        ui.engine.creation.connect_in_progress,
                    );
                    ui.engine.login.lobby_logging_in =
                        session.machine.state == crate::login_state::LOBBY_LOGIN;
                    publish_login_progress(
                        &session.login_progress,
                        session.machine.state,
                        &mut ui.engine,
                    );
                    crate::ui_runtime::host_game::before_tick(&mut ui.engine, game, shell.cursor);
                    let result =
                        crate::client_game::with_game_probed(game, shell.probe.take(), |v| {
                            ui.tick(v)
                        });
                    crate::ui_runtime::host_game::after_tick(&mut ui.engine, game);
                    result
                };
                if let Err(error) = result {
                    crate::logging::warn_repeated!("[client910] UI lifecycle paused: {error:#}");
                    session.polling_dead = true;
                }
                harvested = session_requests(&mut session.ui.engine);
            }
        }
    }
    // The drain order (about_to_wait P10/P11): the script URLs, the cutscene
    // map request, then the engine's script and login requests.
    let mut effects = Vec::with_capacity(harvested.len() + 1);
    let mut harvested = harvested.into_iter().peekable();
    while let Some(url) = harvested.next_if(|e| matches!(e, ClientEffect::ScriptUrl(_))) {
        effects.push(url);
    }
    effects.extend(cutscene_rebuild.map(ClientEffect::CutsceneRebuild));
    effects.extend(harvested);
    effects
}

/// A world login the server resumed in place (login reply 15): the character
/// never left its world, so the session keeps its map, interfaces, variables
/// and npcs and only takes up the new connection. What it drops is what the
/// server re-sends on a resumed session: the player list (re-initialised from
/// the reply's player-positions block), the inventories, the open menu, the
/// walk marker and a scripted camera. The client tells the server its window
/// again, as a new connection expects.
///
/// Fails, leaving the session untouched, when there is no world to resume or
/// the block is not a player-positions block.
pub fn resume_world_session(
    session: &mut Session,
    live: crate::session::LiveState,
) -> anyhow::Result<()> {
    let block = live
        .login
        .resume
        .as_deref()
        .context("the login did not resume the session")?;
    let game = session
        .game
        .as_mut()
        .context("an in-place reconnect needs the session's world")?;
    game.resume_players(block)
        .map_err(|error| anyhow::anyhow!("player positions of the resumed session: {error:?}"))?;
    let stream = crate::login_worker::detach_world_socket(live.stream)?;
    // The new connection starts clean: nothing queued, nothing half read.
    if let Some(previous) = session.io.world.stream.replace(stream) {
        let _ = previous.shutdown(std::net::Shutdown::Both);
    }
    session.io.world.pending.clear();
    session.io.world.pending_writes.clear();
    session.io.world.resync = Default::default();
    session.io.idle_connection = Default::default();
    session.io.incoming_idle = Default::default();
    session.polling_dead = false;
    session.reconnect_started = false;
    let ui = &mut session.ui;
    ui.state.minimenu.reset();
    ui.state.life.map_flag = None;
    ui.engine.inv_cache = Default::default();
    let camera = &mut ui.engine.camera.cam2;
    let default_state = camera.default_state;
    camera.camera_reset(default_state);
    ui.engine.scene.server_roof = [-1, -1];
    ui.engine.login.in_progress = false;
    ui.engine.login.reply = 15;
    ui.engine.login.hoptime = 0;
    ui.engine.login.ban_duration = 0;
    ui.engine.login.queue_position = -1;
    let preferences = &session
        .game
        .as_ref()
        .context("session world")?
        .ui_variables
        .queries
        .preferences;
    if let Some(canvas) = preferences
        .window
        .canvas(preferences.options.get("screenSize").unwrap_or(0))
    {
        let anti_aliasing = preferences.options.get("antiAliasing2").unwrap_or(0);
        session
            .io
            .world
            .pending_writes
            .extend(crate::net::encode_window_status(
                preferences.window.mode as u8,
                canvas.size[0] as u16,
                canvas.size[1] as u16,
                anti_aliasing as u8,
            ));
    }
    Ok(())
}

/// What the login worker reported since the last cycle, as the login screens
/// read it: the queue position, the reply that keeps the login running
/// (queue, device check, advertisement) and the pages the server asked the
/// player to open. `state` is the client state, which says whether the running
/// login is the lobby's or the world's.
pub fn publish_login_progress(
    progress: &crate::net::LoginProgress,
    state: i32,
    engine: &mut crate::ui_runtime::Engine,
) {
    use crate::login_state::{LOBBY_LOGIN, LOBBY_RELOGIN};
    engine.login.queue_position = progress.queue_position();
    if engine.login.in_progress {
        if let Some(reply) = progress.interim_reply() {
            if matches!(state, LOBBY_LOGIN | LOBBY_RELOGIN) {
                engine.login.lobby_reply = reply;
            } else {
                engine.login.reply = reply;
            }
        }
    }
    engine
        .effects
        .browser_urls
        .extend(
            progress
                .take_urls()
                .into_iter()
                .map(|primary| crate::session::UiEvent::UrlOpen {
                    primary,
                    fallback: None,
                    javascript: false,
                }),
        );
}

/// The engine's requests after the interface tick, in their drain order
/// (script URLs first; `update_session_logic` inserts the cutscene map
/// request after them).
pub fn session_requests(engine: &mut crate::ui_runtime::Engine) -> Vec<ClientEffect> {
    let world_switch = engine.login.world_switch.take();
    let login_request = engine.login.request.take();
    let lobby_enter_game = engine.login.lobby_enter_game.take();
    let create_connect = std::mem::take(&mut engine.creation.connect_requested);
    let login_cancel = std::mem::take(&mut engine.login.cancel_requested);
    let login_continue = std::mem::take(&mut engine.login.continue_requested);
    let resend_uid_passport = std::mem::take(&mut engine.login.resend_uid_passport_requested);
    let logout = std::mem::take(&mut engine.login.logout_requested);
    let scripts = std::mem::take(&mut engine.builtins.requests);
    let urls = std::mem::take(&mut engine.effects.browser_urls);
    let mut effects: Vec<ClientEffect> = urls.into_iter().map(ClientEffect::ScriptUrl).collect();
    effects.extend(scripts.into_iter().map(ClientEffect::Script));
    effects.extend(world_switch.map(ClientEffect::WorldSwitch));
    effects.extend(login_cancel.then_some(ClientEffect::LoginCancel));
    effects.extend(login_continue.then_some(ClientEffect::LoginContinue));
    effects.extend(
        resend_uid_passport
            .then(|| ClientEffect::LoginPacket(crate::net::encode_uid_passport_resend_request())),
    );
    effects.extend(logout.then_some(ClientEffect::Logout));
    effects.extend(login_request.map(ClientEffect::LoginRequest));
    effects.extend(lobby_enter_game.map(ClientEffect::LobbyEnterGame));
    effects.extend(create_connect.then_some(ClientEffect::CreateConnect));
    effects
}

/// The session-level work of one live poll, handed to the shell owners
/// (renderer lights/environment/minimap, browser, console, session machine).
pub struct LivePoll {
    pub rebuild: Option<crate::session::RebuildEvent>,
    pub session_events: Vec<crate::session::UiEvent>,
    /// The batch's [`packet_effects`], with the console lines.
    pub effects: Vec<ClientEffect>,
}

pub enum LivePollOutcome {
    /// Nothing to apply: no open stream, polling stopped, or a decoder/VM
    /// failure that stopped it (`polling_dead`).
    Idle,
    /// The I/O error arm (the reconnect transition).
    Lost(String),
    Polled(LivePoll),
}

/// One live poll over the session ([`poll_live_once`]) plus the retained
/// interface application of the frames it collected. Shared by
/// `ViewerApp::poll_live_rebuild` and the headless session replay.
pub fn poll_live_session(session: &mut Session, io: &mut dyn Io) -> LivePollOutcome {
    if session.polling_dead || session.io.world.stream.is_none() {
        return LivePollOutcome::Idle;
    }
    let LiveRead {
        rebuild,
        ui_events,
        mut session_events,
    } = match poll_live_once(session, io) {
        Ok(polled) => polled,
        Err(err) if err.downcast_ref::<ConnectionLost>().is_some() => {
            return LivePollOutcome::Lost(format!("{err:#}"));
        }
        Err(err) => {
            log::warn!(
                "[client910] live poll failed ({err:#}); stopping live reload, keeping rendered terrain"
            );
            session.polling_dead = true;
            return LivePollOutcome::Idle;
        }
    };
    for event in &ui_events {
        if is_client_debug_event(event) {
            session_events.push(event.clone());
            continue;
        }
        if let Err(error) = ui_packet(
            &mut session.ui,
            &session.pack_root,
            session.game.as_mut(),
            event,
        ) {
            log::warn!("[client910] UI packet paused: {error:#}");
            session.polling_dead = true;
            return LivePollOutcome::Idle;
        }
        if is_session_event(event) {
            session_events.push(event.clone());
        }
    }
    LivePollOutcome::Polled(LivePoll {
        rebuild,
        session_events,
        effects: packet_effects(Some(&mut session.ui), true),
    })
}

/// One deterministic input injection (`input_script.rs`), applied to the
/// session's input owners at the injector sub-phase of the logic cycle.
/// Shared by `ViewerApp::about_to_wait` and the headless session replay.
/// The retained-UI half of a winit `MouseInput` (mouse pressed
/// / released): the held button, the queued input telemetry
/// event (presses 0/1/2, releases 3/4/5), this cycle's head `MouseEvent`
/// and, for the left button, the queued click. `action` is 0 left,
/// 1 middle, 2 right; winit reports no click count, so every press counts
/// as 1. Shared by `ViewerApp::window_event` and the headless scenario tests.
pub fn retained_mouse_button(
    ui: &mut crate::ui_runtime::Runtime,
    action: i32,
    pressed: bool,
    now: i64,
) {
    match action {
        0 => ui.input.left_held = pressed,
        1 => ui.input.middle_held = pressed,
        _ => ui.input.right_held = pressed,
    }
    let [x, y] = ui.engine.platform.pending_mouse;
    let queued = if pressed {
        action
    } else {
        [3, 4, 5][action as usize]
    };
    ui.client_watch.queue_event(queued, x, y, now, 1);
    if pressed && ui.input.event.is_none() {
        ui.input.event = Some(crate::ui_defaults::MouseEvent {
            pos: ui.engine.platform.pending_mouse,
            action,
            count: 1,
        });
    }
    if action == 0 && pressed && ui.input.click.is_none() {
        ui.input.click = Some(ui.engine.platform.pending_mouse);
    }
}

/// The retained-UI half of a winit `CursorMoved` at canvas position `pos`
/// (the position update with move tracking).
pub fn retained_mouse_move(ui: &mut crate::ui_runtime::Runtime, pos: [i32; 2], now: i64) {
    ui.engine.platform.pending_mouse = pos;
    ui.client_watch
        .queue_event(crate::client_watch::action::MOVE, pos[0], pos[1], now, 0);
}

/// The retained-UI half of a winit `KeyboardInput`: key pressed
/// / released of the AWT key code, then `keyTyped` for the pressed
/// key's text (AWT order).
pub fn retained_key(
    ui: &mut crate::ui_runtime::Runtime,
    awt: i32,
    pressed: bool,
    text: Option<&str>,
    now: i64,
) {
    ui.keyboard.key(awt, if pressed { 0 } else { 1 }, now);
    if pressed {
        for ch in text.unwrap_or_default().chars() {
            ui.keyboard.typed(ch, now);
        }
    }
}

/// The lobby login reply's profile (login step 157) as the retained UI's CS2 commands read it
/// (`lobby_*`, `userdetail_*`), with the lobby reply 2 and no queue/ban.
/// Shared by `ViewerApp::install_lobby_connection` and the scenario tests.
pub fn apply_lobby_profile(
    ui: &mut crate::ui_runtime::Runtime,
    login: &crate::net::LoginOk,
    now: i64,
) {
    ui.engine.login.in_progress = false;
    ui.engine.login.lobby_reply = 2;
    ui.engine.login.ban_duration = 0;
    ui.engine.login.queue_position = -1;
    ui.engine.account.player_is_members = login.profile.player_is_members;
    ui.engine.account.player_is_quickchat = login.profile.player_is_quickchat;
    ui.engine.account.logged_in_quickchat = login.profile.logged_in_quickchat;
    ui.engine.account.dob_verified = login.profile.dob_verified;
    ui.engine.account.dob = login.profile.lobby_dob;
    ui.engine.account.staff_mod_level = login.profile.staff_mod_level;
    ui.engine.account.player_mod_level = login.profile.player_mod_level;
    ui.engine.lobby.membership = login.lobby.membership;
    ui.engine.lobby.membership_offset = login.lobby.membership - now - login.lobby.membership_delay;
    ui.engine.lobby.membership_flag = login.lobby.membership_flag;
    ui.engine.lobby.unread_messages = login.lobby.unread_messages;
    ui.engine.lobby.recovery_day = login.lobby.recovery_day;
    ui.engine.lobby.cc_expiry = login.lobby.cc_expiry;
    ui.engine.lobby.grace_expiry = login.lobby.grace_expiry;
    ui.engine.lobby.dob_requested = login.lobby.dob_requested;
    ui.engine.lobby.members_stats = login.lobby.members_stats;
    ui.engine.lobby.play_age = login.lobby.play_age;
    ui.engine.lobby.jcoins_balance = login.lobby.jcoins_balance;
    ui.engine.lobby.loyalty_balance = login.lobby.loyalty_balance;
    ui.engine.lobby.last_login_day = login.lobby.last_login_day;
    // The name is looked up in the background.
    ui.engine.login.last_login = Some(rs910_ui::host_name::HostName::lookup(
        login.lobby.player_host,
        crate::logic_clock::monotonic_millis(),
    ));
    ui.engine.lobby.email_status = login.lobby.email_status;
    ui.engine.login.lobby_player_name = login.lobby.player_name.clone();
}

pub fn apply_ui_injection(
    session: &mut Session,
    logic_cycle: i32,
    injection: crate::input_script::UiInjection,
) {
    use crate::input_script::UiInjection;
    match injection {
        // `CLIENT910_KEY_INPUT=cycle,code,mode;...`.
        UiInjection::Key { code, mode } => {
            let ui = &mut session.ui;
            ui.keyboard
                .key(code, mode, crate::logic_clock::monotonic_millis());
        }
        // Deterministic text replay through the same keyboard owner as
        // winit input: `cycle:text;...`, with `\n`/`\t` sent as Enter/Tab
        // (key pressed, typed and released).
        UiInjection::Type { text } => {
            let ui = &mut session.ui;
            let now = crate::logic_clock::monotonic_millis();
            for ch in text.chars() {
                let awt = match ch {
                    '\n' => 10,
                    '\t' => 9,
                    'a'..='z' => ch.to_ascii_uppercase() as i32,
                    'A'..='Z' | '0'..='9' | ' ' => ch as i32,
                    _ => 0,
                };
                ui.keyboard.key(awt, 0, now);
                ui.keyboard.typed(ch, now);
                ui.keyboard.key(awt, 1, now);
            }
            log::info!(
                "[ui-type] cycle={} chars={}",
                logic_cycle,
                text.chars().count()
            );
        }
        // `CLIENT910_WHEEL_INPUT=cycle,delta;...`.
        UiInjection::Wheel { delta } => {
            let ui = &mut session.ui;
            ui.input.wheel += delta;
        }
        // `CLIENT910_TOOLKIT_INPUT=cycle,id;...`: the settings interface's
        // `detail_toolkit(id)` CS2 command at a logic cycle (runtime
        // toolkit switching without navigating the options menus).
        UiInjection::Toolkit { cycle, id } => {
            if let Some(game) = session.game.as_mut() {
                let result = game
                    .ui_variables
                    .queries
                    .preferences
                    .dispatch("detail_toolkit", &mut vec![id]);
                log::info!("[client910] detail_toolkit({id}) at cycle {cycle}: {result:?}");
            }
        }
        // Replay real cache component operations through the same onop
        // and permission path as normal input (no direct preference writes);
        // op 0 is the component's pause button (the menu's "continue" entry).
        UiInjection::Operation { parent, child, op } => {
            let ui = &mut session.ui;
            let action = if op == 0 {
                crate::ui_interaction::Action::Pause { parent, child }
            } else {
                crate::ui_interaction::Action::Op {
                    parent,
                    child,
                    op,
                    base: None,
                }
            };
            ui.state.interaction.actions.push_back(action);
        }
        // Deterministic UI gesture replay (`CLIENT910_UI_INPUT`):
        // semicolon-separated cycle,x,y,held-button-mask,press-action (-1
        // for motion/release).
        UiInjection::Gesture {
            cycle,
            x,
            y,
            held,
            press,
        } => {
            let client_state = session.machine.state;
            let ui = &mut session.ui;
            // The mouse move (and press) this gesture
            // represents, polled this cycle.
            let now = crate::logic_clock::monotonic_millis();
            ui.client_watch.inject_polled(
                crate::client_watch::PointerEvent {
                    action: crate::client_watch::action::MOVE,
                    x,
                    y,
                    time: now,
                    count: 0,
                },
                client_state,
            );
            if press >= 0 {
                ui.client_watch.inject_polled(
                    crate::client_watch::PointerEvent {
                        action: press,
                        x,
                        y,
                        time: now,
                        count: 1,
                    },
                    client_state,
                );
            }
            ui.engine.platform.pending_mouse = [x, y];
            ui.engine.platform.mouse = [x, y];
            ui.input.left_held = held & 1 != 0;
            ui.input.middle_held = held & 2 != 0;
            ui.input.right_held = held & 4 != 0;
            if press >= 0 {
                ui.input.event = Some(crate::ui_defaults::MouseEvent {
                    pos: [x, y],
                    action: press,
                    count: 1,
                });
                if press == 0 {
                    ui.input.click = Some([x, y]);
                }
            }
            log::info!(
                "[ui-input] cycle={} mouse={:?} held={} press={} drag={:?}",
                cycle,
                ui.engine.platform.mouse,
                held,
                press,
                ui.state.interaction.drag.component.as_ref().map(|c| {
                    let c = c.borrow();
                    (c.f.parentlayer, c.f.id)
                })
            );
        }
        // Diagnostic headless click (`CLIENT910_UI_CLICK=x,y,first[,last]`):
        // synthesizes a left press at canvas `[x,y]` (logical pixels,
        // `[first,last]` (single cycle when `last` is absent). Optional
        // fifth field: the MouseEvent action (0 left, 2 right).
        UiInjection::Click {
            x: cx,
            y: cy,
            action,
            first,
        } => {
            let client_state = session.machine.state;
            let ui = &mut session.ui;
            ui.engine.platform.pending_mouse = [cx, cy];
            ui.engine.platform.mouse = [cx, cy];
            ui.input.mouse = [cx, cy];
            if action == 0 {
                ui.input.click = Some([cx, cy]);
            }
            if first {
                // The mouse move + press a native click
                // produces, polled this cycle like input.event.
                let now = crate::logic_clock::monotonic_millis();
                for (kind, count) in [(crate::client_watch::action::MOVE, 0), (action, 1)] {
                    ui.client_watch.inject_polled(
                        crate::client_watch::PointerEvent {
                            action: kind,
                            x: cx,
                            y: cy,
                            time: now,
                            count,
                        },
                        client_state,
                    );
                }
            }
            if ui.input.event.is_none() {
                ui.input.event = Some(crate::ui_defaults::MouseEvent {
                    pos: [cx, cy],
                    action,
                    count: 1,
                });
            }
            log::info!("[client910] ui-click inject cycle={} at=[{cx},{cy}] viewport={:?} scene_options={} menu_open={} active={:?}",
                logic_cycle, ui.state.viewport, ui.input.scene_options.len(), ui.state.minimenu.open, ui.state.minimenu.active);
        }
        UiInjection::ClickMalformed { spec } => {
            log::warn!("[client910] ui-click: ignoring malformed CLIENT910_UI_CLICK={spec:?} (want x,y,first[,last[,action]])");
        }
        // Diagnostic hover (`CLIENT910_UI_HOVER=x,y,first[,last]`): moves the
        // cursor to canvas `[x,y]` over the cycles without a press.
        UiInjection::Hover { x, y } => {
            let ui = &mut session.ui;
            ui.engine.platform.pending_mouse = [x, y];
            ui.engine.platform.mouse = [x, y];
            ui.input.mouse = [x, y];
        }
        // Diagnostic click sequence (`CLIENT910_UI_CLICKS=b,x,y,cycle;...`,
        // `b` = l/m/r): one mouse press per entry at its cycle, with
        // the left/right held state released on the following cycle.
        UiInjection::ClicksPress {
            button: b,
            x: cx,
            y: cy,
            cycle,
            action,
        } => {
            let ui = &mut session.ui;
            ui.engine.platform.pending_mouse = [cx, cy];
            ui.engine.platform.mouse = [cx, cy];
            ui.input.mouse = [cx, cy];
            match action {
                0 => {
                    ui.input.left_held = true;
                    ui.input.click = Some([cx, cy]);
                }
                1 => ui.input.middle_held = true,
                _ => ui.input.right_held = true,
            }
            ui.input.event = Some(crate::ui_defaults::MouseEvent {
                pos: [cx, cy],
                action,
                count: 1,
            });
            log::info!("[client910] ui-clicks inject cycle={cycle} button={b} at=[{cx},{cy}] menu_open={} scene_options={}",
                ui.state.minimenu.open, ui.input.scene_options.len());
        }
        UiInjection::ClicksRelease => {
            let ui = &mut session.ui;
            ui.input.left_held = false;
            ui.input.middle_held = false;
            ui.input.right_held = false;
        }
    }
}

/// `--server-command`: one queued developer command per 100 logic cycles
/// once the map is installed, sent as `CLIENT_CHEAT`. Shared by
/// `ViewerApp::about_to_wait` and the headless session replay.
pub fn dispatch_server_command(session: &mut Session) -> anyhow::Result<()> {
    if session.polling_dead {
        return Ok(());
    }
    let Some(game) = session.game.as_ref() else {
        return Ok(());
    };
    if game.runtime.map_request.is_some()
        || !game.runtime.feed.state.initialized
        || game.cycle < session.next_command_cycle
    {
        return Ok(());
    }
    let Some(command) = session.server_commands.pop_front() else {
        return Ok(());
    };
    let packet = crate::client_command::remote(&command, false, false)?;
    session.io.world.pending_writes.extend(packet);
    session.next_command_cycle = game.cycle.wrapping_add(100);
    log::info!(
        "[client910] server command queued: {}",
        command.split_whitespace().next().unwrap_or("")
    );
    Ok(())
}

/// Once the scene is built: install the
/// prepared CPU map and queue `MAP_BUILD_COMPLETE` with the rebuild time.
/// Shared by `ViewerApp::finish_game_map` (after the GPU upload) and the
/// headless session replay.
pub fn acknowledge_game_map(
    session: &mut Session,
    now: impl FnOnce() -> i64,
) -> anyhow::Result<()> {
    let game = session.game.as_mut().context("map has no owning runtime")?;
    let started = game
        .rebuild_started_ms
        .context("missing rebuild start timestamp")?;
    let prepared = session
        .prepared_map
        .as_ref()
        .context("no prepared map")?
        .clone();
    game.runtime
        .install_map(prepared)
        .map_err(|e| anyhow::anyhow!("install map: {e:?}"))?;
    session.prepared_map = None;
    session.next_command_cycle = game.cycle.wrapping_add(100);
    game.rebuild_started_ms = None;
    let elapsed = now().wrapping_sub(started) as i32;
    if let Some(connection) = session.io.startup_connection.take() {
        connection.finish()?;
    }
    session
        .io
        .world
        .pending_writes
        .extend(crate::net::encode_map_build_complete(elapsed));
    crate::session_record::record(b"MAPI", &elapsed.to_le_bytes());
    log::info!(
        "[client910] entity map installed: base {},{} size {} local {} (acknowledgement queued)",
        game.runtime.map.base_x,
        game.runtime.map.base_z,
        game.runtime.map.width,
        game.runtime.map.local
    );
    Ok(())
}

/// The session half of the startup canvas installation: the retained
/// interface canvas and display modes, `WINDOW_STATUS`, then the interface
/// frames the startup drain collected. Shared by
/// `ViewerApp::install_session_canvas` and the headless session replay.
pub fn install_canvas_state(
    session: &mut Session,
    [w, h]: [u32; 2],
    modes: &[crate::ui_runtime::FullscreenMode],
    debug_events: &mut Vec<crate::session::UiEvent>,
) -> anyhow::Result<()> {
    publish_client_state(session);
    {
        let ui = &mut session.ui;
        if let Some(game) = &session.game {
            ui.engine.platform.window_mode = game.ui_variables.queries.preferences.window.mode;
        }
        ui.resize([w as i32, h as i32])?;
        let screen_size = session.game.as_ref().map_or(0, |game| {
            game.ui_variables
                .queries
                .preferences
                .options
                .live()
                .screen_size
        });
        ui.engine.install_display_modes(modes, screen_size);
    }
    if let Some(game) = &session.game {
        let aa = game
            .ui_variables
            .queries
            .preferences
            .options
            .get("antiAliasing2")
            .unwrap() as u8;
        session
            .io
            .world
            .pending_writes
            .extend(crate::net::encode_window_status(
                session.ui.engine.platform.window_mode as u8,
                w as u16,
                h as u16,
                aa,
            ));
    }
    let mut initial_ui = std::mem::take(&mut session.initial_ui);
    answer_reflection_checks(&mut initial_ui, true, &mut session.io.world.pending_writes);
    for event in initial_ui {
        if is_client_debug_event(&event) {
            debug_events.push(event);
            continue;
        }
        ui_packet(
            &mut session.ui,
            &session.pack_root,
            session.game.as_mut(),
            &event,
        )?;
    }
    Ok(())
}

/// The session owners' half of a window focus loss (keyboard and mouse focus
/// loss). Shared by
/// `ViewerApp::window_event` and the headless session replay.
pub fn session_focus_lost(session: &mut Session) {
    let state = session.machine.state;
    if let Some(game) = session.game.as_mut() {
        let p = &mut game.ui_variables.queries.preferences;
        match p.window.focus_lost(&p.options, state) {
            Ok(true) => p.window.changed = true,
            Ok(false) => {}
            Err(error) => log::warn!("[client910] fullscreen exit: {error:?}"),
        }
    }
    let ui = &mut session.ui;
    ui.keyboard
        .focus_lost(crate::logic_clock::monotonic_millis());
    ui.input.left_held = false;
    ui.input.middle_held = false;
    ui.input.right_held = false;
    ui.input.click = None;
    ui.input.event = None;
}

/// The session half of `ViewerApp::sync_window_settings`
/// lay the retained interface out on the
/// window's canvas and send `WINDOW_STATUS` when the mode or size changed.
/// `pointer` is the last native pointer position (physical) with its scale
/// factor; `set_canvas` hands the canvas to the renderer first. Shared by
/// `ViewerApp` and the headless session replay.
pub fn sync_window_state(
    session: &mut Session,
    pointer: Option<([f64; 2], f64)>,
    set_canvas: &mut dyn FnMut(crate::ui_window::Canvas) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(game) = session.game.as_mut() else {
        return Ok(());
    };
    let preferences = &mut game.ui_variables.queries.preferences;
    if preferences.window.changed {
        // A window-mode change places the canvas at the left/top margins.
        preferences.window.location = None;
    }
    let Some(canvas) = preferences
        .window
        .canvas(preferences.options.live().screen_size)
    else {
        return Ok(());
    };
    let changed = preferences.window.changed;
    set_canvas(canvas)?;
    {
        let ui = &mut session.ui;
        let resized = ui.state.layout.canvas != canvas.size;
        ui.engine.platform.window_mode = preferences.window.mode;
        ui.engine.platform.last_fullscreen_size = preferences.window.last_fullscreen;
        ui.resize(canvas.size)?;
        if changed || resized {
            session
                .io
                .world
                .pending_writes
                .extend(crate::net::encode_window_status(
                    preferences.window.mode as u8,
                    canvas.size[0] as u16,
                    canvas.size[1] as u16,
                    preferences.options.get("antiAliasing2").unwrap() as u8,
                ));
            // Moving the canvas under a stationary pointer changes the mouse
            // coordinates without requiring a native pointer-move event.
            if let Some((position, scale)) = pointer {
                ui.engine.platform.pending_mouse = canvas.mouse(position, scale);
            }
        }
    }
    preferences.window.changed = false;
    Ok(())
}

/// What a login reports of the window and the saved options: the window
/// mode, the canvas, the anti-aliasing level and the encoded options block.
/// Sending the block counts as telling the server about the preferences.
pub fn login_client_report(session: &mut Session) -> crate::net::ClientReport {
    let Some(game) = session.game.as_mut() else {
        return crate::net::ClientReport::default();
    };
    let preferences = &mut game.ui_variables.queries.preferences;
    let live = preferences.options.live();
    let canvas = preferences
        .window
        .canvas(live.screen_size)
        .map_or([800, 600], |canvas| {
            [
                u16::try_from(canvas.size[0]).unwrap_or(u16::MAX),
                u16::try_from(canvas.size[1]).unwrap_or(u16::MAX),
            ]
        });
    preferences.change_notified = true;
    crate::net::ClientReport {
        window_mode: u8::try_from(preferences.window.mode).unwrap_or(0),
        canvas,
        anti_aliasing: u8::try_from(preferences.options.get("antiAliasing2").unwrap_or(0))
            .unwrap_or(0),
        preferences: preferences.options.encode(),
    }
}

/// The launcher's parameters a login reports: the applet parameters, with
/// the user-flow and flag words the login owner retains (the launcher's own
/// until a script changes them).
pub fn launcher_report(session: &Session) -> crate::net::LauncherReport {
    let login = &session.ui.engine.login;
    launcher_report_with(login.user_flow, login.automated_test_flags)
}

/// [`launcher_report`] before a session exists.
pub fn launcher_report_with(user_flow: [i32; 2], flags: [i32; 2]) -> crate::net::LauncherReport {
    let params = crate::applet_params::get();
    crate::net::LauncherReport {
        language: params.language().ok().flatten().unwrap_or(0) as u8,
        game: params.mode_game_id() as u8,
        affiliate: params.player_is_affiliate(),
        user_flow,
        flags,
        label: params.label(),
        additional_info: params.additional_info(),
        javascript: params.javascript_enabled(),
        chrome: params.have_chrome(),
        client_type: params.client_type(),
        build: params.build_word(),
        gamepack: params.gamepack().unwrap_or_default(),
    }
}

/// The preference half of `ViewerApp::install_session_toolkit`
/// (the loading stage of a `--direct-login` session):
/// device capabilities, safe-mode bookkeeping, the anti-aliasing fallback and
/// the window size. `device` is what the hardware toolkits answer, whatever
/// toolkit is active while this runs (the loading screens' toolkit 0 is);
/// the answers the session keeps are those of the toolkit it saves, toolkit 0
/// answering neither anti-aliasing nor bloom. Returns the renderer's scene
/// samples, bloom and whether toolkit 0 (`displayMode` 0) is selected. Shared
/// with the headless session replay.
pub fn install_toolkit_preferences(
    session: &mut Session,
    device: ToolkitCaps,
    supports_scene_samples: &mut dyn FnMut(u32) -> bool,
    window_physical: [u32; 2],
    window_scale: f64,
) -> Option<(u32, bool, bool)> {
    let game = session.game.as_mut()?;
    let preferences = &mut game.ui_variables.queries.preferences;
    // Creating the saved toolkit reapplies the saved bloom when the toolkit
    // supports it (never toolkit 0).
    preferences.anti_aliasing = device.antialiasing;
    preferences.bloom = device.bloom;
    // Safe-mode bookkeeping and the saved toolkit's creation, which captures
    // the device AA level and reapplies saved bloom.
    // The machine's memory decides whether a defaulted toolkit falls back to
    // the software one.
    preferences.loading_toolkit(i32::try_from(preferences.hardware.ram_mb).unwrap_or(i32::MAX));
    let toolkit0 = preferences.options.get("displayMode") == Some(0);
    // From here on the scripts ask the saved toolkit.
    preferences.anti_aliasing = !toolkit0 && device.antialiasing;
    preferences.bloom = !toolkit0 && device.bloom;
    if session.machine.state == crate::login_state::GAME {
        // `--direct-login` entered state 18 through the title
        // login path (4 -> 7 -> 18) before this window existed;
        // apply that path's state-4 confirmation.
        preferences.confirm_safe_mode(crate::login_state::LOGIN, true);
    }
    let level = preferences.active_aa;
    let samples = if level == 0 { 1 } else { (level * 2) as u32 };
    let samples = if supports_scene_samples(samples) {
        samples
    } else {
        // GPU-error recovery: fall back to no anti-aliasing.
        preferences.options.set_field("antiAliasing", 0).unwrap();
        preferences.options.set_field("antiAliasing2", 0).unwrap();
        preferences.active_aa = 0;
        preferences.dirty = true;
        1
    };
    preferences
        .window
        .install_size(window_physical, window_scale);
    let mode = preferences.options.live().window_mode;
    if (1..=2).contains(&mode) {
        preferences.window.mode = mode;
    }
    Some((samples, preferences.bloom_enabled, toolkit0))
}

/// The `--direct-login` session's retained interface runtime, decoding its
/// config stores from the archives the game owner's login read
/// (`configs`). Shared by `online_login` and the headless session replay.
pub fn retained_session_ui(
    pack: &Pack,
    configs: rs910_config::login_configs::LoginConfigs,
    live: &LoginProfile,
    uid192: [u8; 24],
    game: &mut crate::client_game::ClientGame,
) -> anyhow::Result<crate::ui_runtime::Runtime> {
    let mut runtime = crate::ui_runtime::Runtime::new_with(pack.clone(), configs)?;
    runtime.engine.login.uid192 = uid192;
    runtime.engine.account.logged_in_members = live.logged_in_members;
    // The members-only object types follow the world's membership.
    runtime.set_allow_members(live.logged_in_members);
    // (Fresh runtime: already reset.)
    runtime.client_watch.reset();
    runtime.engine.account.player_is_members = live.player_is_members;
    runtime.engine.account.player_is_quickchat = live.player_is_quickchat;
    runtime.engine.account.logged_in_quickchat = live.logged_in_quickchat;
    runtime.engine.account.dob_verified = live.dob_verified;
    runtime.engine.account.dob = live.lobby_dob;
    runtime.engine.account.staff_mod_level = live.staff_mod_level;
    runtime.engine.account.player_mod_level = live.player_mod_level;
    runtime.engine.game_host.owner = live.owner.clone();
    if let Some(clock) = live.server_clock {
        // The server clock minus the local monotonic clock.
        runtime.engine.lobby.membership_offset = clock - crate::logic_clock::monotonic_millis();
    }
    crate::client_game::with_game(game, |v| runtime.initialize_cursors(v))?;
    Ok(runtime)
}

/// The session owners' start of a logic
/// cycle: net stats, the debug overlay inputs, the mouse flip and client-watch
/// event routing. Shared by `ViewerApp::about_to_wait` and the headless
/// session replay.
pub fn begin_session_cycle(
    session: &mut Session,
    logic_cycle: i32,
    offheap_bytes: &dyn Fn() -> Option<u64>,
) {
    let client_state = session.machine.state;
    // The cycle counter advances, then the net stats refresh on both
    // connections.
    for net in &mut session.io.net_stats {
        net.refresh(logic_cycle);
    }
    let ui = &mut session.ui;
    // drawDebug inputs.
    let stats = &mut ui.state.debug_stats;
    stats.fps = crate::ui_runtime::host_builtins::game_shell_fps();
    stats.fps_average = crate::ui_runtime::host_builtins::game_shell_fps_average();
    if ui.state.debug_visible[0] {
        if let Some((used, total)) = crate::debug_overlay::process_memory_kb() {
            stats.mem_used_k = used;
            stats.mem_total_k = total;
        }
        // The toolkit's off-heap memory query, an int.
        if let Some(bytes) = offheap_bytes() {
            stats.offheap_bytes = i32::try_from(bytes).unwrap_or(i32::MAX);
        }
    }
    stats.game = session.io.net_stats[0];
    stats.lobby = session.io.net_stats[1];
    ui.engine.platform.mouse = ui.engine.platform.pending_mouse;
    // Mouse flip, event routing and mouse-event head removal.
    ui.client_watch.mainloop(client_state);
}

/// The map-loading request for an accepted world transaction (packet
/// rebuilds and cutscene rebuilds).
pub fn rebuild_event(
    w: &crate::protocol910::rebuild_state::World,
) -> anyhow::Result<crate::session::RebuildEvent> {
    let rebuild = crate::session::Rebuild {
        zone_x: u16::try_from(w.region_x)?,
        zone_z: u16::try_from(w.region_z)?,
        npc_bits: u8::try_from(w.npc_bits)?,
        map_count: u8::try_from(w.groups.len())?,
        build_area_id: u8::try_from(w.area.context("missing build area")?)?,
        force: true,
        has_high_res_block: false,
        high_res_bytes: 0,
    };
    let mut event = crate::session::RebuildEvent::new(rebuild);
    event.groups = w
        .groups
        .iter()
        .take(w.group_count)
        .map(|&g| u16::try_from(g))
        .collect::<Result<_, _>>()?;
    Ok(event)
}

/// The zone sound-area packets: outside a cutscene rebuild, the local
/// player's first route waypoint must lie within `radius + 1` tiles. The zone
/// level only feeds the sound call's unused level argument; the scene-bounds
/// test ran at decode.
pub fn sound_area_audible(
    listener: Option<(i32, i32, i32)>,
    rebuild_kind: crate::protocol910::rebuild_state::Kind,
    sound: &crate::protocol910::zone_state::SoundArea,
) -> bool {
    if rebuild_kind == crate::protocol910::rebuild_state::Kind::Cutscene {
        return false;
    }
    // The local player is dereferenced here.
    let Some((_, x, z)) = listener else {
        return false;
    };
    let radius = sound.radius.saturating_add(1);
    (x - sound.x).abs() <= radius && (z - sound.z).abs() <= radius
}

#[cfg(test)]
mod tests;
