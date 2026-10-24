//! client910 app entry: CLI, offline/online flow, winit event loop.
//!
//! # Scope
//! Startup accepts CLI credentials; retained lobby login scripts can now pass
//! user credentials to the asynchronous session workers. Live interface / CS2 frames
//! (`IF_OPENTOP` 35/19, `IF_OPENSUB` 38/23, `IF_OPENSUB_ACTIVE_*`
//! 26/61/102/121, `IF_SETTEXT` 181, `IF_SETHIDE` 109, `IF_SETPOSITION` 72,
//! `IF_SETSCROLLPOS` 79, `RUNCLIENTSCRIPT` 156) arrive on the world session,
//! are parsed in `session.rs` (the server's packet encoders and the
//! client's read dispatch as truth) and applied by the retained
//! interface runtime (`ui_runtime::Runtime`), whose hooks and server-pushed
//! `RUNCLIENTSCRIPT`s execute through the real `native910::vm::Vm`.
//! Region JS5 ensure/download runs on a background `std::thread` worker
//! (request `{groups}` → progress `{loaded,total}` / done `{World}` / error);
//! the winit thread never `block_on`s a Tokio runtime (the worker owns its
//! own current-thread runtime; startup `online_login` uses a short-lived
//! runtime before the event loop and drops it).
//!
//! # Controls
//! - Online follows the server player; `--free-camera` enables viewer navigation.
//! - `W`/`A`/`S`/`D`: pan the free-camera target across the ground plane.
//! - `Q` / `E`: lower / raise the camera target.
//! - Left-mouse drag: orbit (yaw + pitch).
//! - Mouse wheel: zoom in/out.
//! - `Esc` or window close: quit.
//! - `Alt` + backquote: developer console; Enter sends, Tab sends while
//!   retaining the entry, Page Up/Page Down recall history, Ctrl-C/V clipboard.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(all(test, alto_sessions))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use rs910_protocol::rsa_block::RsaPublicKey;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::active_toolkit::ActiveToolkit;
use rs910_config::config;
use rs910_config::flo::FloStore;
use rs910_js5::cache::Pack;
use rs910_render_gpu::render::OrbitCamera;
use rs910_scene::game_scene::GameScene;
use rs910_scene::map;

/// Spawn from `server/src/lostcity/entity/Player.ts` (`super(0, 3222, 3222)`).
pub const SPAWN_LEVEL: i32 = 0;
/// Spawn tile X.
pub const SPAWN_X: i32 = 3222;
/// Spawn tile Z.
pub const SPAWN_Z: i32 = 3222;
/// Lumbridge region (+ neighbours stream around it).
pub const REGION_MAIN: u16 = 12850;
/// Region group id for the Lumbridge area.
pub const REGION_GROUP: u16 = 6450;
/// Region 12850 base tile: `(50 * 64, 50 * 64)`.
pub const REGION_BASE_X: i32 = 3200;
/// Region 12850 base tile Z.
pub const REGION_BASE_Z: i32 = 3200;

#[cfg(test)]
mod scene_pick_tests;

