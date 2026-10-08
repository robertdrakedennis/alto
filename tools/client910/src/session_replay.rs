//! Headless session replay (test-audit.md fix programme item 2; the seed of
//! the Phase 0 RTR1 replay harness, target-architecture.md §5).
//!
//! A real `--direct-login` session against the dev server was recorded with
//! `CLIENT910_RECORD` (`session_record.rs`; regenerate with
//! `tools/client910/fixtures/session-replay/record.sh`). This module feeds the
//! recorded world-socket bytes back through the production owners the app
//! uses, in `ViewerApp::about_to_wait`'s order, with no window or GPU and the
//! recorded clock:
//!
//! - startup: `session::drain_frames_mode` → `Game::login` →
//!   `install_client_persistence` / `restore_server_varcs` → `Game::apply_next`
//!   → `prepare_game_scene` → `retained_session_ui` → `new_session`, then the
//!   `resumed` half: `install_toolkit_preferences`, `install_canvas_state`,
//!   `acknowledge_game_map` (`MAP_BUILD_COMPLETE`);
//! - per logic cycle: `begin_session_cycle`, the `CLIENT910_*` injectors
//!   (`apply_ui_injection`), `poll_live_session` (a real loopback socket:
//!   `poll_live_once` → `drain_game_frames` → `handle_sync_frame` /
//!   `Game::apply_next` / `ui_packet`), `apply_hint_arrows`,
//!   `update_session_logic` (`poll_vars`, camera, cutscene input,
//!   `update_scene_state` → `update_actors`, transients, the UI/audio tick),
//!   `sync_window_state` and `dispatch_server_command`.
//!
//! Shell-only owners (renderer, JS5, console, title world, lobby socket) are
//! not run; the recording captures what they handed the logic (canvas, toolkit
//! capabilities, GL formats, cursor, focus, window size).
use super::*;
use crate::session_record::Record;
use crate::test_support::ReclaimedAtExit;
use clap::Parser;
use std::collections::BTreeMap;
use std::io::Write;

pub(super) const FIXTURE: &str = "fixtures/session-replay/session.rtr";

/// The round trip a replayed ping report carries.
pub(super) const REPLAY_PING_MS: i32 = 37;

pub(super) const OBSERVED_BACKEND_HEAD: &str = "observed_backend";
pub(super) const AUTHENTICATED_HEADLESS_BACKEND: &str = "authenticated_headless";
pub(super) const OBSERVED_SERVER_CLOCK_HEAD: &str = "server_clock";
pub(super) const OBSERVED_MEMBERSHIP_CLOCK_HEAD: &str = "observed_membership_millis";
const STARTUP_RECORDING_CYCLE: i32 = -1;
pub(super) const INITIAL_RECORDING_OUTPUT_CYCLE: i32 = 0;
pub(super) const MONOTONIC_CLOCK_HEAD: &str = "monotonic_clock_samples";
pub(super) const MONOTONIC_CLOCK_FORMAT: &str = "scoped-v1";
pub(super) const STARTUP_CLOCK_SAMPLES: [u8; 4] = *b"CLKS";
pub(super) const FRAME_CLOCK_SAMPLES: [u8; 4] = *b"CLKF";
pub(super) const INPUT_CLOCK_SAMPLES: [u8; 4] = *b"CLKI";
pub(super) const REDRAW_CLOCK_SAMPLES: [u8; 4] = *b"CLKD";

fn decode_clock_samples(bytes: &[u8]) -> anyhow::Result<Vec<i64>> {
    const SAMPLE_BYTES: usize = std::mem::size_of::<i64>();
    anyhow::ensure!(
        bytes.len().is_multiple_of(SAMPLE_BYTES),
        "monotonic sample width"
    );
    anyhow::ensure!(
        bytes.len() / SAMPLE_BYTES <= crate::logic_clock::MAX_MONOTONIC_SAMPLES,
        "monotonic sample count"
    );
    bytes
        .chunks_exact(SAMPLE_BYTES)
        .map(|bytes| Ok(i64::from_le_bytes(bytes.try_into()?)))
        .collect()
}

/// A decoded recording.
pub(super) struct Trace {
    pub records: Vec<Record>,
    pub head: Vec<(String, String)>,
}

impl Trace {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let records = crate::session_record::read(&std::fs::read(path)?)?;
        let head = records
            .iter()
            .find(|r| &r.tag == b"HEAD")
            .context("no HEAD record")?;
        let head = std::str::from_utf8(&head.bytes)?
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Ok(Self { records, head })
    }
    pub fn head(&self, key: &str) -> anyhow::Result<&str> {
        self.head
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .with_context(|| format!("HEAD {key}"))
    }
    fn sampled_clock(&self) -> anyhow::Result<bool> {
        match self
            .head
            .iter()
            .find(|(key, _)| key == MONOTONIC_CLOCK_HEAD)
        {
            None => Ok(false),
            Some((_, value)) => {
                anyhow::ensure!(
                    value == MONOTONIC_CLOCK_FORMAT,
                    "unknown monotonic sample format"
                );
                anyhow::ensure!(
                    self.head(OBSERVED_BACKEND_HEAD)? == AUTHENTICATED_HEADLESS_BACKEND,
                    "monotonic tape requires the authenticated headless backend"
                );
                self.validate_clock_samples()?;
                Ok(true)
            }
        }
    }
    /// New tape traces must account for every phase, including empty ones.
    fn validate_clock_samples(&self) -> anyhow::Result<()> {
        const ONE_PHASE: usize = 1;
        let mut frames = std::collections::BTreeSet::new();
        let mut inputs: BTreeMap<i32, usize> = BTreeMap::new();
        let mut phases: BTreeMap<(i32, [u8; 4]), usize> = BTreeMap::new();
        for record in &self.records {
            if record.tag == *b"NOWM" && record.cycle > INITIAL_RECORDING_OUTPUT_CYCLE {
                anyhow::ensure!(
                    record.bytes.len() == std::mem::size_of::<i64>(),
                    "frame clock width"
                );
                anyhow::ensure!(
                    frames.insert(record.cycle),
                    "duplicate frame clock at {}",
                    record.cycle
                );
            }
            if &record.tag == rs910_client::client_core::input_event::RECORD_TAG {
                *inputs.entry(record.cycle).or_default() += ONE_PHASE;
            }
            if [
                STARTUP_CLOCK_SAMPLES,
                FRAME_CLOCK_SAMPLES,
                INPUT_CLOCK_SAMPLES,
                REDRAW_CLOCK_SAMPLES,
            ]
            .contains(&record.tag)
            {
                decode_clock_samples(&record.bytes)?;
                *phases.entry((record.cycle, record.tag)).or_default() += ONE_PHASE;
            }
        }
        anyhow::ensure!(
            phases.get(&(STARTUP_RECORDING_CYCLE, STARTUP_CLOCK_SAMPLES)) == Some(&ONE_PHASE),
            "exactly one startup clock phase is required"
        );
        for ((cycle, tag), count) in &phases {
            anyhow::ensure!(*count == ONE_PHASE, "duplicate clock phase at {cycle}");
            if *tag == STARTUP_CLOCK_SAMPLES {
                anyhow::ensure!(
                    *cycle == STARTUP_RECORDING_CYCLE,
                    "orphan startup clock phase"
                );
            } else {
                anyhow::ensure!(frames.contains(cycle), "orphan clock phase at {cycle}");
                if *tag == INPUT_CLOCK_SAMPLES {
                    anyhow::ensure!(
                        inputs.get(cycle) == Some(&ONE_PHASE),
                        "input clock phase requires one accepted input at {cycle}"
                    );
                }
            }
        }
        for cycle in &frames {
            for tag in [FRAME_CLOCK_SAMPLES, REDRAW_CLOCK_SAMPLES] {
                anyhow::ensure!(
                    phases.get(&(*cycle, tag)) == Some(&ONE_PHASE),
                    "missing frame/redraw clock phase at {cycle}"
                );
            }
        }
        for (cycle, count) in &inputs {
            anyhow::ensure!(
                *count == ONE_PHASE && frames.contains(cycle),
                "accepted input must belong to one actual frame at {cycle}"
            );
            anyhow::ensure!(
                phases.get(&(*cycle, INPUT_CLOCK_SAMPLES)) == Some(&ONE_PHASE),
                "accepted input has no exact clock phase at {cycle}"
            );
        }
        Ok(())
    }
    fn clock_samples(&self, tag: &[u8; 4], cycle: i32) -> anyhow::Result<Vec<i64>> {
        let mut records = self
            .records
            .iter()
            .filter(|record| &record.tag == tag && record.cycle == cycle);
        let record = records.next().context("missing monotonic phase")?;
        anyhow::ensure!(records.next().is_none(), "duplicate monotonic phase");
        decode_clock_samples(&record.bytes)
    }
    /// New recordings carry independent worker-completion ingress. Historical
    /// authenticated traces predate that hook: recover only the observed external
    /// round trip and completion cycle from their ping report, never output bytes.
    fn ping_completions(&self) -> anyhow::Result<BTreeMap<i32, i32>> {
        use crate::connection_upkeep::{PING_COMPLETION_RECORD_TAG, PING_UNANSWERED_MS};
        let independent = self
            .records
            .iter()
            .any(|record| record.tag == PING_COMPLETION_RECORD_TAG);
        let mut completions = BTreeMap::new();
        let mut insert = |cycle, round_trip| -> anyhow::Result<()> {
            anyhow::ensure!(
                cycle > INITIAL_RECORDING_OUTPUT_CYCLE && self.now(cycle).is_some(),
                "ping completion has no recorded logic cycle"
            );
            anyhow::ensure!(
                (0..=PING_UNANSWERED_MS).contains(&round_trip),
                "ping completion round trip is out of range"
            );
            anyhow::ensure!(
                completions.insert(cycle, round_trip).is_none(),
                "duplicate ping completion at cycle {cycle}"
            );
            Ok(())
        };
        if independent {
            for record in &self.records {
                if record.tag == PING_COMPLETION_RECORD_TAG {
                    insert(
                        record.cycle,
                        i32::from_le_bytes(
                            record
                                .bytes
                                .as_slice()
                                .try_into()
                                .context("ping completion width")?,
                        ),
                    )?;
                }
            }
        } else {
            let mut written: BTreeMap<i32, Vec<u8>> = BTreeMap::new();
            for record in &self.records {
                if record.tag == *b"OUT " {
                    written
                        .entry(record.cycle)
                        .or_default()
                        .extend(&record.bytes);
                }
            }
            for (cycle, bytes) in written {
                for (opcode, payload) in client_frames(&bytes)? {
                    if opcode == crate::proto::client::PING_STATISTICS {
                        const PING_PAYLOAD_BYTES: usize = 4;
                        anyhow::ensure!(payload.len() == PING_PAYLOAD_BYTES, "ping report width");
                        insert(
                            cycle,
                            i32::from(u16::from_le_bytes([payload[0], payload[1]])),
                        )?;
                    }
                }
            }
        }
        Ok(completions)
    }

    fn head_all(&self, key: &str) -> Vec<String> {
        self.head
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }
    pub fn bytes(&self, tag: &[u8; 4], cycle: i32) -> Vec<u8> {
        self.records
            .iter()
            .filter(|r| &r.tag == tag && r.cycle == cycle)
            .flat_map(|r| r.bytes.iter().copied())
            .collect()
    }
    fn one(&self, tag: &[u8; 4]) -> anyhow::Result<&Record> {
        self.records
            .iter()
            .find(|r| &r.tag == tag)
            .with_context(|| format!("no {} record", String::from_utf8_lossy(tag)))
    }
    pub fn last_cycle(&self) -> i32 {
        self.records.iter().map(|r| r.cycle).max().unwrap_or(0)
    }
    pub fn now(&self, cycle: i32) -> Option<i64> {
        self.records
            .iter()
            .find(|r| &r.tag == b"NOWM" && r.cycle == cycle)
            .map(|r| i64::from_le_bytes(r.bytes[..8].try_into().unwrap()))
    }
    /// The recorded `CLIENT910_*` injector environment.
    pub fn env(&self) -> BTreeMap<String, String> {
        self.records
            .iter()
            .filter(|r| &r.tag == b"ENV ")
            .filter_map(|r| {
                let text = String::from_utf8_lossy(&r.bytes);
                text.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }
}

/// What one replayed cycle wrote to the world socket.
pub(super) struct CycleOutput {
    pub written: Vec<u8>,
    /// The logic update's requests for the shell (app/effects.rs).
    pub effects: Vec<ClientEffect>,
}

/// The headless client (code-quality programme Phase 5): a `ClientCore`
/// over the recorded session, driven by `ClientCore::frame` through the
/// headless [`Headless`] shell and a `ReplayIo` (the recorded clock, world
/// bytes, cursor and window events; no socket traffic, no window, no GPU),
/// plus the shell state the logic reads.
pub(super) struct Replay {
    pub pack: Pack,
    pub core: ReclaimedAtExit<ClientCore>,
    pub io: ReplayIo,
    pub minimap: ReclaimedAtExit<crate::minimap::Minimap>,
    pending_hints: Vec<Vec<u8>>,
    /// The server end of the world connection's loopback socket: the
    /// session's connection is open (the replay transport is `io`'s; the
    /// app-owned passes of [`Replay::into_app`] use the socket).
    peer: std::net::TcpStream,
    focused: bool,
    observed_backend: bool,
    sampled_clock: bool,
    ping_completions: BTreeMap<i32, i32>,
    gl_formats: Vec<i32>,
    scale: f64,
    pending_resize: crate::ui_window::PendingResize,
    /// The startup scene (`prepare_game_scene`'s graph and live scene),
    /// for the scenario tests' headless pick frames.
    pub assets: ReclaimedAtExit<WorldAssets>,
    /// Recorded server bytes handed to the client so far (startup + live).
    delivered: usize,
    cli: Cli,
    _dir: TempDir,
    _clock: ClockReset,
}

/// The startup drain over a recording's `INIT` bytes: what it collected, the
/// replies it wrote and the bytes it left for the live reader.
fn drain_recorded_startup(
    trace: &Trace,
    entity_state: bool,
) -> anyhow::Result<(crate::session::DrainOutcome, Vec<u8>, Vec<u8>)> {
    let init = trace.bytes(b"INIT", -1);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut client, mut server) = tokio::io::duplex(init.len() + 4096);
        server.write_all(&init).await?;
        let outcome = crate::session::drain_frames_mode(
            &mut client,
            std::time::Duration::from_secs(5),
            entity_state,
        )
        .await?;
        // Replies the drain wrote (the in-memory pipe holds them already).
        let mut replies = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_millis(1), server.read(&mut buf)).await
        {
            if n == 0 {
                break;
            }
            replies.extend_from_slice(&buf[..n]);
        }
        // Bytes the real drain had read but this one left in the pipe
        // reach the live reader next, in the same order.
        drop(server);
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        anyhow::Ok((outcome, replies, rest))
    })
}

/// A scratch directory for the persisted client files (removed on drop).
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Clears the recorded test clock when the replay goes away.
struct ClockReset;
impl Drop for ClockReset {
    fn drop(&mut self) {
        crate::logic_clock::set_test_now(None);
    }
}

/// The recorded device's capability answers (`TOOL`: anti-aliasing, bloom
/// and the one scene-sample query; `GLTF`: the GL texture formats) as the
/// faithful GPU toolkit's `capability::Profile` (renderer plan A3), with the
/// sample count the startup queried.
pub(super) fn recorded_profile(
    trace: &Trace,
) -> anyhow::Result<(rs910_toolkit::capability::Profile, u32)> {
    let tool = &trace.one(b"TOOL")?.bytes;
    let (antialiasing, bloom, supported) = (tool[0] != 0, tool[1] != 0, tool[2] != 0);
    let asked = u32::from_le_bytes(tool[3..7].try_into()?);
    let mut scene_samples: std::collections::BTreeSet<u32> = if antialiasing {
        [2, 4].into()
    } else {
        Default::default()
    };
    if supported {
        scene_samples.insert(asked);
    } else {
        scene_samples.remove(&asked);
    }
    let profile = rs910_toolkit::capability::Profile {
        scene_samples,
        hdr_samples: if bloom {
            [1].into()
        } else {
            Default::default()
        },
        compressed_texture_formats: trace
            .one(b"GLTF")?
            .bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    };
    anyhow::ensure!(
        profile.supports_antialiasing() == antialiasing
            && profile.supports_bloom() == bloom
            && profile.supports_scene_samples(asked) == supported,
        "TOOL record {tool:?} is not a device profile"
    );
    Ok((profile, asked))
}

#[path = "observed_session.rs"]
mod observed_session;

#[derive(Clone, Copy)]
enum StartupSource {
    Recording,
    Authenticated {
        server_clock: i64,
    },
    RecordedAuthenticated {
        server_clock: i64,
        startup_millis: i64,
        membership_millis: i64,
    },
}

impl Replay {
    /// Startup: `online_login` from the recorded login reply and startup
    /// bytes, then `ViewerApp::resumed`'s session half, with the recorded
    /// device's capability answers.
    pub fn start(trace: &Trace) -> anyhow::Result<Self> {
        let (profile, _) = recorded_profile(trace)?;
        Self::start_with(
            trace,
            &rs910_toolkit::capability::Answers::hardware(profile),
        )
    }

