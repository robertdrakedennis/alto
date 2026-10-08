//! Bounded local control transport. Actions are returned to the ordinary input
//! owner, which records and applies them before acknowledging their outcome.
use anyhow::{ensure, Context, Result};
use rs910_client::client_core::input_event::{
    InputEvent, MenuChoice, LEFT_BUTTON, MIDDLE_BUTTON, RIGHT_BUTTON,
};
use rs910_client::client_core::Session;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

const CONTROL_VARIABLE: &str = "CLIENT910_CONTROL";
const SOCKET_MODE: u32 = 0o600;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const READ_CHUNK_BYTES: usize = 4096;
const MAX_READ_BYTES_PER_POLL: usize = 32 * 1024;
const MAX_REQUESTS_PER_POLL: usize = 4;
const MAX_QUERY_ENTRIES: usize = 128;
const DEFAULT_SCAN_LIMIT: usize = 64;
const MAX_LAYOUT_TEMPLATES: usize = 4096;
const MAX_ANCESTORS: usize = 64;
const MAX_ACTION_AGE_CYCLES: i32 = 100;
const MAX_TEXT_BYTES: usize = 256;
const MAX_WHEEL_DELTA: i32 = 200;
const COMPONENT_GROUP_SHIFT: u32 = 16; // not a content id: packed group bit width
const FIRST_OPERATION: usize = 1;
const NO_COMPONENT: i32 = -1;
const FIRST_ROUTE_TILE: usize = 0;
const FINE_UNITS_PER_TILE: f32 = 512.;
const MAX_COMPONENT_OPERATIONS: usize = 10;
const MAX_COMPONENT_PARAMETERS: usize = 16;
const PAUSE_OPERATION: usize = 0;
const TERRAIN_PLANES: i32 = 4;
const NEWLINE: u8 = b'\n';
const MAX_GROUND_SCAN_ROWS: usize = 4096;
const UNIQUE_GROUND_ITEM_ROW: usize = 1;
const GROUND_TILE_FIELDS: usize = 3;
const ZONE_TILE_BITS: u32 = 14;
const ZONE_TILE_MASK: i64 = (1_i64 << ZONE_TILE_BITS) - 1;
const ZONE_PLANE_SHIFT: u32 = ZONE_TILE_BITS * 2;
const ZONE_PLANE_MASK: i64 = TERRAIN_PLANES as i64 - 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MapStamp {
    pub session_instance: u64,
    pub connection_generation: u64,
    pub terrain_generation: u64,
    pub base_x: i32,
    pub base_z: i32,
    pub width: i32,
    pub height: i32,
    pub level: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentTarget {
    pub parent: i32,
    #[serde(default = "static_child")]
    pub child: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileTarget {
    pub x: i32,
    pub z: i32,
    pub level: i32,
}

const fn static_child() -> i32 {
    NO_COMPONENT
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotQuery {
    #[serde(default)]
    pub varps: Vec<i32>,
    #[serde(default)]
    pub varbits: Vec<i32>,
    #[serde(default)]
    pub client_varbits: Vec<i32>,
    #[serde(default)]
    pub component_parameters: Vec<i32>,
    #[serde(default)]
    pub inventories: Vec<i32>,
    #[serde(default)]
    pub components: Vec<ComponentTarget>,
    #[serde(default)]
    pub tiles: Vec<TileTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanQuery {
    pub definition: Option<u32>,
    pub name: Option<String>,
    pub level: Option<i32>,
    pub x: Option<i32>,
    pub z: Option<i32>,
    pub radius: Option<i32>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "scan_limit")]
    pub limit: usize,
}

const fn scan_limit() -> usize {
    DEFAULT_SCAN_LIMIT
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Walk {
        x: i32,
        z: i32,
        run: bool,
    },
    Loc {
        definition: u32,
        x: i32,
        z: i32,
        level: i32,
        shape: i32,
        angle: i32,
        operation: String,
    },
    Npc {
        index: usize,
        definition: u32,
        update_serial: i32,
        operation: String,
    },
    Object {
        definition: u32,
        x: i32,
        z: i32,
        level: i32,
        stack_index: usize,
        count: i32,
        object_revision: u64,
        operation: String,
    },
    Ui {
        target: ComponentTarget,
        serial: u64,
        operation: usize,
        #[serde(default)]
        expected_object: Option<i32>,
        #[serde(default)]
        expected_operation: Option<String>,
    },
    PointerMove {
        x: i32,
        y: i32,
    },
    PointerButton {
        x: i32,
        y: i32,
        button: PointerButton,
        pressed: bool,
    },
    Key {
        code: i32,
        pressed: bool,
        text: Option<String>,
    },
    Wheel {
        delta: i32,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Snapshot {
        #[serde(default)]
        query: SnapshotQuery,
    },
    ScanLocs {
        query: ScanQuery,
    },
    ScanNpcs {
        query: ScanQuery,
    },
    ScanObjects {
        query: ScanQuery,
    },
    ScanComponents {
        parent: ComponentTarget,
        #[serde(default)]
        component_parameters: Vec<i32>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "scan_limit")]
        limit: usize,
    },
    Action {
        map: MapStamp,
        observed_cycle: i32,
        action: Action,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u64,
    request: Command,
}

/// Canonical input is recorded and applied by the app's retained input owner.
#[derive(Clone, Debug)]
pub struct PendingAction {
    pub id: u64,
    pub map: MapStamp,
    pub event: InputEvent,
    pub provenance: &'static str,
}

#[derive(Clone, Debug)]
pub enum ActionOutcome {
    Applied,
    Refused(String),
}

#[derive(Default)]
struct Lines {
    bytes: Vec<u8>,
}

impl Lines {
    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.bytes.len().saturating_add(bytes.len()) <= MAX_REQUEST_BYTES,
            "request buffer exceeds bound"
        );
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        let end = self.bytes.iter().position(|byte| *byte == NEWLINE)?;
        let mut line: Vec<_> = self.bytes.drain(..=end).collect();
        line.pop();
        Some(line)
    }
}

struct Peer {
    stream: UnixStream,
    lines: Lines,
    output: VecDeque<u8>,
    read_closed: bool,
}

/// Exactly one local peer and one action in flight. No worker or socket blocks
/// the logic thread; transport exhaustion disconnects that peer explicitly.
pub struct LiveControl {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    peer: Option<Peer>,
    last_request: Option<u64>,
    pending: Option<u64>,
}