/// CLI: hardcoded login UI. `--username` stands in for the login screen;
/// no password/JAGGRAB handshake is attempted in this milestone.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "client910",
    version,
    about = "A client for revision 910 game servers"
)]
pub struct Cli {
    /// Lobby/world host: the current lobby and the startup world (applet
    /// parameters).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// The current lobby port for the title-screen lobby login.
    #[arg(long, default_value_t = rs910_protocol::proto::LOBBY_PORT)]
    pub lobby_port: u16,
    /// The startup world port (node 1).
    #[arg(long, default_value_t = rs910_protocol::proto::world_port(1))]
    pub world_port: u16,
    /// Numbered applet parameter read at startup, e.g. `27=0` modewhere or
    /// `46=0` modegame; repeatable. Unset keys use the applet viewer's values.
    #[arg(long = "applet-param", value_name = "KEY=VALUE")]
    pub applet_param: Vec<String>,
    /// Developer shortcut: skip the title/lobby states and log straight
    /// into world 1 with `--username`/`--password` (the historical
    /// screenshot path). The default launch enters state 4:
    /// the cache login interface, typed credentials, lobby, then Play.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub direct_login: bool,
    /// `--direct-login` identity (the title screen reads typed input instead).
    #[arg(long, default_value = "tester")]
    pub username: String,
    /// `--direct-login` dev-world password (never a real credential; it
    /// travels in the RSA-encrypted login block).
    #[arg(long, default_value = "password")]
    pub password: String,
    /// The login RSA public key's modulus, in hexadecimal. Needs
    /// `--rsa-exponent`; the environment variables `ALTO_RSA_MODULUS` and
    /// `ALTO_RSA_EXPONENT` override both.
    #[arg(long, value_name = "HEX", requires = "rsa_exponent")]
    pub rsa_modulus: Option<String>,
    /// The login RSA public key's exponent, in hexadecimal.
    #[arg(long, value_name = "HEX", requires = "rsa_modulus")]
    pub rsa_exponent: Option<String>,
    /// A file holding the key as `modulus=<hex>` and `exponent=<hex>` lines
    /// (`ALTO_RSA_KEY_FILE` overrides it). Without any key option the client
    /// reads `keys/login-rsa.pub` beside the pack root's directory
    /// (`server/data/keys/login-rsa.pub`), which the development server writes
    /// (`npm --prefix server run keys:generate`).
    #[arg(long, value_name = "FILE")]
    pub rsa_key_file: Option<PathBuf>,
    /// The content server's JS5 RSA public key modulus, in hexadecimal, for a
    /// server that signs its master index. Needs `--js5-rsa-exponent`.
    /// Without it the master index trailer is read unsigned, as the
    /// development server sends it.
    #[arg(long, value_name = "HEX", requires = "js5_rsa_exponent")]
    pub js5_rsa_modulus: Option<String>,
    /// The JS5 RSA public key's exponent, in hexadecimal.
    #[arg(long, value_name = "HEX", requires = "js5_rsa_modulus")]
    pub js5_rsa_exponent: Option<String>,
    /// `on` (the default) encrypts the login blocks and masks the game
    /// stream's opcodes as the original client does. `off` sends plain
    /// blocks with zero seeds: only for tests, recorded replays and servers
    /// that run without encryption.
    #[arg(long, value_enum, default_value_t = LoginCryptoMode::On)]
    pub login_crypto: LoginCryptoMode,
    /// Cache pack root the map agent reads Lumbridge from.
    #[arg(long, default_value = "../../server/data/pack")]
    pub pack_root: PathBuf,
    /// The client's writable JS5 disk store: groups
    /// and indexes downloaded from the content server. Default
    /// `$CLIENT910_CACHE_DIR`, else `<pack root>/../cache/client910`.
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
    /// Render without a server: cached Lumbridge when the pack exists, else
    /// the synthetic heightfield — either way in a winit window.
    #[arg(long, default_value_t = false)]
    pub offline: bool,
    /// Authoritative entity/runtime path; the retained interface
    /// runtime requires it. Normal online play uses real player bodies.
    #[arg(long, default_value_t = true, action=clap::ArgAction::Set, num_args=0..=1, default_missing_value="true")]
    pub entity_state: bool,
    /// Run the floor build (the scene rebuild slice, see
    /// `rebuild.rs`) around the spawn tile from the pack, print per-level
    /// stats and write `floor_L{n}.bin` dumps into this directory. Pack-only,
    /// no window, no server.
    #[arg(long, value_name = "DIR")]
    pub dump_floor: Option<PathBuf>,
    /// With --dump-floor, record E1 frame decisions for a shared fixture file.
    #[arg(
        long,
        value_name = "FILE",
        requires = "dump_floor",
        conflicts_with = "no_locs"
    )]
    pub draw_frames: Option<PathBuf>,
    /// Dump E3 roof oracle inputs and independently produced masks/bounds.
    #[arg(long, requires = "dump_floor", conflicts_with = "no_locs")]
    pub dump_roof: bool,
    /// Centre tile for `--dump-floor` as `x,z` (default: the spawn tile).
    #[arg(long, value_name = "X,Z")]
    pub centre: Option<String>,
    /// With `--dump-floor`: skip loc placement (no wall shade stamps), to
    /// match the recording, which builds floors without locs.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_locs: bool,
    /// With `--dump-floor`: the `sceneryShadows` preference (default 2; the
    /// oracle harness uses 0).
    #[arg(long, default_value_t = 2)]
    pub scenery_shadows: i32,
    /// With `--dump-floor`: also write the raw js5 containers of the window
    /// into `<DIR>/raw` for an external rebuild of the same window.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub dump_raw: bool,
    /// Debug: unpack every group/file of one pack archive (e.g. `shaders`)
    /// into `<DIR>/<group>/<file>.bin` and exit.
    #[arg(long, num_args = 2, value_names = ["ARCHIVE", "DIR"])]
    pub dump_archive: Option<Vec<String>>,
    /// Write frame `--screenshot-frame` of the viewer to this PNG and exit.
    #[arg(long, value_name = "PNG")]
    pub screenshot: Option<PathBuf>,
    /// Which frame `--screenshot` captures (default 3).
    #[arg(long, default_value_t = 3)]
    pub screenshot_frame: u32,
    /// Camera: `orbitCameraYaw` in 14-bit game units (0 = facing north).
    #[arg(long, default_value_t = 0.0)]
    pub cam_yaw: f32,
    /// Camera: `orbitCameraPitch` in 14-bit game units (client default 1088,
    /// clamped to 1077..2787 like `clampCamera`).
    #[arg(long, default_value_t = rs910_scene::camera::DEFAULT_ORBIT_PITCH)]
    pub cam_pitch: f32,
    /// Camera: viewer-only multiplier on the client's orbit distance.
    #[arg(long, default_value_t = 1.0)]
    pub cam_zoom: f32,
    /// Camera: viewer-only multiplier on the far clip distance.
    #[arg(long, default_value_t = 1.0)]
    pub far_scale: f32,
    /// Camera: orbit target tile x/z (default: the spawn tile).
    #[arg(long, num_args = 2, value_names = ["X", "Z"])]
    pub cam_target: Option<Vec<f32>>,
    /// Keep the diagnostic free camera instead of following the online player.
    #[arg(long)]
    pub free_camera: bool,
    /// Diagnostic remote commands, sent in order after each installed map settles.
    #[arg(long = "server-command")]
    pub server_commands: Vec<String>,
    /// Diagnostic console entry/Enter replay after each settled map. Uses the
    /// live console host and renderer; does not synthesize operating-system keys.
    #[arg(long = "console-command")]
    pub console_commands: Vec<String>,
    /// Developer determinism: replace the wall clock with a fixed clock that
    /// starts at `START_MS` and advances `STEP_MS` (default 20) per logic
    /// cycle, one cycle per event-loop turn (`logic_clock` module docs).
    #[arg(long, value_name = "START_MS[:STEP_MS]")]
    pub fixed_clock: Option<String>,
    /// What draws the scene: the modern renderer (`modern`, the default) or
    /// nothing (`null`, headless). Either answers the device's capabilities,
    /// so packets, game state and saved preferences do not change.
    #[arg(long, value_enum, default_value_t = crate::active_toolkit::RendererKind::Modern)]
    pub renderer: crate::active_toolkit::RendererKind,
}