    /// [`Replay::start`] whose toolkit installation and input-telemetry
    /// texture formats answer from `capabilities` (the active toolkit's
    /// capability answers, `ActiveToolkit`'s). With toolkit 0's answers
    /// (`capabilities.toolkit0`) the session is a toolkit-0 session: its
    /// saved toolkit is 0, as `ActiveToolkit::set_toolkit0` follows.
    pub fn start_with(
        trace: &Trace,
        capabilities: &rs910_toolkit::capability::Answers,
    ) -> anyhow::Result<Self> {
        Self::start_at_install(trace, capabilities, capabilities.toolkit0)
    }

    /// [`Replay::start_with`] where the renderer's toolkit at the moment the
    /// session installs (`renderer.toolkit0`) need not be the saved toolkit
    /// (`saved_toolkit0`): a title-screen login installs the saved hardware
    /// toolkit while the loading screens' toolkit 0 is still the renderer's.
    pub fn start_at_install(
        trace: &Trace,
        renderer: &rs910_toolkit::capability::Answers,
        saved_toolkit0: bool,
    ) -> anyhow::Result<Self> {
        let pack = crate::test_support::require_pack("client.interfaces.js5");
        Self::start_installing(trace, renderer, saved_toolkit0, pack)
    }

    /// [`Replay::start_with`] reading the cache through `pack` (the
    /// pack-free overlay of `export_the_recorded_sessions_pack_overlay`).
    pub fn start_from(
        trace: &Trace,
        capabilities: &rs910_toolkit::capability::Answers,
        pack: Pack,
    ) -> anyhow::Result<Self> {
        Self::start_installing(trace, capabilities, capabilities.toolkit0, pack)
    }

    fn start_installing(
        trace: &Trace,
        capabilities: &rs910_toolkit::capability::Answers,
        saved_toolkit0: bool,
        pack: Pack,
    ) -> anyhow::Result<Self> {
        let startup = match trace
            .head
            .iter()
            .find(|(key, _)| key == OBSERVED_BACKEND_HEAD)
        {
            None => StartupSource::Recording,
            Some((_, backend)) if backend == AUTHENTICATED_HEADLESS_BACKEND => {
                StartupSource::RecordedAuthenticated {
                    server_clock: trace.head(OBSERVED_SERVER_CLOCK_HEAD)?.parse()?,
                    startup_millis: trace
                        .now(STARTUP_RECORDING_CYCLE)
                        .context("recorded authenticated startup clock")?,
                    membership_millis: trace.head(OBSERVED_MEMBERSHIP_CLOCK_HEAD)?.parse()?,
                }
            }
            Some(_) => anyhow::bail!("unknown recorded backend"),
        };
        Self::start_installing_from(trace, capabilities, saved_toolkit0, pack, startup)
    }

    fn start_installing_from(
        trace: &Trace,
        capabilities: &rs910_toolkit::capability::Answers,
        saved_toolkit0: bool,
        pack: Pack,
        startup: StartupSource,
    ) -> anyhow::Result<Self> {
        let sampled_clock = matches!(startup, StartupSource::RecordedAuthenticated { .. })
            && trace.sampled_clock()?;
        let startup_samples = if sampled_clock {
            Some(crate::logic_clock::MonotonicSamples::replay(
                trace.clock_samples(&STARTUP_CLOCK_SAMPLES, STARTUP_RECORDING_CYCLE)?,
            )?)
        } else {
            None
        };
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "client910-session-replay-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("players"))?;
        let dir = TempDir(dir);
        // `preferences_path`/`install_client_varcs`/`uid192_path` resolve
        // `<pack root>/../players/*`: point the session's pack root at the
        // scratch directory so the replay never touches the repository files.
        let pack_root = dir.0.join("pack");
        for (tag, name) in [(b"PREF", "preferences.dat"), (b"VARC", "client-vars.dat")] {
            let bytes = &trace.one(tag)?.bytes;
            if !bytes.is_empty() {
                std::fs::write(dir.0.join("players").join(name), bytes)?;
            }
        }
        let mut args = vec![
            "client910".to_string(),
            "--direct-login".into(),
            // The recording was made against a server without encryption.
            "--login-crypto".into(),
            "off".into(),
            "--username".into(),
            trace.head("username")?.into(),
            "--pack-root".into(),
            pack_root.display().to_string(),
            "--entity-state".into(),
            trace.head("entity_state")?.into(),
        ];
        for command in trace.head_all("server_command") {
            args.push("--server-command".into());
            args.push(command);
        }
        let cli = Cli::try_parse_from(&args)?;
        match startup {
            StartupSource::Recording => {
                let first_now = trace.now(1).context("no cycle 1")?;
                crate::logic_clock::set_test_now(Some(first_now));
            }
            StartupSource::Authenticated { .. } => crate::logic_clock::set_test_now(None),
            StartupSource::RecordedAuthenticated { startup_millis, .. } => {
                crate::logic_clock::set_test_now(Some(startup_millis))
            }
        }

        // session::login_and_drain...: the startup drain over the recorded bytes.
        let (mut outcome, replies, rest) = drain_recorded_startup(trace, cli.entity_state)?;
        anyhow::ensure!(
            replies == trace.bytes(b"IOUT", -1),
            "startup drain replies {replies:02x?}"
        );
        // online_login (app.rs): the entity runtime and retained UI.
        let pid: usize = trace.head("pid")?.parse()?;
        let token: i64 = trace.head("server_token")?.parse()?;
        let flag = |key: &str| -> anyhow::Result<bool> { Ok(trace.head(key)?.parse()?) };
        let profile = LoginProfile {
            logged_in_members: flag("logged_in_members")?,
            player_is_members: flag("player_is_members")?,
            player_is_quickchat: flag("player_is_quickchat")?,
            logged_in_quickchat: flag("logged_in_quickchat")?,
            dob_verified: flag("dob_verified")?,
            lobby_dob: trace.head("lobby_dob")?.parse()?,
            staff_mod_level: trace.head("staff_mod_level")?.parse()?,
            player_mod_level: trace.head("player_mod_level")?.parse()?,
            owner: Some(trace.head("owner")?.to_string()).filter(|o| !o.is_empty()),
            // The recording predates the g6 clock in the replayed profile
            // head; the clock only feeds session-age CS2 commands.
            server_clock: match startup {
                StartupSource::Recording => None,
                StartupSource::Authenticated { server_clock }
                | StartupSource::RecordedAuthenticated { server_clock, .. } => Some(server_clock),
            },
        };
        let uid_hex = trace.head("uid192")?;
        let mut uid192 = [0u8; 24];
        for (i, byte) in uid192.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&uid_hex[i * 2..i * 2 + 2], 16)?;
        }
        // One read of the config archives for the game owner and the
        // retained interface.
        let configs = rs910_config::login_configs::LoginConfigs::read(&pack);
        let mut game = crate::client_game::ClientGame::login_with(
            &pack,
            &configs,
            pid,
            std::mem::take(&mut outcome.entities),
            token as u64,
            profile.logged_in_members,
        )?;
        install_client_persistence(&mut game, &cli.pack_root)?;
        if saved_toolkit0 {
            // Toolkit 0 is the saved toolkit (the installation creates it, so
            // `displayMode` follows).
            game.ui_variables
                .queries
                .preferences
                .options
                .set_field("toolkit", 0)
                .context("toolkit")?;
        }
        restore_server_varcs(&mut game, &trace.one(b"SVRC")?.bytes)?;
        game.apply_next(crate::logic_clock::monotonic_millis())
            .map_err(|e| anyhow::anyhow!("initial entity packet: {e:?}"))?;
        // prepare_game_scene reads only the floor and material stores of
        // the WorldAssets that `build_assets_from_world` returns.
        let mut assets = WorldAssets::synthetic()?;
        assets.flo = Some(FloStore::load(&pack)?);
        assets.material_store = Some(crate::texture::MaterialStore::load(&pack)?);
        let prepared = prepare_game_scene(&mut game, &pack, &mut assets)?;
        if let StartupSource::RecordedAuthenticated {
            membership_millis, ..
        } = startup
        {
            crate::logic_clock::set_test_now(Some(membership_millis));
        }
        let ui = retained_session_ui(&pack, configs, &profile, uid192, &mut game)?;
        let (login_interface, lobby_interface) = title_interfaces(&pack)?;
        // The world connection: a real loopback socket in the client's
        // nonblocking mode (`detach_world_socket`).
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let stream = std::net::TcpStream::connect(listener.local_addr()?)?;
        let (peer, _) = listener.accept()?;
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        peer.set_nodelay(true)?;
        peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut pending = std::mem::take(&mut outcome.pending);
        pending.extend(rest);
        let mut session = new_session(
            &cli,
            &pack,
            SessionStart {
                stream: Some(crate::wire_stream::WireStream::plain(stream)),
                startup_connection: None,
                entities: crate::protocol910::live::Feed::default(),
                game: Some(game),
                prepared_map: Some(prepared),
                ui,
                pending,
                initial_ui: std::mem::take(&mut outcome.ui_events),
                login_interface,
                lobby_interface,
            },
        );
        session.machine.state = crate::login_state::GAME;
        // The replay measures no real round trip: the ping report carries a
        // fixed one.
        if matches!(startup, StartupSource::Recording) {
            session.io.ping = crate::connection_upkeep::PingReporter::with_backend(
                crate::connection_upkeep::PingBackend::Fixed(REPLAY_PING_MS),
            );
        }

        let ping_completions = if matches!(startup, StartupSource::RecordedAuthenticated { .. }) {
            session.io.ping = crate::connection_upkeep::PingReporter::with_backend(
                crate::connection_upkeep::PingBackend::Recorded,
            );
            trace.ping_completions()?
        } else {
            BTreeMap::new()
        };

        // ViewerApp::resumed: toolkit, canvas, map acknowledgement.
        let tool = &trace.one(b"TOOL")?.bytes;
        // The installation reads the device's answers, not the toolkit the
        // renderer runs at that moment (`ViewerApp::install_session_toolkit`).
        let device = rs910_toolkit::capability::Answers::hardware(capabilities.profile.clone());
        let caps = ToolkitCaps {
            antialiasing: device.supports_antialiasing(),
            bloom: device.supports_bloom(),
        };
        let asked = u32::from_le_bytes(tool[3..7].try_into()?);
        let physical = [
            u32::from_le_bytes(tool[7..11].try_into()?),
            u32::from_le_bytes(tool[11..15].try_into()?),
        ];
        let scale = f64::from_le_bytes(tool[15..23].try_into()?);
        install_toolkit_preferences(
            &mut session,
            caps,
            &mut |samples| {
                assert_eq!(samples, asked, "recorded scene sample query");
                capabilities.supports_scene_samples(samples)
            },
            physical,
            scale,
        )
        .context("toolkit preferences")?;
        let canv = &trace.one(b"CANV")?.bytes;
        let int = |at: usize| i32::from_le_bytes(canv[at..at + 4].try_into().unwrap());
        let size = [int(0) as u32, int(4) as u32];
        let modes: Vec<_> = (0..int(8) as usize)
            .map(|i| crate::ui_runtime::FullscreenMode {
                width: int(12 + i * 16),
                height: int(16 + i * 16),
                bit_depth: int(20 + i * 16),
                refresh: int(24 + i * 16),
            })
            .collect();
        let mut debug_events = Vec::new();
        install_canvas_state(&mut session, size, &modes, &mut debug_events)?;
        anyhow::ensure!(debug_events.is_empty(), "startup client debug events");
        // install_session_canvas's tail: the renderer-owned updates, then
        // the "initial canvas" sync_window_settings.
        let mut hints = Vec::new();
        for effect in packet_effects(Some(&mut session.ui), false) {
            match effect {
                ClientEffect::HintArrows(updates) => hints = updates,
                ClientEffect::BrowserUrls { urls, .. } => {
                    anyhow::ensure!(urls.is_empty(), "startup URL_OPEN")
                }
                _ => {}
            }
        }
        let mut minimap = crate::minimap::Minimap::default();
        minimap.install(&pack);
        let mut pending_hints = Vec::new();
        let base = session
            .game
            .as_ref()
            .map(|g| [g.runtime.map.base_x, g.runtime.map.base_z]);
        apply_hint_arrows(&mut minimap, &mut pending_hints, base, hints);
        sync_window_state(&mut session, None, &mut |_| Ok(()))?;
        if !matches!(startup, StartupSource::Authenticated { .. }) {
            anyhow::ensure!(
                trace
                    .records
                    .iter()
                    .any(|r| &r.tag == b"MAPI" && r.cycle == -1),
                "the recorded startup map was acknowledged before the first cycle"
            );
        }
        if matches!(startup, StartupSource::RecordedAuthenticated { .. }) && !sampled_clock {
            let elapsed = trace
                .records
                .iter()
                .find(|record| record.tag == *b"MAPI" && record.cycle == STARTUP_RECORDING_CYCLE)
                .context("recorded authenticated startup map acknowledgement")?;
            let elapsed = i32::from_le_bytes(
                elapsed
                    .bytes
                    .as_slice()
                    .try_into()
                    .context("recorded startup map duration width")?,
            );
            let started = session
                .game
                .as_ref()
                .and_then(|game| game.rebuild_started_ms)
                .context("startup map clock owner")?;
            acknowledge_game_map(&mut session, || started.wrapping_add(i64::from(elapsed)))?;
        } else {
            acknowledge_game_map(&mut session, crate::logic_clock::monotonic_millis)?;
        }
        let gl_formats = capabilities.compressed_texture_formats().to_vec();
        let env = trace.env();
        let mut core = ClientCore::new(Some(session));
        core.script = crate::input_script::InputScript::from_lookup(&|name| {
            env.get(name).map(std::ffi::OsString::from)
        });
        let mut replay = Self {
            pack,
            core: ReclaimedAtExit::new(core),
            io: ReplayIo::new(trace.records.clone()),
            minimap: ReclaimedAtExit::new(minimap),
            pending_hints,
            peer,
            // ViewerApp starts focused; the recorded FOCS events follow.
            focused: true,
            observed_backend: matches!(startup, StartupSource::RecordedAuthenticated { .. }),
            sampled_clock,
            ping_completions,
            gl_formats,
            scale,
            pending_resize: Default::default(),
            assets: ReclaimedAtExit::new(assets),
            delivered: trace.bytes(b"INIT", -1).len(),
            cli,
            _dir: dir,
            _clock: ClockReset,
        };
        replay.window_events(-1)?;
        if let Some(samples) = startup_samples {
            samples.finish()?;
        }
        Ok(replay)
    }

    /// The session (the replay always has one).
    pub fn session(&mut self) -> &mut Session {
        self.core.session.as_mut().expect("the replayed session")
    }

    /// Window events recorded while `cycle` was the last logic cycle, which
    /// the event loop delivered before the next one (`ViewerApp::window_event`'s
    /// session halves).
    fn window_events(&mut self, cycle: i32) -> anyhow::Result<()> {
        for event in self.io.window_events(cycle)? {
            match event {
                WindowRecord::Focused(focused) => {
                    self.focused = focused;
                    if !focused {
                        session_focus_lost(self.session());
                    }
                }
                WindowRecord::Resized(size) => self.pending_resize.receive(size),
                WindowRecord::Input(_) => {}
                WindowRecord::AcceptedInput(input) => {
                    let samples = if self.sampled_clock {
                        Some(crate::logic_clock::MonotonicSamples::replay(
                            decode_clock_samples(self.io.one_bytes(&INPUT_CLOCK_SAMPLES, cycle)?)?,
                        )?)
                    } else {
                        None
                    };
                    input.apply(self.session())?;
                    if let Some(samples) = samples {
                        samples.finish()?;
                    }
                }
            }
        }
        Ok(())
    }

    /// One recorded logic cycle (`ViewerApp::about_to_wait` with one
    /// granted cycle).
    pub fn cycle(&mut self, trace: &Trace, cycle: i32) -> anyhow::Result<CycleOutput> {
        self.cycle_with_picks(trace, cycle, &mut |_, _| {})
    }

    /// [`Replay::cycle`] whose renderer pick refresh is `refresh_picks`
    /// (the scenario tests' headless pick frame).
    pub fn cycle_with_picks(
        &mut self,
        _trace: &Trace,
        cycle: i32,
        refresh_picks: &mut dyn FnMut(
            &mut crate::client_game::ClientGame,
            &mut crate::ui_runtime::Runtime,
        ),
    ) -> anyhow::Result<CycleOutput> {
        self.delivered += self.io.bytes(b"IN  ", cycle).len();
        self.io.set_scripted(false);
        let out = self.run(cycle, refresh_picks, None)?;
        self.window_events(cycle)?;
        if self.observed_backend {
            // Same after-input headless redraw as the authenticated live owner.
            // No GPU/presentation/foreground state is asserted.
            let samples = if self.sampled_clock {
                Some(crate::logic_clock::MonotonicSamples::replay(
                    decode_clock_samples(self.io.one_bytes(&REDRAW_CLOCK_SAMPLES, cycle)?)?,
                )?)
            } else {
                None
            };
            self.redraw();
            if let Some(samples) = samples {
                samples.finish()?;
            }
        }
        Ok(out)
    }

    /// One logic cycle past the recording (the scenario tests): the clock
    /// advances one 20 ms logic interval, `inbound` is what the server
    /// wrote, and `refresh_picks` stands in for the renderer's pick refresh.
    pub fn step_live(
        &mut self,
        cycle: i32,
        inbound: &[u8],
        refresh_picks: &mut dyn FnMut(
            &mut crate::client_game::ClientGame,
            &mut crate::ui_runtime::Runtime,
        ),
    ) -> anyhow::Result<CycleOutput> {
        self.delivered += inbound.len();
        self.io.set_scripted(true);
        self.io.push_inbound(inbound);
        self.run(cycle, refresh_picks, None)
    }

    /// [`Replay::cycle`] that records the phases `ClientCore::frame` ran.
    pub fn cycle_observed(
        &mut self,
        cycle: i32,
        phases: &mut Vec<Phase>,
    ) -> anyhow::Result<CycleOutput> {
        self.delivered += self.io.bytes(b"IN  ", cycle).len();
        self.io.set_scripted(false);
        let out = self.run(cycle, &mut |_, _| {}, Some(phases))?;
        self.window_events(cycle)?;
        Ok(out)
    }

    /// `ClientCore::frame` with one granted cycle numbered `cycle` over the
    /// headless shell; `phases` observes the phase list.
    fn run(
        &mut self,
        cycle: i32,
        refresh_picks: &mut dyn FnMut(
            &mut crate::client_game::ClientGame,
            &mut crate::ui_runtime::Runtime,
        ),
        phases: Option<&mut Vec<Phase>>,
    ) -> anyhow::Result<CycleOutput> {
        // A map transaction the recording acknowledged in this cycle installs
        // here (the app's worker finished; `Headless::rebuild`).
        let install_map = self.io.has(b"MAPI", cycle);
        let recorded_map_elapsed = if self.observed_backend && !self.sampled_clock && install_map {
            Some(i32::from_le_bytes(
                self.io
                    .bytes(b"MAPI", cycle)
                    .as_slice()
                    .try_into()
                    .context("recorded authenticated map duration width")?,
            ))
        } else {
            None
        };
        // The recording's cycle numbers (the scenario tests continue past
        // it at their own).
        self.core.cycle = cycle.wrapping_sub(1);
        let frame_samples = if self.sampled_clock {
            Some(crate::logic_clock::MonotonicSamples::replay(
                decode_clock_samples(self.io.one_bytes(&FRAME_CLOCK_SAMPLES, cycle)?)?,
            )?)
        } else {
            None
        };
        if self.observed_backend {
            anyhow::ensure!(
                self.ping_completions
                    .first_key_value()
                    .is_none_or(|(at, _)| *at >= cycle),
                "recorded ping completion cycle was skipped"
            );
            if let Some(round_trip) = self.ping_completions.remove(&cycle) {
                self.session().io.ping.complete_recorded(round_trip)?;
            }
        }
        let mut shell = Headless {
            core: &mut self.core,
            pack: &self.pack,
            assets: &mut self.assets,
            install_map,
            recorded_map_elapsed,
            minimap: &mut self.minimap,
            pending_hints: &mut self.pending_hints,
            focused: self.focused,
            gl_formats: &self.gl_formats,
            scale: self.scale,
            pending_resize: &mut self.pending_resize,
            refresh_picks,
            effects: Vec::new(),
            error: None,
            phases,
        };
        let end = ClientCore::frame(&mut shell, &mut self.io, 1);
        if let Some(samples) = frame_samples {
            samples.finish()?;
        }
        let Headless { effects, error, .. } = shell;
        anyhow::ensure!(
            !self.session().io.ping.has_recorded_completion(),
            "recorded ping completion was unused at cycle {cycle}"
        );
        if let Some(error) = error {
            return Err(error);
        }
        anyhow::ensure!(
            end == Cycle::Next,
            "cycle {cycle}: the core stopped ({end:?})"
        );
        Ok(CycleOutput {
            written: self.io.take_written(),
            effects,
        })
    }

    /// How many of [`arrivals`] the client has processed: every frame
    /// before the unread `pending` bytes, less the entity frames still
    /// queued in the feed (`drain_game_frames` applies one server tick per
    /// logic cycle).
    pub fn processed(&self, arrivals: &[Arrival]) -> usize {
        let session = self.core.session.as_ref().unwrap();
        let consumed = self.delivered - self.io.unread() - session.io.world.pending.len();
        let read = arrivals.iter().take_while(|a| a.end <= consumed).count();
        read - self.game().runtime.feed.queued()
    }
    pub fn game(&self) -> &crate::client_game::ClientGame {
        self.core
            .session
            .as_ref()
            .and_then(|s| s.game.as_ref())
            .unwrap()
    }
    /// `ViewerApp::new` over this replay's session and startup scene, as
    /// `run_online` builds the app (no window or renderer), for app-owned
    /// passes. The second value is the server end of the world socket; it
    /// also keeps the replay's scratch files and test clock alive.
    pub fn into_app(self) -> (ViewerApp, (std::net::TcpStream, impl Sized, impl Sized)) {
        let Replay {
            pack,
            mut core,
            assets,
            cli,
            peer,
            _dir,
            _clock,
            ..
        } = self;
        let mut app = ViewerApp::new(cli, pack, assets.into_inner(), core.session.take());
        // No window, no device: loc changes write the CPU scene only.
        app.scene.meshless = true;
        (app, (peer, _dir, _clock))
    }
    pub fn ui(&self) -> &crate::ui_runtime::Runtime {
        &self.core.session.as_ref().unwrap().ui
    }
}

