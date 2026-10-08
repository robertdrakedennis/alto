//! Engine commands that have no retained client owner: disabled
//! constant/discard handlers, pure helpers (char/date/trig/coord/text), and
//! the small client singletons they read (telemetry grid, emoji list, Twitch
//! hardware platform, setup status, message box, the frame-rate ring).
//!
//! Declared as a child of `ui_runtime` so handlers can reach `Engine`
//! private state (`chat_changed`) without widening its API. Two entry points:
//! [`super::Engine::host_builtin`] (the last owner before `absent()`) and
//! [`dispatch_hook`] for the few commands whose owner lives in the component
//! host (canvas, fonts, minimenu, preferences, local player).
//!
//! Disabled commands behave exactly like the original: an argument discard
//! stays a discard, a constant push stays a constant. Commands whose owner has
//! no counterpart here fail loudly with `TODO(#gap)` instead of fabricating a
//! value.
use crate::ui_chat::NewChatLine;
use native910::vm::{InstructionContext, Value, VmError, VmResult};
use rs910_core::fault::Fault;
use rs910_core::reader::{Eof, Reader as CoreReader};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Stack access: arguments are read in push order.
// ---------------------------------------------------------------------------

struct Stacks<'a> {
    command: &'a str,
    ints: &'a mut Vec<i32>,
    objs: &'a mut Vec<String>,
    longs: &'a mut Vec<i64>,
}
impl Stacks<'_> {
    fn int(&mut self) -> VmResult<i32> {
        self.ints
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "int" })
    }
    fn obj(&mut self) -> VmResult<String> {
        self.objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })
    }
    fn long(&mut self) -> VmResult<i64> {
        self.longs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "long" })
    }
    /// Removes the top `n` ints, oldest first.
    fn ints_n(&mut self, n: usize) -> VmResult<Vec<i32>> {
        if self.ints.len() < n {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        Ok(self.ints.split_off(self.ints.len() - n))
    }
    fn objs_n(&mut self, n: usize) -> VmResult<Vec<String>> {
        if self.objs.len() < n {
            return Err(VmError::StackUnderflow { stack: "object" });
        }
        Ok(self.objs.split_off(self.objs.len() - n))
    }
    /// A disabled handler's `isp -= a; osp -= b`, then its constant pushes.
    fn constant(
        &mut self,
        pop: [usize; 2],
        ints: &[i32],
        objs: &[&str],
    ) -> VmResult<Option<Value>> {
        self.ints_n(pop[0])?;
        self.objs_n(pop[1])?;
        self.ints.extend_from_slice(ints);
        self.objs.extend(objs.iter().map(|s| (*s).to_owned()));
        Ok(None)
    }
    fn failed(&self, reason: impl Into<String>) -> VmError {
        failed(self.command, reason)
    }
}

fn failed(command: &str, reason: impl Into<String>) -> VmError {
    VmError::TrapFailed {
        command: command.into(),
        reason: reason.into(),
    }
}

fn int(v: i32) -> VmResult<Option<Value>> {
    Ok(Some(Value::Int(v)))
}

fn units(s: &str) -> Vec<u16> {
    native910::jstr::units(s)
}

// ---------------------------------------------------------------------------
// Retained Client singletons owned here.
// ---------------------------------------------------------------------------

/// Requests the app window/console owner applies after the VM tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `quit`: application environment exits.
    Quit,
    /// `doCheat(cmd, false, false)` from `docheat`.
    Cheat(String),
    /// The current console entry, from `mes_typed` type 98.
    ConsoleEntry(String),
}

pub use crate::message_box::MessageBox;

#[derive(Debug)]
pub struct State {
    pub requests: Vec<Request>,
    pub telemetry: TelemetryGrid,
    pub telemetry_error: bool,
    pub emoji: EmojiList,
    pub twitch: Twitch,
    /// The logout reason id (default 0); `None` for an unknown id.
    pub logout_reason: Option<i32>,
    /// Sprite groups requested by `applyDisplayPreference`, drained each
    /// loop by the application's JS5 step.
    pub sprite_prefetch: Vec<i32>,
    /// `graphicsDefaults.performancemetricsmodel` from defaults group 3.
    pub performance_metrics_model: Option<i32>,
    /// `js5Providers` prefetch progress (`Js5System::preload_percent`,
    /// published each logic cycle);
    /// `None` before the providers exist.
    pub preload_progress: Option<i32>,
    /// The installer launcher behind `saveRuneScapeSetup`.
    pub setup: SetupLauncher,
    pub message_box: MessageBox,
    /// Launcher/applet parameters from
    /// [`crate::applet_params`] (values unless overridden).
    pub player_is_affiliate: i32,
    pub from_billing: bool,
    pub current_player_country: i32,
    pub create_email: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            requests: Vec::new(),
            telemetry: TelemetryGrid::default(),
            telemetry_error: false,
            emoji: EmojiList::default(),
            twitch: Twitch::default(),
            logout_reason: Some(0),
            sprite_prefetch: Vec::new(),
            performance_metrics_model: None,
            preload_progress: None,
            setup: SetupLauncher::default(),
            message_box: MessageBox::default(),
            player_is_affiliate: crate::applet_params::get().player_is_affiliate(),
            from_billing: crate::applet_params::get().is_from_billing(),
            current_player_country: crate::applet_params::get().current_player_country(),
            create_email: crate::applet_params::get().create_email(),
        }
    }
}

/// The known logout reason ids; others decode to null.
const LOGOUT_REASONS: [i32; 14] = [21, 3, 2, 101, 10, 4, 20, 0, 100, 6, 102, 103, 1, 5];

impl State {
    /// `read` LOGOUT/LOGOUT_FULL decodes the reason;
    /// A logout latches the telemetry error.
    pub fn logout(&mut self, reason: u8) {
        let id = i32::from(reason);
        self.logout_reason = LOGOUT_REASONS.contains(&id).then_some(id);
        self.telemetry_error = true;
    }
}

// ---------------------------------------------------------------------------
// The installer launcher (`saveRuneScapeSetup`).
// ---------------------------------------------------------------------------

/// File name of the installer, looked up in the client's cache directory.
const SETUP_EXECUTABLE: &str = "RuneScape-Setup.exe";

/// How far the installer got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetupStatus {
    #[default]
    Idle = 0,
    Running = 1,
    Finished = 2,
    Failed = 3,
}

/// Runs the installer executable beside the cache on Windows and reports
/// whether it is running, finished or failed.
///
/// Launching is off unless the user opts in with the environment variable
/// [`ALLOW_SETUP_LAUNCH_ENV`] set to `1` (a warning is logged when it is on).
/// The cache directory is filled by servers and downloads, so a client must
/// never start a program found there on its own. Without the opt-in the
/// launcher behaves as a host with no installer: the launch fails the way the
/// original does when the file is missing, nothing is looked up and nothing is
/// spawned, on every operating system. Hosts other than Windows have no
/// installer at all: launching does nothing and the status stays idle.
#[derive(Debug)]
pub struct SetupLauncher {
    os: &'static str,
    /// The directory holding the installer; the JS5 disk store's directory
    /// when unset.
    dir: Option<std::path::PathBuf>,
    /// The user opted in to starting the installer.
    allowed: bool,
    child: Option<std::process::Child>,
    status: SetupStatus,
}

/// Environment variable that lets the client start the installer.
pub const ALLOW_SETUP_LAUNCH_ENV: &str = "CLIENT910_ALLOW_SETUP_LAUNCH";

impl Default for SetupLauncher {
    fn default() -> Self {
        let mut launcher = Self::at(os_name(), None);
        if std::env::var(ALLOW_SETUP_LAUNCH_ENV).is_ok_and(|v| v == "1") {
            log::warn!(
                "[client910] {ALLOW_SETUP_LAUNCH_ENV}=1: the client may start RuneScape-Setup.exe from its cache directory"
            );
            launcher.allowed = true;
        }
        launcher
    }
}

impl SetupLauncher {
    /// A launcher for operating system `os` (lowercased name) that looks for
    /// the installer in `dir`.
    pub fn at(os: &'static str, dir: Option<std::path::PathBuf>) -> Self {
        Self {
            os,
            dir,
            allowed: false,
            child: None,
            status: SetupStatus::Idle,
        }
    }

    /// The same launcher with the user's opt-in to start the installer.
    #[must_use]
    pub fn allowing_launch(mut self) -> Self {
        self.allowed = true;
        self
    }

    /// The status as last noted, without checking the installer.
    pub fn state(&self) -> SetupStatus {
        self.status
    }

    /// The status, after noting whether the installer has exited.
    pub fn status(&mut self) -> SetupStatus {
        if let Some(child) = &mut self.child {
            if let Ok(Some(exit)) = child.try_wait() {
                self.status = if exit.success() {
                    SetupStatus::Finished
                } else {
                    SetupStatus::Failed
                };
                self.child = None;
            }
        }
        self.status
    }