impl LiveControl {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os(CONTROL_VARIABLE).map(PathBuf::from) else {
            return Ok(None);
        };
        Self::bind(path).map(Some)
    }

    #[cfg(test)]
    pub(crate) fn bind_for_test(path: PathBuf) -> Result<Self> {
        Self::bind(path)
    }

    fn bind(path: PathBuf) -> Result<Self> {
        ensure!(!path.as_os_str().is_empty(), "empty control socket path");
        // Never unlink somebody else's existing path, including stale sockets.
        ensure!(
            std::fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == ErrorKind::NotFound),
            "control socket path already exists"
        );
        let listener = UnixListener::bind(&path).context("bind local control socket")?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let identity = (metadata.dev(), metadata.ino());
        let setup = (|| -> Result<()> {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))?;
            listener.set_nonblocking(true)?;
            Ok(())
        })();
        if let Err(error) = setup {
            if same_socket(&path, identity) {
                let _ = std::fs::remove_file(&path);
            }
            return Err(error);
        }
        Ok(Self {
            listener,
            path,
            identity,
            peer: None,
            last_request: None,
            pending: None,
        })
    }

    /// Read-only queries answer immediately. A semantic action is acknowledged
    /// only by `complete`, after the ordinary recorded input owner applies it.
    pub fn poll(
        &mut self,
        session: &mut Session,
        cycle: i32,
        focused: bool,
        time: i64,
    ) -> Result<Option<PendingAction>> {
        if self.peer.is_none() {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    self.peer = Some(Peer {
                        stream,
                        lines: Lines::default(),
                        output: VecDeque::new(),
                        read_closed: false,
                    });
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        }
        if !self.flush()? || self.pending.is_some() {
            return Ok(None);
        }
        let mut read_bytes = 0;
        let mut buffer = [0; READ_CHUNK_BYTES];
        while read_bytes < MAX_READ_BYTES_PER_POLL
            && self.peer.as_ref().is_some_and(|peer| !peer.read_closed)
        {
            let peer = self.peer.as_mut().expect("accepted peer");
            match peer.stream.read(&mut buffer) {
                Ok(0) => {
                    // A half-close finishes the request stream, not its
                    // queued complete requests or the response stream.
                    peer.read_closed = true;
                    break;
                }
                Ok(count) => {
                    read_bytes += count;
                    if let Err(error) = peer.lines.push(&buffer[..count]) {
                        self.answer(None, cycle, "refused", json!({"reason":error.to_string()}))?;
                        self.flush()?;
                        self.peer = None;
                        return Ok(None);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.peer = None;
                    return Ok(None);
                }
            }
        }
        for _ in 0..MAX_REQUESTS_PER_POLL {
            let Some(line) = self.peer.as_mut().and_then(|peer| peer.lines.pop()) else {
                if let Some(peer) = self.peer.as_mut() {
                    if peer.read_closed && !peer.lines.bytes.is_empty() {
                        peer.lines.bytes.clear();
                        self.answer(
                            None,
                            cycle,
                            "refused",
                            json!({"reason":"unterminated controller request"}),
                        )?;
                    }
                }
                break;
            };
            let request = match decode(&line, self.last_request) {
                Ok(request) => request,
                Err(error) => {
                    let id = serde_json::from_slice::<Value>(&line)
                        .ok()
                        .and_then(|row| row.get("id").and_then(Value::as_u64));
                    self.answer(id, cycle, "refused", json!({"reason":error.to_string()}))?;
                    continue;
                }
            };
            self.last_request = Some(request.id);
            match request.request {
                Command::Action {
                    map,
                    observed_cycle,
                    action,
                } => {
                    match resolve_action(session, cycle, focused, time, observed_cycle, map, action)
                    {
                        Ok(mut action) => {
                            action.id = request.id;
                            self.pending = Some(request.id);
                            self.flush()?;
                            return Ok(Some(action));
                        }
                        Err(error) => self.answer(
                            Some(request.id),
                            cycle,
                            "refused",
                            json!({"reason":error.to_string()}),
                        )?,
                    }
                }
                command => match query(session, cycle, focused, command) {
                    Ok(data) => self.answer(Some(request.id), cycle, "observed", data)?,
                    Err(error) => self.answer(
                        Some(request.id),
                        cycle,
                        "refused",
                        json!({"reason":error.to_string()}),
                    )?,
                },
            }
        }
        if self.flush()?
            && self.peer.as_ref().is_some_and(|peer| {
                peer.read_closed && peer.lines.bytes.is_empty() && peer.output.is_empty()
            })
        {
            self.peer = None;
        }
        Ok(None)
    }

    pub fn complete(
        &mut self,
        action: &PendingAction,
        cycle: i32,
        outcome: ActionOutcome,
    ) -> Result<()> {
        ensure!(
            self.pending == Some(action.id),
            "no matching pending control action"
        );
        self.pending = None;
        let (status, reason) = match outcome {
            ActionOutcome::Applied => ("accepted", None),
            ActionOutcome::Refused(reason) => ("refused", Some(reason)),
        };
        self.answer(Some(action.id), cycle, status, json!({"provenance":action.provenance,"map":action.map,"reason":reason,"meaning":"input application only; observe later gameplay state"}))?;
        self.flush()?;
        Ok(())
    }

    fn answer(&mut self, id: Option<u64>, cycle: i32, status: &str, data: Value) -> Result<()> {
        let Some(peer) = &mut self.peer else {
            return Ok(());
        };
        let mut bytes =
            serde_json::to_vec(&json!({"id":id,"cycle":cycle,"status":status,"data":data}))?;
        bytes.push(NEWLINE);
        if peer.output.len().saturating_add(bytes.len()) > MAX_RESPONSE_BYTES {
            self.peer = None;
            self.pending = None;
            return Ok(());
        }
        peer.output.extend(bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<bool> {
        let Some(peer) = &mut self.peer else {
            return Ok(false);
        };
        while !peer.output.is_empty() {
            let (bytes, _) = peer.output.as_slices();
            match peer.stream.write(bytes) {
                Ok(0) => {
                    self.peer = None;
                    return Ok(false);
                }
                Ok(count) => {
                    peer.output.drain(..count);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.peer = None;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

impl Drop for LiveControl {
    fn drop(&mut self) {
        if same_socket(&self.path, self.identity) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn same_socket(path: &std::path::Path, identity: (u64, u64)) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == identity)
}

fn decode(bytes: &[u8], prior: Option<u64>) -> Result<Request> {
    let request: Request = serde_json::from_slice(bytes).context("malformed control request")?;
    ensure!(
        prior.is_none_or(|id| request.id > id),
        "request id must increase; duplicate actions are not replayed"
    );
    Ok(request)
}

fn map_stamp(session: &Session) -> Result<MapStamp> {
    let game = session.game.as_ref().context("not in a game")?;
    ensure!(
        game.runtime.map_request.is_none()
            && game.runtime.terrain.is_some()
            && game.runtime.feed.state.initialized
            && game.runtime.feed.blocked.is_none()
            && !session.polling_dead,
        "map or connection is not ready"
    );
    let map = &game.runtime.map;
    Ok(MapStamp {
        session_instance: session.instance_id,
        connection_generation: session.reconnect_generation,
        terrain_generation: game.runtime.terrain_generation,
        base_x: map.base_x,
        base_z: map.base_z,
        width: map.width,
        height: map.height,
        level: game.runtime.feed.state.players.current_level,
    })
}

fn check_stamp(actual: &MapStamp, expected: &MapStamp, cycle: i32, observed: i32) -> Result<()> {
    ensure!(actual == expected, "stale installed map");
    let age = cycle
        .checked_sub(observed)
        .context("invalid observed cycle")?;
    ensure!(
        (0..=MAX_ACTION_AGE_CYCLES).contains(&age),
        "stale or future observation"
    );
    Ok(())
}

fn variable(session: &Session, bit: bool, id: i32) -> Option<i32> {
    let game = session.game.as_ref()?;
    let vars = game.runtime.feed.state.varps.as_ref()?;
    if bit {
        vars.get_bit(&game.inputs.bits.get(id, false).ok()?).ok()
    } else {
        vars.get(id).ok()
    }
}

fn client_bit(session: &Session, id: i32) -> Option<i32> {
    let game = session.game.as_ref()?;
    let bit = game.inputs.bits.get(id, false).ok()?;
    let base = bit.binding.as_ref()?;
    if base.domain != native910::vars::VarScope::Client as u8 {
        return None;
    }
    let rs910_ui::ui_vars::Value::Int(value) = game.ui_variables.client.get(base).ok()? else {
        return None;
    };
    bit.get(value).ok()
}

fn resolve_loc(session: &Session, id: u32) -> Option<&rs910_config::config::Loc> {
    let store = session.ui.engine.configs.locs.as_ref()?;
    let base = store.get(id)?;
    if !base.has_multiloc || base.multiloc.is_empty() {
        Some(base)
    } else {
        store.get(base.multi_loc(&|bit, id| variable(session, bit, id))?)
    }
}

fn resolve_npc(session: &Session, id: u32) -> Option<&rs910_config::config::Npc> {
    let store = session.ui.engine.configs.npcs.as_ref()?;
    let base = store.get(id)?;
    if base.multinpc.is_empty() {
        Some(base)
    } else {
        store.get(base.multi_npc(&|bit, id| variable(session, bit, id))?)
    }
}

fn component(session: &Session, target: &ComponentTarget) -> Option<rs910_ui::ui_components::Ref> {
    session
        .ui
        .store
        .loaded(target.parent, target.child)
        .ok()
        .flatten()
}

fn validate_component_parameters(parameters: &[i32]) -> Result<()> {
    ensure!(
        parameters.len() <= MAX_COMPONENT_PARAMETERS,
        "component parameter query exceeds bound"
    );
    let unique: BTreeSet<_> = parameters.iter().copied().collect();
    ensure!(
        unique.len() == parameters.len() && parameters.iter().all(|id| *id >= 0),
        "component parameter query has negative or duplicate identities"
    );
    Ok(())
}

fn observed_parameters(
    node: &rs910_ui::ui_components::Component,
    parameters: &[i32],
) -> Vec<Value> {
    use rs910_ui::ui_components::Arg;
    parameters.iter().map(|id| {
        let retained = node.params.as_ref().and_then(|entries|
            entries.iter().find(|(identity, _)| *identity == i64::from(*id)));
        let value = retained.map(|(_, value)| match value {
            Arg::Int(value) => json!({"kind":"int","value":value}),
            Arg::Long(value) => json!({"kind":"long","decimal":value.to_string()}),
            Arg::String(value) => json!({"kind":"string","value":bounded_text(value),"truncated":value.len()>MAX_TEXT_BYTES}),
            Arg::Null => json!({"kind":"null"}),
        });
        json!({"id":id,"present":retained.is_some(),"value":value})
    }).collect()
}

fn observed_component(session: &Session, target: &ComponentTarget, parameters: &[i32]) -> Value {
    let value = component(session, target).map(|node| {
        let visible = rooted_visible(session, &node);
        let mask = session.ui.state.layout.active_mask(&node);
        let node = node.borrow();
        json!({"serial":node.particle_serial,"rooted_visible":visible,"hidden":node.f.hide,"layer":node.f.layer,"local_rect":[node.f.x,node.f.y,node.f.width,node.f.height],"model":node.f.model,"model_kind":node.f.modelkind,"object":node.f.invobject,"text":node.f.text.as_ref().map(|text| bounded_text(text)),"text_truncated":node.f.text.as_ref().is_some_and(|text|text.len()>MAX_TEXT_BYTES),"ops":node.ops.as_ref().map(|ops| ops.iter().take(MAX_COMPONENT_OPERATIONS).map(|op| op.as_ref().map(|text| bounded_text(text))).collect::<Vec<_>>()),"active_mask":mask,"onop":node.hooks.contains_key("onop"),"parameters":observed_parameters(&node,parameters)})
    });
    json!({"target":target,"value":value})
}

/// Direct retained relationships, including static layers and mounted roots.
/// This observes existing banks; it never opens an interface or creates nodes.
fn loaded_component_children(
    session: &Session,
    parent: &ComponentTarget,
    parameters: &[i32],
) -> Result<Vec<Value>> {
    validate_component_parameters(parameters)?;
    let node = component(session, parent).context("component parent is not installed")?;
    let mut candidates = Vec::new();
    if let Some(children) = node.borrow().children.clone() {
        ensure!(
            children.borrow().len() <= MAX_LAYOUT_TEMPLATES,
            "component child bank exceeds bound"
        );
        candidates.extend(
            children
                .borrow()
                .iter()
                .flatten()
                .cloned()
                .map(|child| (child, "runtime")),
        );
    }
    if parent.child == NO_COMPONENT {
        let group = parent.parent >> COMPONENT_GROUP_SHIFT;
        if let Some(interface) = session.ui.store.interfaces.get(&group) {
            let components = interface.borrow().components.clone();
            ensure!(
                components.borrow().len() <= MAX_LAYOUT_TEMPLATES,
                "static component bank exceeds bound"
            );
            candidates.extend(
                components
                    .borrow()
                    .iter()
                    .flatten()
                    .filter(|child| {
                        let child = child.borrow();
                        child.f.layer == parent.parent && child.runtime_parent().is_none()
                    })
                    .cloned()
                    .map(|child| (child, "static")),
            );
        }
        for sub in session.ui.state.life.subs.ordered() {
            let sub = sub.borrow();
            if sub.parent != parent.parent {
                continue;
            }
            let Some(interface) = session.ui.store.interfaces.get(&sub.id) else {
                continue;
            };
            let components = interface.borrow().components.clone();
            ensure!(
                components.borrow().len() <= MAX_LAYOUT_TEMPLATES,
                "mounted component bank exceeds bound"
            );
            candidates.extend(
                components
                    .borrow()
                    .iter()
                    .flatten()
                    .filter(|child| {
                        let child = child.borrow();
                        child.f.layer == NO_COMPONENT && child.runtime_parent().is_none()
                    })
                    .cloned()
                    .map(|child| (child, "mounted")),
            );
        }
    }
    ensure!(
        candidates.len() <= MAX_LAYOUT_TEMPLATES,
        "loaded child relationships exceed bound"
    );
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for (child, relation) in candidates {
        let target = {
            let child = child.borrow();
            ComponentTarget {
                parent: child.f.parentlayer,
                child: child.f.id,
            }
        };
        if !seen.insert((target.parent, target.child)) {
            continue;
        }
        ensure!(
            component(session, &target)
                .is_some_and(|installed| std::rc::Rc::ptr_eq(&installed, &child)),
            "retained child is unavailable in its loaded namespace"
        );
        let mut row = observed_component(session, &target, parameters);
        row.as_object_mut()
            .context("component observation object")?
            .insert("relation".into(), json!(relation));
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
pub(crate) fn observed_children_for_test(
    session: &Session,
    parent: i32,
    child: i32,
) -> Result<Vec<Value>> {
    loaded_component_children(session, &ComponentTarget { parent, child }, &[])
}

#[cfg(test)]
pub(crate) fn observed_children_parameters_for_test(
    session: &Session,
    parent: i32,
    child: i32,
    parameters: &[i32],
) -> Result<Vec<Value>> {
    loaded_component_children(session, &ComponentTarget { parent, child }, parameters)
}

#[cfg(test)]
pub(crate) fn observed_snapshot_for_test(
    session: &Session,
    cycle: i32,
    snapshot: SnapshotQuery,
) -> Result<Value> {
    query(session, cycle, true, Command::Snapshot { query: snapshot })
}

#[cfg(test)]
pub(crate) fn observed_client_bit_for_test(session: &Session, id: i32) -> Option<i32> {
    client_bit(session, id)
}

fn validate_component(
    session: &Session,
    target: &ComponentTarget,
    serial: u64,
    operation: usize,
    expected_object: Option<i32>,
    expected_operation: Option<&str>,
) -> Result<()> {
    let node = component(session, target).context("component is not installed")?;
    ensure!(
        node.borrow().particle_serial == serial && rooted_visible(session, &node),
        "stale, hidden or unrooted component"
    );
    ensure!(
        operation <= MAX_COMPONENT_OPERATIONS,
        "invalid component operation"
    );
    ensure!(
        expected_object.is_none_or(|object| node.borrow().f.invobject == object),
        "component object changed after observation"
    );
    ensure!(
        expected_operation.is_none_or(|label| !label.is_empty() && label.len() <= MAX_TEXT_BYTES),
        "invalid expected component operation"
    );
    let mask = session.ui.state.layout.active_mask(&node);
    if operation == PAUSE_OPERATION {
        ensure!(
            expected_operation.is_none(),
            "Continue uses its native pause mask"
        );
        ensure!(
            rs910_ui::ui_minimenu::mask_pausebutton(mask)
                && session.ui.state.life.pressed_continue.is_none(),
            "component Continue is unavailable"
        );
    } else {
        let mut menu = rs910_ui::ui_minimenu::MiniMenu::default();
        menu.add_component_options(&node, mask);
        ensure!(
            menu.entries.iter().any(|slot| {
                let entry = menu.entry(*slot);
                entry.entity_id == operation as i64
                    && expected_operation.is_none_or(|label| entry.op.eq_ignore_ascii_case(label))
            }),
            "component operation has no native menu entry"
        );
    }
    Ok(())
}

fn rooted_visible(session: &Session, target: &rs910_ui::ui_components::Ref) -> bool {
    let mut current = target.clone();
    for _ in 0..MAX_ANCESTORS {
        let (hidden, layer, group, parent) = {
            let node = current.borrow();
            (
                node.runtime_entry_hidden().unwrap_or(node.f.hide),
                node.f.layer,
                node.f.parentlayer >> COMPONENT_GROUP_SHIFT,
                node.runtime_parent(),
            )
        };
        if hidden {
            return false;
        }
        if let Some(parent) = parent {
            current = parent;
            continue;
        }
        if layer != NO_COMPONENT {
            let Some(parent) = component(
                session,
                &ComponentTarget {
                    parent: layer,
                    child: NO_COMPONENT,
                },
            ) else {
                return false;
            };
            current = parent;
            continue;
        }
        if group == session.ui.state.life.top {
            return true;
        }
        let mounted = session.ui.state.life.subs.ordered().find_map(|sub| {
            let sub = sub.borrow();
            (sub.id == group).then_some(sub.parent)
        });
        let Some(parent) = mounted.and_then(|parent| {
            component(
                session,
                &ComponentTarget {
                    parent,
                    child: NO_COMPONENT,
                },
            )
        }) else {
            return false;
        };
        current = parent;
    }
    false
}

fn resolve_action(
    session: &mut Session,
    cycle: i32,
    focused: bool,
    time: i64,
    observed: i32,
    expected: MapStamp,
    action: Action,
) -> Result<PendingAction> {
    check_stamp(&map_stamp(session)?, &expected, cycle, observed)?;
    ensure!(focused, "window is not focused");
    let game = session.game.as_ref().context("not in a game")?;
    let (event, provenance) = match &action {
        Action::Walk { x, z, run } => {
            let tile = local_tile(&expected, *x, *z)?;
            let staff_teleport = session.ui.engine.account.staff_mod_level > 0
                && session
                    .ui
                    .engine
                    .configs
                    .minimenu
                    .as_ref()
                    .is_some_and(|defaults| {
                        rs910_config::ui_defaults::key_binding_held(
                            &defaults.shiftteleport,
                            |key| session.ui.keyboard.held(key),
                        )
                    });
            ensure!(
                !staff_teleport,
                "release the staff teleport modifier before walking"
            );
            ensure!(
                !run,
                "explicit run requires ordinary retained modifier input"
            );
            ensure!(
                !session.ui.state.interaction.target.active,
                "coordinate target mode is active"
            );
            (
                menu_event(session.ui.walk_scene_option(tile), session.ui.input.mouse),
                "coordinate-menu",
            )
        }
        Action::Loc {
            definition,
            x,
            z,
            level,
            shape,
            angle,
            operation: label,
        } => {
            ensure!(*level == expected.level, "loc is on another plane");
            let tile = local_tile(&expected, *x, *z)?;
            let installed = game
                .scene_locs
                .iter()
                .find_map(|(&(installed_level, _, installed_x, installed_z), loc)| {
                    (installed_level == *level
                        && [installed_x, installed_z] == tile
                        && loc.shape == *shape)
                        .then_some(loc)
                })
                .context("loc is not installed")?;
            let native = resolve_loc(session, u32::try_from(installed.id)?)
                .context("loc cache definition unavailable")?;
            ensure!(
                native.id == *definition && installed.angle == *angle,
                "stale loc identity or orientation"
            );
            find_operation(native.ops_for(game.allow_members), label)?;
            let target = rs910_ui::ui_scene_options::LocTarget {
                id: installed.id,
                shape: installed.shape,
                angle: installed.angle,
                tile,
            };
            let options = session.ui.loc_scene_options(
                target,
                true,
                game.runtime.feed.state.varps.as_ref(),
                Some(&game.inputs.bits),
            );
            (
                menu_event(select_option(options, label)?, session.ui.input.mouse),
                "native-loc-menu",
            )
        }
        Action::Npc {
            index,
            definition,
            update_serial,
            operation: label,
        } => {
            ensure!(
                game.runtime.feed.state.npcs.slots.contains(index),
                "NPC is not in the live roster"
            );
            let actor = game
                .runtime
                .feed
                .state
                .npcs
                .entities
                .get(index)
                .context("NPC is absent")?;
            let native = resolve_npc(session, u32::try_from(actor.type_id)?)
                .context("NPC cache definition unavailable")?;
            ensure!(
                native.active
                    && native.id == *definition
                    && actor.update_serial == *update_serial
                    && actor.path.level == expected.level,
                "stale or unavailable NPC"
            );
            let selected = find_operation(native.ops_for(game.allow_members), label)?;
            ensure!(
                actor.op_mask & (1 << (selected - FIRST_OPERATION)) == 0,
                "NPC operation is disabled by server mask"
            );
            let mut scene = rs910_ui::ui_cam2::SceneInput::new_with_objects(
                &game.runtime.map,
                &game.runtime.feed.state.players,
                Some(&game.runtime.feed.state.zones.objects),
                game.runtime.terrain.as_ref(),
                game.runtime.terrain_generation,
            );
            scene.npcs = Some(&game.runtime.feed.state.npcs);
            scene.locations = Some(&game.scene_locs);
            let options = session.ui.npc_scene_options(
                &scene,
                *index,
                false,
                game.runtime.feed.state.varps.as_ref(),
                Some(&game.inputs.bits),
            );
            (
                menu_event(select_option(options, label)?, session.ui.input.mouse),
                "native-npc-menu",
            )
        }
        Action::Object {
            definition,
            x,
            z,
            level,
            stack_index,
            count,
            object_revision,
            operation: label,
        } => {
            ensure!(*level == expected.level, "ground item is on another plane");
            let tile = local_tile(&expected, *x, *z)?;
            let objects = &game.runtime.feed.state.zones.objects;
            ensure!(
                objects.revision == *object_revision,
                "stale ground-object revision"
            );
            let stack = objects
                .stacks
                .get(&ground_key(*level, *x, *z)?)
                .context("ground stack is not installed")?;
            let ground = stack
                .get(*stack_index)
                .context("ground stack index is absent")?;
            ensure!(
                u32::try_from(ground.id)? == *definition && ground.count == *count,
                "stale ground item definition or count"
            );
            ensure!(
                stack.iter().filter(|other| other.id == ground.id).count()
                    == UNIQUE_GROUND_ITEM_ROW,
                "native ground operation cannot distinguish duplicate item definitions on one tile"
            );
            ensure!(
                !label.is_empty() && label.len() <= MAX_TEXT_BYTES,
                "invalid operation name"
            );
            let scene = rs910_ui::ui_cam2::SceneInput::new_with_objects(
                &game.runtime.map,
                &game.runtime.feed.state.players,
                Some(objects),
                game.runtime.terrain.as_ref(),
                game.runtime.terrain_generation,
            );
            let options = session
                .ui
                .object_scene_options(&scene, *level, tile)
                .into_iter()
                .filter(|option| {
                    option.entity_id == i64::from(*definition)
                        && usize::try_from(option.sub_id).ok() == Some(*stack_index)
                })
                .collect();
            (
                menu_event(select_option(options, label)?, session.ui.input.mouse),
                "native-ground-menu",
            )
        }
        Action::Ui {
            target,
            serial,
            operation: selected,
            expected_object,
            expected_operation,
        } => {
            validate_component(
                session,
                target,
                *serial,
                *selected,
                *expected_object,
                expected_operation.as_deref(),
            )?;
            (
                InputEvent::Component {
                    parent: target.parent,
                    child: target.child,
                    operation: i32::try_from(*selected)?,
                },
                "native-component",
            )
        }
        Action::PointerMove { x, y } => {
            validate_pointer(session, *x, *y)?;
            (
                InputEvent::Move {
                    position: [*x, *y],
                    time,
                },
                "pointer",
            )
        }
        Action::PointerButton {
            x,
            y,
            button,
            pressed,
        } => {
            validate_pointer(session, *x, *y)?;
            ensure!(
                session.ui.engine.platform.pending_mouse == [*x, *y],
                "move the retained pointer before sending a button event"
            );
            let action = match button {
                PointerButton::Left => LEFT_BUTTON,
                PointerButton::Middle => MIDDLE_BUTTON,
                PointerButton::Right => RIGHT_BUTTON,
            };
            (
                InputEvent::Button {
                    action,
                    pressed: *pressed,
                    time,
                },
                "pointer",
            )
        }
        Action::Key {
            code,
            pressed,
            text,
        } => {
            let typed_only =
                *code == 0 && *pressed && text.as_ref().is_some_and(|text| !text.is_empty());
            ensure!(
                typed_only || session.ui.keyboard.supports_awt_code(*code),
                "invalid native AWT key code"
            );
            ensure!(
                text.as_ref()
                    .is_none_or(|text| text.len() <= MAX_TEXT_BYTES),
                "typed text exceeds bound"
            );
            (
                InputEvent::Key {
                    code: *code,
                    pressed: *pressed,
                    text: text.clone(),
                    time,
                },
                "keyboard",
            )
        }
        Action::Wheel { delta } => {
            ensure!(
                delta.unsigned_abs() <= MAX_WHEEL_DELTA as u32,
                "wheel delta exceeds bound"
            );
            (InputEvent::Wheel { delta: *delta }, "wheel")
        }
    };
    Ok(PendingAction {
        id: 0,
        map: expected,
        event,
        provenance,
    })
}

fn local_tile(map: &MapStamp, x: i32, z: i32) -> Result<[i32; 2]> {
    let x = x.checked_sub(map.base_x).context("invalid tile x")?;
    let z = z.checked_sub(map.base_z).context("invalid tile z")?;
    ensure!(
        (0..map.width).contains(&x) && (0..map.height).contains(&z),
        "target outside installed map"
    );
    Ok([x, z])
}

fn ground_key(level: i32, x: i32, z: i32) -> Result<i64> {
    ensure!(
        (0..TERRAIN_PLANES).contains(&level)
            && (0..=ZONE_TILE_MASK).contains(&i64::from(x))
            && (0..=ZONE_TILE_MASK).contains(&i64::from(z)),
        "ground tile is outside the native zone encoding"
    );
    Ok(i64::from(level) << ZONE_PLANE_SHIFT | i64::from(z) << ZONE_TILE_BITS | i64::from(x))
}

fn ground_tile(key: i64) -> [i32; GROUND_TILE_FIELDS] {
    [
        (key & ZONE_TILE_MASK) as i32,
        ((key >> ZONE_TILE_BITS) & ZONE_TILE_MASK) as i32,
        ((key >> ZONE_PLANE_SHIFT) & ZONE_PLANE_MASK) as i32,
    ]
}

fn validate_pointer(session: &Session, x: i32, y: i32) -> Result<()> {
    let [width, height] = session.ui.state.layout.canvas;
    ensure!(
        (0..width).contains(&x) && (0..height).contains(&y),
        "pointer outside current native canvas"
    );
    Ok(())
}

fn select_option(
    options: Vec<rs910_ui::ui_scene_options::SceneOption>,
    label: &str,
) -> Result<rs910_ui::ui_scene_options::SceneOption> {
    let mut matches = options
        .into_iter()
        .filter(|option| option.enabled && option.op.eq_ignore_ascii_case(label));
    let selected = matches
        .next()
        .context("operation has no current native scene menu entry")?;
    ensure!(
        matches.next().is_none(),
        "native menu operation is ambiguous"
    );
    Ok(selected)
}

fn menu_event(option: rs910_ui::ui_scene_options::SceneOption, position: [i32; 2]) -> InputEvent {
    InputEvent::Menu {
        choice: MenuChoice {
            op: option.op,
            target: option.target,
            cursor: option.cursor,
            action: option.action,
            obj_id: option.obj_id,
            entity_id: option.entity_id,
            tile_x: option.tile[0],
            tile_z: option.tile[1],
            enabled: option.enabled,
            has_arrow: option.has_arrow,
            sub_id: option.sub_id,
            force_submenu: option.force_submenu,
            detail: option.detail,
        },
        position,
        from_menu: false,
    }
}

fn find_operation(ops: [Option<&str>; 5], requested: &str) -> Result<usize> {
    ensure!(
        !requested.is_empty() && requested.len() <= MAX_TEXT_BYTES,
        "invalid operation name"
    );
    let mut found = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.is_some_and(|op| op.eq_ignore_ascii_case(requested)));
    let index = found.next().context("cache operation unavailable")?.0;
    ensure!(found.next().is_none(), "ambiguous cache operation");
    Ok(index + FIRST_OPERATION)
}

fn validate_scan(scan: &ScanQuery) -> Result<()> {
    ensure!(
        (1..=MAX_QUERY_ENTRIES).contains(&scan.limit),
        "scan limit outside bound"
    );
    ensure!(
        scan.name
            .as_ref()
            .is_none_or(|name| name.len() <= MAX_TEXT_BYTES),
        "scan name exceeds bound"
    );
    ensure!(
        scan.x.is_some() == scan.z.is_some(),
        "scan center requires x and z"
    );
    ensure!(
        scan.radius
            .is_none_or(|radius| radius >= 0 && radius <= MAX_QUERY_ENTRIES as i32)
            && (scan.radius.is_none() || scan.x.is_some()),
        "invalid scan radius"
    );
    Ok(())
}

fn matches(scan: &ScanQuery, id: u32, name: &str, level: i32, x: i32, z: i32) -> bool {
    scan.definition.is_none_or(|wanted| wanted == id)
        && scan.level.is_none_or(|wanted| wanted == level)
        && scan
            .name
            .as_ref()
            .is_none_or(|wanted| name.to_lowercase().contains(&wanted.to_lowercase()))
        && scan.radius.is_none_or(|radius| {
            (i64::from(x) - i64::from(scan.x.unwrap_or(x)))
                .abs()
                .max((i64::from(z) - i64::from(scan.z.unwrap_or(z))).abs())
                <= i64::from(radius)
        })
}

fn page(rows: Vec<Value>, offset: usize, limit: usize) -> Value {
    let total = rows.len();
    let entries: Vec<_> = rows.into_iter().skip(offset).take(limit).collect();
    let next = offset.saturating_add(entries.len());
    json!({"entries":entries,"total":total,"next_offset":(next < total).then_some(next)})
}

fn animation_node_snapshot(node: &crate::entities910::animation_state::Node) -> Value {
    json!({
        "sequence": node.id(),
        "skeletal": node.sequence.as_ref().map(|sequence| sequence.skeletal),
        "time": node.time,
        "frame": node.frame,
        "delay": node.delay,
        "finished": node.finished,
        "skeletal_range": node.skeletal_range,
        "moving_priority": node.sequence.as_ref().map(|sequence| sequence.moving),
        "stationary_priority": node.sequence.as_ref().map(|sequence| sequence.stationary)
    })
}

fn query(session: &Session, cycle: i32, focused: bool, command: Command) -> Result<Value> {
    let map = match map_stamp(session) {
        Ok(map) => map,
        Err(error) if matches!(&command, Command::Snapshot { .. }) => {
            return Ok(
                json!({"cycle":cycle,"focused":focused,"state":session.machine.state,"ready":false,"map":null,"reason":error.to_string(),"meaning":"connection or map has not installed; no gameplay action is admitted"}),
            )
        }
        Err(error) => return Err(error),
    };
    let game = session.game.as_ref().context("not in a game")?;
    match command {
        Command::Snapshot { query } => {
            ensure!(
                [
                    query.varps.len(),
                    query.varbits.len(),
                    query.client_varbits.len(),
                    query.inventories.len(),
                    query.components.len(),
                    query.tiles.len()
                ]
                .into_iter()
                .all(|length| length <= MAX_QUERY_ENTRIES),
                "snapshot query exceeds bound"
            );
            validate_component_parameters(&query.component_parameters)?;
            let local = game
                .runtime
                .feed
                .state
                .players
                .players
                .get(game.runtime.map.local)
                .and_then(Option::as_ref);
            let player = local.map(|player| json!({"x":map.base_x+(player.fine_x/FINE_UNITS_PER_TILE).floor() as i32,"z":map.base_z+(player.fine_z/FINE_UNITS_PER_TILE).floor() as i32,"route_head":{"x":map.base_x+player.x[FIRST_ROUTE_TILE],"z":map.base_z+player.z[FIRST_ROUTE_TILE]},"level":player.level,"fine_x":player.fine_x,"fine_z":player.fine_z,"route_length":player.route_length,"target":player.target,"hits":player.combat.as_ref().map(|combat| &combat.hits),"animation":{"main":animation_node_snapshot(&player.animation.main),"spots":player.animation.spots.iter().enumerate().take(MAX_QUERY_ENTRIES).map(|(slot,spot)| json!({"slot":slot,"effect":spot.id,"request_delay":spot.delay,"looping":spot.looping,"cancels_on_move":spot.cancels_on_move,"node":animation_node_snapshot(&spot.node)})).collect::<Vec<_>>(),"total_spots":player.animation.spots.len(),"spots_truncated":player.animation.spots.len()>MAX_QUERY_ENTRIES}}));
            let components: Vec<_> = query
                .components
                .iter()
                .map(|target| observed_component(session, target, &query.component_parameters))
                .collect();
            let inventories: Vec<_> = query.inventories.iter().map(|id| json!({"id":id,"value":session.ui.engine.inv_cache.inventory(*id,false).map(|inventory| json!({"items":inventory.obj_ids.iter().take(MAX_QUERY_ENTRIES).collect::<Vec<_>>(),"counts":inventory.counts.iter().take(MAX_QUERY_ENTRIES).collect::<Vec<_>>(),"total_slots":inventory.obj_ids.len(),"truncated":inventory.obj_ids.len()>MAX_QUERY_ENTRIES}))})).collect();
            let tiles: Vec<_> = query.tiles.iter().map(|target| {
                let observed = local_tile(&map,target.x,target.z).ok().and_then(|[x,z]| {
                    if !(0..TERRAIN_PLANES).contains(&target.level) { return None }
                    let terrain = game.runtime.terrain.as_ref()?;
                    let (level,x,z)=(usize::try_from(target.level).ok()?,usize::try_from(x).ok()?,usize::try_from(z).ok()?);
                    if x>=terrain.width || z>=terrain.height { return None }
                    Some(json!({"terrain_flags":terrain.tiles.get(terrain.tile(level,x,z))?.flags,"vertex_height":terrain.heights.get(terrain.point(level,x,z))?,"meaning":"installed terrain; not a route collision/standability oracle"}))
                });
                json!({"target":target,"value":observed})
            }).collect();
            let stats = game.ui_variables.stats.as_ref().map(|stats| stats.stats.iter().take(MAX_QUERY_ENTRIES).enumerate().map(|(index,stat)| json!({"index":index,"value":stat.as_ref().map(|stat|json!({"level":stat.level,"base_level":stat.xp_level,"xp":stat.xp}))})).collect::<Vec<_>>());
            let region = game.runtime.installed_region.as_ref().map(|region| {
                ensure!(region.templates.len() <= MAX_LAYOUT_TEMPLATES, "installed layout exceeds response bound");
                Ok::<Value,anyhow::Error>(json!({"chunks_x":region.chunks_x,"chunks_z":region.chunks_z,"templates":region.templates}))
            }).transpose()?;
            let mounted: Vec<_> = session
                .ui
                .state
                .life
                .subs
                .ordered()
                .take(MAX_QUERY_ENTRIES)
                .map(|sub| {
                    let sub = sub.borrow();
                    json!({"parent":sub.parent,"group":sub.id,"kind":sub.kind})
                })
                .collect();
            Ok(
                json!({"cycle":cycle,"packets_applied":game.packets_applied,"focused":focused,"state":session.machine.state,"ready":true,"map":map,"region":region,"terrain_present":game.runtime.terrain.is_some(),"player":player,"stats":stats,"pointer":{"position":session.ui.engine.platform.pending_mouse,"canvas":session.ui.state.layout.canvas},"top_interface":session.ui.state.life.top,"mounted":mounted,"varps":query.varps.iter().map(|id| json!({"id":id,"value":variable(session,false,*id)})).collect::<Vec<_>>(),"varbits":query.varbits.iter().map(|id| json!({"id":id,"value":variable(session,true,*id)})).collect::<Vec<_>>(),"client_varbits":query.client_varbits.iter().map(|id|json!({"id":id,"value":client_bit(session,*id)})).collect::<Vec<_>>(),"inventories":inventories,"components":components,"tiles":tiles,"meaning":"installed decoded state; not proof of rendering or server transaction"}),
            )
        }
        Command::ScanComponents {
            parent,
            component_parameters,
            offset,
            limit,
        } => {
            ensure!(
                (1..=MAX_QUERY_ENTRIES).contains(&limit),
                "scan limit outside bound"
            );
            let rows = loaded_component_children(session, &parent, &component_parameters)?;
            Ok(
                json!({"map":map,"cycle":cycle,"scan":page(rows,offset,limit),"meaning":"loaded runtime/static children and mounted roots; local_rect is not a rendered or clipped pointer bound"}),
            )
        }
        Command::ScanLocs { query: scan } => {
            validate_scan(&scan)?;
            let mut rows = Vec::new();
            for (&(level, layer, x, z), installed) in &game.scene_locs {
                let Some(native) = u32::try_from(installed.id)
                    .ok()
                    .and_then(|id| resolve_loc(session, id))
                else {
                    continue;
                };
                let (x, z) = (x + map.base_x, z + map.base_z);
                if matches(&scan, native.id, &native.name, level, x, z) {
                    rows.push(json!({"definition":native.id,"base_definition":installed.id,"name":native.name,"x":x,"z":z,"level":level,"layer":layer,"shape":installed.shape,"angle":installed.angle,"width":native.width,"length":native.length,"ops":native.ops_for(game.allow_members)}));
                }
            }
            Ok(json!({"map":map,"scan":page(rows,scan.offset,scan.limit)}))
        }
        Command::ScanNpcs { query: scan } => {
            validate_scan(&scan)?;
            let mut rows = Vec::new();
            for index in &game.runtime.feed.state.npcs.slots {
                let Some(actor) = game.runtime.feed.state.npcs.entities.get(index) else {
                    continue;
                };
                let Some(native) = u32::try_from(actor.type_id)
                    .ok()
                    .and_then(|id| resolve_npc(session, id))
                else {
                    continue;
                };
                let (x, z) = (
                    map.base_x + actor.path.x[FIRST_ROUTE_TILE],
                    map.base_z + actor.path.z[FIRST_ROUTE_TILE],
                );
                if matches(&scan, native.id, &native.name, actor.path.level, x, z) {
                    rows.push(json!({"index":index,"definition":native.id,"base_definition":actor.type_id,"name":native.name,"x":x,"z":z,"level":actor.path.level,"update_serial":actor.update_serial,"size":native.size,"op_mask":actor.op_mask,"ops":native.ops_for(game.allow_members),"stats":actor.stats,"stat_max":actor.stat_max,"hits":actor.path.combat.as_ref().map(|combat| &combat.hits)}));
                }
            }
            Ok(json!({"map":map,"scan":page(rows,scan.offset,scan.limit)}))
        }
        Command::ScanObjects { query: scan } => {
            validate_scan(&scan)?;
            let objects = &game.runtime.feed.state.zones.objects;
            ensure!(
                objects.stacks.len() <= MAX_GROUND_SCAN_ROWS,
                "ground tile scan exceeds work bound"
            );
            let definitions = session
                .ui
                .engine
                .configs
                .objs
                .as_ref()
                .context("ground object cache is unavailable")?;
            let mut examined = usize::default();
            let mut rows = Vec::new();
            for (&key, stack) in &objects.stacks {
                let [x, z, level] = ground_tile(key);
                ensure!(
                    key == ground_key(level, x, z)?,
                    "invalid retained ground tile key"
                );
                if local_tile(&map, x, z).is_err() {
                    continue;
                }
                for (stack_index, ground) in stack.iter().enumerate() {
                    examined += UNIQUE_GROUND_ITEM_ROW;
                    ensure!(
                        examined <= MAX_GROUND_SCAN_ROWS,
                        "ground item scan exceeds work bound"
                    );
                    let Some(native) = u32::try_from(ground.id)
                        .ok()
                        .and_then(|id| definitions.get(id))
                    else {
                        continue;
                    };
                    if matches(&scan, native.id, &native.name, level, x, z) {
                        rows.push(
                            json!({"definition":native.id,"name":native.name,"x":x,"z":z,
                            "level":level,"stack_index":stack_index,"count":ground.count,
                            "object_revision":objects.revision,"cache_ops":native.ops}),
                        );
                    }
                }
            }
            Ok(
                json!({"map":map,"object_revision":objects.revision,"scan":page(rows,scan.offset,scan.limit)}),
            )
        }
        Command::Action { .. } => anyhow::bail!("action must use the recorded input owner"),
    }
}

fn bounded_text(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.len().min(MAX_TEXT_BYTES)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ndjson_request_waits_for_complete_line_and_keeps_next_request() {
        let mut lines = Lines::default();
        lines
            .push(b"{\"id\":1,\"request\":{\"command\":\"snap")
            .unwrap();
        assert!(lines.pop().is_none());
        lines
            .push(b"shot\"}}\n{\"id\":2,\"request\":{\"command\":\"snapshot\"}}\n")
            .unwrap();
        assert_eq!(decode(&lines.pop().unwrap(), None).unwrap().id, 1);
        assert_eq!(decode(&lines.pop().unwrap(), Some(1)).unwrap().id, 2);
        assert!(lines.pop().is_none());
    }

    #[test]
    fn socket_fragments_preserve_framing_before_request_decode() {
        const SOCKET_TEST_TIMEOUT_SECONDS: u64 = 1;
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(
                SOCKET_TEST_TIMEOUT_SECONDS,
            )))
            .unwrap();
        let fragments: [&[u8]; 3] = [
            b"{\"id\":1,",
            b"\"request\":{\"command\":\"snapshot\"}}",
            b"\n",
        ];
        let mut lines = Lines::default();
        for (index, fragment) in fragments.iter().enumerate() {
            sender.write_all(fragment).unwrap();
            let mut received = vec![0; fragment.len()];
            receiver.read_exact(&mut received).unwrap();
            lines.push(&received).unwrap();
            if index + FIRST_OPERATION < fragments.len() {
                assert!(lines.pop().is_none());
            }
        }
        assert!(matches!(
            decode(&lines.pop().unwrap(), None).unwrap().request,
            Command::Snapshot { .. }
        ));
        assert!(lines.pop().is_none());
    }

    #[test]
    fn malformed_and_duplicate_action_requests_cannot_become_pending_inputs() {
        assert!(decode(b"{\"id\":1,\"request\":{\"command\":\"teleport\"}}", None).is_err());
        assert!(decode(
            b"{\"id\":1,\"request\":{\"command\":\"snapshot\"},\"extra\":true}",
            None
        )
        .is_err());
        assert!(decode(
            b"{\"id\":1,\"request\":{\"command\":\"snapshot\"}}",
            Some(1)
        )
        .is_err());
        assert!(decode(
            b"{\"id\":2,\"request\":{\"command\":\"snapshot\"}}",
            Some(1)
        )
        .is_ok());
    }

    #[test]
    fn map_replacement_and_future_observation_refuse_even_same_coordinates() {
        let prior = MapStamp {
            session_instance: 1,
            connection_generation: 0,
            terrain_generation: 1,
            base_x: 0,
            base_z: 0,
            width: 1,
            height: 1,
            level: 0,
        };
        let mut current = prior.clone();
        current.session_instance += 1;
        assert!(check_stamp(&current, &prior, 2, 1).is_err());
        current = prior.clone();
        current.terrain_generation += 1;
        assert!(check_stamp(&current, &prior, 2, 1).is_err());
        assert!(check_stamp(&prior, &prior, 1, 2).is_err());
        assert!(check_stamp(&prior, &prior, MAX_ACTION_AGE_CYCLES + 1, 0).is_err());
        assert!(check_stamp(&prior, &prior, 2, 1).is_ok());
    }

    #[test]
    fn unterminated_input_and_ambiguous_cache_operation_are_bounded() {
        let mut lines = Lines::default();
        lines.push(&vec![b'x'; MAX_REQUEST_BYTES]).unwrap();
        assert!(lines.push(b"x").is_err());
        assert!(
            find_operation([Some("Search"), Some("search"), None, None, None], "Search").is_err()
        );
        assert_eq!(
            find_operation([None, Some("Search"), None, None, None], "search").unwrap(),
            2
        );
    }
}