/// The headless client's `Shell`: no window, toolkit, JS5, console, title
/// world or lobby socket (their phases are the trait's defaults); the
/// recording supplies what they handed the logic (focus, window size,
/// texture formats; the cursor comes through the `ReplayIo`). P8 applies
/// the live read's hint arrows to the minimap, enters the rebuild state on a
/// map transaction and refuses what a replay cannot follow (a session
/// transition, a lost connection); P4 installs the transaction's map in the
/// cycle the recording acknowledged it (the JS5 squares arrived then);
/// P10/P11 hand the logic's requests to the caller and run the session halves
/// of the preference sync and the server commands.
struct Headless<'a> {
    core: &'a mut ClientCore,
    pack: &'a Pack,
    /// The scene owners a mid-session map transaction rebuilds.
    assets: &'a mut WorldAssets,
    /// The recording acknowledged a map transaction in this cycle.
    install_map: bool,
    recorded_map_elapsed: Option<i32>,
    minimap: &'a mut crate::minimap::Minimap,
    pending_hints: &'a mut Vec<Vec<u8>>,
    focused: bool,
    gl_formats: &'a [i32],
    scale: f64,
    pending_resize: &'a mut crate::ui_window::PendingResize,
    refresh_picks:
        &'a mut dyn FnMut(&mut crate::client_game::ClientGame, &mut crate::ui_runtime::Runtime),
    /// The logic update's requests (P10, then P11's).
    effects: Vec<ClientEffect>,
    /// The first failure; the cycle's result.
    error: Option<anyhow::Error>,
    phases: Option<&'a mut Vec<Phase>>,
}

impl Headless<'_> {
    fn fail(&mut self, error: anyhow::Error) {
        self.error.get_or_insert(error);
    }
    fn session(&mut self) -> &mut Session {
        self.core.session.as_mut().expect("the replayed session")
    }
}

impl Headless<'_> {
    /// A live map transaction (`ViewerApp::reload_for_event`): a fresh
    /// environment fade, the rebuild clock, and state 3 until the map is
    /// installed (the live read holds the stream meanwhile).
    fn enter_rebuild_state(&mut self) -> anyhow::Result<()> {
        self.core.environment.fade_reset = true;
        self.core.environment.fade = None;
        self.core.environment.fade_target = None;
        let session = self.session();
        let game = session
            .game
            .as_mut()
            .context("map transaction without a game")?;
        anyhow::ensure!(
            game.runtime.map_request.is_some(),
            "a rebuild without a map request"
        );
        game.rebuild_started_ms
            .get_or_insert_with(crate::logic_clock::monotonic_millis);
        set_session_state(session, crate::login_state::REBUILD_GAME)
    }

    /// The worker's `Done` (`ViewerApp::poll_prefetch_responses`): the
    /// loading box's last stage, the scene build, the acknowledgement
    /// (`MAP_BUILD_COMPLETE`) and the return to the game state.
    fn install_rebuilt_map(&mut self) -> anyhow::Result<()> {
        let pack = self.pack;
        let recorded_elapsed = self.recorded_map_elapsed;
        let assets = &mut *self.assets;
        let session = self.core.session.as_mut().context("session")?;
        anyhow::ensure!(
            crate::login_state::is_rebuild(session.machine.state),
            "a map acknowledged outside a rebuild"
        );
        session.rebuild.load_maps(0);
        session.rebuild.load_locs(0);
        session.rebuild.begin_build();
        let game = session.game.as_mut().context("map has no owning runtime")?;
        session.prepared_map = Some(prepare_game_scene(game, pack, assets)?);
        if let Some(elapsed) = recorded_elapsed {
            let started = session
                .game
                .as_ref()
                .and_then(|game| game.rebuild_started_ms)
                .context("recorded map clock owner")?;
            acknowledge_game_map(session, || started.wrapping_add(i64::from(elapsed)))?;
        } else {
            acknowledge_game_map(session, crate::logic_clock::monotonic_millis)?;
        }
        let state = crate::login_state::rebuilt_state(session.machine.state);
        set_session_state(session, state)
    }
}

/// `ViewerApp::set_client_state` for the states a map transaction passes:
/// entering a rebuild completes the loading tracker's last one.
fn set_session_state(session: &mut Session, next: i32) -> anyhow::Result<()> {
    let ctx = session.login_context();
    let mut effects = Vec::new();
    session.machine.set_state(next, &ctx, &mut effects);
    for effect in effects {
        match effect {
            crate::login_state::Effect::CompleteRebuild => session.rebuild.complete(),
            other => anyhow::bail!("unexpected login effect {other:?} entering state {next}"),
        }
    }
    Ok(())
}

impl Shell for Headless<'_> {
    fn core(&mut self) -> &mut ClientCore {
        self.core
    }
    fn phase(&mut self, phase: Phase) {
        if let Some(phases) = self.phases.as_mut() {
            phases.push(phase);
        }
    }
    /// P0: the latest native size (`ViewerApp::resize_phase`'s session half).
    fn resize(&mut self) {
        let Some([w, h]) = self.pending_resize.take() else {
            return;
        };
        let scale = self.scale;
        let session = self.session();
        if let Some(game) = session.game.as_mut() {
            game.ui_variables
                .queries
                .preferences
                .window
                .observe_size([w, h], scale);
            game.ui_variables.queries.preferences.window.changed = true;
        }
        if let Err(error) = sync_window_state(session, None, &mut |_| Ok(())) {
            self.fail(error);
        }
    }
    fn live_read(&mut self, io: &mut dyn Io) -> bool {
        let cycle = self.core.cycle;
        match self.core.poll_live(io) {
            LivePollOutcome::Idle => {}
            LivePollOutcome::Lost(reason) => {
                self.fail(anyhow::anyhow!("cycle {cycle}: connection lost: {reason}"))
            }
            LivePollOutcome::Polled(poll) => {
                if !poll.session_events.is_empty() {
                    self.fail(anyhow::anyhow!(
                        "cycle {cycle}: session events {:?}",
                        poll.session_events
                    ));
                    return false;
                }
                let base = self
                    .session()
                    .game
                    .as_ref()
                    .map(|g| [g.runtime.map.base_x, g.runtime.map.base_z]);
                for effect in poll.effects {
                    if let ClientEffect::HintArrows(updates) = effect {
                        apply_hint_arrows(self.minimap, self.pending_hints, base, updates);
                    }
                }
                if poll.rebuild.is_some() {
                    if let Err(error) = self.enter_rebuild_state() {
                        self.fail(error.context(format!("cycle {cycle}: live rebuild")));
                    }
                }
            }
        }
        false
    }
    /// P4: the map transaction finishes in the cycle the recording
    /// acknowledged it (the app's worker delivered the squares then).
    fn rebuild(&mut self) {
        if self.install_map {
            if let Err(error) = self.install_rebuilt_map() {
                self.fail(error);
            }
        }
    }
    fn update_game(&mut self, io: &mut dyn Io) -> Vec<ClientEffect> {
        self.core.update_game(
            self.pack,
            io,
            InputFrame {
                console_open: false,
                focused: self.focused,
                gl_texture_formats: Some(self.gl_formats.to_vec()),
                // The recorded cursor (`ReplayIo::cursor`).
                cursor: i32::MIN,
                refresh_picks: &mut *self.refresh_picks,
                probe: None,
            },
        )
    }
    fn apply_effects(&mut self, effects: Vec<ClientEffect>) -> bool {
        self.effects.extend(effects);
        false
    }
    fn title_and_preferences(&mut self, after_title: Vec<ClientEffect>) {
        self.effects.extend(after_title);
        let session = self.session();
        let result = sync_window_state(session, None, &mut |_| Ok(()))
            .and_then(|()| dispatch_server_command(session));
        if let Err(error) = result {
            self.fail(error);
        }
    }
}

/// The headless client's `RedrawShell` (lane E-A1): no toolkit, scene or
/// camera, so a full redraw runs the core's own steps only (R3's
/// partial environment update, the cross state, R8's positioned
/// sound and the scene draw's scene cycle/minimap arrival).
struct HeadlessRedraw<'a> {
    core: &'a mut ClientCore,
}

impl RedrawShell for HeadlessRedraw<'_> {
    fn core(&mut self) -> &mut ClientCore {
        self.core
    }
}

impl Replay {
    /// One `RedrawRequested` over the headless redraw shell: a full redraw
    /// after a cycle, a present otherwise (`ClientCore::redraw`).
    pub fn redraw(&mut self) -> RedrawKind {
        ClientCore::redraw(&mut HeadlessRedraw {
            core: &mut self.core,
        })
    }
}

/// Split client packets (client packet framing).
pub(super) fn client_frames(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    let mut frames = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let opcode = bytes[pos];
        pos += 1;
        let size = crate::proto::client::size(opcode)
            .with_context(|| format!("unknown client opcode {opcode}"))?;
        let len = match size {
            -1 => {
                pos += 1;
                usize::from(bytes[pos - 1])
            }
            -2 => {
                pos += 2;
                usize::from(u16::from_be_bytes([bytes[pos - 2], bytes[pos - 1]]))
            }
            n => n as usize,
        };
        anyhow::ensure!(pos + len <= bytes.len(), "truncated client opcode {opcode}");
        frames.push((opcode, bytes[pos..pos + len].to_vec()));
        pos += len;
    }
    Ok(frames)
}

