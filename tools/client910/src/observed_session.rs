//! Fixture-only authenticated transport over the ordinary headless session.
//! Profile/startup bytes come from the owning authenticated broker; the core
//! owns variables, entities, maps, CS2, routes and packet output. No window.
use super::*;
use crate::live_control::{ActionOutcome, LiveControl};
use rs910_client::client_core::io::{Conn, RecordingIo, WindowRecord};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const OBSERVED_LINE_BYTES: usize = 2 * 1024 * 1024;
const OBSERVED_PENDING_BYTES: usize = 4 * 1024 * 1024;
const OBSERVED_READ_BYTES: usize = 64 * 1024;
const OBSERVED_BOOT_SECONDS: u64 = 30;
const OBSERVED_SESSION_SECONDS: u64 = 720;
const OBSERVED_MIN_SESSION_SECONDS: u64 = 1;
const OBSERVED_MAX_SESSION_SECONDS: u64 = 1800;
const OBSERVED_SESSION_BUDGET_ENV: &str = "CLIENT910_OBSERVED_SESSION_SECONDS";
const OBSERVED_LOGIC_INTERVAL: Duration = Duration::from_millis(20);
const OBSERVED_POLL_INTERVAL: Duration = Duration::from_millis(2);
const OBSERVED_FIRST_CYCLE: i32 = 1;
const OBSERVED_STARTUP_CYCLE: i32 = -1;
const OBSERVED_NO_CYCLE: i32 = 0;
const OBSERVED_FIRST_SEQUENCE: u64 = 0;
const OBSERVED_SEQUENCE_STEP: u64 = 1;
const OBSERVED_ZERO_BYTES: usize = 0;
const OBSERVED_ZERO_BYTE: u8 = 0;
const OBSERVED_LINE_TERMINATOR_BYTES: usize = 1;
const OBSERVED_BOOT_MESSAGES: usize = 1;
const OBSERVED_NEWLINE: u8 = b'\n';

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatedProfile {
    username: String,
    pid: u16,
    server_token: String,
    logged_in_members: bool,
    player_is_members: bool,
    player_is_quickchat: bool,
    logged_in_quickchat: bool,
    dob_verified: bool,
    lobby_dob: i32,
    staff_mod_level: i32,
    player_mod_level: i32,
    owner: String,
}
impl AuthenticatedProfile {
    fn validate(&self, account: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !account.is_empty() && self.username.eq_ignore_ascii_case(account),
            "authenticated account/profile differs"
        );
        self.server_token
            .parse::<u64>()
            .context("authenticated unsigned token")?;
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatedBoot {
    device_envelope: PathBuf,
    profile: AuthenticatedProfile,
    server_varcs: Vec<u8>,
    startup_wire: Vec<u8>,
    account: String,
    server_clock: i64,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ObservedInbound {
    Boot { boot: AuthenticatedBoot },
    Received { sequence: u64, wire: Vec<u8> },
    Close,
}

/// Bounded nonblocking broker link. One authenticated TCP connection owns it.
struct ObservedLink {
    stream: UnixStream,
    read: Vec<u8>,
    write: VecDeque<u8>,
    closed: bool,
}

impl ObservedLink {
    fn connect(path: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            read: Vec::new(),
            write: VecDeque::new(),
            closed: false,
        })
    }
    fn poll(&mut self) -> anyhow::Result<Vec<ObservedInbound>> {
        let mut chunk = [OBSERVED_ZERO_BYTE; OBSERVED_READ_BYTES];
        let mut read = OBSERVED_ZERO_BYTES;
        while read < OBSERVED_LINE_BYTES && !self.closed {
            match self.stream.read(&mut chunk) {
                Ok(OBSERVED_ZERO_BYTES) => self.closed = true,
                Ok(count) => {
                    read += count;
                    self.read.extend_from_slice(&chunk[..count]);
                    anyhow::ensure!(
                        self.read.len() <= OBSERVED_PENDING_BYTES,
                        "peer input bound"
                    );
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        let mut messages = Vec::new();
        while let Some(end) = self.read.iter().position(|byte| *byte == OBSERVED_NEWLINE) {
            anyhow::ensure!(end <= OBSERVED_LINE_BYTES, "peer line bound");
            let line: Vec<_> = self.read.drain(..=end).collect();
            messages.push(serde_json::from_slice(
                &line[..line.len() - OBSERVED_LINE_TERMINATOR_BYTES],
            )?);
        }
        anyhow::ensure!(
            !self.closed || self.read.is_empty(),
            "peer broker truncated a line"
        );
        Ok(messages)
    }
    fn send(&mut self, row: serde_json::Value) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(&row)?;
        anyhow::ensure!(bytes.len() <= OBSERVED_LINE_BYTES, "peer response bound");
        bytes.push(OBSERVED_NEWLINE);
        anyhow::ensure!(
            self.write.len() + bytes.len() <= OBSERVED_PENDING_BYTES,
            "peer output bound"
        );
        self.write.extend(bytes);
        self.flush()
    }
    fn flush(&mut self) -> anyhow::Result<()> {
        while !self.write.is_empty() {
            let (bytes, _) = self.write.as_slices();
            match self.stream.write(bytes) {
                Ok(OBSERVED_ZERO_BYTES) => anyhow::bail!("peer output closed"),
                Ok(count) => {
                    self.write.drain(..count);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

/// Buffered transport only; the ordinary monotonic clock remains authoritative.
/// ReplayIo's recorded/synthetic clock is deliberately not installed.
struct ObservedIo {
    buffer: ReplayIo,
    received: Vec<u8>,
    sequence: Option<u64>,
}
impl ObservedIo {
    fn new() -> Self {
        let mut buffer = ReplayIo::new(Vec::new());
        buffer.set_scripted(true);
        Self {
            buffer,
            received: Vec::new(),
            sequence: None,
        }
    }
}
impl ObservedIo {
    fn receive(&mut self, sequence: u64, wire: &[u8]) -> anyhow::Result<()> {
        let expected = self
            .sequence
            .map_or(Some(OBSERVED_FIRST_SEQUENCE), |prior| {
                prior.checked_add(OBSERVED_SEQUENCE_STEP)
            })
            .context("observed receive sequence overflow")?;
        anyhow::ensure!(sequence == expected, "observed TCP input sequence");
        anyhow::ensure!(
            wire.len() <= OBSERVED_LINE_BYTES
                && self.buffer.unread() + wire.len() <= OBSERVED_PENDING_BYTES,
            "observed input bound"
        );
        self.sequence = Some(sequence);
        self.buffer.push_inbound(wire);
        Ok(())
    }
    fn clear_connection(&mut self) {
        // No old inbound bytes, output or sequence may cross authentication.
        self.buffer = ReplayIo::new(Vec::new());
        self.buffer.set_scripted(true);
        self.received.clear();
        self.sequence = None;
    }
}
impl Io for ObservedIo {
    fn begin_cycle(&mut self, _: i32) {}
    fn flush(
        &mut self,
        conn: Conn,
        stream: &mut crate::wire_stream::WireStream<std::net::TcpStream>,
        pending: &mut Vec<u8>,
        wrote: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<()> {
        self.buffer.flush(conn, stream, pending, wrote)
    }
    fn read(
        &mut self,
        conn: Conn,
        stream: &mut crate::wire_stream::WireStream<std::net::TcpStream>,
        pending: &mut Vec<u8>,
        read: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<bool> {
        let (buffer, received) = (&mut self.buffer, &mut self.received);
        buffer.read(conn, stream, pending, &mut |bytes| {
            received.extend_from_slice(bytes);
            read(bytes);
        })
    }
    fn cursor(&mut self, current: i32) -> i32 {
        current
    }
    fn window_event(&mut self, _: WindowRecord) {}
}

fn record_monotonic_samples(tag: &[u8; 4], samples: Vec<i64>) {
    let bytes: Vec<u8> = samples.into_iter().flat_map(i64::to_le_bytes).collect();
    // Empty phases remain explicit zero-length chunks.
    crate::session_record::record(tag, &bytes);
}

/// Construct fresh startup from ACTUAL authenticated peer packets. The old RTR
/// supplies device answers only, never old players, maps, vars, inputs or peers.
fn observed_startup(boot: &AuthenticatedBoot) -> anyhow::Result<Replay> {
    boot.profile.validate(&boot.account)?;
    let mut trace = Trace::load(&boot.device_envelope)?;
    trace.records.retain(|record| {
        record.cycle == OBSERVED_STARTUP_CYCLE
            && [*b"HEAD", *b"TOOL", *b"CANV", *b"GLTF"].contains(&record.tag)
    });
    trace.head.retain(|(key, _)| key != "server_command");
    let profile: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::to_value(&boot.profile)?)?;
    for (key, value) in &profile {
        let text = if key == "server_token" {
            (value.as_str().context("peer token")?.parse::<u64>()? as i64).to_string()
        } else {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        };
        let field = trace
            .head
            .iter_mut()
            .find(|(name, _)| name == key)
            .with_context(|| format!("device envelope lacks peer profile field {key}"))?;
        field.1 = text;
    }
    trace
        .head
        .iter_mut()
        .find(|(key, _)| key == "entity_state")
        .context("strict peer entity mode")?
        .1 = true.to_string();
    let startup = [
        (*b"INIT", boot.startup_wire.clone()),
        (*b"IOUT", Vec::new()),
        (*b"SVRC", boot.server_varcs.clone()),
        (*b"PREF", Vec::new()),
        (*b"VARC", Vec::new()),
    ];
    for (tag, bytes) in startup {
        trace.records.push(Record {
            cycle: OBSERVED_STARTUP_CYCLE,
            tag,
            bytes,
        });
    }
    crate::logic_clock::set_test_now(None);
    let (profile, _) = recorded_profile(&trace)?;
    let capabilities = rs910_toolkit::capability::Answers::hardware(profile);
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    crate::session_record::start_from_flags();
    crate::session_record::cycle(
        OBSERVED_STARTUP_CYCLE,
        crate::logic_clock::monotonic_millis(),
    );
    // These inherited device answers and fresh authenticated startup bytes are
    // actually used below. The rewritten HEAD is serialized after UI startup.
    for record in trace.records.iter().filter(|record| record.tag != *b"HEAD") {
        crate::session_record::record(&record.tag, &record.bytes);
    }
    let startup_samples = crate::logic_clock::MonotonicSamples::record()?;
    let mut replay = Replay::start_installing_from(
        &trace,
        &capabilities,
        capabilities.toolkit0,
        pack,
        StartupSource::Authenticated {
            server_clock: boot.server_clock,
        },
    )?;
    record_monotonic_samples(&STARTUP_CLOCK_SAMPLES, startup_samples.finish()?);
    trace
        .head
        .push((MONOTONIC_CLOCK_HEAD.into(), MONOTONIC_CLOCK_FORMAT.into()));
    trace.head.push((
        OBSERVED_BACKEND_HEAD.into(),
        AUTHENTICATED_HEADLESS_BACKEND.into(),
    ));
    trace.head.push((
        OBSERVED_SERVER_CLOCK_HEAD.into(),
        boot.server_clock.to_string(),
    ));
    // The exact clock read by retained_session_ui, not an inferred frame clock.
    trace.head.push((
        OBSERVED_MEMBERSHIP_CLOCK_HEAD.into(),
        boot.server_clock
            .wrapping_sub(replay.ui().engine.lobby.membership_offset)
            .to_string(),
    ));
    let mut head = String::new();
    for (key, value) in &trace.head {
        head.push_str(key);
        head.push('=');
        head.push_str(value);
        head.push('\n');
    }
    crate::session_record::record(b"HEAD", head.as_bytes());
    // Preserve startup writes produced by the real prepare/ack/UI owners.
    replay.io.set_scripted(true);
    crate::logic_clock::set_test_now(None);
    // No ENV records survived the device-only prefix; no script is installed.
    anyhow::ensure!(
        trace.env().is_empty(),
        "peer must have no historical timed script"
    );
    Ok(replay)
}

impl Replay {
    fn observed_cycle(
        &mut self,
        io: &mut RecordingIo<ObservedIo>,
        cycle: i32,
    ) -> anyhow::Result<CycleOutput> {
        // A local pack is supplied. The existing map owner must really prepare
        // terrain/collision and succeed before it acknowledges a pending map.
        // Missing native source maps fail; no readiness flag is manufactured.
        let install_map = self.core.session.as_ref().is_some_and(|session| {
            crate::login_state::is_rebuild(session.machine.state)
                && session
                    .game
                    .as_ref()
                    .is_some_and(|game| game.runtime.map_request.is_some())
        });
        let mut picks =
            |_: &mut crate::client_game::ClientGame, _: &mut crate::ui_runtime::Runtime| {};
        let mut shell = Headless {
            core: &mut self.core,
            pack: &self.pack,
            assets: &mut self.assets,
            install_map,
            recorded_map_elapsed: None,
            minimap: &mut self.minimap,
            pending_hints: &mut self.pending_hints,
            focused: true,
            gl_formats: &self.gl_formats,
            scale: self.scale,
            pending_resize: &mut self.pending_resize,
            refresh_picks: &mut picks,
            effects: Vec::new(),
            error: None,
            phases: None,
        };
        let frame_samples = crate::logic_clock::MonotonicSamples::record()?;
        let end = ClientCore::frame(&mut shell, io, OBSERVED_FIRST_CYCLE);
        record_monotonic_samples(&FRAME_CLOCK_SAMPLES, frame_samples.finish()?);
        let Headless { effects, error, .. } = shell;
        if let Some(error) = error {
            return Err(error);
        }
        anyhow::ensure!(end == Cycle::Next, "peer core stopped at {cycle}: {end:?}");
        Ok(CycleOutput {
            written: io.inner.buffer.take_written(),
            effects,
        })
    }
}

/// A declared fixture-owner wall deadline; default startup and clocks stay exact.
/// Validate before authentication or acquiring the transport/controller owners.
fn observed_session_budget(value: Option<std::ffi::OsString>) -> anyhow::Result<Duration> {
    let seconds = match value {
        None => OBSERVED_SESSION_SECONDS,
        Some(value) => value
            .into_string()
            .map_err(|_| anyhow::anyhow!("observed session budget is not UTF-8"))?
            .parse::<u64>()
            .context("observed session budget must be unsigned whole seconds")?,
    };
    anyhow::ensure!(
        (OBSERVED_MIN_SESSION_SECONDS..=OBSERVED_MAX_SESSION_SECONDS).contains(&seconds),
        "observed session budget is outside the finite fixture-owner bound"
    );
    Ok(Duration::from_secs(seconds))
}

/// One fixture-only process/account, never a gameplay-domain engine.
/// Its owning recorder must authenticate the transport and finally wait it.
pub(super) fn run_observed_session(transport: &Path) -> anyhow::Result<()> {
    let session_budget = observed_session_budget(std::env::var_os(OBSERVED_SESSION_BUDGET_ENV))?;
    let started = Instant::now();
    let mut link = ObservedLink::connect(transport)?;
    let boot_deadline = started + Duration::from_secs(OBSERVED_BOOT_SECONDS);
    let boot = loop {
        anyhow::ensure!(Instant::now() < boot_deadline, "peer boot timeout");
        let messages = link.poll()?;
        if !messages.is_empty() {
            anyhow::ensure!(
                messages.len() == OBSERVED_BOOT_MESSAGES,
                "broker must await booted before live bytes"
            );
            match messages.into_iter().next().context("peer boot message")? {
                ObservedInbound::Boot { boot } => break boot,
                _ => anyhow::bail!("live bytes preceded authenticated startup"),
            }
        }
        anyhow::ensure!(!link.closed, "peer broker closed before boot");
        std::thread::sleep(OBSERVED_POLL_INTERVAL);
    };
    let mut replay = observed_startup(&boot)?;
    let mut control = LiveControl::from_env()?.context("per-peer CLIENT910_CONTROL is required")?;
    let mut io = RecordingIo::interactive(ObservedIo::new());
    let initial = replay.io.take_written();
    crate::session_record::cycle(OBSERVED_NO_CYCLE, crate::logic_clock::monotonic_millis());
    crate::session_record::record(b"OUT ", &initial);
    link.send(serde_json::json!({"kind":"written","cycle":OBSERVED_NO_CYCLE,"wire":initial}))?;
    link.send(serde_json::json!({"kind":"booted","account":boot.account,"virtual_headless":true,"rendered":false,"session_seconds":session_budget.as_secs()}))?;
    let mut next_frame = Instant::now();
    let deadline = started + session_budget;
    let mut closing = false;
    while !closing && !link.closed {
        anyhow::ensure!(Instant::now() < deadline, "peer session deadline");
        for message in link.poll()? {
            match message {
                ObservedInbound::Received {
                    sequence: next,
                    wire,
                } => {
                    io.inner.receive(next, &wire)?;
                }
                ObservedInbound::Close => closing = true,
                ObservedInbound::Boot { .. } => {
                    io.inner.clear_connection();
                    link.write.clear();
                    anyhow::bail!("authentication replacement requires a fresh observed owner");
                }
            }
        }
        link.flush()?;
        if closing {
            break;
        }
        if Instant::now() < next_frame {
            std::thread::sleep(OBSERVED_POLL_INTERVAL);
            continue;
        }
        let cycle = replay
            .core
            .cycle
            .checked_add(OBSERVED_FIRST_CYCLE)
            .context("observed cycle overflow")?;
        let output = replay.observed_cycle(&mut io, cycle)?;
        // The ordinary app polls its controller in frame_tail, after logic.
        // Applied input is consumed/flushed by the following ordinary cycle.
        if let Some(action) = control.poll(
            replay.session(),
            cycle,
            true,
            crate::logic_clock::monotonic_millis(),
        )? {
            let input_samples = crate::logic_clock::MonotonicSamples::record()?;
            let applied = action.event.apply(replay.session());
            let input_samples = input_samples.finish()?;
            let outcome = match applied {
                Ok(()) => {
                    // Accepted canonical input remains after logic and before
                    // redraw. Refused input stays only in its outcome receipt.
                    record_monotonic_samples(&INPUT_CLOCK_SAMPLES, input_samples);
                    io.window_event(WindowRecord::AcceptedInput(action.event.clone()));
                    ActionOutcome::Applied
                }
                Err(error) => ActionOutcome::Refused(error.to_string()),
            };
            let outcome_receipt = match &outcome {
                ActionOutcome::Applied => serde_json::json!({"status":"applied"}),
                ActionOutcome::Refused(reason) => {
                    serde_json::json!({"status":"refused","reason":reason})
                }
            };
            control.complete(&action, cycle, outcome)?;
            link.send(
                serde_json::json!({"kind":"input","cycle":cycle,"account":boot.account,
                "event":action.event.encode()?,"outcome":outcome_receipt}),
            )?;
        }
        let redraw_samples = crate::logic_clock::MonotonicSamples::record()?;
        replay.redraw();
        record_monotonic_samples(&REDRAW_CLOCK_SAMPLES, redraw_samples.finish()?);
        link.send(serde_json::json!({"kind":"logic_cycle","account":boot.account,"cycle":replay.core.cycle,
            "read_wire":std::mem::take(&mut io.inner.received),
            "rendered":false,"qualification":"actual headless core read/cycle; no GPU scene/present assertion"}))?;
        if !output.written.is_empty() {
            // Split with the GENERATED client packet sizes; transport then masks
            // each opcode using only this same authenticated peer's own ISAAC.
            client_frames(&output.written)?;
            link.send(serde_json::json!({"kind":"written","cycle":cycle,"wire":output.written}))?;
        }
        next_frame = Instant::now() + OBSERVED_LOGIC_INTERVAL;
    }
    link.send(
        serde_json::json!({"kind":"closed","account":boot.account,"cycle":replay.core.cycle}),
    )?;
    let flush_deadline = Instant::now() + Duration::from_secs(OBSERVED_BOOT_SECONDS);
    while !link.write.is_empty() && Instant::now() < flush_deadline {
        link.flush()?;
        std::thread::sleep(OBSERVED_POLL_INTERVAL);
    }
    anyhow::ensure!(
        link.write.is_empty(),
        "peer final receipt was not delivered"
    );
    Ok(())
}
#[test]
#[ignore = "fixture-only authenticated transport; its owning recorder supplies sockets and finally waits this process"]
fn authenticated_observed_session() -> anyhow::Result<()> {
    let transport = std::env::var_os("CLIENT910_OBSERVED_TRANSPORT")
        .context("one owned authenticated observed transport is required")?;
    run_observed_session(Path::new(&transport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs910_symbols::varc;

    const DEVICE_RECORDING: &str = "fixtures/session-replay/legacy-interface/session.rtr";
    const LOGIN_FIXTURE: &str = "fixtures/recorded/toolbar-input/login.json";
    const SERVER_CLOCK_TEST_VALUE: i64 = 1_000_000;
    const VARC_TEST_VALUE: i32 = 0x0102_0304; // not a content id
    const SINGLE_ACK: usize = 1;
    const EPHEMERAL_TCP_PORT: u16 = 0;

    #[test]
    fn declared_session_budget_preserves_default_and_refuses_invalid_bounds() -> anyhow::Result<()>
    {
        assert_eq!(
            observed_session_budget(None)?,
            Duration::from_secs(OBSERVED_SESSION_SECONDS)
        );
        for seconds in [OBSERVED_MIN_SESSION_SECONDS, OBSERVED_MAX_SESSION_SECONDS] {
            assert_eq!(
                observed_session_budget(Some(seconds.to_string().into()))?,
                Duration::from_secs(seconds)
            );
        }
        let above_maximum = OBSERVED_MAX_SESSION_SECONDS
            .checked_add(OBSERVED_SEQUENCE_STEP)
            .context("test session-budget overflow")?;
        for value in [
            OBSERVED_ZERO_BYTES.to_string(),
            "-1".to_owned(),
            above_maximum.to_string(),
            "not-seconds".to_owned(),
            u128::MAX.to_string(),
        ] {
            assert!(observed_session_budget(Some(value.into())).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let invalid_utf8 = std::ffi::OsString::from_vec(vec![u8::MAX]);
            assert!(observed_session_budget(Some(invalid_utf8)).is_err());
        }
        Ok(())
    }

    fn captured_boot() -> anyhow::Result<AuthenticatedBoot> {
        let root = rs910_core::test_support::client_dir();
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(LOGIN_FIXTURE))?)?;
        let profile: AuthenticatedProfile = serde_json::from_value(fixture["profile"].clone())?;
        let account = profile.username.to_ascii_lowercase();
        Ok(AuthenticatedBoot {
            device_envelope: root.join(DEVICE_RECORDING),
            profile,
            account,
            server_varcs: serde_json::from_value(fixture["serverVarcs"].clone())?,
            startup_wire: serde_json::from_value(fixture["initial"]["wire"].clone())?,
            // Named synthetic clock tests the supplied login clock consumer;
            // it is not presented as a clock captured in this older fixture.
            server_clock: SERVER_CLOCK_TEST_VALUE,
        })
    }

    #[test]
    fn authenticated_profile_rejects_account_or_token_mismatch() -> anyhow::Result<()> {
        let mut boot = captured_boot()?;
        assert!(boot.profile.validate(&boot.account).is_ok());
        assert!(boot
            .profile
            .validate("different-authenticated-account")
            .is_err());
        boot.profile.server_token = "not-an-unsigned-token".into();
        assert!(boot.profile.validate(&boot.account).is_err());
        Ok(())
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn authenticated_startup_installs_profile_varcs_and_prepared_map_ack() -> anyhow::Result<()> {
        let mut boot = captured_boot()?;
        // Exercise nonempty login varcs using the ordinary named binding and
        // encoder. The value is a test datum, not generated gameplay content.
        rs910_game::ui_var_store::encode(
            varc::KEYBIND_PACK_6.id(),
            &rs910_game::client_vars::Value::Int(VARC_TEST_VALUE),
            &mut boot.server_varcs,
        )?;
        let before = crate::logic_clock::monotonic_millis();
        let mut replay = observed_startup(&boot)?;
        let after = crate::logic_clock::monotonic_millis();
        let session = replay.session();
        let account = &session.ui.engine.account;
        assert_eq!(account.logged_in_members, boot.profile.logged_in_members);
        assert_eq!(account.player_is_members, boot.profile.player_is_members);
        assert_eq!(account.staff_mod_level, boot.profile.staff_mod_level);
        let offset = session.ui.engine.lobby.membership_offset;
        assert!((boot.server_clock - after..=boot.server_clock - before).contains(&offset));
        assert!(
            session.prepared_map.is_none(),
            "ordinary map owner consumed its prepared map"
        );
        let game = session.game.as_mut().context("authenticated game")?;
        assert!(
            game.runtime.map_request.is_none(),
            "ordinary map owner installed actual pack terrain"
        );
        assert_eq!(
            game.ui_variables
                .client
                .values
                .get(&varc::KEYBIND_PACK_6.id()),
            Some(&rs910_game::client_vars::Value::Int(VARC_TEST_VALUE))
        );
        let pending = client_frames(&session.io.world.pending_writes)?;
        assert_eq!(
            pending
                .iter()
                .filter(|(opcode, _)| *opcode == crate::proto::client::MAP_BUILD_COMPLETE)
                .count(),
            SINGLE_ACK,
            "real prepare/install/ack queues exactly one generated acknowledgement"
        );
        let mut io = RecordingIo::interactive(ObservedIo::new());
        let output = replay.observed_cycle(&mut io, OBSERVED_FIRST_CYCLE)?;
        assert_eq!(
            client_frames(&output.written)?
                .iter()
                .filter(|(opcode, _)| *opcode == crate::proto::client::MAP_BUILD_COMPLETE)
                .count(),
            SINGLE_ACK,
            "ordinary ClientCore flush emits the acknowledgement"
        );
        // A second account cannot inherit the previous server variable value.
        let game = replay
            .session()
            .game
            .as_mut()
            .context("authenticated game")?;
        crate::client_core::restore_server_varcs(game, &[])?;
        assert!(!game
            .ui_variables
            .client
            .values
            .contains_key(&varc::KEYBIND_PACK_6.id()));
        Ok(())
    }

    #[test]
    fn observed_transport_preserves_output_and_clears_replacement() -> anyhow::Result<()> {
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, EPHEMERAL_TCP_PORT))?;
        let socket = std::net::TcpStream::connect(listener.local_addr()?)?;
        let (_server, _) = listener.accept()?;
        let mut stream = crate::wire_stream::WireStream::plain(socket);
        let mut io = ObservedIo::new();
        let expected = vec![crate::proto::client::NO_TIMEOUT];
        let mut pending = expected.clone();
        let mut observed = Vec::new();
        io.flush(Conn::World, &mut stream, &mut pending, &mut |bytes| {
            observed.extend_from_slice(bytes)
        })?;
        assert!(pending.is_empty());
        assert_eq!(observed, expected);
        assert_eq!(io.buffer.take_written(), expected);
        let wire = server_frame(crate::proto::server::SERVER_TICK_END, &[]);
        io.receive(OBSERVED_FIRST_SEQUENCE, &wire)?;
        assert!(
            io.receive(OBSERVED_FIRST_SEQUENCE, &wire).is_err(),
            "duplicate transport delivery refused"
        );
        let mut pending = expected.clone();
        io.flush(Conn::World, &mut stream, &mut pending, &mut |_| {})?;
        io.clear_connection();
        assert_eq!(io.buffer.unread(), OBSERVED_ZERO_BYTES);
        assert!(
            io.buffer.take_written().is_empty(),
            "old output cannot cross authentication replacement"
        );
        assert!(io.received.is_empty());
        assert!(io.sequence.is_none());
        io.receive(OBSERVED_FIRST_SEQUENCE, &wire)?;
        Ok(())
    }
}