/// `--login-crypto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum LoginCryptoMode {
    /// RSA-encrypted login blocks and ISAAC-masked opcodes.
    On,
    /// Plain blocks and opcodes (tests, recorded replays).
    Off,
}

impl Cli {
    /// The login protection the command line asks for. The key is looked up
    /// once; one that cannot be found fails each login with a message
    /// naming the places searched.
    pub fn login_crypto(&self) -> rs910_client::login_crypto::LoginCrypto {
        match self.login_crypto {
            LoginCryptoMode::Off => rs910_client::login_crypto::LoginCrypto::Plain,
            LoginCryptoMode::On => rs910_client::login_crypto::LoginCrypto::from_source(
                &rs910_client::login_crypto::LoginKeySource {
                    modulus: self.rsa_modulus.clone(),
                    exponent: self.rsa_exponent.clone(),
                    key_file: self.rsa_key_file.clone(),
                    pack_root: Some(self.pack_root.clone()),
                },
            ),
        }
    }

    /// The key that opens the master index signature, when one is given.
    pub fn js5_master_index_key(&self) -> anyhow::Result<Option<RsaPublicKey>> {
        match (&self.js5_rsa_modulus, &self.js5_rsa_exponent) {
            (Some(modulus), Some(exponent)) => RsaPublicKey::from_hex(modulus, exponent)
                .context("the JS5 RSA key is invalid")
                .map(Some),
            _ => Ok(None),
        }
    }
}

/// The app side of the loading stages.
#[path = "app_loading.rs"]
mod loading_host;

/// `ViewerApp`'s owned sub-states (Phase 4.4), one module each.
mod input_router;
use input_router::*;
mod cache_cleaning;
mod console_commands;
mod console_host;
mod device_recovery;
use console_host::*;
mod scene_host;
use scene_host::*;
mod environment;
use environment::*;
mod entity_renderer;
use entity_renderer::*;
mod particle_host;
use particle_host::*;
mod view_camera;
use view_camera::*;
mod frame_clock;
use frame_clock::*;
mod diagnostics;
use diagnostics::*;
mod lifecycle;
use lifecycle::*;

/// The session owners' logic shared by the windowed shell (`ViewerApp`) (session_core.rs).
mod session_core;
use session_core::*;

mod map_build;
/// Startup and world assets: the `--direct-login` world login, session (startup.rs).
mod startup;
use startup::*;

/// `ViewerApp`'s session controller: the login flow (session_controller.rs).
mod session_controller;

/// `ViewerApp`'s preference owners: the toolkit selection, graphics (preferences.rs).
mod preferences;

/// The redraw as named steps (`render_frame`).
mod redraw;
use redraw::RedrawFrame;

/// The logic loop as a named phase list (`about_to_wait`).
mod mainloop;
use mainloop::*;

/// Typed cross-owner requests (`ClientEffect`) and their drain points.
mod effects;

/// App entry: `--offline` renders the cached/synthetic terrain immediately;
/// otherwise attempts a real world login via `net::login_world` first, then
/// opens the same viewer (live `REBUILD_NORMAL` tile streaming lands next —
/// the session is kept open in [`ViewerApp`] for that feed).
pub fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(spec) = &cli.fixed_clock {
        let (start, step) = rs910_core::logic_clock::parse_fixed(spec)?;
        rs910_core::logic_clock::install_fixed(start, step)?;
        log::info!("[client910] fixed clock: {start} ms + {step} ms per logic cycle");
    }
    rs910_core::applet_params::install(&cli.applet_param)?;
    // Checked once here so a bad key stops the launch; the JS5 owners
    // built later read the same arguments.
    cli.js5_master_index_key()?;
    if !cli.offline && cli.dump_floor.is_none() && cli.dump_archive.is_none() {
        // The JS5 disk store every Pack reads through.
        let dir = js5_cache_dir(&cli);
        rs910_js5::cache::install_disk_overlay(&dir);
        log::info!("[client910] js5 disk store {}", dir.display());
    }
    // The client's one cache reader (after the overlay install above): every
    // owner, frame path and worker reads through clones of this handle, so
    // archive indexes decode once instead of once per `Pack::open`.
    let pack = Pack::open(&cli.pack_root);
    log::info!(
        "[client910] user={} region={} group={} base={},{} (spawn lvl{} {},{})",
        cli.username,
        REGION_MAIN,
        REGION_GROUP,
        REGION_BASE_X,
        REGION_BASE_Z,
        SPAWN_LEVEL,
        SPAWN_X,
        SPAWN_Z
    );
    let drive_with = |assets: WorldAssets, session: Option<Session>| -> anyhow::Result<()> {
        log::info!(
            "[client910] world assets: spawn {},{} | floors {} levels | scene graph {}",
            assets.spawn_x,
            assets.spawn_z,
            assets.floors.iter().flatten().count(),
            assets.scene_graph.is_some(),
        );
        let event_loop = EventLoop::new().context("create winit event loop")?;
        let mut app = ViewerApp::new(cli.clone(), pack.clone(), assets, session);
        #[cfg(all(unix, any(test, feature = "dev")))]
        {
            app.control = crate::live_control::LiveControl::from_env()?;
        }
        event_loop
            .run_app(&mut app)
            .map_err(|err| anyhow::anyhow!("event loop failed: {err}"))?;
        Ok(())
    };
    let drive_cached = |session: Option<Session>| -> anyhow::Result<()> {
        let assets = match load_world_assets(&pack) {
            Ok(assets) => assets,
            Err(err) => {
                log::warn!("[client910] {err:#}; falling back to synthetic heightfield");
                WorldAssets::synthetic()?
            }
        };
        drive_with(assets, session)
    };
    if let Some(spec) = &cli.dump_archive {
        let pack = Pack::open(&cli.pack_root);
        let dir = PathBuf::from(&spec[1]);
        let index = pack.read_archive_index(&spec[0])?;
        let mut files_written = 0;
        for &group in &index.group_id {
            let files = pack.read_group(&spec[0], group)?;
            let sub = dir.join(group.to_string());
            std::fs::create_dir_all(&sub)?;
            for (file, bytes) in files {
                std::fs::write(sub.join(format!("{file}.bin")), bytes)?;
                files_written += 1;
            }
        }
        log::info!(
            "[client910] archive {}: {} groups, {files_written} files -> {}",
            spec[0],
            index.group_id.len(),
            dir.display()
        );
        return Ok(());
    }
    if let Some(dir) = &cli.dump_floor {
        return run_dump_floor(&cli, dir);
    }
    if cli.offline {
        drive_cached(None)
    } else if cli.direct_login {
        let (assets, session) = online_login(&cli, &pack)?;
        drive_with(assets, Some(session))
    } else {
        // The startup (client state 5): the window opens first and the
        // loading updates build the client owners under the loading
        // screens, entering state 4 from the last stage.
        anyhow::ensure!(
            cli.entity_state,
            "the title/lobby flow needs --entity-state"
        );
        // No scene exists before a world login (the title/lobby draw shows
        // interfaces only); the placeholder assets are never presented. The
        // pack root feeds the cursor and interface-model owners.
        let mut assets = WorldAssets::synthetic()?;
        assets.pack_root = Some(cli.pack_root.clone());
        let event_loop = EventLoop::new().context("create winit event loop")?;
        let mut app = ViewerApp::new(cli.clone(), pack.clone(), assets, None);
        #[cfg(all(unix, any(test, feature = "dev")))]
        {
            app.control = crate::live_control::LiveControl::from_env()?;
        }
        app.lifecycle.loading = Some(loading_host::LoadingOwner::startup(&cli, pack.clone()));
        event_loop
            .run_app(&mut app)
            .map_err(|err| anyhow::anyhow!("event loop failed: {err}"))?;
        Ok(())
    }
}