/// One server frame of the recording.
pub(super) struct Arrival {
    /// Offset just past the frame in the recorded server stream.
    pub end: usize,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// The recorded server stream (startup + live reads) split into frames
/// (server packet framing).
pub(super) fn arrivals(trace: &Trace) -> anyhow::Result<Vec<Arrival>> {
    let stream: Vec<u8> = trace
        .records
        .iter()
        .filter(|r| &r.tag == b"INIT" || &r.tag == b"IN  ")
        .flat_map(|r| r.bytes.iter().copied())
        .collect();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some((frame, used)) = crate::net::decode_frame(&stream[pos..])? {
        pos += used;
        out.push(Arrival {
            end: pos,
            opcode: frame.opcode,
            payload: frame.payload,
        });
    }
    Ok(out)
}

/// Observable state after one cycle.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Observed {
    pub local: Option<ActorView>,
    pub npcs: BTreeMap<usize, (i32, ActorView)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActorView {
    pub tile: [i32; 3],
    pub angle: i32,
    pub desired_angle: i32,
    pub speed: i8,
    pub route_length: usize,
    pub walk_anim: i32,
    pub main_anim: i32,
}

fn actor_view(p: &crate::entities910::Player, base: [i32; 2]) -> ActorView {
    ActorView {
        tile: [base[0] + p.x[0], base[1] + p.z[0], p.level],
        angle: p.angle,
        desired_angle: p.desired_angle,
        speed: p.speeds[0],
        route_length: p.route_length,
        walk_anim: p.actor.walk.node.id(),
        main_anim: p.animation.main.id(),
    }
}

pub(super) fn observe(game: &crate::client_game::ClientGame) -> Observed {
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    let state = &game.runtime.feed.state;
    Observed {
        local: state.players.players[game.runtime.map.local]
            .as_ref()
            .map(|p| actor_view(p, base)),
        npcs: state
            .npcs
            .entities
            .iter()
            .map(|(&i, n)| (i, (n.type_id, actor_view(&n.path, base))))
            .collect(),
    }
}

/// `MAP_BUILD_COMPLETE`'s payload is `monotonic_millis() - rebuild_started_ms`
/// (`acknowledge_game_map`), `SEND_PING_REPLY`'s last
/// byte is `p1_alt2(fps)`, `PING_STATISTICS` carries the round trip and the
/// frame rate: all measure the recording machine's wall clock / frame rate. Every other byte of
/// every client packet must match the recording.
pub(super) fn mask_wall_clock(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    let mut frames = client_frames(bytes)?;
    for (opcode, payload) in &mut frames {
        match *opcode {
            crate::proto::client::MAP_BUILD_COMPLETE => payload.fill(0),
            crate::proto::client::SEND_PING_REPLY => {
                if let Some(fps) = payload.last_mut() {
                    *fps = 0;
                }
            }
            // The round trip and the frame rate are the machine's.
            crate::proto::client::PING_STATISTICS => payload[..3].fill(0),
            _ => {}
        }
    }
    Ok(frames)
}

/// The dev server's own `PLAYER_INFO` / `NPC_INFO` positions for the recorded
/// player (`ALTO_TRACE_INFO=1`, the server player and NPC updates), in send order.
/// `[x, z, level, angle]` per local `PLAYER_INFO`, and per `NPC_INFO` the
/// five-int rows of the server trace's `npcs` array.
pub(super) type ServerTrace = (Vec<[i32; 4]>, Vec<Vec<[i32; 5]>>);

fn server_trace(root: &Path) -> anyhow::Result<ServerTrace> {
    server_trace_file(&root.join("fixtures/session-replay/server-trace.jsonl"))
}

/// [`server_trace`] of any recording's `server-trace.jsonl`.
pub(super) fn server_trace_file(path: &Path) -> anyhow::Result<ServerTrace> {
    let mut players = Vec::new();
    let mut npcs = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let int = |v: &serde_json::Value| v.as_i64().map(|v| v as i32).context("int");
        match v["t"].as_str() {
            Some("player_info") => players.push([
                int(&v["x"])?,
                int(&v["z"])?,
                int(&v["level"])?,
                int(&v["angle"])?,
            ]),
            Some("npc_info") => npcs.push(
                v["npcs"]
                    .as_array()
                    .context("npcs")?
                    .iter()
                    .map(|n| {
                        let n = n.as_array().context("npc")?;
                        Ok([
                            int(&n[0])?,
                            int(&n[1])?,
                            int(&n[2])?,
                            int(&n[3])?,
                            int(&n[4])?,
                        ])
                    })
                    .collect::<anyhow::Result<_>>()?,
            ),
            _ => anyhow::bail!("server trace line {line}"),
        }
    }
    Ok((players, npcs))
}

/// Fix programme item 2 (test-audit.md): a recorded dev-server session
/// replayed through the production owners. Expected values are the recorded
/// client bytes, the dev server's own position trace, the scripted inputs
/// (`record.sh`) and recorded packet layouts, never this code's output.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_session_replays_through_the_production_owners() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, server_npcs) = server_trace(root)?;
    // The scripted server events (the server encoders of the `record.sh`
    // server commands), located in the recorded stream.
    let find = |opcode: u8, payload: Option<&[u8]>| -> anyhow::Result<usize> {
        arrivals
            .iter()
            .position(|a| a.opcode == opcode && payload.is_none_or(|p| a.payload == p))
            .with_context(|| format!("{} {payload:02x?} not in the fixture", sp::name(opcode)))
    };
    // `varp 1001 -5`: p1(-5), p2_alt2(1001).
    let varp_small = find(
        sp::VARP_SMALL,
        Some(&[0xFB, 0x03, 0xE9u8.wrapping_add(128)]),
    )?;
    // `varp 1002 123456`: p2_alt1(1002), p4(123456).
    let varp_large = find(sp::VARP_LARGE, Some(&[0xEA, 0x03, 0x00, 0x01, 0xE2, 0x40]))?;
    // `varbit 1000 1`: p1_alt3(1), p2_alt2(1000).
    let varbit = find(
        sp::VARBIT_SMALL,
        Some(&[0x7F, 0x03, 0xE8u8.wrapping_add(128)]),
    )?;
    // `hintarrow tile 3224 3224`: slot 0 type 2, sprite 0, level 0, x, z,
    // height 0, distance 64, model -1.
    let hint = find(
        sp::HINT_ARROW,
        Some(&[
            0x02, 0x00, 0x00, 0x0C, 0x98, 0x0C, 0x98, 0x00, 0x00, 0x40, 0xFF, 0xFF, 0xFF, 0xFF,
        ]),
    )?;
    // `song 2`: p2_alt3(2), p1_alt2(255).
    let song = find(sp::MIDI_SONG, Some(&[0x82, 0x00, 0x01]))?;
    let no_timeout = find(sp::NO_TIMEOUT, Some(&[]))?;
    // `ping 1234 5678`: p4, p4.
    let ping = find(sp::SEND_PING, Some(&[0, 0, 0x04, 0xD2, 0, 0, 0x16, 0x2E]))?;
    let game_message = find(sp::MESSAGE_GAME, None)?;
    let public_chat = find(sp::MESSAGE_PUBLIC, None)?;
    let cutscene = find(sp::CUTSCENE, None)?;
    let cutscene_payload = &arrivals[cutscene].payload;
    // g2 id 2, g2 parameter 64, g1 length, appearance.
    anyhow::ensure!(cutscene_payload[..4] == [0, 2, 0, 64]);
    let count =
        |opcode: u8, frames: &[Arrival]| frames.iter().filter(|a| a.opcode == opcode).count();
    assert!(
        count(sp::PLAYER_INFO, &arrivals) > 20 && count(sp::NPC_INFO, &arrivals) > 20,
        "~20+ ticks of entity updates"
    );
    assert!(
        server_players.len() >= count(sp::PLAYER_INFO, &arrivals)
            && server_npcs.len() >= count(sp::NPC_INFO, &arrivals),
        "the server trace covers every recorded entity update"
    );

    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let mut tiles: Vec<[i32; 3]> = Vec::new();
    // The local player at the end of the previous cycle.
    let mut settled: Option<ActorView> = None;
    // The BAS type list (config group 32, as the runtime decoded it) and the
    // local player's appearance BAS.
    let bas_types = replay.game().inputs.bas.clone();
    let local_bas = |game: &crate::client_game::ClientGame| -> anyhow::Result<i32> {
        Ok(
            game.runtime.feed.state.players.players[game.runtime.map.local]
                .as_ref()
                .context("local player")?
                .appearance
                .bas,
        )
    };
    let (mut run_steps, mut walk_steps, mut turned) = (0, 0, 0);
    let mut npc_moves = 0;
    let mut last_npcs = BTreeMap::new();
    let mut cutscene_rebuild = None;
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // Exact outgoing bytes, cycle by cycle.
        assert_eq!(
            mask_wall_clock(&out.written)?,
            mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        if out
            .effects
            .iter()
            .any(|e| matches!(e, ClientEffect::CutsceneRebuild(_)))
        {
            cutscene_rebuild.get_or_insert(cycle);
        }
        assert!(
            !out.effects.iter().any(|e| matches!(
                e,
                ClientEffect::WorldSwitch(_)
                    | ClientEffect::LoginRequest(_)
                    | ClientEffect::Script(_)
            )),
            "cycle {cycle}: unexpected shell request"
        );
        let start = done;
        done = replay.processed(&arrivals);
        let fresh = &arrivals[start..done];
        let has = |index: usize| done > index;
        let game = replay.game();
        let base = [game.runtime.map.base_x, game.runtime.map.base_z];
        let view = observe(game);
        // Entities against the server's own positions, one entry per update
        // (until the cutscene takes over the scene).
        if count(sp::PLAYER_INFO, fresh) > 0 && !has(cutscene) {
            let local = view.local.as_ref().context("local player")?;
            let k = count(sp::PLAYER_INFO, &arrivals[..done]);
            let [x, z, level, _] = server_players[k - 1];
            assert_eq!(
                local.tile,
                [x, z, level],
                "cycle {cycle}: local player tile vs server"
            );
            let bas = &bas_types[&local_bas(replay.game())?];
            if k >= 2 {
                let [px, pz, _, angle] = server_players[k - 2];
                // The previous tick settled: facing the server's last step
                // direction (atan2(-dx, -dz) * 2607.59, the
                // actor angle) and back to the BAS ready anim.
                if let Some(settled) = settled
                    .as_ref()
                    .filter(|v: &&ActorView| v.route_length == 0)
                {
                    assert_eq!(
                        settled.desired_angle & 0x3FFF,
                        angle,
                        "cycle {cycle}: facing vs server"
                    );
                    assert_eq!(
                        settled.walk_anim, bas.readyanim,
                        "cycle {cycle}: ready anim"
                    );
                    turned += usize::from(angle != 0);
                }
                let step = (x - px).abs().max((z - pz).abs());
                if step > 0 {
                    // The route speed PLAYER_INFO installed (move speed 1 walk,
                    // 2 run) and the BAS movement anim it selects.
                    assert_eq!(i32::from(local.speed), step, "cycle {cycle}: move speed");
                    let anim = if step == 2 { bas.runanim } else { bas.walkanim };
                    assert_eq!(local.walk_anim, anim, "cycle {cycle}: movement anim");
                    run_steps += usize::from(step == 2);
                    walk_steps += usize::from(step == 1);
                }
            }
            tiles.push([x, z, level]);
        }
        settled = view.local.clone();
        if count(sp::NPC_INFO, fresh) > 0 && !has(cutscene) {
            let expected: BTreeMap<usize, (i32, [i32; 3])> = server_npcs
                [count(sp::NPC_INFO, &arrivals[..done]) - 1]
                .iter()
                .map(|&[index, id, x, z, level]| (index as usize, (id, [x, z, level])))
                .collect();
            let actual: BTreeMap<usize, (i32, [i32; 3])> = view
                .npcs
                .iter()
                .map(|(&index, (id, npc))| (index, (*id, npc.tile)))
                .collect();
            assert_eq!(actual, expected, "cycle {cycle}: NPCs vs server");
            npc_moves += actual
                .iter()
                .filter(|(i, v)| last_npcs.get(*i).is_some_and(|old| old != *v))
                .count();
            last_npcs = actual;
        }
        // CS2-visible variables, read through the script var domain.
        let game = replay.core.session.as_mut().unwrap().game.as_mut().unwrap();
        let (varp1001, varp1002, bit1000) = crate::client_game::with_game(game, |v| {
            let int = |v: native910::vm::Value| match v {
                native910::vm::Value::Int(v) => v,
                other => panic!("varp type {other:?}"),
            };
            Ok((
                int(v.get(native910::vars::VarScope::Player, 1001, false)?),
                int(v.get(native910::vars::VarScope::Player, 1002, false)?),
                v.get_bit(1000, false)?,
            ))
        })?;
        assert_eq!(
            varp1001,
            if has(varp_small) { -5 } else { 0 },
            "cycle {cycle}: varp 1001"
        );
        assert_eq!(
            varp1002,
            if has(varp_large) { 123_456 } else { 0 },
            "cycle {cycle}: varp 1002"
        );
        assert_eq!(
            bit1000,
            i32::from(has(varbit)),
            "cycle {cycle}: varbit 1000"
        );
        // Chat history lines (MESSAGE_GAME type 0; the server's
        // MESSAGE_PUBLIC echo of the typed line, type 2 under the name).
        let ui = replay.ui();
        let chat: Vec<(i32, String, String)> = (0..=ui.engine.messages.history.last_uid())
            .filter_map(|uid| ui.engine.messages.history.get_by_uid(uid))
            .map(|l| (l.chat_type, l.name.clone(), l.message.clone()))
            .collect();
        let mut expected_chat = Vec::new();
        if has(game_message) {
            expected_chat.push((0, String::new(), "hello from the replay".to_string()));
        }
        if has(public_chat) {
            expected_chat.push((2, "replay".to_string(), "hello replay".to_string()));
        }
        assert_eq!(chat, expected_chat, "cycle {cycle}: chat history");
        // Hint arrow: a type-2 tile target at the tile centre (256, 256).
        let hint_state = replay.minimap.hint_arrow_state[0].as_ref();
        if has(hint) {
            let arrow = hint_state.context("hint arrow slot 0")?;
            assert_eq!(
                (
                    arrow.hint_type,
                    arrow.fine,
                    arrow.level,
                    arrow.distance_tiles,
                    arrow.model
                ),
                (
                    2,
                    Some([(3224 - base[0]) * 512 + 256, (3224 - base[1]) * 512 + 256]),
                    Some(0),
                    64,
                    -1
                ),
                "cycle {cycle}: hint arrow"
            );
        } else {
            assert!(
                hint_state.is_none(),
                "cycle {cycle}: hint arrow before HINT_ARROW"
            );
        }
        // Audio owner: MIDI_SONG 2 -> the audio API plays the song.
        assert_eq!(
            replay.ui().audio.current_song(),
            if has(song) { 2 } else { -1 },
            "cycle {cycle}: current song"
        );
        // Cutscene owner: the cutscene id, its parameter and
        // the appearance block; sceneState leaves 3.
        let c = &replay.game().cutscene;
        if has(cutscene) {
            assert_eq!(
                (c.client_id, c.map_capacity, c.appearance.as_deref()),
                (2, 64, Some(&cutscene_payload[5..])),
                "cycle {cycle}: cutscene"
            );
            assert_ne!(c.scene_state, 3, "cycle {cycle}: cutscene scene state");
        } else {
            assert_eq!(
                (c.client_id, c.scene_state),
                (-1, 3),
                "cycle {cycle}: no cutscene yet"
            );
        }
        // NO_TIMEOUT (83) is not answered by the read that takes it (the
        // client's own keepalive is a timer, so a quiet cycle writes
        // nothing); SEND_PING echoes p4_alt1 / p4_alt3 (the ping echo).
        let written = client_frames(&out.written)?;
        if (start..done).contains(&no_timeout) {
            assert!(
                !written.contains(&(cp::NO_TIMEOUT, vec![])),
                "cycle {cycle}: the server's NO_TIMEOUT answered"
            );
        }
        if (start..done).contains(&ping) {
            let reply = written
                .iter()
                .find(|(op, _)| *op == cp::SEND_PING_REPLY)
                .context("SEND_PING_REPLY")?;
            assert_eq!(reply.1[..8], [0xD2, 0x04, 0, 0, 0, 0, 0x2E, 0x16]);
        }
        if cycle == 1 {
            // The startup map acknowledgement.
            assert!(written
                .iter()
                .any(|(op, p)| *op == cp::MAP_BUILD_COMPLETE && p.len() == 4));
        }
    }
    assert!(done > cutscene, "the replay reached the CUTSCENE packet");
    assert!(walk_steps >= 2, "the player walked: {tiles:?}");
    assert!(
        run_steps >= 2,
        "the run toggle made the player run: {tiles:?}"
    );
    assert!(turned >= 2, "settled facings after moves were checked");
    let distinct: std::collections::BTreeSet<_> = tiles.iter().collect();
    assert!(distinct.len() >= 4, "the player walked: {tiles:?}");
    assert!(npc_moves > 0, "NPCs wandered");
    // CUTSCENE -> a cutscene map rebuild: the map request the recorded
    // client installed right after the fixture ends.
    assert!(cutscene_rebuild.is_some(), "cutscene rebuild request");
    Ok(())
}

/// One server frame as the dev server writes it (no ISAAC; the opcode /
/// length header and the server packet size table).
pub(super) fn server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = if opcode < 128 {
        vec![opcode]
    } else {
        vec![0x80, opcode]
    };
    match crate::proto::server::size(opcode) {
        Some(-1) => out.push(payload.len() as u8),
        Some(-2) => out.extend_from_slice(&(payload.len() as u16).to_be_bytes()),
        Some(n) => assert_eq!(
            n as usize,
            payload.len(),
            "{}",
            crate::proto::server::name(opcode)
        ),
        None => panic!("unknown server opcode {opcode}"),
    }
    out.extend_from_slice(payload);
    out
}