    /// Starts the installer. Fails when it is missing or unreadable, still
    /// running from an earlier launch, or cannot be started.
    pub fn launch(&mut self) -> Result<(), &'static str> {
        if !self.os.starts_with("win") {
            return Ok(());
        }
        if !self.allowed {
            return Err("setup executable missing");
        }
        self.status();
        let exe = self
            .dir
            .clone()
            .or_else(|| crate::cache::disk_overlay().map(|overlay| overlay.dir().to_owned()))
            .map(|dir| dir.join(SETUP_EXECUTABLE))
            .filter(|path| path.is_file())
            .ok_or("setup executable missing")?;
        std::fs::File::open(&exe).map_err(|_| "setup executable unreadable")?;
        if self.status == SetupStatus::Running {
            return Err("setup already running");
        }
        let child = std::process::Command::new(&exe)
            .spawn()
            .map_err(|_| "setup executable failed to start")?;
        self.child = Some(child);
        self.status = SetupStatus::Running;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// fps (mainredrawwrapper).
// ---------------------------------------------------------------------------

struct FpsRing {
    times: [i64; 32],
    index: usize,
    fps: i32,
    /// `fpsAverage`: ms since the previous redraw.
    average: i32,
}
static FPS: Mutex<FpsRing> = Mutex::new(FpsRing {
    times: [0; 32],
    index: 0,
    fps: 0,
    average: 0,
});

/// mainredrawwrapper, called once per presented frame.
pub fn record_redraw(now: i64) {
    let mut ring = FPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = ring.times[(ring.index + 31) & 0x1F];
    let oldest = ring.times[ring.index];
    let index = ring.index;
    ring.times[index] = now;
    ring.index = (ring.index + 1) & 0x1F;
    if oldest != 0 && now > oldest {
        let elapsed = (now - oldest) as i32;
        ring.average = (now - previous) as i32;
        ring.fps = ((elapsed >> 1) + 32000) / elapsed;
    }
}
pub fn game_shell_fps_average() -> i32 {
    FPS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .average
}
pub fn game_shell_fps() -> i32 {
    FPS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .fps
}

// ---------------------------------------------------------------------------
// The lowercased operating-system name scripts see.
// ---------------------------------------------------------------------------

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "mac os x",
        "windows" => "windows",
        "linux" => "linux",
        other => other,
    }
}
// ---------------------------------------------------------------------------
// Telemetry grid and groups.
// ---------------------------------------------------------------------------

/// A failed telemetry operation: an index outside a list, a breached limit or
/// duplicate id, or a truncated read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryFault {
    class: Fault,
    detail: &'static str,
}
impl TelemetryFault {
    const fn new(class: Fault, detail: &'static str) -> Self {
        Self { class, detail }
    }
}
impl std::fmt::Display for TelemetryFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class.label(), self.detail)
    }
}
pub type Telemetry<T> = Result<T, TelemetryFault>;
const LIST_INDEX: TelemetryFault =
    TelemetryFault::new(Fault::IndexOutOfRange, "telemetry list index");

fn list_add<T>(list: &mut Vec<T>, index: i32, value: T) -> Telemetry<()> {
    let index = usize::try_from(index).map_err(|_| LIST_INDEX)?;
    if index > list.len() {
        return Err(LIST_INDEX);
    }
    list.insert(index, value);
    Ok(())
}
fn list_index(len: usize, index: i32) -> Telemetry<usize> {
    usize::try_from(index)
        .ok()
        .filter(|&i| i < len)
        .ok_or(LIST_INDEX)
}
fn list_remove<T>(list: &mut Vec<T>, index: i32) -> Telemetry<T> {
    let index = list_index(list.len(), index)?;
    Ok(list.remove(index))
}

const UNPINNED: i32 = -1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetryGroup {
    pub id: i32,
    /// Column ids.
    columns: Vec<i32>,
    /// Row ids.
    rows: Vec<i32>,
    /// A pinned row stores its own index, else [`UNPINNED`].
    pins: Vec<i32>,
    /// One value list per row, `null` as `None`.
    values: Vec<Vec<Option<i32>>>,
}
impl TelemetryGroup {
    pub fn new(id: i32) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }
    pub fn column_count(&self) -> i32 {
        self.columns.len() as i32
    }
    pub fn row_count(&self) -> i32 {
        self.rows.len() as i32
    }
    pub fn row_index(&self, id: i32) -> i32 {
        self.rows
            .iter()
            .position(|&r| r == id)
            .map_or(-1, |i| i as i32)
    }
    pub fn row_id(&self, index: i32) -> Telemetry<i32> {
        Ok(self.rows[list_index(self.rows.len(), index)?])
    }
    pub fn column_index(&self, id: i32) -> i32 {
        self.columns
            .iter()
            .position(|&c| c == id)
            .map_or(-1, |i| i as i32)
    }
    pub fn column_id(&self, index: i32) -> Telemetry<i32> {
        Ok(self.columns[list_index(self.columns.len(), index)?])
    }
    pub fn set_row_pinned(&mut self, row: i32, pinned: bool) -> Telemetry<()> {
        let at = list_index(self.pins.len(), row)?;
        self.pins[at] = if pinned { row } else { UNPINNED };
        Ok(())
    }
    fn set_pin(&mut self, row: i32, value: i32) -> Telemetry<()> {
        let at = list_index(self.pins.len(), row)?;
        self.pins[at] = value;
        Ok(())
    }
    pub fn is_row_pinned(&self, row: i32) -> Telemetry<bool> {
        Ok(self.pins[list_index(self.pins.len(), row)?] != UNPINNED)
    }
    pub fn add_column(&mut self, id: i32, at: i32) -> Telemetry<i32> {
        if self.columns.len() == 8 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "telemetry column limit",
            ));
        }
        if self.column_index(id) != -1 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "duplicate telemetry column",
            ));
        }
        let at = if at == -1 {
            self.columns.len() as i32
        } else {
            at
        };
        list_add(&mut self.columns, at, id)?;
        for row in &mut self.values {
            list_add(row, at, None)?;
        }
        Ok(at)
    }
    pub fn remove_column(&mut self, index: i32) -> Telemetry<()> {
        list_remove(&mut self.columns, index)?;
        for row in &mut self.values {
            list_remove(row, index)?;
        }
        Ok(())
    }
    pub fn move_row(&mut self, from: i32, to: i32) -> Telemetry<()> {
        self.relocate_row(from, to)?;
        for (index, pin) in self.pins.iter_mut().enumerate() {
            if *pin != UNPINNED && *pin != index as i32 {
                *pin = index as i32;
            }
        }
        Ok(())
    }
    fn relocate_row(&mut self, from: i32, to: i32) -> Telemetry<()> {
        let row = list_remove(&mut self.rows, from)?;
        list_add(&mut self.rows, to, row)?;
        let pin = list_remove(&mut self.pins, from)?;
        list_add(&mut self.pins, to, pin)?;
        let values = list_remove(&mut self.values, from)?;
        list_add(&mut self.values, to, values)
    }
    pub fn add_row(&mut self, id: i32, at: i32) -> Telemetry<i32> {
        if self.rows.len() == 40 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "telemetry row limit",
            ));
        }
        if self.row_index(id) != -1 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "duplicate telemetry row",
            ));
        }
        let at = if at == -1 { self.rows.len() as i32 } else { at };
        list_add(&mut self.rows, at, id)?;
        list_add(&mut self.pins, at, UNPINNED)?;
        list_add(&mut self.values, at, vec![None; self.columns.len()])?;
        let mut index = at as usize + 1;
        while index < self.rows.len() {
            let pin = self.pins[index];
            if pin != UNPINNED && pin < index as i32 {
                self.relocate_row(index as i32, index as i32 - 1)?;
            }
            index += 1;
        }
        Ok(self.row_index(id))
    }
    /// Removes a row and renumbers the pins of the rows after it.
    pub fn remove_row(&mut self, index: i32) -> Telemetry<()> {
        list_remove(&mut self.rows, index)?;
        list_remove(&mut self.pins, index)?;
        list_remove(&mut self.values, index)?;
        let mut next = index;
        let mut row = index;
        while (row as usize) < self.rows.len() {
            if !self.is_row_pinned(row)? {
                if next != row {
                    self.relocate_row(row, next)?;
                }
                next = row + 1;
            }
            row += 1;
        }
        Ok(())
    }
    pub fn move_column(&mut self, from: i32, to: i32) -> Telemetry<()> {
        let column = list_remove(&mut self.columns, from)?;
        list_add(&mut self.columns, to, column)?;
        for row in &mut self.values {
            let value = list_remove(row, from)?;
            list_add(row, to, value)?;
        }
        Ok(())
    }
    pub fn grid_value(&self, row: i32, column: i32) -> Telemetry<Option<i32>> {
        let values = &self.values[list_index(self.values.len(), row)?];
        Ok(values[list_index(values.len(), column)?])
    }
    /// `clearGridValue` (also the value setter).
    pub fn set_grid_value(&mut self, row: i32, column: i32, value: Option<i32>) -> Telemetry<()> {
        let at = list_index(self.values.len(), row)?;
        let values = &mut self.values[at];
        let column = list_index(values.len(), column)?;
        values[column] = value;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetryGrid {
    groups: Vec<TelemetryGroup>,
}
impl TelemetryGrid {
    pub fn group_count(&self) -> i32 {
        self.groups.len() as i32
    }
    pub fn group_index(&self, id: i32) -> i32 {
        self.groups
            .iter()
            .position(|g| g.id == id)
            .map_or(-1, |i| i as i32)
    }
    pub fn group(&self, index: i32) -> Telemetry<&TelemetryGroup> {
        Ok(&self.groups[list_index(self.groups.len(), index)?])
    }
    fn group_mut(&mut self, index: i32) -> Telemetry<&mut TelemetryGroup> {
        let at = list_index(self.groups.len(), index)?;
        Ok(&mut self.groups[at])
    }
    pub fn add_group(&mut self, group: TelemetryGroup, at: i32) -> Telemetry<i32> {
        if self.groups.len() == 5 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "telemetry group limit",
            ));
        }
        if self.group_index(group.id) != -1 {
            return Err(TelemetryFault::new(
                Fault::InvalidState,
                "duplicate telemetry group",
            ));
        }
        let at = if at == -1 {
            self.groups.len() as i32
        } else {
            at
        };
        list_add(&mut self.groups, at, group)?;
        Ok(at)
    }
    pub fn remove_group(&mut self, index: i32) -> Telemetry<()> {
        list_remove(&mut self.groups, index).map(drop)
    }
}

/// Packet reads used by the telemetry handlers.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    /// One `rs910_core::reader` read at `pos` (the arithmetic, including the
    /// `_alt` transforms, lives there).
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> Telemetry<T> {
        let mut r = CoreReader::at(self.bytes, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|_| TelemetryFault::new(Fault::IndexOutOfRange, "telemetry packet read"))
    }
    fn g1(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1).map(i32::from)
    }
    fn g1b(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1b).map(i32::from)
    }
    /// `g1_alt1`: `data - 128 & 0xFF`.
    fn g1_alt1(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1_alt1).map(i32::from)
    }
    /// `g1_alt2`: `-data & 0xFF`.
    fn g1_alt2(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1_alt2).map(i32::from)
    }
    /// `g1_alt3`: `128 - data & 0xFF`.
    fn g1_alt3(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1_alt3).map(i32::from)
    }
    /// `g1b_alt1`: `(byte)(data - 128)`.
    fn g1b_alt1(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g1b_alt1).map(i32::from)
    }
    fn g4s(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g4s)
    }
    /// `g4_alt1`: little-endian.
    fn g4_alt1(&mut self) -> Telemetry<i32> {
        self.read(CoreReader::g4_alt1)
    }
}