/// `--dump-floor`: the scene-rebuild floor slice over the
/// pack, reported and dumped for diffing against the recording.
fn run_dump_floor(cli: &Cli, dir: &Path) -> anyhow::Result<()> {
    let (cx, cz) = match cli.centre.as_deref() {
        Some(spec) => {
            let (a, b) = spec
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("--centre expects x,z"))?;
            (a.trim().parse::<i32>()?, b.trim().parse::<i32>()?)
        }
        None => (SPAWN_X, SPAWN_Z),
    };
    let pack = Pack::open(&cli.pack_root);
    let flo = FloStore::load(&pack).context("load flo configs")?;
    let tables = rs910_scene::maploader::FloTables::from_store(&flo);
    let materials = rs910_config::texture::MaterialStore::load(&pack).context("load materials")?;
    let locs = if cli.no_locs {
        None
    } else {
        Some(config::LocStore::load(&pack).context("load loc configs")?)
    };
    let prefs = rs910_scene::rebuild::BuildPrefs {
        scenery_shadows: cli.scenery_shadows,
        ..rs910_scene::rebuild::BuildPrefs::default()
    };
    log::info!(
        "[client910] --dump-floor: centre {cx},{cz} locs {} prefs {prefs:?}",
        !cli.no_locs
    );
    let mut result = rs910_scene::rebuild::rebuild_normal(
        &pack,
        &tables,
        &materials,
        locs.as_ref(),
        cx,
        cz,
        &prefs,
    )?;
    rs910_scene::rebuild::report_with(&result, Some(dir), Some(&materials))?;
    if cli.dump_raw {
        rs910_scene::rebuild::dump_raw_inputs(
            &pack,
            &dir.join("raw"),
            &result.squares,
            &result.model_ids,
        )?;
    }
    if cli.dump_roof {
        rs910_scene::roof_fixtures::write(&result, dir)?;
    }
    if let Some(input) = &cli.draw_frames {
        rs910_scene::draw::write_oracle(&mut result, input, &dir.join("frames.bin"), &materials)?;
        std::fs::write(
            dir.join("raster.bin"),
            rs910_scene::occlusion_fixtures::raster_trace().encode(),
        )?;
    }
    Ok(())
}

// Phase 4: the login workers and their runtime are rs910-client's
// (`login_worker`).
use rs910_client::login_worker::*;

// Phase 5: the session core is rs910-client's (`client_core`).
use rs910_client::client_core::*;

// Phase 4: `ToolkitCaps` is rs910-client's (`toolkit_caps`).
pub(crate) use rs910_client::toolkit_caps::*;

/// The world login reply fields the retained interface runtime starts from
/// (`GAMELOGIN_CONTINUE`): the login reply's own
/// `net::LoginProfile` (lane Q-SESSION).
#[cfg(all(test, alto_sessions))]
use rs910_client::net::LoginProfile;

/// Scene levels baked (0..3, matches the four scene level tiles).
pub(crate) const NUM_LEVELS: usize = 4;

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