/// Every loc change ends with a minimap refresh at the local player's
/// level: LOC_DEL / LOC_ADD_CHANGE /
/// UPDATE_ZONE_FULL_FOLLOWS bytes on the world socket of the recorded
/// Lumbridge session (player at 3222,3222 level 0), applied by
/// `ViewerApp::apply_location_changes`, change the minimap's map-element loc
/// queue (drawn by the minimap draw).
///
/// Expected queues are hand-derived from the refresh rules against
/// the startup queue: tiles are visited x-major then z, each
/// tile queues the first mappable loc of ground decoration, entity, wall,
/// wall decoration, and only levels from the base level to `level + 1` are
/// scanned. Frame bytes follow the dev-server encoders (LOC_DEL /
/// LOC_ADD_CHANGE / UPDATE_ZONE_PARTIAL_FOLLOWS with the p1_alt2/p1_alt3/
/// p4_alt3 packet writers) and, for FULL_FOLLOWS (no server encoder), the
/// wire layout (g1b_alt2 z, g1_alt2 level, g1b_alt3 x).
/// What a login reports comes from the live session: the window mode and
/// canvas of the window owner, the anti-aliasing level and the encoded
/// options, and sending it marks the preferences as told to the server.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn login_reports_the_live_window_and_options() -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    let session = replay.session();
    {
        let preferences = &mut session
            .game
            .as_mut()
            .context("game")?
            .ui_variables
            .queries
            .preferences;
        preferences.change_notified = false;
        preferences.options.set_field("brightness", 1);
    }
    let report = login_client_report(session);
    let preferences = &session
        .game
        .as_ref()
        .unwrap()
        .ui_variables
        .queries
        .preferences;
    assert_eq!(report.preferences, preferences.options.encode());
    assert_eq!(report.preferences.len(), 58);
    assert_eq!(i32::from(report.window_mode), preferences.window.mode);
    assert_eq!(
        report.anti_aliasing,
        preferences.options.get("antiAliasing2").unwrap() as u8
    );
    assert!(preferences.change_notified);
    let canvas = preferences
        .window
        .canvas(preferences.options.live().screen_size)
        .context("canvas")?;
    assert_eq!(
        report.canvas,
        [canvas.size[0] as u16, canvas.size[1] as u16]
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn loc_changes_refresh_the_minimap_loc_queue() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let (mut app, (mut peer, _dir, _clock)) = Replay::start(&trace)?.into_app();
    // `minimap_frames`' first-draw setup (install, members flags).
    let pack = app.pack.clone();
    app.minimap.install(&pack);
    {
        let session = app.core.session.as_ref().context("session")?;
        let ui = &session.ui;
        app.minimap.logged_in_members = ui.engine.account.logged_in_members;
        let allow = session.game.as_ref().context("game")?.allow_members;
        if let Some(locs) = &app.minimap.locs {
            locs.allow_members.set(allow);
        }
    }
    let (base, local) = {
        let game = app.core.session.as_ref().unwrap().game.as_ref().unwrap();
        let p = game.runtime.feed.state.players.players[game.runtime.map.local]
            .as_ref()
            .context("local player")?;
        (
            [game.runtime.map.base_x, game.runtime.map.base_z],
            [p.x[0], p.z[0], p.level],
        )
    };
    assert_eq!(
        [base[0] + local[0], base[1] + local[1], local[2]],
        [3222, 3222, 0],
        "the recorded session starts at the Lumbridge spawn"
    );
    // The startup queue, as `rebuildMinimapBase` leaves it.
    {
        let game = app.core.session.as_ref().unwrap().game.as_ref().unwrap();
        app.minimap.refresh(
            app.scene.graph.as_ref().context("scene graph")?,
            game.runtime.terrain.as_ref().context("terrain")?,
            0,
            Some([local[0], local[1]]),
            &Default::default(),
        );
    }
    let startup = app.minimap.queued_locs.clone();
    // Cache facts: the water source icon loc (map element 595) is a ground
    // decoration at 3222,3226; the range icon loc (map element 596) is next
    // to it; 3223,3223 has none.
    let water_source_icon = rs910_symbols::loc::MAP_ICON_WATER_SOURCE.id() as u32;
    let range_icon = rs910_symbols::loc::MAP_ICON_RANGE.id() as u32;
    let safe = rs910_symbols::loc::MISTHALIN_SAFE.id() as u32;
    let bank_icon = rs910_symbols::loc::MAP_ICON_BANK.id();
    let (fx, fz) = (3222 - base[0], 3226 - base[1]);
    let (ax, az) = (3223 - base[0], 3223 - base[1]);
    assert_eq!(
        startup
            .iter()
            .filter(|&&(_, x, z)| (x, z) == (fx, fz))
            .count(),
        1
    );
    assert!(
        startup.contains(&(water_source_icon, fx, fz)),
        "{startup:?}"
    );
    assert!(!startup.iter().any(|&(_, x, z)| (x, z) == (ax, az)));
    let insert = |queue: &[(u32, i32, i32)], entry: (u32, i32, i32)| {
        let at = queue
            .iter()
            .take_while(|&&(_, x, z)| (x, z) < (entry.1, entry.2))
            .count();
        let mut out = queue.to_vec();
        out.insert(at, entry);
        out
    };
    // Scene-local zone origin bytes: UPDATE_ZONE_PARTIAL_FOLLOWS
    // p1(level), p1(-(z >> 3)), p1(x >> 3).
    let partial = |level: u8, x: i32, z: i32| {
        server_frame(
            sp::UPDATE_ZONE_PARTIAL_FOLLOWS,
            &[level, (-(z >> 3)) as u8, (x >> 3) as u8],
        )
    };
    let coord = |x: i32, z: i32| (((x & 7) << 4) | (z & 7)) as u8;
    // LOC_DEL: p1(shape << 2 | angle), p1_alt3(coord).
    let loc_del = |x: i32, z: i32, shape: u8, angle: u8| {
        server_frame(
            sp::LOC_DEL,
            &[shape << 2 | angle, 128u8.wrapping_sub(coord(x, z))],
        )
    };
    // LOC_ADD_CHANGE: p1(shape << 2 | angle), p4_alt3(id), p1_alt2(coord).
    let loc_add = |x: i32, z: i32, id: u32, shape: u8, angle: u8| {
        server_frame(
            sp::LOC_ADD_CHANGE,
            &[
                shape << 2 | angle,
                (id >> 16) as u8,
                (id >> 24) as u8,
                id as u8,
                (id >> 8) as u8,
                coord(x, z).wrapping_neg(),
            ],
        )
    };
    let sun = crate::floor::SunLighting::environment_default(3, 0.0);
    // Deliver one server tick through the client's socket read and apply
    // the retained requests the way the redraw does.
    let mut deliver = |app: &mut ViewerApp, frames: Vec<Vec<u8>>| -> anyhow::Result<()> {
        let mut bytes: Vec<u8> = frames.concat();
        bytes.extend(server_frame(sp::SERVER_TICK_END, &[]));
        peer.write_all(&bytes)?;
        let session = app.core.session.as_mut().context("session")?;
        let started = std::time::Instant::now();
        // As `Replay::cycle`: the bytes are readable before the poll.
        let stream = session.io.world.stream.as_ref().context("world stream")?;
        let mut probe = vec![0u8; bytes.len()];
        while !matches!(stream.peek(&mut probe), Ok(n) if n >= bytes.len()) {
            anyhow::ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "loopback delivery stalled"
            );
            std::thread::yield_now();
        }
        loop {
            match poll_live_session(session, &mut LiveIo) {
                LivePollOutcome::Lost(reason) => anyhow::bail!("connection lost: {reason}"),
                LivePollOutcome::Idle | LivePollOutcome::Polled(_) => {}
            }
            let game = session.game.as_ref().context("game")?;
            if session.io.world.pending.is_empty() && game.runtime.feed.front().is_none() {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "the tick was not read"
            );
            std::thread::yield_now();
        }
        app.apply_location_changes(&sun)
    };
    let zone = |x: i32| x & !7;

    // LOC_DEL of the water source icon: its tile queues nothing now.
    deliver(
        &mut app,
        vec![partial(0, zone(fx), zone(fz)), loc_del(fx, fz, 22, 0)],
    )?;
    let deleted: Vec<_> = startup
        .iter()
        .copied()
        .filter(|&(_, x, z)| (x, z) != (fx, fz))
        .collect();
    assert_eq!(app.minimap.queued_locs, deleted, "after LOC_DEL");

    // LOC_ADD_CHANGE of a range icon (ground decoration) on an empty tile:
    // queued at its x-major position.
    deliver(
        &mut app,
        vec![
            partial(0, zone(ax), zone(az)),
            loc_add(ax, az, range_icon, 22, 0),
        ],
    )?;
    let added = insert(&deleted, (range_icon, ax, az));
    assert_eq!(app.minimap.queued_locs, added, "after LOC_ADD_CHANGE");

    // A centrepiece (shape 10, layer 2: the scene entity layer) on an empty tile:
    // a Misthalin safe loc, map element 4129 (a thieving spot icon),
    // 1x1, primary-layer flag 1 (a primary-layer entity).
    let (ex, ez) = (3224 - base[0], 3224 - base[1]);
    assert!(!startup.iter().any(|&(_, x, z)| (x, z) == (ex, ez)));
    deliver(
        &mut app,
        vec![partial(0, zone(ex), zone(ez)), loc_add(ex, ez, safe, 10, 0)],
    )?;
    let added = insert(&added, (safe, ex, ez));
    assert_eq!(
        app.minimap.queued_locs, added,
        "after an entity LOC_ADD_CHANGE"
    );

    // A bank icon on level 2: the minimap refresh at the local player level 0 scans
    // levels 0..=1 only, so the queue is unchanged.
    deliver(
        &mut app,
        vec![
            partial(2, zone(ax), zone(az)),
            loc_add(ax, az, bank_icon as u32, 22, 0),
        ],
    )?;
    {
        let game = app.core.session.as_ref().unwrap().game.as_ref().unwrap();
        assert!(game
            .runtime
            .feed
            .state
            .zones
            .locations
            .iter()
            .any(|r| r.id == bank_icon && (r.level, r.layer, r.x, r.z) == (2, 3, ax, az)));
    }
    assert_eq!(
        app.minimap.queued_locs, added,
        "after a level-2 LOC_ADD_CHANGE"
    );

    // UPDATE_ZONE_FULL_FOLLOWS on the deleted icon's zone marks its request
    // as remove-requested; the replacement of the old id restores the map's own loc.
    let full = server_frame(
        sp::UPDATE_ZONE_FULL_FOLLOWS,
        &[
            (-(zone(fz) >> 3)) as u8,
            0u8.wrapping_neg(),
            128u8.wrapping_sub((zone(fx) >> 3) as u8),
        ],
    );
    deliver(&mut app, vec![full])?;
    assert_eq!(
        app.minimap.queued_locs,
        insert(&insert(&startup, (range_icon, ax, az)), (safe, ex, ez)),
        "after UPDATE_ZONE_FULL_FOLLOWS"
    );
    Ok(())
}

/// FNV-1a over a value's `Debug` rendering, streamed (no string).
fn debug_digest(value: &impl std::fmt::Debug) -> u64 {
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for &b in s.as_bytes() {
                self.0 ^= u64::from(b);
                self.0 = self.0.wrapping_mul(0x100000001b3);
            }
            Ok(())
        }
    }
    let mut h = Fnv(0xcbf29ce484222325);
    std::fmt::write(&mut h, format_args!("{value:?}")).unwrap();
    h.0
}

/// Phase 0 clock injection (programme §8, `logic_clock` module docs): the
/// recorded session replayed twice on the injected clock (each cycle's
/// recorded `NOWM`) yields identical per-cycle state digests — the whole
/// entity/zone/var world (`live::State`), the logic cycle and the
/// observed actors — and byte-identical outgoing packets with nothing
/// masked: `MAP_BUILD_COMPLETE`'s rebuild duration and `SEND_PING_REPLY`'s
/// fps byte read the injected clock as well. (Against the *recording*,
/// [`mask_wall_clock`] stays: those bytes measured the recording machine's
/// wall clock inside one cycle, which the per-cycle `NOWM` does not carry.)
/// The two replays run concurrently, as the other tests' [`ReplayPlan`]
/// replays do.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn two_replays_on_the_injected_clock_have_identical_cycle_digests() -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let run = |_: &usize| -> anyhow::Result<Vec<(Vec<u8>, u64)>> {
        let mut replay = Replay::start(&trace)?;
        let mut cycles = Vec::new();
        for cycle in 1..=trace.last_cycle() {
            let out = replay.cycle(&trace, cycle)?;
            let game = replay.game();
            let digest = debug_digest(&(
                game.cycle,
                crate::logic_clock::monotonic_millis(),
                &game.runtime.feed.state,
                observe(game),
            ));
            cycles.push((out.written, digest));
        }
        Ok(cycles)
    };
    // The two replays run at the same time on their own threads, so they
    // also show that concurrent replays (`ReplayPlan`) keep apart.
    let mut plan = ReplayPlan::default();
    let (first, second) = (plan.add(0), plan.add(1));
    let results = plan.run(run)?;
    let (first, second) = (&results[first], &results[second]);
    anyhow::ensure!(first.len() > 20, "{} cycles", first.len());
    for (cycle, (a, b)) in (1..).zip(first.iter().zip(second)) {
        assert_eq!(
            a.0, b.0,
            "cycle {cycle}: outgoing bytes differ between replays"
        );
        assert_eq!(
            a.1, b.1,
            "cycle {cycle}: state digest differs between replays"
        );
    }
    Ok(())
}

/// Renderer plan A3 inertness gate: the `--renderer` choice changes what
/// draws, never what the client does. The recorded session is replayed
/// under each [`crate::active_toolkit::RendererKind`] (faithful-gpu, null and,
/// since renderer plan M1, modern), each with the capability answers that
/// renderer's `ActiveToolkit` gives (`RendererKind::capability_profile`
/// over the recorded device, through `capability::Answers`), and every
/// cycle's outbound bytes, entity/zone/var state, saved `ClientOptions`
/// bytes, the capability answers the scripts read
/// (`Preferences.anti_aliasing/bloom`, the UI's input-telemetry texture
/// formats) and CS2 hook executions (id, result, int stack, pc) must equal
/// the default renderer's.
///
/// The toolkit a session installs is its saved one, whatever toolkit the
/// renderer runs at that moment: each session is also replayed installing
/// under the other toolkit's state (the title screen's loading-screen
/// toolkit 0 with a saved hardware toolkit, and the reverse) and must equal
/// the consistent one, its capability answers included.
///
/// Lane DROP-SW: toolkit 0 (`displayMode` 0) is game state drawn by the
/// GPU renderer. A toolkit-0 session (saved `displayMode` 0, the toolkit-0
/// answers) replays identically under every renderer too, and its answers
/// are the software toolkit's (no anti-aliasing, no bloom),
/// as when the software toolkit drew
/// it. Sensitivity: the toolkit-0 session must differ from the hardware one
/// by the first cycle, and so must a hardware profile without capabilities,
/// so neither comparison is vacuous.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn renderer_choice_is_observationally_inert() -> anyhow::Result<()> {
    use crate::active_toolkit::RendererKind;
    use clap::ValueEnum;
    use rs910_toolkit::capability::{Answers, Profile};
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let (device, _) = recorded_profile(&trace)?;
    // Per cycle: outbound bytes, the state digest and the capability answers
    // the scripts read (`Preferences.anti_aliasing`, `bloom`).
    type Cycles = Vec<(Vec<u8>, u64, (bool, bool))>;
    let run = |answers: &Answers, saved_toolkit0: bool, last: i32| -> anyhow::Result<Cycles> {
        let mut replay = Replay::start_at_install(&trace, answers, saved_toolkit0)?;
        {
            let ui = &mut replay.core.session.as_mut().unwrap().ui;
            ui.diagnostics.capture = true;
        }
        let mut cycles = Vec::new();
        for cycle in 1..=last {
            let out = replay.cycle(&trace, cycle)?;
            let ui = &mut replay.core.session.as_mut().unwrap().ui;
            let hooks = std::mem::take(&mut ui.diagnostics.executions);
            let formats = ui.engine.platform.gl_texture_formats.clone();
            let game = replay.game();
            let preferences = &game.ui_variables.queries.preferences;
            let digest = debug_digest(&(
                game.cycle,
                &game.runtime.feed.state,
                observe(game),
                preferences.options.encode(),
                // The capability answers the scripts read.
                (
                    preferences.anti_aliasing,
                    preferences.bloom,
                    preferences.bloom_enabled,
                    preferences.active_aa,
                ),
                formats,
                hooks,
            ));
            let answers = (preferences.anti_aliasing, preferences.bloom);
            cycles.push((out.written, digest, answers));
        }
        Ok(cycles)
    };
    let same = |label: &str, a: &Cycles, b: &Cycles| {
        assert_eq!(a.len(), b.len(), "{label}: cycle count");
        for (cycle, (a, b)) in (1..).zip(a.iter().zip(b)) {
            assert_eq!(a.0, b.0, "{label} cycle {cycle}: outgoing bytes differ");
            assert_eq!(a.1, b.1, "{label} cycle {cycle}: state digest differs");
        }
    };
    let last = trace.last_cycle();
    // Renderer plan M1: the modern renderer is one of the replayed choices.
    anyhow::ensure!(RendererKind::value_variants().contains(&RendererKind::Modern));
    let answers = |kind: RendererKind, toolkit0: bool| Answers {
        toolkit0,
        profile: kind.capability_profile(device.clone()),
    };
    // A replay's whole input is (capability answers, saved toolkit, cycles):
    // renderers whose answers coincide replay the same input, so each
    // distinct input replays once (a replay is deterministic:
    // `two_replays_on_the_injected_clock_have_identical_cycle_digests`), and
    // the distinct replays run in parallel. Every renderer's comparison is
    // still made.
    let mut plan = ReplayPlan::default();
    let mut comparisons = Vec::new();
    // The hardware toolkit (the recorded session) and a toolkit-0 session,
    // each under every renderer.
    let mut sessions = Vec::new();
    for toolkit0 in [false, true] {
        let reference = plan.add((answers(RendererKind::FaithfulGpu, toolkit0), toolkit0, last));
        for &kind in RendererKind::value_variants() {
            if kind != RendererKind::FaithfulGpu {
                let other = plan.add((answers(kind, toolkit0), toolkit0, last));
                comparisons.push((format!("{kind:?} toolkit0={toolkit0}"), reference, other));
            }
            // The session installs its saved toolkit while the renderer still
            // runs the other one: a title-screen login installs the saved
            // hardware toolkit during the loading screens' toolkit 0, and a
            // toolkit-0 session on a device that starts as hardware. The
            // saved toolkit's answers are the session's either way.
            let at_install = plan.add((answers(kind, !toolkit0), toolkit0, last));
            comparisons.push((
                format!(
                    "{kind:?} saved toolkit0={toolkit0} installed under toolkit0={}",
                    !toolkit0
                ),
                reference,
                at_install,
            ));
        }
        sessions.push(reference);
    }
    // Sensitivity: toolkit 0's answers (no bloom, no anti-aliasing, no
    // texture formats) as a hardware profile are not the device's and must
    // show by the first cycle.
    let software_profile = Profile {
        scene_samples: [1].into(),
        ..Default::default()
    };
    anyhow::ensure!(
        software_profile != device,
        "the recorded device has no capability"
    );
    let leaked = plan.add((Answers::hardware(software_profile), false, 1));
    let results =
        plan.run(|(answers, saved_toolkit0, last)| run(answers, *saved_toolkit0, *last))?;
    for &reference in &sessions {
        let cycles = results[reference].len();
        anyhow::ensure!(cycles > 20, "{cycles} cycles");
    }
    for (label, reference, other) in &comparisons {
        same(label, &results[*reference], &results[*other]);
    }
    let (hardware, toolkit0) = (&results[sessions[0]], &results[sessions[1]]);
    // Toolkit 0 answers as the software toolkit whatever draws it; the recorded
    // device answers anti-aliasing and bloom.
    anyhow::ensure!(
        device.supports_antialiasing() || device.supports_bloom(),
        "the recorded device has no capability"
    );
    for (cycle, c) in (1..).zip(toolkit0) {
        assert_eq!(
            c.2,
            (false, false),
            "toolkit 0 cycle {cycle}: anti-aliasing/bloom answers are not the software toolkit's"
        );
    }
    assert_ne!(
        hardware[0].1, toolkit0[0].1,
        "the toolkit-0 state went unnoticed"
    );
    assert_ne!(
        results[leaked][0], hardware[0],
        "a non-faithful capability profile went unnoticed"
    );
    Ok(())
}