impl State {
    /// Applies a telemetry grid packet (FULL, CLEAR_GRID_VALUE, ADD_ROW,
    /// REMOVE_COLUMN, REMOVE_GROUP, ADD_GROUP, MOVE_ROW, REMOVE_ROW,
    /// VALUES_DELTA, SET_ROW_PINNED, MOVE_COLUMN, ADD_COLUMN). Delta failures
    /// are caught and reported by queueing `TELEMETRY_ERROR` and latching the
    /// error.
    pub fn telemetry_packet(
        &mut self,
        opcode: u8,
        bytes: &[u8],
        outgoing: &mut Vec<u8>,
    ) -> anyhow::Result<()> {
        use crate::proto::server as p;
        let mut r = Reader { bytes, pos: 0 };
        if opcode == p::TELEMETRY_GRID_FULL {
            // A malformed FULL fails the packet read instead of latching the
            // telemetry error.
            let grid = &mut self.telemetry;
            grid.groups.clear();
            (|| -> Telemetry<()> {
                for _ in 0..r.g1()? {
                    let at = grid.add_group(TelemetryGroup::new(r.g4s()?), -1)?;
                    let group = grid.group_mut(at)?;
                    let rows = r.g1()?;
                    for _ in 0..rows {
                        group.add_row(r.g4s()?, -1)?;
                    }
                    let columns = r.g1()?;
                    for _ in 0..columns {
                        group.add_column(r.g4s()?, -1)?;
                    }
                    for row in 0..rows {
                        group.set_pin(row, r.g1b()?)?;
                        for column in 0..columns {
                            let value = if r.g1()? == 0 { None } else { Some(r.g4s()?) };
                            group.set_grid_value(row, column, value)?;
                        }
                    }
                }
                Ok(())
            })()
            .map_err(|e| anyhow::anyhow!("TELEMETRY_GRID_FULL: {e}"))?;
            self.telemetry_error = false;
            return Ok(());
        }
        if self.telemetry_error {
            return Ok(());
        }
        let grid = &mut self.telemetry;
        let result = (|| -> Telemetry<()> {
            match opcode {
                p::TELEMETRY_CLEAR_GRID_VALUE => {
                    let group = r.g1()?;
                    let row = r.g1_alt3()?;
                    let column = r.g1_alt3()?;
                    grid.group_mut(group)?.set_grid_value(row, column, None)
                }
                p::TELEMETRY_GRID_ADD_ROW => {
                    let at = r.g1b_alt1()?;
                    let id = r.g4s()?;
                    let group = r.g1_alt3()?;
                    grid.group_mut(group)?.add_row(id, at).map(drop)
                }
                p::TELEMETRY_GRID_REMOVE_COLUMN => {
                    let group = r.g1_alt2()?;
                    let column = r.g1_alt3()?;
                    grid.group_mut(group)?.remove_column(column)
                }
                p::TELEMETRY_GRID_REMOVE_GROUP => {
                    let group = r.g1()?;
                    grid.remove_group(group)
                }
                p::TELEMETRY_GRID_ADD_GROUP => {
                    let id = r.g4_alt1()?;
                    let at = r.g1b_alt1()?;
                    grid.add_group(TelemetryGroup::new(id), at).map(drop)
                }
                p::TELEMETRY_GRID_MOVE_ROW => {
                    let to = r.g1_alt1()?;
                    let group = r.g1_alt3()?;
                    let from = r.g1()?;
                    grid.group_mut(group)?.move_row(from, to)
                }
                p::TELEMETRY_GRID_REMOVE_ROW => {
                    let group = r.g1_alt3()?;
                    let row = r.g1()?;
                    grid.group_mut(group)?.remove_row(row)
                }
                p::TELEMETRY_GRID_VALUES_DELTA => {
                    let mut group = r.g1b()?;
                    while group != -1 {
                        let mut row = r.g1b()?;
                        while row != -1 {
                            let mut column = r.g1b()?;
                            while column != -1 {
                                let value = r.g4s()?;
                                grid.group_mut(group)?
                                    .set_grid_value(row, column, Some(value))?;
                                column = r.g1b()?;
                            }
                            row = r.g1b()?;
                        }
                        group = r.g1b()?;
                    }
                    Ok(())
                }
                p::TELEMETRY_GRID_SET_ROW_PINNED => {
                    let row = r.g1_alt1()?;
                    let group = r.g1_alt1()?;
                    let pinned = r.g1_alt3()? == 1;
                    grid.group_mut(group)?.set_row_pinned(row, pinned)
                }
                p::TELEMETRY_GRID_MOVE_COLUMN => {
                    let from = r.g1_alt2()?;
                    let group = r.g1()?;
                    let to = r.g1()?;
                    grid.group_mut(group)?.move_column(from, to)
                }
                p::TELEMETRY_GRID_ADD_COLUMN => {
                    let at = r.g1b_alt1()?;
                    let id = r.g4s()?;
                    let group = r.g1_alt3()?;
                    grid.group_mut(group)?.add_column(id, at).map(drop)
                }
                _ => Err(TelemetryFault::new(
                    Fault::InvalidState,
                    "not a telemetry packet",
                )),
            }
        })();
        if let Err(error) = result {
            // report(null, e); notifyTelemetryError(connection).
            log::warn!(
                "[client910] {}: {error}",
                crate::proto::server::name(opcode)
            );
            outgoing.push(crate::proto::client::TELEMETRY_ERROR);
            self.telemetry_error = true;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EmojiList (rs2/client/clientscript/emoji/).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Emoji {
    name: Vec<u16>,
    sprite_group: i32,
    sprite_file: i32,
}

/// `autochat` gates the overhead-chat substitution of
/// `draw2DEntityElements`, applied
/// by the app's scene chat owner before the backend measures and draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmojiList {
    pub autochat: bool,
    emojis: Vec<Emoji>,
    first_chars: Vec<u16>,
}
impl EmojiList {
    pub fn is_valid_char(c: u16) -> bool {
        (33..48).contains(&c)
            || ((58..=64).contains(&c) && c != 60)
            || (91..=95).contains(&c)
            || c >= 123
    }
    pub fn add(&mut self, name: &[u16], group: i32, file: i32) -> bool {
        if name.is_empty() || name.len() > 10 || !Self::is_valid_char(name[0]) {
            return false;
        }
        if let Some(i) = self.emojis.iter().position(|e| e.name == name) {
            self.emojis.remove(i);
        }
        self.emojis.push(Emoji {
            name: name.to_vec(),
            sprite_group: group,
            sprite_file: file,
        });
        self.rebuild_index();
        true
    }
    /// Removes an emoji by name (the first-character index is not rebuilt).
    pub fn remove(&mut self, name: &[u16]) {
        if let Some(i) = self.emojis.iter().position(|e| e.name == name) {
            self.emojis.remove(i);
        }
    }
    pub fn remove_all(&mut self) {
        self.emojis.clear();
        self.first_chars.clear();
    }
    #[allow(
        clippy::len_without_is_empty,
        reason = "the emoji list has a size but no emptiness query"
    )]
    pub fn len(&self) -> usize {
        self.emojis.len()
    }
    pub fn substitute(&self, text: &[u16]) -> Vec<u16> {
        if self.emojis.is_empty() {
            return text.to_vec();
        }
        let mut out = Vec::with_capacity(text.len());
        let mut in_tag = false;
        let mut i = 0;
        while i < text.len() {
            let c = text[i];
            let mut replaced = false;
            if in_tag {
                if c == u16::from(b'>') {
                    in_tag = false;
                }
            } else if c == u16::from(b'<') {
                in_tag = true;
            } else if self.first_chars.contains(&c) {
                for emoji in &self.emojis {
                    let n = emoji.name.len();
                    if i + n <= text.len() && text[i..i + n] == emoji.name[..] {
                        let tag = if emoji.sprite_file > 0 {
                            format!("<sprite={},{}>", emoji.sprite_group, emoji.sprite_file)
                        } else {
                            format!("<sprite={}>", emoji.sprite_group)
                        };
                        out.extend(tag.encode_utf16());
                        replaced = true;
                        i += n - 1;
                        break;
                    }
                }
            }
            if !replaced {
                out.push(c);
            }
            i += 1;
        }
        out
    }
    fn rebuild_index(&mut self) {
        self.first_chars.clear();
        for emoji in &self.emojis {
            let c = emoji.name[0];
            if !self.first_chars.contains(&c) {
                self.first_chars.push(c);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Live-streaming platform.
// The streaming SDK is a retired third-party native library that this client
// does not ship, so the platform probe answers "unsupported" on every host,
// the same answer the original gives on hosts without an SDK build. Scripts
// gate every other `ttv_*` command behind a successful probe; a script that
// calls one anyway hits the null-SDK faults the original raises.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Twitch {
    /// 0 idle, 3 unsupported.
    pub hardware_state: i32,
    /// Smooth resize.
    pub smooth_resize: bool,
    /// Frame capture pending.
    pub capture_pending: bool,
    /// A webcam frame buffer exists.
    pub webcam_frame: bool,
    /// Webcam flips (horizontal, vertical).
    pub flip: [bool; 2],
    /// Live-stream cursor.
    pub livestream_cursor: i32,
}

impl Default for Twitch {
    fn default() -> Self {
        Self {
            hardware_state: 0,
            smooth_resize: false,
            capture_pending: false,
            webcam_frame: false,
            flip: [false; 2],
            // The live-stream cursor starts at -1.
            livestream_cursor: -1,
        }
    }
}

/// `login_accountappeal` result: the accounts service could not be reached.
const ACCOUNT_APPEAL_UNAVAILABLE: i32 = 5;

const TWITCH_SDK_ABSENT: &str = "Twitch SDK unavailable";

impl Twitch {
    /// The platform probe: no SDK build exists for this client.
    fn hardware_platform(&mut self) -> i32 {
        self.hardware_state = 3;
        -1
    }
    /// `reset` (toolkit capture reset has no retained owner).
    fn reset(&mut self) {
        self.capture_pending = false;
    }
    fn has_prerequisites() -> bool {
        let os = os_name();
        if os.starts_with("win") {
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| "null".into());
            ["msvcr110.dll", "msvcp110.dll"].iter().all(|dll| {
                let path = std::path::Path::new(&windir).join("system32").join(dll);
                path.exists() && !path.is_dir()
            })
        } else if os.starts_with("mac") {
            let Ok(output) = std::process::Command::new("ps").arg("-few").output() else {
                return false;
            };
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.to_lowercase().contains("soundflowerbed"))
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers.
// ---------------------------------------------------------------------------

pub fn char_is_printable(c: u16) -> bool {
    (32..=126).contains(&c)
        || (160..=255).contains(&c)
        || matches!(c, 8364 | 338 | 8212 | 339 | 376)
}
/// Whether a code unit is a valid Cp1252 character (the 0x80..0x9F range
/// goes through the extension table).
pub fn cp1252_char_is_valid(c: u16) -> bool {
    const TABLE: [u16; 32] = [
        8364, 0, 8218, 402, 8222, 8230, 8224, 8225, 710, 8240, 352, 8249, 338, 0, 381, 0, 0, 8216,
        8217, 8220, 8221, 8226, 8211, 8212, 732, 8482, 353, 8250, 339, 0, 382, 376,
    ];
    (33..127).contains(&c)
        || (128..160).contains(&c)
        || (161..=255).contains(&c)
        || (c != 0 && TABLE.contains(&c))
}
/// Simple upper- or lower-case mapping of one UTF-16 unit.
fn unit_case(c: u16, upper: bool) -> u16 {
    if upper {
        crate::char_case::to_upper(c)
    } else {
        crate::char_case::to_lower(c)
    }
}
/// `distance` (Levenshtein over UTF-16 units).
pub fn string_distance(a: &[u16], b: &[u16]) -> i32 {
    if a.is_empty() {
        return b.len() as i32;
    }
    if b.is_empty() {
        return a.len() as i32;
    }
    let mut prev: Vec<i32> = (0..=a.len() as i32).collect();
    let mut cur = vec![0; a.len() + 1];
    for (j, &cb) in b.iter().enumerate() {
        cur[0] = j as i32 + 1;
        for i in 1..=a.len() {
            cur[i] = (cur[i - 1] + 1)
                .min(prev[i] + 1)
                .min(prev[i - 1] + i32::from(a[i - 1] != cb));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[a.len()]
}
/// `urlencode` (Cp1252.encode for escaped units).
pub fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in units(text) {
        let keep = (97..=122).contains(&c)
            || (65..=90).contains(&c)
            || (48..=57).contains(&c)
            || matches!(c, 46 | 45 | 42 | 95);
        if keep {
            out.push(c as u8 as char);
        } else if c == 32 {
            out.push('+');
        } else {
            let b = crate::client_command::byte(c);
            out.push('%');
            for nibble in [b >> 4, b & 0xF] {
                out.push(char::from(if nibble >= 10 {
                    nibble + 55
                } else {
                    nibble + 48
                }));
            }
        }
    }
    out
}
pub fn base36_upper(v: i64) -> String {
    let negative = v < 0;
    let mut n = (v as i128).unsigned_abs();
    let mut digits = Vec::new();
    loop {
        let d = (n % 36) as u8;
        digits.push(if d < 10 { b'0' + d } else { b'A' + d - 10 });
        n /= 36;
        if n == 0 {
            break;
        }
    }
    if negative {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).unwrap()
}
pub fn is_leap_year(year: i32) -> bool {
    if year < 0 {
        (year + 1) % 4 == 0
    } else if year < 1582 {
        year % 4 == 0
    } else if year % 4 != 0 {
        false
    } else if year % 100 != 0 {
        true
    } else {
        year % 400 == 0
    }
}

// Gregorian calendar day arithmetic: Julian before the default cutover
// (1582-10-15 Gregorian = day -141427 since the epoch).
const CUTOVER_DAY: i64 = -141_427;
const EPOCH_JDN: i64 = 2_440_588;

fn gregorian_days(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
fn julian_days(year: i64, month: i64, day: i64) -> i64 {
    let a = (14 - month).div_euclid(12);
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    day + (153 * m + 2).div_euclid(5) + 365 * y + y.div_euclid(4) - 32083 - EPOCH_JDN
}
/// Proleptic astronomical year, 1-based month and day for epoch day `days`.
fn calendar_fields(days: i64) -> (i64, i64, i64) {
    if days >= CUTOVER_DAY {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (yoe + era * 400 + i64::from(m <= 2), m, d)
    } else {
        let c = days + EPOCH_JDN + 32082;
        let d = (4 * c + 3).div_euclid(1461);
        let e = c - (1461 * d).div_euclid(4);
        let m = (5 * e + 2).div_euclid(153);
        let day = e - (153 * m + 2).div_euclid(5) + 1;
        let month = m + 3 - 12 * m.div_euclid(10);
        (d - 4800 + m.div_euclid(10), month, day)
    }
}
/// `Calendar.get(YEAR)` is the year of era (1 BC = 1).
fn year_of_era(year: i64) -> i64 {
    if year <= 0 {
        1 - year
    } else {
        year
    }
}
/// Noon GMT of a calendar date (lenient month/day), in epoch milliseconds.
pub fn gmt_noon_millis(day: i32, month0: i32, year: i32) -> i64 {
    let year = i64::from(year) + i64::from(month0).div_euclid(12);
    let month = i64::from(month0).rem_euclid(12) + 1;
    let gregorian = gregorian_days(year, month, 1);
    let first = if year > 1582 || (year == 1582 && gregorian >= CUTOVER_DAY) {
        gregorian
    } else {
        julian_days(year, month, 1)
    };
    (first + i64::from(day) - 1) * 86_400_000 + 12 * 3_600_000
}
pub fn runeday_from_date(day: i32, month0: i32, year: i32) -> i32 {
    let millis = gmt_noon_millis(day, month0, year);
    // Long division truncates toward zero.
    let runeday = (millis / 86_400_000) as i32 - 11745;
    if year < 1970 {
        runeday - 1
    } else {
        runeday
    }
}
/// The RuneDay start in epoch milliseconds: the day offset is an int
/// addition before widening to long.
pub fn runeday_millis(runeday: i32) -> i64 {
    i64::from(runeday.wrapping_add(11745)) * 86_400_000
}
/// `getRuneDay` in `UTC`: (day, month0, year).
pub fn runeday_to_date(runeday: i32) -> (i32, i32, i32) {
    let millis = runeday_millis(runeday);
    let (year, month, day) = calendar_fields(millis.div_euclid(86_400_000));
    (day as i32, month as i32 - 1, year_of_era(year) as i32)
}
/// Offset of the default JVM time zone (the process local zone) at `millis`.
fn local_offset_millis(millis: i64) -> Option<i64> {
    #[cfg(unix)]
    {
        let seconds: libc::time_t = millis.div_euclid(1000);
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        // Reentrant: writes only the caller-owned tm.
        if unsafe { libc::localtime_r(&seconds, tm.as_mut_ptr()) }.is_null() {
            return None;
        }
        Some(unsafe { tm.assume_init() }.tm_gmtoff * 1000)
    }
    #[cfg(not(unix))]
    {
        let _ = millis;
        None
    }
}
/// Local calendar fields (year, month, day) at `millis`.
fn local_fields(millis: i64) -> Option<(i64, i64, i64)> {
    let local = millis + local_offset_millis(millis)?;
    let (year, month, day) = calendar_fields(local.div_euclid(86_400_000));
    Some((year_of_era(year), month, day))
}
/// Formats a date in the local time zone (language 3 orders day, month, year).
pub fn format_date_local(millis: i64, language: i32) -> Option<String> {
    let (year, month, day) = local_fields(millis)?;
    let two = |v: i64| format!("{}{}", v / 10, v % 10);
    if language == 3 {
        return Some(format!(
            "{}/{}/{}{}",
            two(day),
            two(month),
            year % 100 / 10,
            year % 10
        ));
    }
    let months = crate::ui_time::MONTHS.get(usize::try_from(language).ok()?)?;
    Some(format!(
        "{}-{}-{}",
        two(day),
        months[month as usize - 1],
        year
    ))
}

// ---------------------------------------------------------------------------
// Outgoing packets (message framing written explicitly).
// ---------------------------------------------------------------------------

/// `Packet.pjstr`: Cp1252 per UTF-16 unit, NUL terminated.
fn pjstr(out: &mut Vec<u8>, command: &str, text: &str) -> VmResult<()> {
    let text = units(text);
    if text.contains(&0) {
        return Err(failed(
            command,
            Fault::InvalidState.message("string contains NUL"),
        ));
    }
    out.extend(text.into_iter().map(crate::client_command::byte));
    out.push(0);
    Ok(())
}
/// `Packet.pjstr2`: version byte 0, Cp1252 string, NUL.
fn pjstr2(out: &mut Vec<u8>, command: &str, text: &str) -> VmResult<()> {
    out.push(0);
    pjstr(out, command, text)
}
fn jlen(text: &str) -> usize {
    native910::jstr::len(text)
}

// ---------------------------------------------------------------------------
// Commands whose owner lives in the component host.
// ---------------------------------------------------------------------------

const HOOK_COMMANDS: [&str; 7] = [
    "pushCanvasSize",
    "window_getinsets",
    "pushFontMetrics",
    "text_gender",
    "fromdate",
    "setsubmenuminlength",
    "autosetup_blackflaglast",
];

pub fn hook_handles(command: &str) -> bool {
    HOOK_COMMANDS.contains(&command)
}

/// Component-host owners: the layout canvas, the font provider, the submenu
/// minimum length, preferences, the language and the local player.
pub fn dispatch_hook(
    command: &str,
    properties: &mut crate::ui_properties::State,
    vars: Option<&mut crate::ui_vars::Variables<'_>>,
    ints: &mut Vec<i32>,
    objs: &mut Vec<String>,
) -> VmResult<Option<Value>> {
    let mut longs = Vec::new();
    let mut s = Stacks {
        command,
        ints,
        objs,
        longs: &mut longs,
    };
    match command {
        // canvasSize.
        "pushCanvasSize" => {
            s.ints.extend(properties.layout.canvas);
            Ok(None)
        }
        // window_getinsets.
        "window_getinsets" => {
            let [w, h] = properties.layout.canvas;
            s.ints.extend([0, 0, w, h]);
            Ok(None)
        }
        // font_getmetrics caches and requests sprites; a missing result is a
        // missing-value fault.
        "pushFontMetrics" => {
            let id = s.int()?;
            let fonts = properties
                .fonts
                .as_ref()
                .ok_or_else(|| s.failed("font provider not installed"))?;
            let m = fonts
                .get_metrics(id, true, true)
                .map_err(|e| s.failed(format!("{e:#}")))?
                .ok_or_else(|| s.failed(Fault::MissingValue.message("font metrics")))?;
            s.ints
                .extend([m.space_width, m.ascent, m.descent, m.metric_a, m.metric_b]);
            Ok(None)
        }
        // setsubmenuminlength.
        "setsubmenuminlength" => {
            properties.minimenu.min_length = s.int()?;
            Ok(None)
        }
        // text_gender.
        "text_gender" => {
            let pair = s.objs_n(2)?;
            let vars = vars.ok_or_else(|| s.failed("local player owner not lent"))?;
            let player = vars
                .scene
                .local_player
                .and_then(|local| {
                    vars.scene
                        .players?
                        .players
                        .get(local.index as usize)?
                        .as_ref()
                })
                .ok_or_else(|| s.failed(Fault::MissingValue.message("local player entity")))?;
            let female = player.appearance.model.as_ref().is_some_and(|m| m.female);
            let [male_text, female_text]: [String; 2] = pair.try_into().unwrap();
            s.objs.push(if female { female_text } else { male_text });
            Ok(None)
        }
        // fromdate formats in the default time zone with the language id.
        "fromdate" => {
            let runeday = s.int()?;
            let vars = vars.ok_or_else(|| s.failed("language owner not lent"))?;
            let language = vars
                .state
                .queries
                .language
                .ok_or_else(|| s.failed(Fault::MissingValue.message("language")))?;
            let millis = runeday_millis(runeday);
            let text = format_date_local(millis, language as i32)
                .ok_or_else(|| s.failed(Fault::IndexOutOfRange.message("month name table")))?;
            s.objs.push(text);
            Ok(None)
        }
        // autosetup_blackflaglast.
        "autosetup_blackflaglast" => {
            let vars = vars.ok_or_else(|| s.failed("preferences owner not lent"))?;
            let preferences = &mut vars.state.queries.preferences;
            if preferences.autosetup_display_mode == 1 {
                preferences.blackflag_mode4 = true;
            } else if preferences.autosetup_display_mode == 3 {
                preferences.blackflag_mode3 = true;
            }
            Ok(None)
        }
        _ => Err(VmError::UnknownCommand {
            command: command.into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Engine owners.
// ---------------------------------------------------------------------------

impl super::Engine {
    /// The final retained owner before `absent()`; `None` when the command is
    /// not in this module's partition.
    pub(super) fn host_builtin(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        let mut s = Stacks {
            command: c.command,
            ints,
            objs,
            longs,
        };
        match self.builtin(&mut s) {
            Err(VmError::UnknownCommand { .. }) => None,
            result => Some(result),
        }
    }

    /// `Err(UnknownCommand)` (stacks untouched) outside this partition.
    fn builtin(&mut self, s: &mut Stacks<'_>) -> VmResult<Option<Value>> {
        let command = s.command;
        match command {
            // --- Disabled handlers ---
            // applyDisplayPreference queues the sprite group for prefetch.
            "applyDisplayPreference" => (|| {
                let a = s.ints_n(3)?;
                if !self.builtins.sprite_prefetch.contains(&a[1]) {
                    self.builtins.sprite_prefetch.push(a[1]);
                }
                Ok(None)
            })(),
            // -style pair discards.
            "discardDetailSettingPair"
            | "discardDisplaySettingPair"
            | "discardDragResizePair"
            | "discardDragTargetPair"
            // setwalkmarker.
            | "setwalkmarker" => s.constant([2, 0], &[], &[]),
            "discardDisplayBounds" | "discardFourInterfaceArgs" => s.constant([4, 0], &[], &[]),
            // Discard one int: notifications_cancellocal, shader_preload_allow
            // and shader_preload_throttle.
            "discardDragTarget"
            | "discardFontArg"
            | "notifications_cancellocal"
            | "shader_preload_allow"
            | "shader_preload_throttle" => s.constant([1, 0], &[], &[]),
            "discardNotificationGroup" => s.constant([0, 1], &[], &[]),
            "discardWorldMapMenuAction" => s.constant([2, 1], &[], &[]),
            // Commands that return without effect: the opcode-1144 command,
            // runjavascript and show_software_license.
            "disabled_command_1144"
            | "noopAutosetupCommand"
            | "noopInterfaceCommand"
            | "noopLicenseCommand"
            | "noopTargetModeCommand"
            | "noopWalkMarkerCommand"
            | "runjavascript"
            | "show_software_license" => Ok(None),
            // preload_download_complete, battery_ischarging and the
            // availability probes answer true.
            "pushFontAvailable"
            | "pushLicenseAvailable"
            | "pushNotificationPermissionGranted"
            | "preload_download_complete"
            | "battery_ischarging"
            | "can_run_classic_client" => int(1),
            // playerdemo;
            // os_driver_outdated; os_isandroid; os_isios;
            // preload_download_*; has_html5.
            "pushShopUnavailable"
            | "pushTargetModeUnavailable"
            | "pushTargetModeZero"
            | "pushWorldMapUnavailable"
            | "playerdemo"
            | "os_driver_outdated"
            | "os_isandroid"
            | "os_isios"
            | "preload_download_downloadedsize"
            | "preload_download_rate"
            | "preload_download_remainingsize"
            | "preload_download_totalsize"
            | "has_html5" => int(0),
            // os_driver_vendor.
            "os_driver_vendor" => int(-1),
            // shader_preload_percent; battery_getlevelpercent.
            "shader_preload_percent" | "battery_getlevelpercent" => int(100),
            //  (isp--, push 0).
            "pushNotificationUnavailable" => s.constant([1, 0], &[0], &[]),
            "pushUnsupportedCommandDefaults" => s.constant([1, 0], &[0], &[""]),
            "pushUnsupportedCommandTriple" => s.constant([2, 0], &[0, 0], &[""]),
            "pushUnsupportedPairFalse"
            | "pushUnsupportedPairFalseAlt"
            | "pushUnsupportedPairFalseFifth"
            | "pushUnsupportedPairFalseFourth"
            | "pushUnsupportedPairFalseThird" => s.constant([2, 0], &[0], &[]),
            "pushUnsupportedQuadFalse" => s.constant([4, 0], &[0], &[]),
            "pushZeroInsets" => s.constant([0, 0], &[0, 0, 0, 0], &[]),
            // notifications_sendgroupedlocal (osp -= 3; isp -= 3).
            "notifications_sendgroupedlocal" => (|| {
                s.objs_n(3)?;
                s.constant([3, 0], &[0], &[])
            })(),
            // notifications_sendlocal.
            "notifications_sendlocal" => (|| {
                s.objs_n(2)?;
                s.constant([2, 0], &[0], &[])
            })(),
            // The setup status.
            "pushRuneScapeSetupValue" => int(self.builtins.setup.status() as i32),
            // Starts the installer on Windows; a failure aborts the script.
            "saveRuneScapeSetup" => self
                .builtins
                .setup
                .launch()
                .map(|()| None)
                .map_err(|reason| s.failed(Fault::InvalidState.message(reason))),
            // Canvas/font/text owners live in the component host.
            "pushCanvasSize" | "window_getinsets" | "pushFontMetrics" | "text_gender"
            | "fromdate" | "setsubmenuminlength" | "autosetup_blackflaglast" => Err(s.failed(
                "owner is lent only to the component hook host (ui_host_builtins::dispatch_hook)",
            )),
            // detail_toolkit and detailget_performance_metric mutate
            // `preferences`, whose owner is the game variable domain:
            // `ui_host::Queries::dispatch` → `Preferences::mutate_graphics`
            // (the RecreateToolkit consumer is `App::apply_toolkit_preferences`).
            "detail_toolkit" | "detailget_performance_metric" => Err(s.failed(
                "preferences are lent only through the game variable domains (ui_host::Queries)",
            )),

            // --- Client fields ---
            // affiliate.
            "affiliate" => int(self.builtins.player_is_affiliate),
            // frombilling.
            "frombilling" => int(i32::from(self.builtins.from_billing)),
            // playercountry.
            "playercountry" => int(self.builtins.current_player_country),
            // create_get_email.
            "create_get_email" => Ok(Some(Value::Str(
                self.builtins.create_email.clone().unwrap_or_default(),
            ))),
            // logout_getreason (an unknown reason is a missing value).
            "logout_getreason" => self
                .builtins
                .logout_reason
                .map(|id| Some(Value::Int(id)))
                .ok_or_else(|| s.failed(Fault::MissingValue.message("logout reason"))),
            // is_gamescreen_state: states 18/3/9 are the world connection.
            "is_gamescreen_state" => int(i32::from(self.login.world_list_game)),
            // fps_stats.
            "fps_stats" => {
                let fps = game_shell_fps();
                s.ints.extend([fps, fps, 1]);
                Ok(None)
            }
            // Installed physical memory in megabytes (0 until the shell has
            // probed the machine).
            "os_physicalmemorysize" => int(self.platform.physical_memory_mb),
            // A machine under 512 MB, or one already in (or having chosen)
            // safe mode, offers the safe-mode choice.
            "detailget_canchoosesafemode" => int(i32::from(
                self.platform.physical_memory_mb < 512
                    || self.platform.safe_mode
                    || self.platform.chose_safe_mode,
            )),
            // os_islinux/os_ismac/os_iswindows.
            "os_islinux" => int(i32::from(os_name().starts_with("linux"))),
            "os_ismac" => int(i32::from(os_name().starts_with("mac"))),
            "os_iswindows" => int(i32::from(os_name().starts_with("win"))),
            // profile_cpu → profile.
            "profile_cpu" => int(crate::ui_preferences::cpu_profile()),
            // preload_percent / preload_progress read the resource providers'
            // prefetch-all progress.
            "preload_percent" | "preload_progress" => self
                .builtins
                .preload_progress
                .map(|value| Some(Value::Int(value)))
                .ok_or_else(|| s.failed(Fault::MissingValue.message("resource providers"))),
            // --- Coordinates ---
            // coord.
            "coord" => {
                let player = self
                    .camera.cam2
                    .scene
                    .local_player
                    .ok_or_else(|| s.failed(Fault::MissingValue.message("local player entity")))?;
                int((player.level << 28) | ((player.coord[0] >> 9) << 14) | (player.coord[2] >> 9))
            }
            // coordx.
            "coordx" => s.int().map(|v| Some(Value::Int(v >> 14 & 0x3FFF))),
            // coordy.
            "coordy" => s.int().map(|v| Some(Value::Int(v >> 28))),
            // coordz.
            "coordz" => s.int().map(|v| Some(Value::Int(v & 0x3FFF))),
            // movecoord.
            "movecoord" => s.ints_n(4).map(|a| {
                let packed = (a[1] << 14).wrapping_add(a[0]);
                let packed = (a[2] << 28).wrapping_add(packed);
                Some(Value::Int(a[3].wrapping_add(packed)))
            }),

            // --- Pure helpers ---
            // hsvtorgb (through the colour table).
            "hsvtorgb" => s.int().map(|v| Some(Value::Int(hsv_table()[(v & 0xFFFF) as usize]))),
            // sin_deg / cos_deg replace
            // the top of stack in place.
            "sin_deg" | "cos_deg" => {
                let top = s.ints.last_mut().ok_or(VmError::StackUnderflow { stack: "int" })?;
                *top = if command == "sin_deg" {
                    crate::trig::sin(*top)
                } else {
                    crate::trig::cos(*top)
                };
                Ok(None)
            }
            // atan2_deg.
            "atan2_deg" => s.ints_n(2).map(|a| Some(Value::Int(crate::trig::atan2(a[0], a[1])))),
            // char_isalpha/.. on `(char) int`.
            "char_isalpha" | "char_isalphanumeric" | "char_isnumeric" | "char_isprintable"
            | "char_isvalid" => s.int().map(|v| {
                let c = v as u16;
                let alpha = (65..=90).contains(&c) || (97..=122).contains(&c);
                let digit = (48..=57).contains(&c);
                Some(Value::Int(i32::from(match command {
                    "char_isalpha" => alpha,
                    "char_isalphanumeric" => alpha || digit,
                    "char_isnumeric" => digit,
                    "char_isprintable" => char_is_printable(c),
                    _ => cp1252_char_is_valid(c),
                })))
            }),
            // char_tolowercase / char_touppercase.
            "char_tolowercase" | "char_touppercase" => s.int().map(|v| {
                Some(Value::Int(i32::from(unit_case(v as u16, command == "char_touppercase"))))
            }),
            // string_distance.
            "string_distance" => s
                .objs_n(2)
                .map(|p| Some(Value::Int(string_distance(&units(&p[0]), &units(&p[1]))))),
            // text_switch.
            "text_switch" => (|| {
                let pair = s.objs_n(2)?;
                let [first, second]: [String; 2] = pair.try_into().unwrap();
                Ok(Some(Value::Str(if s.int()? == 1 { first } else { second })))
            })(),
            // urlencode.
            "urlencode" => s.obj().map(|t| Some(Value::Str(urlencode(&t)))),
            // clanforumqfc_tostring.
            "clanforumqfc_tostring" => s.long().map(|v| {
                Some(Value::Str(if v == -1 { String::new() } else { base36_upper(v) }))
            }),
            // seqlength: an absent file decodes to a default sequence (length
            // 0); the length is the sum of the frame lengths.
            "seqlength" => (|| {
                let id = s.int()?;
                let seqs = self
                    .configs.seqs
                    .as_ref()
                    .ok_or_else(|| s.failed("sequence types not installed"))?;
                let seq = u32::try_from(id).ok().and_then(|id| seqs.get(id));
                int(seq.map_or(0, |seq| {
                    seq.frame_lengths
                        .iter()
                        .fold(0i32, |sum, &v| sum.wrapping_add(i32::from(v)))
                }))
            })(),

            // --- Dates ---
            // date_isleapyear.
            "date_isleapyear" => s.int().map(|y| Some(Value::Int(i32::from(is_leap_year(y))))),
            // date_minutes.
            "date_minutes" => int((crate::logic_clock::monotonic_millis() / 60_000) as i32),
            // date_minutes_fromruneday.
            "date_minutes_fromruneday" => s
                .int()
                .map(|d| Some(Value::Int((runeday_millis(d) / 60_000) as i32))),
            // date_runeday_fromdate.
            "date_runeday_fromdate" => s
                .ints_n(3)
                .map(|a| Some(Value::Int(runeday_from_date(a[0], a[1], a[2])))),
            // date_runeday_todate.
            "date_runeday_todate" => s.int().map(|d| {
                let (day, month0, year) = runeday_to_date(d);
                s.ints.extend([day, month0, year]);
                None
            }),
            // date_year.
            "date_year" => local_fields(crate::logic_clock::monotonic_millis())
                .map(|(year, _, _)| Some(Value::Int(year as i32)))
                .ok_or_else(|| s.failed("default time zone unavailable")),

            // --- Emoji ---
            // emoji_add.
            "emoji_add" => (|| {
                let a = s.ints_n(2)?;
                let name = units(&s.obj()?);
                if name.is_empty() {
                    return Err(s.failed(Fault::IndexOutOfRange.message("empty emoji name")));
                }
                if !EmojiList::is_valid_char(name[0]) || name.len() > 10 {
                    return Err(s.failed(Fault::InvalidState.message("invalid emoji")));
                }
                if !self.builtins.emoji.add(&name, a[0], a[1]) {
                    return Err(s.failed(Fault::InvalidState.message("emoji add")));
                }
                Ok(None)
            })(),
            // emoji_enable_auto_chatline.
            "emoji_enable_auto_chatline" => s.int().map(|v| {
                self.builtins.emoji.autochat = v != 0;
                None
            }),
            // emoji_remove.
            "emoji_remove" => s.obj().map(|name| {
                self.builtins.emoji.remove(&units(&name));
                None
            }),
            // emoji_removeall.
            "emoji_removeall" => {
                self.builtins.emoji.remove_all();
                Ok(None)
            }
            // emoji_substitute: the argument stays on
            // the stack when the list is empty.
            "emoji_substitute" => {
                if self.builtins.emoji.len() > 0 {
                    s.obj().map(|text| {
                        Some(Value::Str(native910::jstr::from_units(
                            &self.builtins.emoji.substitute(&units(&text)),
                        )))
                    })
                } else {
                    Ok(None)
                }
            }

            // --- Script bridge (scripting is disabled in the application; the
            // missing-applet failure is swallowed) ---
            // video_advert_allow_skip, video_advert_force_remove,
            // notify_accountcreated, notify_accountcreatestarted.
            "video_advert_allow_skip"
            | "video_advert_force_remove"
            | "notify_accountcreated"
            | "notify_accountcreatestarted" => Ok(None),
            // video_advert_has_finished.
            "video_advert_has_finished" => int(1),
            // video_advert_play: the argument is popped only
            // inside the javascriptEnabled branch.
            "video_advert_play" => int(0),

            // --- Telemetry (telemetry) ---
            "telemetry_get_column_count"
            | "telemetry_get_column_id"
            | "telemetry_get_column_index"
            | "telemetry_get_grid_value"
            | "telemetry_get_group_id"
            | "telemetry_get_group_index"
            | "telemetry_get_row_count"
            | "telemetry_get_row_id"
            | "telemetry_get_row_index"
            | "telemetry_is_grid_processor_set"
            | "telemetry_is_row_pinned" => self.telemetry_command(s),

            // --- Twitch ---
            _ if command.starts_with("ttv_") => match self.twitch_command(s) {
                Some(result) => result,
                None => Err(VmError::UnknownCommand {
                    command: command.into(),
                }),
            },

            // --- Console / app ---
            // writeconsole: System.out.println.
            "writeconsole" => s.obj().map(|text| {
                println!("{text}");
                None
            }),
            // docheat.
            "docheat" => s.obj().map(|cheat| {
                self.builtins.requests.push(Request::Cheat(cheat));
                None
            }),
            // quit: the application environment
            // leaves fullscreen and exits through the window owner.
            "quit" => {
                self.builtins.requests.push(Request::Quit);
                Ok(None)
            }
            // mes_typed.
            "mes_typed" => (|| {
                let a = s.ints_n(2)?;
                let text = s.obj()?;
                match a[0] {
                    99 => self.effects.console_messages.push(text),
                    98 => self.builtins.requests.push(Request::ConsoleEntry(text)),
                    _ => {
                        self.messages.history.add_message(NewChatLine {
                            flags: a[1],
                            ..NewChatLine::system(a[0], text)
                        });
                        self.messages.changed = true;
                    }
                }
                Ok(None)
            })(),
            // getclipboard: the system clipboard's
            // string flavour, "" when absent or not text.
            "getclipboard" => Ok(Some(Value::Str(
                native910::jstr::from_text(&crate::clipboard::get_text().unwrap_or_default()),
            ))),
            // openurlraw: flag 0 targets the absent applet (swallowed); flag 1
            // opens the desktop browser through the existing URL_OPEN consumer.
            "openurlraw" => (|| {
                let url = s.obj()?;
                if s.int()? == 1 {
                    self.effects.browser_urls.push(crate::server_prot::UiEvent::UrlOpen {
                        primary: url,
                        fallback: None,
                        javascript: false,
                    });
                }
                Ok(None)
            })(),
            // openurl_nologin prefixes the site URL (mode/world/site settings).
            // It first leaves fullscreen when fullscreen is allowed; the window
            // owner applies that when it drains the queue (openurlraw does
            // not).
            "openurl_nologin" => (|| {
                self.effects.browser_fullscreen_exit |= self.platform.fullscreen_allowed;
                let path = s.obj()?;
                let flag = s.int()? == 1;
                let prefix = crate::applet_params::get()
                    .site_url(&self.login.site_settings)
                    .map_err(|e| s.failed(format!("{e:#}")))?;
                // Same as openurlraw.
                if flag {
                    self.effects.browser_urls.push(crate::server_prot::UiEvent::UrlOpen {
                        primary: prefix + &path,
                        fallback: None,
                        javascript: false,
                    });
                }
                Ok(None)
            })(),
            // openurl / openurl_shim →
            // openUrl queues URL_REQUEST (the server answers URL_OPEN).
            "openurl" | "openurl_shim" => (|| {
                let shim = command == "openurl_shim";
                let strings = s.objs_n(if shim { 3 } else { 2 })?;
                let flag = s.int()? == 1;
                let a = &strings[0];
                let b = &strings[1];
                let c = if shim { strings[2].as_str() } else { "" };
                let size = jlen(a) + 1 + jlen(b) + 1 + jlen(c) + 1 + 1;
                let mut packet = vec![crate::proto::client::URL_REQUEST, (size >> 8) as u8, size as u8];
                pjstr(&mut packet, command, a)?;
                pjstr(&mut packet, command, b)?;
                pjstr(&mut packet, command, c)?;
                packet.push(u8::from(flag) | if shim { 2 } else { 0 });
                self.outgoing.extend(packet);
                Ok(None)
            })(),
            // bug_report → sendBugReport.
            "bug_report" => (|| {
                let kind = s.int()?;
                let pair = s.objs_n(2)?;
                let (a, b) = (&pair[0], &pair[1]);
                if jlen(a) <= 500 && jlen(b) <= 500 {
                    let size = jlen(a) + 2 + 1 + jlen(b) + 2;
                    let mut packet = vec![crate::proto::client::BUG_REPORT, (size >> 8) as u8, size as u8];
                    pjstr2(&mut packet, command, a)?;
                    packet.push((kind as u8).wrapping_neg());
                    pjstr2(&mut packet, command, b)?;
                    self.outgoing.extend(packet);
                }
                Ok(None)
            })(),
            // resume_clanforumqfcdialog.
            "resume_clanforumqfcdialog" => (|| {
                let text = s.obj()?;
                let mut packet = vec![
                    crate::proto::client::RESUME_P_CLANFORUMQFCDIALOG,
                    (jlen(&text) + 1) as u8,
                ];
                pjstr(&mut packet, command, &text)?;
                self.outgoing.extend(packet);
                Ok(None)
            })(),
            // email_validation_submit_code.
            "email_validation_submit_code" => (|| {
                let code = s.obj()?;
                let mut packet = vec![
                    crate::proto::client::SEND_EMAIL_VALIDATION_CODE,
                    (jlen(&code) + 1) as u8,
                ];
                pjstr(&mut packet, command, &code)?;
                self.outgoing.extend(packet);
                Ok(None)
            })(),
            // email_validation_change_address: the
            // top string is written first.
            "email_validation_change_address" => (|| {
                let first = s.obj()?;
                let second = s.obj()?;
                let size = jlen(&first) + 1 + jlen(&second) + 1;
                let mut packet = vec![
                    crate::proto::client::CHANGE_EMAIL_ADDRESS,
                    (size >> 8) as u8,
                    size as u8,
                ];
                pjstr(&mut packet, command, &first)?;
                pjstr(&mut packet, command, &second)?;
                self.outgoing.extend(packet);
                Ok(None)
            })(),
            // email_validation_add_new_address.
            "email_validation_add_new_address" => (|| {
                let address = s.objs_n(1)?.remove(0);
                let flags = s.ints_n(3)?;
                let size = jlen(&address) + 1 + 1;
                let mut packet = vec![
                    crate::proto::client::ADD_NEW_EMAIL_ADDRESS,
                    (size >> 8) as u8,
                    size as u8,
                ];
                pjstr(&mut packet, command, &address)?;
                packet.push(
                    u8::from(flags[0] == 1) | u8::from(flags[1] == 1) << 1 | u8::from(flags[2] == 1) << 2,
                );
                self.outgoing.extend(packet);
                Ok(None)
            })(),
            // resend_uid_passport_request asks the server to email the device
            // check again; it sends only while a lobby login is running
            // (state 17), on the connection that login waits on.
            "resend_uid_passport_request" => {
                if self.login.lobby_logging_in {
                    self.login.resend_uid_passport_requested = true;
                }
                Ok(None)
            }

            // --- Login ---
            // login_continue resumes a login parked on reply 1 (the
            // advertisement countdown); the session owner acts on it only
            // while one is parked.
            "login_continue" => {
                self.login.continue_requested = true;
                Ok(None)
            }
            // login_request_social_network / lobby_enterlobby_social_network:
            // sign on to the lobby or the world through a social network
            // account. No effect unless the login is ready.
            "login_request_social_network" | "lobby_enterlobby_social_network" => (|| {
                let flags = s.ints_n(2)?;
                let new_auth_preference = s.obj()?;
                if self.login.ready {
                    self.login.ready = false;
                    self.login.request = Some(crate::ui_runtime::LoginRequest {
                        username: String::new(),
                        password: String::new(),
                        new_auth_preference,
                        auth_dont_trust: flags[1] == 1,
                        lobby: command == "lobby_enterlobby_social_network",
                        sso: Some(flags[0]),
                    });
                    self.login.in_progress = true;
                    if command == "login_request_social_network" {
                        self.login.reply = -3;
                        self.login.hoptime = 0;
                        self.login.disallow_result = -1;
                        self.login.disallow_trigger = -1;
                    } else {
                        self.login.lobby_reply = -3;
                    }
                    self.login.ban_duration = 0;
                    self.login.queue_position = -1;
                }
                Ok(None)
            })(),
            // login_accountappeal answers 5 when the accounts service does not
            // answer. This client has no accounts service to ask, so the
            // request always ends that way.
            "login_accountappeal" => s.obj().map(|_| Some(Value::Int(ACCOUNT_APPEAL_UNAVAILABLE))),
            // setup_messagebox configures the message box.
            "setup_messagebox" => (|| {
                let a = s.ints_n(11)?;
                let align = |v: i32| usize::try_from(v).ok().filter(|&v| v < 3);
                let (Some(halign), Some(valign)) = (align(a[0]), align(a[1])) else {
                    return Err(s.failed(Fault::IndexOutOfRange.message("loading screen alignment")));
                };
                // Drawn by crate::message_box
                // by mainredraw in states 14/19.
                self.builtins.message_box = MessageBox {
                    setup: true,
                    halign,
                    valign,
                    box_xy: [a[2], a[3]],
                    min_size: [a[4], a[5]],
                    border_corner: a[6],
                    border_line: a[7],
                    background: a[8],
                    colour: a[9],
                    font: a[10],
                };
                Ok(None)
            })(),
            // get_displayname_withextras →
            // getNameWithExtras(true).
            "get_displayname_withextras" => match self.scene.active_entity.as_ref() {
                Some(super::ActiveEntity::Player { name, title, .. }) => Ok(Some(Value::Str(
                    match title {
                        Some(title) => title.replace("<name>", name),
                        None => name.clone(),
                    },
                ))),
                _ => Err(s.failed(Fault::WrongValueType.message("active entity is not a player"))),
            },
            _ => Err(VmError::UnknownCommand {
                command: command.into(),
            }),
        }
    }

    /// Telemetry CS2 reads.
    fn telemetry_command(&mut self, s: &mut Stacks<'_>) -> VmResult<Option<Value>> {
        let grid = &self.builtins.telemetry;
        let command = s.command;
        let e = |e: TelemetryFault| failed(command, e.to_string());
        let value = match command {
            "telemetry_get_group_index" => grid.group_index(s.int()?),
            "telemetry_get_group_id" => grid.group(s.int()?).map_err(e)?.id,
            "telemetry_get_row_count" => grid.group(s.int()?).map_err(e)?.row_count(),
            "telemetry_get_column_count" => grid.group(s.int()?).map_err(e)?.column_count(),
            _ => {
                let three = matches!(
                    command,
                    "telemetry_get_grid_value" | "telemetry_is_grid_processor_set"
                );
                let a = s.ints_n(if three { 3 } else { 2 })?;
                let group = grid.group(a[0]).map_err(e)?;
                match command {
                    "telemetry_get_row_index" => group.row_index(a[1]),
                    "telemetry_get_row_id" => group.row_id(a[1]).map_err(e)?,
                    "telemetry_is_row_pinned" => i32::from(group.is_row_pinned(a[1]).map_err(e)?),
                    "telemetry_get_column_index" => group.column_index(a[1]),
                    "telemetry_get_column_id" => group.column_id(a[1]).map_err(e)?,
                    //  (null → 0).
                    "telemetry_get_grid_value" => {
                        group.grid_value(a[1], a[2]).map_err(e)?.unwrap_or(0)
                    }
                    _ => i32::from(group.grid_value(a[1], a[2]).map_err(e)?.is_some()),
                }
            }
        };
        int(value)
    }

    /// Twitch (`ttv_*`) commands.
    /// `None` when `s.command` is not a `ttv_` command.
    fn twitch_command(&mut self, s: &mut Stacks<'_>) -> Option<VmResult<Option<Value>>> {
        let t = &mut self.builtins.twitch;
        let command = s.command;
        let null = || -> VmResult<Option<Value>> {
            Err(failed(
                command,
                Fault::MissingValue.message(TWITCH_SDK_ABSENT),
            ))
        };
        Some(match s.command {
            // ttv_library_request → getHardwarePlatform.
            "ttv_library_request" => int(t.hardware_platform()),
            // ttv_library_getstate → getHardwareState.
            "ttv_library_getstate" => int(t.hardware_state),
            // ttv_login: loginReady is false without the SDK → 12.
            "ttv_login" => s.objs_n(2).map(|_| Some(Value::Int(12))),
            // ttv_logout: loginReady false → 12.
            "ttv_logout" => int(12),
            // getLoginState/getStreamState/getViewerCount/getChatState/
            // getWebcamState dereference twitchTV.
            "ttv_login_getstate"
            | "ttv_stream_getstate"
            | "ttv_stream_getviewers"
            | "ttv_chat_getstate"
            | "ttv_webcam_getstate" => null(),
            // ttv_stream_start: GetRecommendedSettings on twitchTV.
            "ttv_stream_start" => s.ints_n(4).and_then(|_| null()),
            // ttv_stream_stop → stopStream: reset() then twitchTV.StopStreaming.
            "ttv_stream_stop" => {
                t.reset();
                null()
            }
            // ttv_stream_setsmoothresize → setStreamSmoothResize.
            "ttv_stream_setsmoothresize" => s.int().map(|v| {
                let smooth = v != 0;
                if t.smooth_resize != smooth {
                    t.reset();
                    t.smooth_resize = smooth;
                }
                None
            }),
            // ttv_stream_settitle / ttv_chat_sendmessage pop then call twitchTV.
            "ttv_stream_settitle" | "ttv_chat_sendmessage" => s.obj().and_then(|_| null()),
            // ttv_stream_getquality: isStreaming() is false with a null SDK.
            "ttv_stream_getquality" => int(0),
            // ttv_webcam_supported → isWebcamSupported.
            "ttv_webcam_supported" => int(i32::from(os_name().starts_with("win"))),
            // ttv_webcam_getdevice_count: webcamDevices null → 0.
            "ttv_webcam_getdevice_count" => int(0),
            // getWebcamDeviceByIndex/ByName return null for a null list.
            "ttv_webcam_getdevice_byindex" | "ttv_webcam_getdevice_byuniquename" => {
                let popped = if s.command.ends_with("byindex") {
                    s.int().map(drop)
                } else {
                    s.obj().map(drop)
                };
                popped.map(|()| {
                    s.ints.push(-1);
                    s.objs.extend([String::new(), String::new()]);
                    None
                })
            }
            "ttv_webcam_getcap_count" => s.int().map(|_| Some(Value::Int(-1))),
            "ttv_webcam_getcap_byindex" | "ttv_webcam_getcap_byuniqueid" => s.ints_n(2).map(|_| {
                s.ints.extend([-1; 5]);
                None
            }),
            // Starting / stopping a webcam device dereferences the absent
            // device list for a non-negative index.
            "ttv_webcam_start" => s.ints_n(2).and_then(|a| {
                t.webcam_frame = true;
                if a[0] >= 0 {
                    return null();
                }
                int(-1)
            }),
            "ttv_webcam_stop" => s.int().and_then(|device| {
                t.webcam_frame = false;
                if device >= 0 {
                    return null();
                }
                int(-1)
            }),
            // ttv_webcam_flip → flipWebcamDevice.
            "ttv_webcam_flip" => s.ints_n(2).map(|a| {
                t.flip = [a[0] != 0, a[1] != 0];
                None
            }),
            // ttv_livestreams_update: the SDK call fails.
            "ttv_livestreams_update" => null(),
            // ttv_livestreams_getstream_start / _next return -1 while the
            // livestream list is absent.
            "ttv_livestreams_getstream_start" | "ttv_livestreams_getstream_next" => {
                if s.command.ends_with("start") {
                    t.livestream_cursor = -1;
                }
                s.ints.push(-1);
                s.objs.extend(std::iter::repeat_n(String::new(), 4));
                Ok(None)
            }
            // ttv_setdebugoutput: isp -= 3; debugging is false.
            "ttv_setdebugoutput" => s.ints_n(3).map(|_| None),
            // ttv_hasprerequisites → hasPrerequisites.
            "ttv_hasprerequisites" => int(i32::from(Twitch::has_prerequisites())),
            _ => return None,
        })
    }

    /// Server telemetry packets (see [`State::telemetry_packet`]).
    pub(super) fn telemetry_packet(&mut self, opcode: u8, bytes: &[u8]) -> anyhow::Result<()> {
        self.builtins
            .telemetry_packet(opcode, bytes, &mut self.outgoing)
    }
}

fn hsv_table() -> &'static [i32] {
    static TABLE: std::sync::OnceLock<Vec<i32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(crate::colour::build_hsv_table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_predicates_match_the_reference_tables() {
        assert!(char_is_printable(32) && char_is_printable(126) && !char_is_printable(127));
        assert!(char_is_printable(8364) && !char_is_printable(8218));
        // Cp1252.charIsValid excludes space, DEL and NBSP but keeps 128..159.
        assert!(!cp1252_char_is_valid(32) && !cp1252_char_is_valid(127));
        assert!(!cp1252_char_is_valid(160) && cp1252_char_is_valid(161));
        assert!(
            cp1252_char_is_valid(130) && cp1252_char_is_valid(8218) && !cp1252_char_is_valid(0)
        );
        assert_eq!(unit_case(u16::from(b'A'), false), u16::from(b'a'));
        assert_eq!(unit_case(0xFF, true), 0x178);
        assert_eq!(unit_case(u16::from(b'1'), true), u16::from(b'1'));
    }

    #[test]
    fn string_distance_is_levenshtein() {
        assert_eq!(string_distance(&units(""), &units("abc")), 3);
        assert_eq!(string_distance(&units("kitten"), &units("sitting")), 3);
        assert_eq!(string_distance(&units("flaw"), &units("lawn")), 2);
    }

    #[test]
    fn urlencode_and_base36_follow_the_reference() {
        assert_eq!(urlencode("a b*_.-Z9"), "a+b*_.-Z9");
        assert_eq!(urlencode("/?"), "%2F%3F");
        assert_eq!(urlencode("€é"), "%80%E9");
        assert_eq!(base36_upper(35), "Z");
        assert_eq!(base36_upper(36 * 36), "100");
        assert_eq!(base36_upper(-71), "-1Z");
    }

    #[test]
    fn dates_follow_the_gregorian_calendar() {
        // RuneDay 0 is 2002-02-27 UTC.
        assert_eq!(runeday_to_date(0), (27, 1, 2002));
        assert_eq!(runeday_from_date(27, 1, 2002), 0);
        // Lenient month roll-over and pre-1970 truncation adjustment.
        assert_eq!(
            runeday_from_date(1, 12, 2001),
            runeday_from_date(1, 0, 2002)
        );
        assert_eq!(runeday_from_date(31, 11, 1969), -11745 - 1);
        assert_eq!(runeday_to_date(-11745), (1, 0, 1970));
        // Julian calendar before the 1582 cutover.
        assert_eq!(calendar_fields(CUTOVER_DAY), (1582, 10, 15));
        assert_eq!(calendar_fields(CUTOVER_DAY - 1), (1582, 10, 4));
        assert!(is_leap_year(1500) && !is_leap_year(1900) && is_leap_year(2000));
        assert!(is_leap_year(-1) && !is_leap_year(-2));
    }

    #[test]
    fn telemetry_rows_keep_pinned_positions() {
        let mut g = TelemetryGroup::new(7);
        g.add_column(100, -1).unwrap();
        for id in [1, 2, 3] {
            g.add_row(id, -1).unwrap();
        }
        g.set_row_pinned(0, true).unwrap();
        // Inserting at the head shifts the pinned row back into slot 0.
        assert_eq!(g.add_row(9, 0).unwrap(), 1);
        assert_eq!(g.rows, [1, 9, 2, 3]);
        g.set_grid_value(2, 0, Some(5)).unwrap();
        assert_eq!(g.grid_value(2, 0).unwrap(), Some(5));
        g.remove_row(1).unwrap();
        assert_eq!(g.rows, [1, 2, 3]);
        assert!(g.add_column(100, -1).is_err());
        assert!(g.row_id(3).is_err());
        let mut state = State::default();
        let mut out = Vec::new();
        // FULL: one group id 5, one row (id 11), one column (id 22), value 33.
        let full = [
            1, 0, 0, 0, 5, 1, 0, 0, 0, 11, 1, 0, 0, 0, 22, 0xFF, 1, 0, 0, 0, 33,
        ];
        state
            .telemetry_packet(crate::proto::server::TELEMETRY_GRID_FULL, &full, &mut out)
            .unwrap();
        let group = state.telemetry.group(0).unwrap();
        assert_eq!((group.id, group.grid_value(0, 0).unwrap()), (5, Some(33)));
        // A failing delta reports TELEMETRY_ERROR once and latches.
        state
            .telemetry_packet(
                crate::proto::server::TELEMETRY_GRID_REMOVE_GROUP,
                &[4],
                &mut out,
            )
            .unwrap();
        assert_eq!(out, [crate::proto::client::TELEMETRY_ERROR]);
        assert!(state.telemetry_error);
    }

    #[test]
    fn emoji_substitute_skips_tags() {
        let mut list = EmojiList::default();
        assert!(list.add(&units(":)"), 5, 0));
        assert!(list.add(&units(";p"), 6, 2));
        assert!(!list.add(&units("a"), 1, 0));
        let out = list.substitute(&units("hi :) <b:)> ;p"));
        assert_eq!(
            String::from_utf16(&out).unwrap(),
            "hi <sprite=5> <b:)> <sprite=6,2>"
        );
    }
}