/// Winit viewer: owns the window, renderer, scene, camera and input
/// state.
struct ViewerApp {
    /// The client core (Phase 5): the live world session (online mode;
    /// [`ViewerApp::poll_live_rebuild`] polls it once per logic cycle) and
    /// the logic cycle counter.
    core: ClientCore,
    /// The core's clock, transport and platform boundary (`LiveIo`, or the
    /// RTR1 `RecordingIo` under `CLIENT910_RECORD`); lent to
    /// `ClientCore::frame` by `about_to_wait`, so absent only during it.
    io: Option<Box<dyn Io>>,
    #[cfg(all(unix, any(test, feature = "dev")))]
    control: Option<crate::live_control::LiveControl>,
    window: Option<Arc<Window>>,
    renderer: Option<ActiveToolkit>,
    /// The client's one cache reader (see `run`): per-frame and per-cycle
    /// consumers clone this handle instead of re-opening the pack, so the
    /// decoded archive indexes are shared. Gated by `pack_root` as before.
    pack: Pack,
    /// Minimap static state (minimap.rs).
    minimap: rs910_scene::minimap::Minimap,
    /// Launch options, kept for the loading stages that build the session.
    cli: Cli,
    /// The JS5 TCP and HTTP clients, disk cache, client, providers and
    /// archives, and the JS5 connect state.
    js5: rs910_js5::js5net::Js5System,
    /// A rebuild waiting for its map squares (the landscape progress)
    /// before the map worker loads it.
    js5_map_wait: Option<rs910_client::session::RebuildEvent>,
    map_build: map_build::MapBuild,
    /// Window input the shell routes to the interface runtime, the developer
    /// console and the free camera: the `Cursor` owner, the mouse wheel
    /// accumulator, held modifiers and keys, focus, the free-camera drag and
    /// the pending native resize.
    input: InputRouter,
    /// The developer console and its shell inputs: the keys
    /// and wheel the window events queue for the console update,
    /// the `--console-command` replay and the lines
    /// queued for it.
    console: ConsoleHost,
    /// The installed scene (the scene rebuild and the scene graph): the floors, the placed scene graph and live scene,
    /// static lights, the underwater level, loc-change slots, the build
    /// preferences it was built with and the packets waiting for it.
    scene: SceneHost,
    /// The environment's toolkit side: the
    /// colour remappers and the skybox cache (the map, override and fade are
    /// the core's).
    environment: EnvironmentController,
    /// The scene entities' draw state: the per-frame model caches (effect,
    /// hint, ground, loc and NPC type models, obj stack
    /// radii), the loc store they read, the players renderer
    /// (player and NPC bodies and picking) and the animation,
    /// billboard and emitter stores.
    entities: EntityRenderer,
    /// The particle system and GPU particle renderer state and the
    /// particle bindings of scene entities.
    particles: ParticleHost,
    /// The viewer camera (`OrbitCamera` and the client camera it derives) and
    /// whether it follows the server player.
    view: ViewCamera,
    /// The logic clock (20 ms logic cycles, the logic cycle counter) and
    /// the redraw counters (fps, frames
    /// rendered, window-title refresh).
    clock: FrameClock,
    /// The redraw's hand-offs between its steps and the last full redraw's
    /// frame, which presents draw again (app/redraw.rs).
    frame: RedrawFrame,
    /// Headless verification state (client-only): the `--screenshot` target
    /// and frame.
    diag: Diagnostics,
    /// The client lifecycle around the session: the loading owner (states
    /// 5/11/1), whether the login UI was shown, the title/lobby world,
    /// the canvas size, the window-title username and a
    /// fatal cheat's shutdown request.
    lifecycle: Lifecycle,
    /// When the caches are cleaned (`app/cache_cleaning.rs`), created at the
    /// first redraw after loading.
    cache_schedule: Option<rs910_core::cache_schedule::Schedule>,
    /// Where the answers to GPU device faults are (`app/device_recovery.rs`).
    device_ladder: device_recovery::Ladder,
}