/// At most this many replays of one test run at once: a started replay
/// holds about 1.2 GB (the decoded interfaces, the scene and the minimap),
/// and the gate runs the replay tests side by side. nextest reserves a
/// test slot per replay thread (`threads-required` in
/// `tools/.config/nextest.toml`), so the gate never runs more replays than
/// it has slots.
const REPLAY_THREADS: usize = 4;

/// The distinct replays a test needs, each run once, up to
/// [`REPLAY_THREADS`] at a time on their own threads (`logic_clock`'s test
/// clock is per thread, so concurrent replays keep their recorded clocks
/// apart). [`ReplayPlan::add`] returns the index of the input's result; an
/// input added twice shares one replay.
struct ReplayPlan<K> {
    inputs: Vec<K>,
}

impl<K> Default for ReplayPlan<K> {
    fn default() -> Self {
        Self { inputs: Vec::new() }
    }
}

impl<K: PartialEq + Sync> ReplayPlan<K> {
    fn add(&mut self, input: K) -> usize {
        match self.inputs.iter().position(|k| *k == input) {
            Some(index) => index,
            None => {
                self.inputs.push(input);
                self.inputs.len() - 1
            }
        }
    }

    /// `replay` over every input; the results in input order. A replay's
    /// panic is raised again here.
    fn run<T: Send>(
        &self,
        replay: impl Fn(&K) -> anyhow::Result<T> + Sync,
    ) -> anyhow::Result<Vec<T>> {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<_> = self
            .inputs
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..self.inputs.len().min(REPLAY_THREADS))
                .map(|_| {
                    std::thread::Builder::new()
                        .name("replay".into())
                        .stack_size(16 << 20)
                        .spawn_scoped(scope, || loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(input) = self.inputs.get(index) else {
                                break;
                            };
                            let result = replay(input);
                            *results[index].lock().unwrap() = Some(result);
                        })
                        .expect("replay thread")
                })
                .collect();
            for worker in workers {
                if let Err(panic) = worker.join() {
                    std::panic::resume_unwind(panic);
                }
            }
        });
        results
            .into_iter()
            .map(|slot| slot.into_inner().unwrap().expect("every input replayed"))
            .collect()
    }
}

/// The cycles a sensitivity replay runs: enough for the change it makes to
/// show (it shows from the first cycle).
const SENSITIVITY_CYCLES: i32 = 20;

/// Lane E-A1 (engine audit §1, programme §8 "Phase 5 redraw logic"): the
/// redraw cadence is the client's — one full redraw (`ClientCore::full_redraw_state`,
/// every state-writing step) after each frame with logic cycles, presents
/// otherwise (`ClientCore::redraw`) — so the number of redraws between
/// cycles is not observable. Both recorded sessions replay with 1 and with
/// 3 redraws after every cycle; per cycle the outbound bytes and a digest
/// of the entity/zone/var world, the observed actors, the saved
/// `ClientOptions`, the scene locs, the UI's mouse and cross state, every
/// CS2 hook execution and the core's redraw state (the scene cycle, the
/// minimap flag, the environment's fade, the positioned loc sounds) must be
/// identical. Sensitivity: making every redraw a full redraw must show
/// (within the first [`SENSITIVITY_CYCLES`] cycles). One test per session,
/// so the sessions run side by side.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn present_rate_is_observationally_inert() -> anyhow::Result<()> {
    present_rate_inertness(SESSIONS[0])
}

/// [`present_rate_is_observationally_inert`] over the woodcutting session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn present_rate_is_observationally_inert_in_the_woodcutting_session() -> anyhow::Result<()> {
    present_rate_inertness(SESSIONS[1])
}

/// One recorded session with 1 and 3 redraws per cycle, and the
/// sensitivity replay; the three replays in parallel.
fn present_rate_inertness(fixture: &str) -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    let trace = &Trace::load(&root.join(fixture))?;
    // Every (redraws per cycle, every redraw full, cycles) replays once.
    type Cycles = Vec<(Vec<u8>, u64)>;
    let run = |&(redraws, every_redraw_full, last): &(usize, bool, i32)| -> anyhow::Result<Cycles> {
        let mut replay = Replay::start(trace)?;
        replay.session().ui.diagnostics.capture = true;
        let mut cycles = Vec::new();
        for cycle in 1..=last {
            let out = replay.cycle(trace, cycle)?;
            for redraw in 0..redraws {
                if every_redraw_full {
                    replay.core.request_full_redraw();
                }
                let kind = replay.redraw();
                anyhow::ensure!(
                    every_redraw_full || (kind == RedrawKind::Full) == (redraw == 0),
                    "cycle {cycle} redraw {redraw}: {kind:?}"
                );
            }
            let hooks = std::mem::take(&mut replay.session().ui.diagnostics.executions);
            let core = &replay.core;
            let ui = replay.ui();
            let redraw_state = (
                core.scene_cycle,
                core.minimap,
                &core.environment.fade_target,
                core.environment.fade.is_some(),
                ui.audio.positioned.locs.len(),
                ui.target.cross,
                ui.drawn_scene_delta,
                (
                    ui.engine.platform.mouse,
                    ui.engine.platform.pending_mouse,
                    ui.input.click,
                ),
            );
            let game = replay.game();
            let digest = debug_digest(&(
                game.cycle,
                crate::logic_clock::monotonic_millis(),
                &game.runtime.feed.state,
                observe(game),
                game.ui_variables.queries.preferences.options.encode(),
                &game.scene_locs,
                redraw_state,
                hooks,
            ));
            cycles.push((out.written, digest));
        }
        Ok(cycles)
    };
    let mut plan = ReplayPlan::default();
    let last = trace.last_cycle();
    let (one, three) = (plan.add((1, false, last)), plan.add((3, false, last)));
    // Sensitivity: three full redraws per cycle run the state steps three
    // times (the scene cycle alone counts them), so the digests differ from
    // the first cycle on; a difference within the first cycles is a
    // difference of the sessions.
    let forced = plan.add((3, true, last.min(SENSITIVITY_CYCLES)));
    let results = plan.run(run)?;
    let (one, three, forced) = (&results[one], &results[three], &results[forced]);
    anyhow::ensure!(one.len() > 20, "{fixture}: {} cycles", one.len());
    assert_eq!(one.len(), three.len(), "{fixture}: cycle count");
    for (cycle, (a, b)) in (1..).zip(one.iter().zip(three)) {
        assert_eq!(a.0, b.0, "{fixture} cycle {cycle}: outbound bytes differ");
        assert_eq!(a.1, b.1, "{fixture} cycle {cycle}: state digest differs");
    }
    assert_ne!(
        one[..forced.len()].iter().map(|c| c.1).collect::<Vec<_>>(),
        forced.iter().map(|c| c.1).collect::<Vec<_>>(),
        "{fixture}: extra full redraws went unnoticed"
    );
    Ok(())
}

/// Engine profiler inertness (lane E-A4, `rs910_core::profile` module
/// docs): both recorded sessions replayed with the profiler recording (one
/// frame per cycle, the P0-P11 and scene scopes, a CSV dump) give every
/// cycle the same outbound bytes and the same digest of the entity/zone/var
/// world, the observed actors, the saved `ClientOptions` and the CS2 hook
/// executions as with it off. Not vacuous: the lib tests compile the
/// profiler in (client910's rs910-core dev-dependency) and the profiled run
/// must have recorded every phase scope. This test replays the scripted
/// session; [`profiler_is_observationally_inert_in_the_woodcutting_session`]
/// the other, so the two run side by side.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn profiler_is_observationally_inert() -> anyhow::Result<()> {
    profiler_inertness(SESSIONS[0])
}

/// [`profiler_is_observationally_inert`] over the woodcutting session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn profiler_is_observationally_inert_in_the_woodcutting_session() -> anyhow::Result<()> {
    profiler_inertness(SESSIONS[1])
}

/// The profiler's switch is process-wide: profiler tests sharing a process
/// (`cargo test`) take turns.
static PROFILER_SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One recorded session replayed with the profiler off, then recording.
fn profiler_inertness(fixture: &str) -> anyhow::Result<()> {
    use rs910_core::profile;
    let _turn = PROFILER_SWITCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = &rs910_core::test_support::client_dir();
    anyhow::ensure!(
        profile::COMPILED,
        "the lib tests compile the profiler in (rs910-core dev-dependency)"
    );
    let trace = Trace::load(&root.join(fixture))?;
    let run = |profiled: bool| -> anyhow::Result<Vec<(Vec<u8>, u64)>> {
        if profiled {
            profile::enable();
            profile::with_profiler(|p| p.set_dump(Box::new(std::io::sink())));
        }
        let mut replay = Replay::start(&trace)?;
        replay.core.session.as_mut().unwrap().ui.diagnostics.capture = true;
        let mut cycles = Vec::new();
        for cycle in 1..=trace.last_cycle() {
            profile::frame_mark();
            let out = replay.cycle(&trace, cycle)?;
            let ui = &mut replay.core.session.as_mut().unwrap().ui;
            let hooks = std::mem::take(&mut ui.diagnostics.executions);
            let game = replay.game();
            let digest = debug_digest(&(
                game.cycle,
                &game.runtime.feed.state,
                observe(game),
                game.ui_variables.queries.preferences.options.encode(),
                hooks,
            ));
            cycles.push((out.written, digest));
        }
        profile::frame_mark();
        profile::disable();
        Ok(cycles)
    };
    let off = run(false)?;
    let on = run(true)?;
    anyhow::ensure!(off.len() > 20, "{fixture}: {} cycles", off.len());
    assert_eq!(off.len(), on.len());
    for (cycle, (a, b)) in (1..).zip(off.iter().zip(&on)) {
        assert_eq!(a.0, b.0, "{fixture} cycle {cycle}: outgoing bytes differ");
        assert_eq!(a.1, b.1, "{fixture} cycle {cycle}: state digest differs");
    }
    let names: Vec<&str> = profile::with_profiler(|p| {
        p.summary(usize::MAX)
            .scopes
            .iter()
            .map(|s| s.name)
            .collect()
    })
    .unwrap_or_default();
    for phase in [
        "logic",
        "logic cycle",
        "P0 resize",
        "P1 js5",
        "P2 loading",
        "P3 cycle",
        "P4 rebuild",
        "P5 session cycle",
        "P6 input script",
        "P7 connections",
        "P8 live read",
        "P9 update game",
        "P10 effects",
        "P11 title prefs",
    ] {
        assert!(
            names.contains(&phase),
            "{fixture}: the profiled replay recorded no {phase:?} scope ({names:?})"
        );
    }
    Ok(())
}

/// The headless replay's harness before Phase 5 (frozen copy of
/// `Replay::step`, 8319da27): the session owners stepped by hand in
/// `ViewerApp::about_to_wait`'s order over a real loopback socket
/// (`LiveIo`), with the recorded clock set before the step. Kept to prove
/// that `ClientCore::frame` + `ReplayIo` replays a recording identically.
mod legacy {
    use super::*;
    use std::io::Read;

    /// One logic cycle the old way; `cursor` is the old harness's field.
    pub(super) fn cycle(
        replay: &mut Replay,
        trace: &Trace,
        cycle: i32,
        cursor: &mut i32,
    ) -> anyhow::Result<CycleOutput> {
        crate::logic_clock::set_test_now(trace.now(cycle));
        let inbound = trace.bytes(b"IN  ", cycle);
        // about_to_wait's frame start: the latest native size.
        if let Some([w, h]) = replay.pending_resize.take() {
            let session = replay.core.session.as_mut().unwrap();
            if let Some(game) = session.game.as_mut() {
                game.ui_variables
                    .queries
                    .preferences
                    .window
                    .observe_size([w, h], replay.scale);
                game.ui_variables.queries.preferences.window.changed = true;
            }
            sync_window_state(session, None, &mut |_| Ok(()))?;
        }
        if let Some(record) = trace
            .records
            .iter()
            .rfind(|r| &r.tag == b"CURS" && r.cycle == cycle)
        {
            *cursor = i32::from_le_bytes(record.bytes[..4].try_into()?);
        }
        anyhow::ensure!(
            !trace
                .records
                .iter()
                .any(|r| &r.tag == b"MAPI" && r.cycle == cycle),
            "cycle {cycle}: a map transaction completed mid-session (not replayed)"
        );
        let core = &mut *replay.core;
        let session = core.session.as_mut().unwrap();
        begin_session_cycle(session, cycle, &|| None);
        for injection in core.script.ui_injections(cycle) {
            apply_ui_injection(session, cycle, injection);
        }
        if let Some(game) = session.game.as_mut() {
            game.cycle = cycle;
        }
        // The bytes the socket had for this cycle's reads.
        replay.delivered += inbound.len();
        if !inbound.is_empty() {
            replay.peer.write_all(&inbound)?;
            let stream = session.io.world.stream.as_ref().context("world stream")?;
            let mut probe = vec![0u8; inbound.len() + 1];
            let started = std::time::Instant::now();
            loop {
                match stream.peek(&mut probe) {
                    Ok(n) if n >= inbound.len() => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
                anyhow::ensure!(
                    started.elapsed() < std::time::Duration::from_secs(5),
                    "loopback delivery stalled"
                );
                std::thread::yield_now();
            }
        }
        let sent_before = session.io.net_stats[0].sent();
        match poll_live_session(session, &mut LiveIo) {
            LivePollOutcome::Idle => {}
            LivePollOutcome::Lost(reason) => {
                anyhow::bail!("cycle {cycle}: connection lost: {reason}")
            }
            LivePollOutcome::Polled(poll) => {
                anyhow::ensure!(poll.rebuild.is_none(), "cycle {cycle}: live rebuild");
                anyhow::ensure!(
                    poll.session_events.is_empty(),
                    "cycle {cycle}: session events {:?}",
                    poll.session_events
                );
                let base = session
                    .game
                    .as_ref()
                    .map(|g| [g.runtime.map.base_x, g.runtime.map.base_z]);
                for effect in poll.effects {
                    if let ClientEffect::HintArrows(updates) = effect {
                        apply_hint_arrows(
                            &mut replay.minimap,
                            &mut replay.pending_hints,
                            base,
                            updates,
                        );
                    }
                }
            }
        }
        let written_len = session.io.net_stats[0].sent().wrapping_sub(sent_before) as usize;
        let mut written = vec![0u8; written_len];
        replay.peer.read_exact(&mut written)?;
        let effects = update_session_logic(
            session,
            &replay.pack,
            InputFrame {
                console_open: false,
                focused: replay.focused,
                gl_texture_formats: Some(replay.gl_formats.clone()),
                cursor: *cursor,
                refresh_picks: &mut |_, _| {},
                probe: None,
            },
            // The frozen harness predates the core's environment (lane
            // E-A1); the compared digests do not include it.
            None,
        );
        sync_window_state(session, None, &mut |_| Ok(()))?;
        dispatch_server_command(session)?;
        replay.window_events(cycle)?;
        Ok(CycleOutput { written, effects })
    }
}

/// A name per effect (`ClientEffect` has no `Debug`).
fn effect_kinds(effects: &[ClientEffect]) -> Vec<&'static str> {
    effects
        .iter()
        .map(|e| match e {
            ClientEffect::PointLights(_) => "PointLights",
            ClientEffect::EnvironmentOverrides(_) => "EnvironmentOverrides",
            ClientEffect::HintArrows(_) => "HintArrows",
            ClientEffect::BrowserUrls { .. } => "BrowserUrls",
            ClientEffect::ConsoleMessages(_) => "ConsoleMessages",
            ClientEffect::ScriptUrl(_) => "ScriptUrl",
            ClientEffect::CutsceneRebuild(_) => "CutsceneRebuild",
            ClientEffect::Script(_) => "Script",
            ClientEffect::WorldSwitch(_) => "WorldSwitch",
            ClientEffect::LoginCancel => "LoginCancel",
            ClientEffect::LoginContinue => "LoginContinue",
            ClientEffect::LoginPacket(_) => "LoginPacket",
            ClientEffect::Logout => "Logout",
            ClientEffect::LoginRequest(_) => "LoginRequest",
            ClientEffect::LobbyEnterGame(_) => "LobbyEnterGame",
            ClientEffect::CreateConnect => "CreateConnect",
        })
        .collect()
}