impl ViewerApp {
    fn new(cli: Cli, pack: Pack, assets: WorldAssets, mut session: Option<Session>) -> Self {
        let colour_remappers = rs910_render_gpu::postprocess::RemapperCache::new(
            assets.pack_root.as_ref().map(|_| pack.clone()),
        );
        let launch = cli.clone();
        let spawn_h = assets.spawn_ground();
        let target = glam::Vec3::new(assets.spawn_x as f32, spawn_h, assets.spawn_z as f32);
        let follow_camera = !cli.free_camera
            && cli.cam_target.is_none()
            && session.as_ref().is_some_and(|s| s.game.is_some());
        if let Some(game) = session.as_mut().and_then(|s| s.game.as_mut()) {
            game.camera.pitch = cli.cam_pitch;
            game.camera.yaw = cli.cam_yaw;
        }
        // The old literal's initialisers, in its order.
        let cursor_state = Default::default();
        let applied_scene_preferences = session.as_ref().and_then(|s| s.game.as_ref()).map(|g| {
            rs910_scene::rebuild::BuildPrefs::from_options(
                &g.ui_variables.queries.preferences.options,
            )
        });
        let console = Default::default();
        let console_commands = cli.console_commands.into();
        let mouse_wheel = Default::default();
        let particle_builder = rs910_scene::particle_quads::Builder::default();
        let minimap = rs910_scene::minimap::Minimap::default();
        let screenshot = cli.screenshot.clone().map(|p| (p, cli.screenshot_frame));
        let camera = {
            let mut camera = OrbitCamera::new(target);
            if let Some(t) = cli.cam_target.as_ref().filter(|t| t.len() == 2) {
                camera.target.x = t[0];
                camera.target.z = t[1];
            }
            camera.yaw = cli.cam_yaw;
            camera.pitch = cli.cam_pitch;
            camera.dist = cli.cam_zoom;
            camera.far_scale = cli.far_scale;
            camera
        };
        let pending_resize = Default::default();
        let last_frame = rs910_core::logic_clock::now();
        let logic_clock = rs910_core::logic_clock::Clock::new();
        let last_title = Instant::now();
        let last_render = Instant::now();
        let js5 = new_js5_system(&launch);
        let title_world = Default::default();
        let mut core = ClientCore::new(session);
        core.profile = rs910_render_gpu::render_debug_flags::flags().profile;
        core.audio.pending_loc_sounds = assets.loc_sounds;
        core.environment.env = assets.env;
        core.environment.fade_reset = true;
        Self {
            core,
            io: Some(
                if rs910_client::client_debug_flags::flags().record.is_some() {
                    Box::new(
                        if rs910_client::client_debug_flags::flags().record_interactive {
                            RecordingIo::interactive(LiveIo)
                        } else {
                            RecordingIo::new(LiveIo)
                        },
                    )
                } else {
                    Box::new(LiveIo)
                },
            ),
            #[cfg(all(unix, any(test, feature = "dev")))]
            control: None,
            window: None,
            renderer: None,
            pack,
            minimap,
            js5,
            js5_map_wait: None,
            map_build: Default::default(),
            cli: launch,
            input: InputRouter {
                cursor_resources: None,
                cursor_state,
                mouse_wheel,
                modifiers: ModifiersState::empty(),
                focused: true,
                pressed: HashSet::new(),
                dragging: false,
                last_cursor: None,
                pending_resize,
            },
            console: ConsoleHost {
                developer: console,
                keys: vec![],
                commands: console_commands,
                pending_messages: vec![],
                next_cycle: 100,
                wheel: 0,
                output: None,
                commands_anywhere: console_commands::COMMANDS_ANYWHERE,
            },
            scene: SceneHost {
                applied_preferences: applied_scene_preferences,
                focus_level: assets.focus_level,
                flo: assets.flo,
                floors: assets.floors,
                material_store: assets.material_store,
                pack_root: assets.pack_root,
                floor_base: assets.floor_base,
                underwater: assets.underwater_floor.map(|floor| {
                    Arc::new(rs910_scene::scene_snapshot::UnderwaterScene::new(
                        floor,
                        assets.underwater_models,
                    ))
                }),
                graph: assets.scene_graph,
                live: assets.live_scene,
                lights: assets.lights,
                loc_changes: HashMap::new(),
                loc_slots: None,
                pending_point_lights: Vec::new(),
                pending_hint_arrows: Vec::new(),
                meshless: false,
            },
            environment: EnvironmentController {
                sky: None,
                colour_remappers,
            },
            entities: EntityRenderer {
                effect_models: Default::default(),
                hint_models: Default::default(),
                ground_models: Default::default(),
                obj_stack_radius: HashMap::new(),
                ground_stacks: Default::default(),
                npc_definitions: Default::default(),
                loc_models: Default::default(),
                added_loc_animations: HashMap::new(),
                added_loc_random: rs910_game::animation_playback::AnimationRandom::new(0),
                loc_store: None,
                npcs: Default::default(),
                npc_picks: HashMap::new(),
                players: None,
                animation_assets: None,
                billboards: None,
                emitters: None,
                cover_marker_memory: Default::default(),
            },
            particles: ParticleHost {
                npc_bindings: HashMap::new(),
                temporary: HashMap::new(),
                resets_seen: HashMap::new(),
                effect_bindings: Vec::new(),
                runtime: None,
                builder: particle_builder,
                base: None,
            },
            view: ViewCamera {
                camera,
                follow: follow_camera,
                last_redraw: None,
                camera_unready: false,
            },
            clock: FrameClock {
                last_frame,
                logic: logic_clock,
                last_title,
                last_render,
                next_render: last_render,
                occluded: false,
                frames: 0,
                fps: 0.0,
            },
            frame: Default::default(),
            diag: Diagnostics {
                screenshot,
                screenshot_queued: false,
                screenshot_series_next: 0,
            },
            lifecycle: Lifecycle {
                loading: None,
                login_ui_shown: false,
                title_world,
                client_frame: rs910_ui::loading::DEFAULT_FRAME,
                username: cli.username,
                shutdown_requested: false,
            },
            cache_schedule: None,
            device_ladder: Default::default(),
        }
    }
}