/// One replayed cycle's outbound bytes, logic effects and state digest.
type CycleDigest = (Vec<u8>, Vec<&'static str>, u64);

/// Phase 5 (code-quality programme §5): the recorded sessions (the
/// scripted walk/chat/cutscene session, and in the tests below the
/// woodcutting, bank, shop and trade sessions) replay through
/// `ClientCore::frame` + `ReplayIo` exactly as through the harness they
/// replaced (`legacy`, the session owners stepped by hand over a loopback
/// socket). Per cycle: identical outbound bytes (nothing masked), logic
/// effects, and a digest of the entity/zone/var world, the observed actors,
/// the saved `ClientOptions`, the minimap's hint arrows, the UI's mouse
/// state and every CS2 hook execution (id, result, stack, pc). One test per
/// session, so the sessions run side by side.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn client_core_replays_match_the_frozen_loopback_harness() -> anyhow::Result<()> {
    client_core_matches_the_loopback_harness(FIXTURE)
}

/// [`client_core_replays_match_the_frozen_loopback_harness`] over the
/// woodcutting session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn client_core_replays_match_the_frozen_loopback_harness_in_the_woodcutting_session(
) -> anyhow::Result<()> {
    client_core_matches_the_loopback_harness("fixtures/session-replay/woodcutting/session.rtr")
}

/// [`client_core_replays_match_the_frozen_loopback_harness`] over the bank
/// session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn client_core_replays_match_the_frozen_loopback_harness_in_the_bank_session() -> anyhow::Result<()>
{
    client_core_matches_the_loopback_harness("fixtures/session-replay/bank/session.rtr")
}

/// [`client_core_replays_match_the_frozen_loopback_harness`] over the shop
/// session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn client_core_replays_match_the_frozen_loopback_harness_in_the_shop_session() -> anyhow::Result<()>
{
    client_core_matches_the_loopback_harness("fixtures/session-replay/shop/session.rtr")
}

/// [`client_core_replays_match_the_frozen_loopback_harness`] over the trade
/// session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn client_core_replays_match_the_frozen_loopback_harness_in_the_trade_session() -> anyhow::Result<()>
{
    client_core_matches_the_loopback_harness("fixtures/session-replay/trade/session.rtr")
}

/// One recorded session through the old harness and through the core, the
/// two replays in parallel.
fn client_core_matches_the_loopback_harness(fixture: &str) -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(fixture))?;
    let run = |&core: &bool| -> anyhow::Result<Vec<CycleDigest>> {
        let trace = &trace;
        let mut replay = Replay::start(trace)?;
        replay.session().ui.diagnostics.capture = true;
        let mut cursor = i32::MIN;
        let mut cycles = Vec::new();
        for cycle in 1..=trace.last_cycle() {
            let out = if core {
                replay.cycle(trace, cycle)?
            } else {
                legacy::cycle(&mut replay, trace, cycle, &mut cursor)?
            };
            let hooks = std::mem::take(&mut replay.session().ui.diagnostics.executions);
            let ui = replay.ui();
            let mouse = (
                ui.engine.platform.mouse,
                ui.engine.platform.pending_mouse,
                ui.input.click,
            );
            let game = replay.game();
            let digest = debug_digest(&(
                game.cycle,
                crate::logic_clock::monotonic_millis(),
                &game.runtime.feed.state,
                observe(game),
                game.ui_variables.queries.preferences.options.encode(),
                &replay.minimap.hint_arrow_state,
                mouse,
                hooks,
            ));
            cycles.push((out.written, effect_kinds(&out.effects), digest));
        }
        Ok(cycles)
    };
    let mut plan = ReplayPlan::default();
    let (old, new) = (plan.add(false), plan.add(true));
    let results = plan.run(run)?;
    let (old, new) = (&results[old], &results[new]);
    anyhow::ensure!(old.len() > 20, "{fixture}: {} cycles", old.len());
    for (cycle, (a, b)) in (1..).zip(old.iter().zip(new)) {
        assert_eq!(a.0, b.0, "{fixture} cycle {cycle}: outbound bytes differ");
        assert_eq!(a.1, b.1, "{fixture} cycle {cycle}: logic effects differ");
        assert_eq!(a.2, b.2, "{fixture} cycle {cycle}: state digest differs");
    }
    Ok(())
}

/// Phase 5 headless client: the recorded session runs with no window, no
/// toolkit and no socket traffic through `ClientCore::frame` over the
/// headless shell, every cycle running the full phase list in order (P0
/// when the frame has a cycle, P1-P11), and writes exactly the recorded
/// client packets (bar the two wall-clock fields, `mask_wall_clock`).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn headless_client_core_runs_the_recorded_session_phase_by_phase() -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    let expected = [
        Phase::Resize,
        Phase::Js5,
        Phase::Loading,
        Phase::Cycle,
        Phase::Rebuild,
        Phase::SessionCycle,
        Phase::InputScript,
        Phase::Connections,
        Phase::LiveRead,
        Phase::UpdateGame,
        Phase::Effects,
        Phase::TitleAndPreferences,
    ];
    let mut written = 0;
    for cycle in 1..=trace.last_cycle() {
        let mut phases = Vec::new();
        let out = replay.cycle_observed(cycle, &mut phases)?;
        assert_eq!(phases, expected, "cycle {cycle}: phase list");
        assert_eq!(replay.core.cycle, cycle, "logic cycle counter");
        assert_eq!(
            mask_wall_clock(&out.written)?,
            mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        written += out.written.len();
    }
    assert!(written > 0, "the session wrote client packets");
    anyhow::ensure!(replay.io.unread() == 0, "every recorded byte was read");
    Ok(())
}

/// A reconnect the server resumes in place (login reply 15): the recorded
/// session, played to its end, takes up a new connection and the reply's
/// player-positions block (the recorded login's own) and keeps everything
/// else: its map, npcs, zones, variables and interfaces. The player list
/// restarts from the block, the session's inventories and menu reset, the
/// window is announced on the new connection and the login screens see reply 15.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_resumed_reconnect_keeps_the_recorded_session() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in 1..=trace.last_cycle() {
        replay.cycle(&trace, cycle)?;
    }
    // The block the recorded server opened its login with.
    let login = arrivals(&trace)?
        .into_iter()
        .find(|a| a.opcode == sp::REBUILD_NORMAL)
        .context("the recorded login rebuild")?;
    let session = replay.core.session.as_mut().context("session")?;
    let game = session.game.as_mut().context("game")?;
    let local = game.runtime.map.local;
    let block = login.payload[..(30 + 18 * (2047 - usize::from(local != 0))).div_ceil(8)].to_vec();
    let start = u32::from_be_bytes([block[0], block[1], block[2], block[3]]) >> 2;
    let (base_x, base_z) = (game.runtime.map.base_x, game.runtime.map.base_z);
    let login_tile = (
        ((start >> 14) & 0x3fff) as i32 - base_x,
        (start & 0x3fff) as i32 - base_z,
    );
    // What must survive, and something in view that must not.
    let before = game.runtime.feed.state.clone();
    assert!(before.players.players.iter().flatten().count() > 0);
    let top = session.ui.state.life.top;
    let (peer_side, session_side) = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let client = std::net::TcpStream::connect(listener.local_addr()?)?;
        client.set_nonblocking(true)?;
        (listener.accept()?.0, client)
    };
    let world = crate::net::LoginOk {
        server_varcs: vec![],
        pid: None,
        server_token: 1,
        profile: Default::default(),
        lobby: Default::default(),
        resume: Some(block),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let live = crate::session::LiveState {
            login: world,
            drain: Default::default(),
            stream: crate::wire_stream::WireStream::plain(tokio::net::TcpStream::from_std(
                session_side,
            )?),
        };
        resume_world_session(session, live)
    })?;

    let game = session.game.as_ref().context("game")?;
    let after = &game.runtime.feed.state;
    // The world is the session's own.
    assert_eq!(after.world, before.world);
    assert_eq!(after.zones, before.zones);
    assert_eq!(after.varps, before.varps);
    assert_eq!(
        after.npcs.entities.keys().collect::<Vec<_>>(),
        before.npcs.entities.keys().collect::<Vec<_>>()
    );
    assert_eq!(session.ui.state.life.top, top, "the interfaces stay open");
    // The player list restarted from the block: the local player alone, on
    // the block's tile, every other player forgotten.
    let players: Vec<usize> = after
        .players
        .players
        .iter()
        .enumerate()
        .filter_map(|(index, player)| player.as_ref().map(|_| index))
        .collect();
    assert_eq!(players, [local]);
    let me = after.players.players[local].as_ref().unwrap();
    assert_eq!((me.x[0], me.z[0]), login_tile);
    // The connection is the new one and starts clean; the server hears the
    // window again, nothing else.
    let socket = session.io.world.stream.as_ref().context("world stream")?;
    assert_eq!(socket.peer_addr()?, peer_side.local_addr()?);
    let written = client_frames(&session.io.world.pending_writes)?;
    assert_eq!(
        written
            .iter()
            .map(|(opcode, _)| *opcode)
            .collect::<Vec<_>>(),
        [cp::WINDOW_STATUS]
    );
    assert!(session.io.world.pending.is_empty());
    assert!(!session.polling_dead);
    assert_eq!(session.ui.engine.login.reply, 15);
    assert!(!session.ui.engine.login.in_progress);
    Ok(())
}

/// One scripted world server for a single login: it answers the handshake,
/// reads the login block and hands the socket and the block to `script`. The
/// thread returns the block.
fn scripted_world(
    script: impl FnOnce(&mut std::net::TcpStream) + Send + 'static,
) -> anyhow::Result<(u16, std::thread::JoinHandle<anyhow::Result<Vec<u8>>>)> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> anyhow::Result<Vec<u8>> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut init = [0u8; 1];
        socket.read_exact(&mut init)?;
        anyhow::ensure!(init[0] == 14, "INIT_GAME_CONNECTION first");
        socket.write_all(&[0, 1, 2, 3, 4, 5, 6, 7, 8])?;
        let mut head = [0u8; 3];
        socket.read_exact(&mut head)?;
        let mut body = vec![0u8; usize::from(u16::from_be_bytes([head[1], head[2]]))];
        socket.read_exact(&mut body)?;
        anyhow::ensure!(head[0] == 16, "GAMELOGIN, not opcode {}", head[0]);
        script(&mut socket);
        Ok(body)
    });
    Ok((port, server))
}

/// Run the app's login owners until `done` holds (the worker threads answer
/// through `poll_reconnect`).
fn pump_logins(app: &mut ViewerApp, done: impl Fn(&Session) -> bool) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !done(app.core.session.as_ref().context("session")?) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the login never finished"
        );
        app.poll_reconnect();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

/// The world-switch flag and the reconnect flag of a GAMELOGIN block.
fn login_flags(body: &[u8]) -> (bool, bool) {
    // The block ends: switch flag, lobby node (2 bytes), 41 checksums; the
    // byte after the revision says reconnect.
    (body[body.len() - 41 * 4 - 3] == 1, body[8] == 1)
}

/// World hop and reconnect through the session's own owners (the app's login
/// requests, its login worker threads and their results) against scripted
/// worlds, which read the login blocks the client sends:
///
/// 1. from the lobby, `worldlist_switch` picks world 2 (its port derived from
///    the world id) and `lobby_entergame` logs into it: a fresh login that
///    says it enters a switched world; the world refuses and the client stays
///    in the lobby;
/// 2. picking the world the lobby advertised sends the same login without
///    the switch flag;
/// 3. in the game, `LOGOUT_TRANSFER` enters state 19 and logs into the other
///    world; it refuses, and because the transfer is cancellable the client
///    returns to the world it left with a reconnect login;
/// 4. that world still holds the character and resumes the session in place
///    (reply 15): the client is in game again with the world it had.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn world_hop_and_reconnect_run_through_the_session_owners() -> anyhow::Result<()> {
    use crate::login_state::{GAME, LOBBY, LOBBY_ENTER_GAME, TRANSFER};
    use crate::proto::server as sp;
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in 1..=trace.last_cycle() {
        replay.cycle(&trace, cycle)?;
    }
    let login_rebuild = arrivals(&trace)?
        .into_iter()
        .find(|a| a.opcode == sp::REBUILD_NORMAL)
        .context("the recorded login rebuild")?;
    let (mut app, _keep) = replay.into_app();
    let local = app.core.session.game().context("game")?.runtime.map.local;
    let block =
        login_rebuild.payload[..(30 + 18 * (2047 - usize::from(local != 0))).div_ceil(8)].to_vec();
    let map_before = app
        .core
        .session
        .game()
        .unwrap()
        .runtime
        .feed
        .state
        .world
        .clone();

    // A lobby connection that is up, and a lobby address nothing listens on:
    // the failed worlds below fall back to the connected lobby.
    let lobby_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let lobby_socket = std::net::TcpStream::connect(lobby_listener.local_addr()?)?;
    let _lobby_peer = lobby_listener.accept()?;
    {
        let session = app.core.session.as_mut().unwrap();
        session.io.lobby.stream = Some(crate::wire_stream::WireStream::plain(lobby_socket));
        let unused = std::net::TcpListener::bind(("127.0.0.1", 0))?
            .local_addr()?
            .port();
        session.current_lobby.port = unused;
        session.current_lobby.port2 = unused;
        session.machine.state = LOBBY;
        session.machine.logged_in = true;
        session.target_world = Some(ServerAddress {
            node: 1,
            host: "127.0.0.1".into(),
            port: 0,
            port2: 0,
            use_secondary_port: true,
            use_proxy: false,
        });
    }
    let refuse = |socket: &mut std::net::TcpStream| {
        use std::io::Write;
        socket.write_all(&[5]).unwrap();
    };

    // 1. World 2 from the world list.
    let (port, world_2) = scripted_world(refuse)?;
    app.core.session.as_mut().unwrap().world_port_base = port - 2;
    app.start_world_switch(crate::ui_runtime::WorldSwitchRequest {
        world_id: 2,
        host: "127.0.0.1".into(),
    });
    {
        let world = &app.core.session.as_ref().unwrap().current_world;
        assert_eq!((world.node, world.socket_port()), (2, port));
    }
    app.start_game_from_lobby(crate::ui_runtime::LobbyEnterGameRequest {
        new_auth_preference: String::new(),
        auth_dont_trust: false,
    });
    assert_eq!(
        app.core.session.as_ref().unwrap().machine.state,
        LOBBY_ENTER_GAME
    );
    pump_logins(&mut app, |s| !s.reconnect_started)?;
    let body = world_2.join().expect("world 2")?;
    assert_eq!(
        login_flags(&body),
        (true, false),
        "a fresh login into a switched world"
    );
    let name = format!("{}\0", app.core.session.as_ref().unwrap().username);
    assert!(
        body.windows(name.len()).any(|w| w == name.as_bytes()),
        "the login names the player"
    );
    assert_eq!(
        app.core.session.as_ref().unwrap().machine.state,
        LOBBY,
        "back in the lobby"
    );
    assert_eq!(app.core.session.as_ref().unwrap().ui.engine.login.reply, 5);

    // 2. The world the lobby advertised.
    let (port, world_1) = scripted_world(refuse)?;
    app.core.session.as_mut().unwrap().world_port_base = port - 1;
    app.start_world_switch(crate::ui_runtime::WorldSwitchRequest {
        world_id: 1,
        host: "127.0.0.1".into(),
    });
    app.start_game_from_lobby(crate::ui_runtime::LobbyEnterGameRequest {
        new_auth_preference: String::new(),
        auth_dont_trust: false,
    });
    pump_logins(&mut app, |s| !s.reconnect_started)?;
    let body = world_1.join().expect("world 1")?;
    assert_eq!(
        login_flags(&body),
        (false, false),
        "the advertised world is not a switch"
    );

    // 3. In the game: the world it stands in, and the transfer's target.
    let (previous_port, previous) = scripted_world({
        let block = block.clone();
        move |socket| {
            use std::io::Write;
            let mut reply = vec![15];
            reply.extend((block.len() as u16).to_be_bytes());
            reply.extend(&block);
            socket.write_all(&reply).unwrap();
            // Keep the connection until the client has taken it up.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    })?;
    let (target_port, target) = scripted_world(refuse)?;
    {
        let session = app.core.session.as_mut().unwrap();
        session.machine.state = GAME;
        session.set_world(ServerAddress {
            node: 1,
            host: "127.0.0.1".into(),
            port: previous_port,
            port2: previous_port,
            use_secondary_port: true,
            use_proxy: false,
        });
        session.io.world.stream = Some(crate::wire_stream::WireStream::plain(
            std::net::TcpStream::connect(lobby_listener.local_addr()?)?,
        ));
    }
    app.apply_session_events(&[crate::session::UiEvent::LogoutTransfer {
        world_id: 2,
        host: "127.0.0.1".into(),
        port: target_port,
        port2: target_port,
        cancellable: true,
    }]);
    assert_eq!(app.core.session.as_ref().unwrap().machine.state, TRANSFER);
    // The transfer is refused (reply 5), the cancellable transfer goes back to
    // the world it left, which resumes the session.
    pump_logins(&mut app, |s| {
        s.machine.state == GAME && !s.reconnect_started
    })?;
    let transfer_body = target.join().expect("transfer target")?;
    assert_eq!(
        login_flags(&transfer_body),
        (true, false),
        "the transfer is a fresh login"
    );
    let reconnect_body = previous.join().expect("previous world")?;
    assert!(
        login_flags(&reconnect_body).1,
        "the way back is a reconnect login"
    );
    let session = app.core.session.as_ref().unwrap();
    assert_eq!(session.current_world.node, 1, "the previous world again");
    assert_eq!(
        session.ui.engine.login.last_transfer_reply, 5,
        "the refusal is kept for the scripts"
    );
    assert_eq!(
        session.ui.engine.login.reply, 15,
        "the reconnect resumed in place"
    );
    assert_eq!(
        session.game.as_ref().unwrap().runtime.feed.state.world,
        map_before,
        "the world it had"
    );
    Ok(())
}

/// The shell logs the player out to the login screen for a packet that did
/// not decode, as the original does (it closes the connections and shows the
/// login screen, not the lobby).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_malformed_packet_logs_the_player_out() -> anyhow::Result<()> {
    use crate::login_state::{GAME, LOGIN};
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in 1..=20 {
        replay.cycle(&trace, cycle)?;
    }
    let (mut app, _keep) = replay.into_app();
    assert_eq!(app.core.session.as_ref().unwrap().machine.state, GAME);
    app.apply_session_events(&[crate::session::UiEvent::MalformedPacket {
        opcode: crate::proto::server::IF_SETTEXT,
        size: 2,
        reason: "IF_SETTEXT: need 4 bytes".into(),
    }]);
    let session = app.core.session.as_ref().unwrap();
    assert_eq!(session.machine.state, LOGIN);
    assert!(session.io.world.stream.is_none(), "the world is closed");
    Ok(())
}

/// The recordings whose outgoing bytes the replay tests compare.
const REBASED: [&str; 6] = [
    "fixtures/session-replay/session.rtr",
    "fixtures/session-replay/bank/session.rtr",
    "fixtures/session-replay/entities/session.rtr",
    "fixtures/session-replay/shop/session.rtr",
    "fixtures/session-replay/trade/session.rtr",
    "fixtures/session-replay/woodcutting/session.rtr",
];

/// Rewrite the recordings' `OUT ` and `IOUT` records with what the client
/// writes now, for a deliberate change of its outgoing packets (the
/// recordings were made against the client of before). Every other record
/// stays as the recorded session; the replay tests then compare against the
/// new bytes. Run: `cargo test --lib rebase_recorded_outgoing_bytes --
/// --ignored`, review the changed bytes with the replay tests' failures
/// (the first cycle with a difference) before committing.
#[test]
#[ignore = "rewrites the committed recordings"]
fn rebase_recorded_outgoing_bytes() -> anyhow::Result<()> {
    let root = &rs910_core::test_support::client_dir();
    for fixture in REBASED {
        let path = root.join(fixture);
        let raw = std::fs::read(&path)?;
        let trace = Trace::load(&path)?;
        let (_, replies, _) = drain_recorded_startup(
            &trace,
            trace.head("entity_state")?.parse::<bool>().unwrap_or(false),
        )?;
        let mut replay = Replay::start(&trace)?;
        let mut written = BTreeMap::new();
        for cycle in 1..=trace.last_cycle() {
            let bytes = if fixture.contains("woodcutting") {
                super::scenario_woodcutting::replay_cycle_output(&mut replay, &trace, cycle)?
            } else {
                replay.cycle(&trace, cycle)?.written
            };
            written.insert(cycle, bytes);
        }
        // The records in their order, each cycle's new `OUT ` after its others.
        let mut records: Vec<Record> = Vec::new();
        let flush = |records: &mut Vec<Record>, cycle: i32| {
            if let Some(bytes) = written.get(&cycle).filter(|b| !b.is_empty()) {
                records.push(Record {
                    cycle,
                    tag: *b"OUT ",
                    bytes: bytes.clone(),
                });
            }
        };
        let mut previous = None;
        for record in &trace.records {
            if let Some(cycle) = previous.filter(|&cycle| cycle != record.cycle) {
                flush(&mut records, cycle);
            }
            previous = Some(record.cycle);
            match &record.tag {
                b"OUT " => {}
                b"IOUT" => records.push(Record {
                    bytes: replies.clone(),
                    ..record.clone()
                }),
                _ => records.push(record.clone()),
            }
        }
        if let Some(cycle) = previous {
            flush(&mut records, cycle);
        }
        let mut out = raw[..8].to_vec();
        for record in &records {
            out.extend(record.cycle.to_le_bytes());
            out.extend(record.tag);
            out.extend((record.bytes.len() as u32).to_le_bytes());
            out.extend(&record.bytes);
        }
        std::fs::write(&path, out)?;
        println!("rebased {fixture}: {} cycles", written.len());
    }
    Ok(())
}

/// The recorded sessions of the replay gate.
const SESSIONS: [&str; 2] = [FIXTURE, "fixtures/session-replay/woodcutting/session.rtr"];

/// Logic benchmark (programme Phase 6 harness, `tools/perf`): both recorded
/// sessions through the headless `ClientCore` + `ReplayIo` (packet decode,
/// entity apply, CS2 hooks, the UI tick; no rendering), `ROUNDS` times
/// (`CLIENT910_REPLAY_BENCH_ROUNDS`, default 5). Per session: CPU time and
/// heap allocations per logic cycle (this thread's, from the lib test
/// allocator of `entity_apply_bench`) and the digest of every cycle's
/// outbound bytes and world state, which must not change between rounds or
/// builds. Run: `cargo test --release --lib client_core_replay_bench --
/// --ignored --nocapture`. `CLIENT910_REPLAY_BENCH_PROFILE=1` runs it with
/// the engine profiler recording (a frame per cycle; lane E-A4's
/// enabled-overhead A/B; the lib tests compile the profiler in).
#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture (tools/perf)"]
fn client_core_replay_bench() -> anyhow::Result<()> {
    use std::hash::{Hash, Hasher};
    let root = &rs910_core::test_support::client_dir();
    let rounds: usize = std::env::var("CLIENT910_REPLAY_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let profiled = std::env::var_os("CLIENT910_REPLAY_BENCH_PROFILE").is_some();
    if profiled {
        rs910_core::profile::enable();
    }
    for fixture in SESSIONS {
        let trace = Trace::load(&root.join(fixture))?;
        let mut micros = Vec::new();
        let (mut bytes, mut allocs, mut cycles) = (0, 0, 0);
        let mut digest = None;
        for _ in 0..rounds {
            let mut replay = Replay::start(&trace)?;
            let mut chain = std::collections::hash_map::DefaultHasher::new();
            for cycle in 1..=trace.last_cycle() {
                let (b0, a0) = crate::entity_apply_bench::counters();
                let start = std::time::Instant::now();
                if profiled {
                    rs910_core::profile::frame_mark();
                }
                let out = replay.cycle(&trace, cycle)?;
                micros.push(start.elapsed().as_secs_f64() * 1e6);
                let (b1, a1) = crate::entity_apply_bench::counters();
                (bytes, allocs, cycles) = (bytes + b1 - b0, allocs + a1 - a0, cycles + 1);
                let game = replay.game();
                out.written.hash(&mut chain);
                debug_digest(&(
                    game.cycle,
                    &game.runtime.feed.state,
                    observe(game),
                    game.ui_variables.queries.preferences.options.encode(),
                ))
                .hash(&mut chain);
            }
            let d = chain.finish();
            assert!(
                digest.is_none_or(|p| p == d),
                "{fixture}: replay is not deterministic"
            );
            digest = Some(d);
        }
        micros.sort_by(f64::total_cmp);
        let n = micros.len() as f64;
        println!(
            "client_core_replay_bench {fixture}: {} cycles x {rounds}, mean {:.1} us p50 {:.1} us p90 {:.1} us, {:.1} allocs {:.0} bytes per cycle, digest {:016x}",
            cycles / rounds,
            micros.iter().sum::<f64>() / n,
            micros[micros.len() / 2],
            micros[micros.len() * 9 / 10],
            allocs as f64 / cycles as f64,
            bytes as f64 / cycles as f64,
            digest.unwrap()
        );
    }
    Ok(())
}

/// Every recorded cycle of `trace` through the headless client: per cycle
/// the outbound bytes and a digest of the entity/zone/var world, the
/// observed actors and the saved `ClientOptions`.
fn replay_digests(trace: &Trace, replay: &mut Replay) -> anyhow::Result<Vec<(Vec<u8>, u64)>> {
    let mut cycles = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(trace, cycle)?;
        let game = replay.game();
        let digest = debug_digest(&(
            game.cycle,
            &game.runtime.feed.state,
            observe(game),
            game.ui_variables.queries.preferences.options.encode(),
        ));
        cycles.push((out.written, digest));
    }
    Ok(cycles)
}

/// Phase 5 golden-session support (programme §5, "golden RTR1 sessions in
/// CI"): writes the cache the recorded sessions read, and nothing else, as
/// a JS5 disk-store overlay (`<dir>/255/<archive>.dat` indexes and
/// `<dir>/<archive>/<group>.dat` groups, `DiskOverlay`'s layout) to
/// `CLIENT910_REPLAY_OVERLAY_OUT`. A `Pack` over an empty root and this
/// overlay replays them with no `server/data/pack`
/// (`recorded_sessions_replay_from_the_pack_free_overlay`).
#[test]
#[ignore = "exporter: needs server/data/pack and CLIENT910_REPLAY_OVERLAY_OUT=<dir>"]
fn export_the_recorded_sessions_pack_overlay() -> anyhow::Result<()> {
    use crate::cache::{archive_id_for_name, read_log, DiskOverlay, DiskStore, ARCHIVE_SET};
    let out = std::env::var_os("CLIENT910_REPLAY_OVERLAY_OUT")
        .context("CLIENT910_REPLAY_OVERLAY_OUT=<dir>")?;
    let root = &rs910_core::test_support::client_dir();
    read_log::start();
    for fixture in SESSIONS {
        let trace = Trace::load(&root.join(fixture))?;
        replay_digests(&trace, &mut Replay::start(&trace)?)?;
    }
    let reads = read_log::take();
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    let source = DiskOverlay::new(std::env::temp_dir().join("client910-no-overlay"));
    let overlay = DiskOverlay::new(PathBuf::from(out));
    let (mut groups, mut bytes) = (0, 0);
    for (archive, group) in &reads {
        let id = archive_id_for_name(archive).with_context(|| format!("archive {archive}"))?;
        let (store, key) = group.map_or((ARCHIVE_SET, id), |group| (id, group));
        let stored = DiskStore::new(store, pack.clone(), source.clone())
            .read(key)
            .with_context(|| format!("{archive} {group:?}"))?;
        overlay.write(store, key, &stored)?;
        groups += usize::from(group.is_some());
        bytes += stored.len();
    }
    log::info!(
        "[replay] pack overlay: {} indexes, {groups} groups, {bytes} bytes",
        reads.len() - groups
    );
    Ok(())
}

/// Phase 5 golden sessions without the pack: the recorded sessions replay
/// through `ClientCore` + `ReplayIo` from a `Pack` over an empty root and
/// the exported overlay (`export_the_recorded_sessions_pack_overlay`),
/// writing the recorded client packets (`mask_wall_clock`) with the same
/// per-cycle bytes and state digests as over `server/data/pack`. CI can
/// run it once the overlay is available to the runner (programme §5,
/// Phase 5 status: ~13 MB of cache excerpts, not committed).
#[test]
#[ignore = "needs the exported overlay: CLIENT910_REPLAY_OVERLAY=<dir>"]
fn recorded_sessions_replay_from_the_pack_free_overlay() -> anyhow::Result<()> {
    use crate::cache::DiskOverlay;
    let dir =
        std::env::var_os("CLIENT910_REPLAY_OVERLAY").context("CLIENT910_REPLAY_OVERLAY=<dir>")?;
    let empty = std::env::temp_dir().join(format!("client910-empty-pack-{}", std::process::id()));
    std::fs::create_dir_all(&empty)?;
    let pack = Pack::open_with_overlay(&empty, Some(DiskOverlay::new(PathBuf::from(dir))));
    let root = &rs910_core::test_support::client_dir();
    for fixture in SESSIONS {
        let trace = Trace::load(&root.join(fixture))?;
        let (profile, _) = recorded_profile(&trace)?;
        let from_overlay = replay_digests(
            &trace,
            &mut Replay::start_from(
                &trace,
                &rs910_toolkit::capability::Answers::hardware(profile),
                pack.clone(),
            )?,
        )?;
        for (cycle, (written, _)) in (1..).zip(&from_overlay) {
            // The woodcutting session's pick cycles need the renderer's pick
            // frame (scenario_woodcutting); its other cycles, and every
            // cycle of the scripted session, write the recording.
            if fixture == FIXTURE {
                assert_eq!(
                    mask_wall_clock(written)?,
                    mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
                    "{fixture} cycle {cycle}: client packets differ from the recording"
                );
            }
        }
        if rs910_js5::test_support::pack_root()
            .join("client.interfaces.js5")
            .is_file()
        {
            let from_pack = replay_digests(&trace, &mut Replay::start(&trace)?)?;
            assert_eq!(from_overlay, from_pack, "{fixture}: overlay vs pack");
        }
    }
    let _ = std::fs::remove_dir_all(&empty);
    Ok(())
}

#[test]
fn recorded_ping_ingress_keeps_historical_results_and_rejects_invalid_events() {
    use crate::connection_upkeep::PING_COMPLETION_RECORD_TAG;
    const REPORT_CYCLE: i32 = 2;
    const LOGIC_INTERVAL_MS: i64 = 20;
    const ROUND_TRIP_MS: i32 = 2;
    const FRAME_RATE: i32 = 50;
    let clock = Record {
        cycle: REPORT_CYCLE,
        tag: *b"NOWM",
        bytes: LOGIC_INTERVAL_MS.to_le_bytes().to_vec(),
    };
    let report = Record {
        cycle: REPORT_CYCLE,
        tag: *b"OUT ",
        bytes: crate::net::encode_ping_statistics(
            ROUND_TRIP_MS,
            FRAME_RATE,
            crate::net::NO_COLLECTOR_PERCENT,
        ),
    };
    let make = |records| Trace {
        records,
        head: Vec::new(),
    };
    let historical = make(vec![clock.clone(), report.clone()]);
    assert_eq!(
        historical.ping_completions().unwrap(),
        BTreeMap::from([(REPORT_CYCLE, ROUND_TRIP_MS)])
    );
    assert!(make(vec![clock.clone(), report.clone(), report.clone()])
        .ping_completions()
        .is_err());
    assert!(
        make(vec![report.clone()]).ping_completions().is_err(),
        "no actual clock cycle"
    );
    let ingress = Record {
        cycle: REPORT_CYCLE,
        tag: PING_COMPLETION_RECORD_TAG,
        bytes: ROUND_TRIP_MS.to_le_bytes().to_vec(),
    };
    assert_eq!(
        make(vec![clock.clone(), ingress.clone()])
            .ping_completions()
            .unwrap(),
        historical.ping_completions().unwrap(),
        "independent ingress does not read output"
    );
    assert!(make(vec![clock.clone(), ingress.clone(), ingress.clone()])
        .ping_completions()
        .is_err());
    let malformed = Record {
        bytes: vec![0],
        ..ingress.clone()
    };
    assert!(
        make(vec![clock.clone(), malformed])
            .ping_completions()
            .is_err(),
        "typed field width"
    );
    let negative = Record {
        bytes: (-1_i32).to_le_bytes().to_vec(),
        ..ingress
    };
    assert!(
        make(vec![clock, negative]).ping_completions().is_err(),
        "invalid measurement"
    );
}