impl ApplicationHandler for ViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut attributes =
            Window::default_attributes().with_title(rs910_core::applet_params::get().game_title());
        if let Some([width, height]) = crate::debug_flags::flags().window_size {
            attributes = attributes.with_inner_size(winit::dpi::LogicalSize::new(width, height));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::warn!("[client910] create window failed: {err}");
                event_loop.exit();
                return;
            }
        };
        let renderer =
            match pollster::block_on(ActiveToolkit::new(Arc::clone(&window), self.cli.renderer)) {
                Ok(renderer) => renderer,
                Err(err) => {
                    log::warn!("[client910] renderer init failed: {err:#}");
                    event_loop.exit();
                    return;
                }
            };
        log::info!("[client910] GPU: {}", renderer.adapter_info());
        rs910_core::hardware::set_gpu_description(&renderer.adapter_info());
        // Interface opens arrive live on the session and the retained UI
        // runtime applies them in `poll_live_rebuild`; nothing hardcoded
        // opens here. Offline builds have no session and draw the scene only.
        if self.core.session.is_some() {
            log::info!("[client910] live UI owns interfaces (see poll_live_rebuild)");
        }
        self.lifecycle.login_ui_shown = true;

        self.window = Some(window);
        self.renderer = Some(renderer);
        // Canvas units are AWT user-space, i.e. winit logical pixels: physical pieces
        // (surface, mouse events) convert through the window scale factor.
        let sf = self.window.as_ref().unwrap().scale_factor();
        let renderer = self.renderer.as_mut().unwrap();
        renderer.set_ui_scale(sf);
        renderer.set_modern_preferences(crate::modern_display::load_preferences(
            &crate::modern_display::path(&self.cli.pack_root),
        ));
        // A session built before the window (`--direct-login`) passed the
        // loading stages already: the toolkit selection (the last loading
        // stage) precedes its canvas. The startup loading owner applies both
        // from its own stages instead.
        self.install_session_toolkit();
        let debug_events = self.install_session_canvas();
        self.install_scene_floors();
        if let Err(error) = self.finish_game_map() {
            log::warn!("[client910] initial map not acknowledged: {error:#}");
        }
        self.apply_session_events(&debug_events);
        if let Some(connection) = self
            .core
            .session
            .as_mut()
            .and_then(|s| s.io.startup_connection.take())
        {
            if let Err(error) = connection.finish() {
                log::warn!("[client910] startup connection: {error}");
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let own_window = matches!(self.window.as_ref(), Some(window) if window.id() == window_id);
        if !own_window {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.io()
                    .window_event(WindowRecord::Resized([size.width, size.height]));
                self.input.pending_resize.receive([size.width, size.height]);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_ui_scale(scale_factor);
                }
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    self.input.pending_resize.receive([size.width, size.height]);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.input.modifiers = modifiers.state(),
            WindowEvent::Focused(focused) => {
                self.io().window_event(WindowRecord::Focused(focused));
                self.input.focused = focused;
                if !focused {
                    // Losing focus releases held input.
                    self.input.pressed.clear();
                    self.input.dragging = false;
                    self.input.last_cursor = None;
                    self.input.modifiers = ModifiersState::empty();
                    if let Some(session) = self.core.session.as_mut() {
                        session_focus_lost(session);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Scripted recordings isolate their injector input. Interactive
                // recordings retain native input at the same event boundary.
                if self.io().drops_os_input() {
                    self.io().window_event(WindowRecord::Input(1));
                    return;
                }
                // The loading sequence is the only reader of the keyboard events in
                // the loading states.
                if let Some(owner) = self.lifecycle.loading.as_mut() {
                    if event.state == ElementState::Pressed {
                        if let Some(text) = event.text.as_ref() {
                            owner.keys.extend(text.chars());
                        }
                    }
                    return;
                }
                if event.state == ElementState::Pressed {
                    // The console key with CTRL held toggles the free camera for
                    // staff >= 2.
                    if event.physical_key == PhysicalKey::Code(KeyCode::Backquote)
                        && self.input.modifiers.control_key()
                        && !event.repeat
                    {
                        if let Some(ui) = self.core.session.ui_mut() {
                            if ui.engine.account.staff_mod_level >= 2 {
                                if ui.engine.camera.free_camera.take().is_none() {
                                    if let Some(player) = ui.engine.camera.cam2.scene.local_player {
                                        ui.engine.camera.free_camera =
                                            Some(rs910_ui::ui_cam2::FreeCamera::create(
                                                player.level,
                                                player.coord,
                                                ui.engine.platform.mouse,
                                                &ui.engine.camera.cam2.scene,
                                            ));
                                    }
                                }
                                return;
                            }
                        }
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::Backquote) && !event.repeat
                    {
                        // The console shortcut needs Alt
                        // unless the console-key preference is 0; otherwise
                        // the key falls through with the shortcut hint.
                        let key_press = self.core.session.game().map_or(1, |g| {
                            g.ui_variables
                                .queries
                                .preferences
                                .options
                                .live()
                                .console_key
                        });
                        if key_press == 0 || self.input.modifiers.alt_key() {
                            if let Err(error) = self.toggle_console() {
                                log::warn!("[client910] console: {error:#}");
                            }
                            return;
                        }
                        if let Some(ui) = self.core.session.ui_mut() {
                            // The developer console shortcut hint.
                            ui.engine.messages.system_message(
                                rs910_core::texts::Msg::DeveloperConsoleShortcutInfo.get(),
                            );
                        }
                    }
                    if self.console.developer.open {
                        self.queue_console_key(event.physical_key, event.text.as_deref());
                        return;
                    }
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.accept_input(InputEvent::Key {
                        code: crate::ui_keyboard_winit::awt_keycode(code),
                        pressed: event.state == ElementState::Pressed,
                        text: event.text.map(|text| text.to_string()),
                        time: rs910_core::logic_clock::monotonic_millis(),
                    });
                    if event.state == ElementState::Pressed {
                        if code == KeyCode::Escape && self.core.session.is_none() {
                            event_loop.exit();
                            return;
                        }
                        self.input.pressed.insert(code);
                    } else {
                        self.input.pressed.remove(&code);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Scripted recordings isolate their injector input. Interactive
                // recordings retain native input at the same event boundary.
                if self.io().drops_os_input() {
                    self.io().window_event(WindowRecord::Input(2));
                    return;
                }
                if self.console.developer.open {
                    return;
                }
                // Button action 0 left, 1
                // middle, 2 right; one queued event per cycle (the head). winit
                // reports no click count, so every press counts as 1.
                let action = match button {
                    MouseButton::Left => Some(0),
                    MouseButton::Middle => Some(1),
                    MouseButton::Right => Some(2),
                    _ => None,
                };
                if let Some(action) = action {
                    self.accept_input(InputEvent::Button {
                        action,
                        pressed: state.is_pressed(),
                        time: rs910_core::logic_clock::monotonic_millis(),
                    });
                }
                let retained = self.core.session.is_some();
                if !retained && button == MouseButton::Left {
                    self.input.dragging = state.is_pressed();
                    self.input.last_cursor = None;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Scripted recordings isolate their injector input. Interactive
                // recordings retain native input at the same event boundary.
                if self.io().drops_os_input() {
                    self.io().window_event(WindowRecord::Input(3));
                    return;
                }
                let canvas = self.core.session.game().and_then(|g| {
                    g.ui_variables.queries.preferences.window.canvas(
                        g.ui_variables
                            .queries
                            .preferences
                            .options
                            .live()
                            .screen_size,
                    )
                });
                if self.core.session.is_some() {
                    // winit reports physical pixels; the mouse works in canvas
                    // (logical) units, like every other UI coordinate.
                    let sf = self
                        .window
                        .as_ref()
                        .map(|w| w.scale_factor())
                        .unwrap_or(1.0)
                        .max(1.0);
                    let canvas_pos = canvas.map_or(
                        [
                            (position.x / sf).round() as i32,
                            (position.y / sf).round() as i32,
                        ],
                        |c| c.mouse([position.x, position.y], sf),
                    );
                    self.accept_input(InputEvent::Move {
                        position: canvas_pos,
                        time: rs910_core::logic_clock::monotonic_millis(),
                    });
                    self.input.last_cursor = Some((position.x, position.y));
                }
                if self.input.dragging && self.core.session.is_none() {
                    if let Some((last_x, last_y)) = self.input.last_cursor {
                        let dx = (position.x - last_x) as f32;
                        let dy = (position.y - last_y) as f32;
                        if self.view.follow {
                            if let Some(game) = self.core.session.game_mut() {
                                game.camera.yaw -= dx * 13.;
                                game.camera.pitch = (game.camera.pitch + dy * 13.).clamp(
                                    rs910_scene::camera::ORBIT_PITCH_MIN,
                                    rs910_scene::camera::ORBIT_PITCH_MAX,
                                );
                            }
                        } else {
                            self.view.camera.rotate(dx, dy);
                        }
                    }
                    self.input.last_cursor = Some((position.x, position.y));
                }
                if let Err(error) = self.sync_cursor(event_loop) {
                    crate::logging::warn_repeated!("[client910] cursor move: {error:#}");
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Scripted recordings isolate their injector input. Interactive
                // recordings retain native input at the same event boundary.
                if self.io().drops_os_input() {
                    self.io().window_event(WindowRecord::Input(4));
                    return;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => lines,
                    MouseScrollDelta::PixelDelta(pos) => (pos.y / 100.0) as f32,
                };
                let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.);
                let rotation = self.input.mouse_wheel.rotation(delta, scale);
                if self.console.developer.open {
                    self.console.wheel = self.console.wheel.wrapping_add(rotation);
                    return;
                }
                if self.core.session.is_some() {
                    // AWT wheel rotation is positive downwards.
                    self.accept_input(InputEvent::Wheel { delta: rotation });
                    return;
                }
                // Wheel up (positive) zooms in.
                let factor = (1.0 - lines * 0.08).clamp(0.25, 4.0);
                self.view.camera.zoom(factor);
            }
            WindowEvent::Occluded(occluded) => {
                self.clock.occluded = occluded;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_occluded(occluded);
                }
            }
            WindowEvent::RedrawRequested => {
                self.service_gpu_device();
                // A full redraw (the first after logic cycles) records the fps
                // ring (the fps script stats, the ping reply's fps) and, after
                // the game draw, updates the cursor. A
                // present writes neither (`ClientCore::redraw`).
                let full_redraw = self.core.redraw_due();
                if full_redraw {
                    rs910_ui::ui_runtime::host_builtins::record_redraw(
                        rs910_core::logic_clock::monotonic_millis(),
                    );
                }
                let elapsed = self.clock.presentation_started(Instant::now());
                if elapsed > 0.0 {
                    self.clock.fps = self.clock.fps * 0.9 + (1.0 / elapsed.max(1e-4)) * 0.1;
                }
                rs910_core::profile::scope!("redraw", self.render_frame());
                if full_redraw {
                    if let Err(error) = self.sync_cursor(event_loop) {
                        crate::logging::warn_repeated!(
                            "[client910] cursor update failed: {error:#}"
                        );
                    }
                }
                let cpu = self.core.session.game().map_or(4, |g| {
                    g.ui_variables.queries.preferences.options.live().cpu_usage
                });
                rs910_core::profile::scope!(
                    "cpuUsage sleep",
                    rs910_client::graphics_runtime::finish_frame(cpu)
                );
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Shut the client down.
        self.mainquit();
        // Let the GPU finish before the toolkit drops (shutdown_signal.rs).
        if let Some(renderer) = &mut self.renderer {
            renderer.wait_idle();
        }
        rs910_core::profile::finish();
    }

    /// The logic loop once per granted logic cycle (`ClientCore::frame`
    /// over the windowed shell), then the frame tail (app/mainloop.rs).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(renderer) = &mut self.renderer {
            renderer.poll_render();
        }
        if crate::shutdown_signal::requested() {
            event_loop.exit();
            return;
        }
        rs910_core::profile::frame_mark();
        let (dt, logic_steps) = self.clock.begin_frame();
        if logic_steps > 0 {
            self.end_presentation();
        }
        let mut io = self
            .io
            .take()
            .expect("the shell's Io is lent only during a frame");
        let end = ClientCore::frame(
            &mut Windowed {
                app: self,
                event_loop,
            },
            &mut *io,
            logic_steps,
        );
        self.io = Some(io);
        if end == Cycle::Exit {
            event_loop.exit();
            return;
        }
        if !rs910_core::profile::scope!("F1 frame tail", self.frame_tail(dt)) {
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(
            self.wake_deadline()
                .map_or(ControlFlow::Poll, ControlFlow::WaitUntil),
        );
    }
}

#[cfg(test)]
mod tests;

/// Maintainer-only session replays; `build.rs` enables them when the gitignored
/// folder is present.
#[cfg(all(test, alto_sessions))]
#[path = "../fixtures/session-replay/rust/sessions.rs"]
mod sessions;
