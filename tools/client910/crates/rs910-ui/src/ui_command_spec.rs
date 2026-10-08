//! Per-command behaviour tables for the `ui_host_builtins` and
//! `ui_host_game` command partitions — test-audit.md fix programme
//! item 7 — plus the engine-owned commands in [`ENGINE_PARTITION`]. Every command runs as a one-instruction script through the
//! production path (`Pool::execute` → VM → `HookHost` with the game variable
//! domains → `ScriptHost` → component/engine owners) from a realistic input
//! plus the minimal client state it reads, and is compared with an
//! independent expectation:
//!
//! * recorded rows (`rec()`): the output of the original client's handler for
//!   the same input and state, recorded once into
//!   `fixtures/cs2-commands/recorded.tsv` (stacks, failure class, observed
//!   client state such as outgoing packet bytes).
//! * hand rows: values derived by hand where the handler needs a client owner
//!   the recording harness could not build (fonts, config lists, the
//!   scene/local player, the cam2 camera, the platform).
//!
//! Every case starts from the sentinels int 24301 / object "sentinel" /
//! long 24301 under its inputs, so an extra or missing pop is visible in
//! the final stacks. A case that fails in the original client must fail with
//! the same [`Fault`] class (`fixtures/cs2-commands/fault-classes.tsv` maps the
//! recorded failure names to classes); any other `Err` fails the test.
//! Successful cases are also checked against the stack contract table
//! (`native910::semantics::effect`). Known divergences are listed in
//! [`KNOWN_DIVERGENCES`]
//! (a ratchet: a new divergence or a fixed one fails the test).
#[path = "ui_command_spec/client_cases.rs"]
mod client_cases;
use client_cases::client_fields;
use client_cases::disabled_handlers;
use client_cases::emoji;
use client_cases::hook_owners;
use client_cases::packets;
use client_cases::pure_helpers;
use client_cases::telemetry;
use client_cases::toolkit_availability;
use client_cases::twitch;
#[path = "ui_command_spec/component_cases.rs"]
mod component_cases;

use component_cases::interfaces;
use component_cases::npc_customisation;
use component_cases::test_fonts;
#[path = "ui_command_spec/camera_cases.rs"]
mod camera_cases;
use camera_cases::cam2_mode;
use camera_cases::camera;
use camera_cases::local_player;
#[path = "ui_command_spec/platform_cases.rs"]
mod platform_cases;
use platform_cases::config_queries;
use platform_cases::platform;
use platform_cases::setup_launcher;
#[path = "ui_command_spec/engine_cases.rs"]
mod engine_cases;
use engine_cases::engine_partition;
use engine_cases::ENGINE_PARTITION;
#[path = "ui_command_spec/partition.rs"]
mod partition;
use partition::PARTITION;

use super::Engine;

use crate::{
    ui_components::{Active, Arg, Component, Interface, Ref, Store},
    ui_hook_host::{Domains, ScriptRun},
    ui_hooks::{Pool, Request},
    ui_vars::Variables,
};

use native910::{
    script::{CompiledScript, Counts, Instruction, Operand},
    vm::VmError,
};

use rs910_core::fault::Fault;

use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

const SENTINEL: i32 = 24301;

const SENTINEL_STR: &str = "sentinel";

const FIXTURE: &str = "fixtures/cs2-commands";

/// Interface used by component cases; `C0`/`C1` are its components.
const IFACE: i32 = 77;

const C0: i32 = IFACE << 16;

const C1: i32 = (IFACE << 16) | 1;

// ---------------------------------------------------------------------------
// Minimal client state (applied to the Rust owners here; the recording was
// made under the same directive text).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum ComField {
    Name(&'static str),
    ServerTriggers(i32),
    InvObject(i32),
    /// The default active mask: the given event mask with no key properties.
    Events(i32),
}

#[derive(Clone, Copy, Debug)]
enum Set {
    Canvas(i32, i32),
    Owner(&'static str),
    Orbit(i32, i32),
    CamState(i32),
    Follow(i32),
    Cursor(i32),
    Affiliate(i32),
    Country(i32),
    Billing(bool),
    Email(&'static str),
    State(i32),
    /// `currentPlayerUid` (the scene's local player index).
    Uid(i32),
    TopIf(i32),
    Autosetup(i32),
    /// A saved options file that holds toolkit `n`, loaded through the file
    /// codec (which reads the byte without the can-set rule).
    LoadedToolkit(i32),
    Emoji(&'static str, i32, i32),
    /// A TELEMETRY_GRID_FULL payload decoded by the production decoder.
    Telemetry(&'static [u8]),
    Spline(i32, i32),
    ObjFind(&'static [i32], i32),
    /// Interface `id` with `count` top-level components `id:0..count`.
    Iface(i32, i32),
    Com(i32, i32, ComField),
    /// The (secondary-selected) active component.
    Active(i32, i32),
    Friends(&'static [&'static str]),
    FriendsState(i32),
    ClanChat(&'static [&'static str]),
    ClanUser(&'static str, i32),
}

impl Set {
    fn apply(self, w: &mut World, secondary: bool) {
        let e = &mut w.engine;
        match self {
            Self::Canvas(width, height) => w.props.layout.canvas = [width, height],
            Self::Owner(s) => e.game_host.owner = Some(s.into()),
            Self::Orbit(p, y) => {
                e.game_host.orbit_pitch = p as f32;
                e.game_host.orbit_yaw = y as f32;
            }
            Self::CamState(v) => e.camera.cam2.camera_state = v,
            Self::Follow(v) => e.game_host.follow_height = v,
            Self::Cursor(v) => e.game_host.current_cursor = v,
            Self::Affiliate(v) => e.builtins.player_is_affiliate = v,
            Self::Country(v) => e.builtins.current_player_country = v,
            Self::Billing(v) => e.builtins.from_billing = v,
            Self::Email(s) => e.builtins.create_email = Some(s.into()),
            // isStateGame: 18/3/9 are in game.
            Self::State(v) => e.login.world_list_game = matches!(v, 18 | 3 | 9),
            Self::Uid(v) => {
                w.local = Some(crate::ui_cam2::Trackable {
                    kind: 0,
                    index: v,
                    level: 0,
                    coord: [0; 3],
                    yaw: 0,
                })
            }
            Self::TopIf(v) => w.props.life.top = v,
            Self::Autosetup(v) => w.vars.queries.preferences.autosetup_display_mode = v,
            Self::LoadedToolkit(v) => {
                let options = &mut w.vars.queries.preferences.options;
                for name in ["toolkit", "displayMode"] {
                    options.values
                        [crate::client_options::ClientOptions::field_index(name).unwrap()] = v;
                }
                *options = crate::client_options::ClientOptions::decode(
                    &options.encode(),
                    options.profile,
                )
                .unwrap();
            }
            Self::Emoji(n, g, f) => {
                assert!(e.builtins.emoji.add(&units(n), g, f));
            }
            Self::Telemetry(b) => e
                .telemetry_packet(crate::proto::server::TELEMETRY_GRID_FULL, b)
                .unwrap(),
            Self::Spline(i, n) => {
                e.game_host.cutscene_spline[i as usize] = Some(vec![vec![0; 4]; (n << 1) as usize])
            }
            Self::ObjFind(ids, index) => {
                e.game_host.obj_find_results = Some(ids.to_vec());
                e.game_host.obj_find_index = index as usize;
            }
            Self::Iface(id, n) => {
                let comps = (0..n)
                    .map(|k| {
                        let mut c = Component::default();
                        c.f.parentlayer = (id << 16) | k;
                        c.f.id = -1;
                        Some(Rc::new(RefCell::new(c)))
                    })
                    .collect();
                w.store.interfaces.insert(id, Interface::new(comps));
            }
            Self::Com(id, k, f) => {
                let com = w.com(id, k);
                let mut c = com.borrow_mut();
                match f {
                    ComField::Name(s) => c.f.name = Some(units(s)),
                    ComField::ServerTriggers(v) => c.f.serverTriggers = v,
                    ComField::InvObject(v) => c.f.invobject = v,
                    ComField::Events(v) => c.default_active[0] = v,
                }
            }
            Self::Active(id, k) => {
                let component = w.com(id, k);
                let interface = w.store.interfaces.get(&id).cloned();
                w.active[usize::from(secondary)] = Active {
                    interface,
                    component: Some(component),
                };
            }
            Self::Friends(n) => w.props.game.friends = n.iter().map(|s| (*s).into()).collect(),
            Self::FriendsState(v) => w.props.game.friends_list_state = v,
            Self::ClanChat(n) => {
                w.props.game.friend_chat = Some(n.iter().map(|s| (*s).into()).collect())
            }
            Self::ClanUser(name, rank) => {
                e.social.affined_channel = Some(crate::ui_social::ClanChannel {
                    users: vec![crate::ui_social::ClanChannelUser {
                        name: name.into(),
                        rank,
                        world: 1,
                    }],
                    ..Default::default()
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Observers of owner state (the recording printed the same text).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Obs {
    /// Queued outgoing packet bytes (lobby + game connections).
    Out,
    Prefetch,
    MinLength,
    /// Follow-camera velocities after the queued cameraInc/Dec inputs.
    OrbitVel,
    Camera,
    Roof,
    Follow,
    MoveAlong,
    Spline(i32),
    ObjFindIndex,
    Com(i32, i32),
    Blackflag,
    /// The saved toolkit and the active display mode.
    ToolkitPref,
    EmojiSub(&'static str),
    EmojiAuto,
    /// Hardware state, webcam flips, smooth resize, webcam frame buffer.
    Twitch,
    /// The live-stream cursor.
    TwitchCursor,
    Pressed,
    /// The installer launcher's status, as last recorded (no polling).
    Setup,
}

impl Obs {
    fn text(self) -> String {
        match self {
            Self::Out => "out".into(),
            Self::Prefetch => "prefetch".into(),
            Self::MinLength => "minlength".into(),
            Self::OrbitVel => "orbitvel".into(),
            Self::Camera => "camera".into(),
            Self::Roof => "roof".into(),
            Self::Follow => "follow".into(),
            Self::MoveAlong => "movealong".into(),
            Self::Spline(i) => format!("spline={i}"),
            Self::ObjFindIndex => "objfindindex".into(),
            Self::Com(id, k) => format!("com={id},{k}"),
            Self::Blackflag => "blackflag".into(),
            Self::ToolkitPref => "toolkitpref".into(),
            Self::EmojiSub(s) => format!("emojisub={}", hex(s)),
            Self::EmojiAuto => "emojiauto".into(),
            Self::Twitch => "twitch".into(),
            Self::TwitchCursor => "twitchcursor".into(),
            Self::Pressed => "pressed".into(),
            Self::Setup => "setup".into(),
        }
    }

    fn name(self) -> String {
        self.text().split('=').next().unwrap().to_owned()
    }

    fn rust(self, w: &World) -> String {
        let e = &w.engine;
        let v = match self {
            Self::Out => [&e.outgoing[..], &w.props.interaction.outgoing[..]]
                .concat()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            Self::Prefetch => format!("{:?}", e.builtins.sprite_prefetch).replace(' ', ""),
            Self::MinLength => w.props.minimenu.min_length.to_string(),
            Self::OrbitVel => {
                // The production consumer (`host_game::after_tick`).
                let mut follow = crate::camera_follow::Follow::default();
                for &(pitch, positive) in &e.game_host.orbit_inputs {
                    follow.input(pitch, positive);
                }
                format!("{:?},{:?}", follow.pitch_velocity, follow.yaw_velocity)
            }
            Self::Camera => format!(
                "{},{},{}",
                e.camera.cam2.camera_state, e.scene.server_roof[0], e.scene.server_roof[1]
            ),
            Self::Roof => format!("{},{}", e.scene.server_roof[0], e.scene.server_roof[1]),
            Self::Follow => e.game_host.follow_height.to_string(),
            Self::MoveAlong => match &e.camera.cam2.legacy.move_along {
                Some(m) => format!(
                    "{},{},{},{},{},{},{}",
                    m.pos_spline,
                    m.pos_keyframe,
                    m.target_spline,
                    m.target_keyframe,
                    m.progress,
                    m.speed_min,
                    m.speed_max
                ),
                None => "none".into(),
            },
            Self::Spline(i) => match &e.game_host.cutscene_spline[i as usize] {
                None => "null".into(),
                Some(rows) => rows
                    .iter()
                    .map(|r| join(r).replace(',', ":"))
                    .collect::<Vec<_>>()
                    .join("|"),
            },
            Self::ObjFindIndex => e.game_host.obj_find_index.to_string(),
            Self::Com(id, k) => {
                let com = w.com(id, k);
                let c = com.borrow();
                format!(
                    "{},{},{},{},{}",
                    c.f.modelkind,
                    c.f.model,
                    c.f.link
                        .as_deref()
                        .map_or("null".into(), |l| hex(&String::from_utf16_lossy(l))),
                    c.group_kind.map_or("null".into(), |k| k.to_string()),
                    if c.npc_customisation.is_some() {
                        "custom"
                    } else {
                        "nocustom"
                    }
                )
            }
            Self::Blackflag => {
                let p = &w.vars.queries.preferences;
                format!("{},{}", p.blackflag_mode3, p.blackflag_mode4)
            }
            Self::ToolkitPref => {
                let options = &w.vars.queries.preferences.options;
                format!(
                    "{},{}",
                    options.get("toolkit").unwrap(),
                    options.get("displayMode").unwrap()
                )
            }
            Self::EmojiSub(s) => hex(&String::from_utf16_lossy(
                &e.builtins.emoji.substitute(&units(s)),
            )),
            Self::EmojiAuto => e.builtins.emoji.autochat.to_string(),
            Self::Twitch => {
                let t = &e.builtins.twitch;
                format!(
                    "{},{},{},{},{}",
                    t.hardware_state, t.flip[0], t.flip[1], t.smooth_resize, t.webcam_frame
                )
            }
            Self::TwitchCursor => e.builtins.twitch.livestream_cursor.to_string(),
            Self::Setup => (e.builtins.setup.state() as i32).to_string(),
            Self::Pressed => w
                .props
                .life
                .pressed_continue
                .as_ref()
                .map_or("null".into(), |c| c.borrow().f.parentlayer.to_string()),
        };
        format!("{}={v}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Cases.
// ---------------------------------------------------------------------------

/// Hand-derived expectation (stacks above the sentinels).
#[derive(Default)]
struct Hand {
    ints: Vec<i32>,
    objs: Vec<String>,
    longs: Vec<i64>,
    throws: Option<Fault>,
    obs: Vec<String>,
}

/// Extra hand-derived owner checks.
type Check = fn(&World) -> Result<(), String>;

enum Want {
    /// Recorded from the original handler (fixtures/cs2-commands/recorded.tsv).
    Recorded,
    /// Hand-derived.
    Hand(Hand),
}

struct Case {
    cmd: &'static str,
    note: &'static str,
    sec: bool,
    set: Vec<Set>,
    ints: Vec<i32>,
    objs: Vec<String>,
    longs: Vec<i64>,
    obs: Vec<Obs>,
    /// Extra Rust-owner state for hand rows (fonts, configs, scene).
    with: Option<fn(&mut World)>,
    /// The stacks above the sentinels are verified by `check` (values that
    /// come from the clock, the clipboard or a timing measurement).
    free: bool,
    /// Extra hand-derived owner checks.
    check: Option<Check>,
    want: Want,
}

fn c(cmd: &'static str) -> Case {
    Case {
        cmd,
        note: "",
        sec: false,
        set: vec![],
        ints: vec![],
        objs: vec![],
        longs: vec![],
        obs: vec![],
        with: None,
        free: false,
        check: None,
        want: Want::Hand(Hand::default()),
    }
}

impl Case {
    fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }
    fn sec(mut self) -> Self {
        self.sec = true;
        self
    }
    fn set(mut self, s: Set) -> Self {
        self.set.push(s);
        self
    }
    fn i(mut self, v: &[i32]) -> Self {
        self.ints = v.to_vec();
        self
    }
    fn s(mut self, v: &[&str]) -> Self {
        self.objs = v.iter().map(|s| (*s).to_owned()).collect();
        self
    }
    fn l(mut self, v: &[i64]) -> Self {
        self.longs = v.to_vec();
        self
    }
    fn obs(mut self, o: Obs) -> Self {
        self.obs.push(o);
        self
    }
    fn with(mut self, f: fn(&mut World)) -> Self {
        self.with = Some(f);
        self
    }
    fn check(mut self, f: Check) -> Self {
        self.check = Some(f);
        self
    }
    fn free(mut self) -> Self {
        self.free = true;
        self
    }
    fn recorded(mut self) -> Self {
        self.want = Want::Recorded;
        self
    }
    fn hand(&mut self) -> &mut Hand {
        match &mut self.want {
            Want::Hand(hand) => hand,
            Want::Recorded => panic!("{}: hand expectation on a recorded row", self.cmd),
        }
    }
    /// Hand-derived int stack above the sentinel after the handler.
    fn wi(mut self, v: &[i32]) -> Self {
        self.hand().ints = v.to_vec();
        self
    }
    fn ws(mut self, v: &[&str]) -> Self {
        self.hand().objs = v.iter().map(|s| (*s).to_owned()).collect();
        self
    }
    fn throws(mut self, kind: Fault) -> Self {
        self.hand().throws = Some(kind);
        self
    }
    /// Hand-derived observation text (`name=value`).
    fn wo(mut self, o: Obs, value: &str) -> Self {
        self.obs.push(o);
        self.hand().obs.push(format!("{}={value}", o.name()));
        self
    }
}

/// The frequent recorded row shape.
fn rec(cmd: &'static str) -> Case {
    c(cmd).recorded()
}

fn cases() -> Vec<Case> {
    let mut v = vec![];
    v.extend(disabled_handlers());
    v.extend(pure_helpers());
    v.extend(client_fields());
    v.extend(telemetry());
    v.extend(emoji());
    v.extend(twitch());
    v.extend(packets());
    v.extend(hook_owners());
    v.extend(toolkit_availability());
    v.extend(interfaces());
    v.extend(npc_customisation());
    v.extend(camera());
    v.extend(local_player());
    v.extend(config_queries());
    v.extend(setup_launcher());
    v.extend(platform());
    v.extend(engine_partition());
    v
}

// ---------------------------------------------------------------------------
// Known divergences (ratchet): behaviour that differs from the recorded
// expectation of the named case. The test fails when this list does not
// match exactly, so a fix or a new divergence must update it.
// ---------------------------------------------------------------------------

const KNOWN_DIVERGENCES: &[&str] = &[];

// ---------------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------------

struct World {
    engine: Engine,
    store: Store,
    props: crate::ui_properties::State,
    pool: Pool,
    active: [Active; 2],
    vars: crate::ui_vars::State,
    defs: crate::entity_runtime::bits_pack::Inputs,
    players: crate::protocol910::Players,
    local: Option<crate::ui_cam2::Trackable>,
    outcome: Option<Outcome>,
}

#[derive(Debug)]
struct Outcome {
    ints: Vec<i32>,
    objs: Vec<Option<String>>,
    longs: Vec<i64>,
    error: Option<String>,
}

impl World {
    fn new() -> Self {
        let mut engine = Engine::default();
        // An in-game client: the game connection exists (before_tick).
        engine.game_host.game_connection = true;
        Self {
            engine,
            store: Store::default(),
            props: crate::ui_properties::State::default(),
            pool: Pool::default(),
            active: Default::default(),
            vars: crate::ui_vars::State::default(),
            defs: crate::entity_runtime::bits_pack::Inputs {
                definitions: BTreeMap::new(),
                raw: BTreeMap::new(),
                count: 0,
            },
            players: crate::protocol910::Players::default(),
            local: None,
            outcome: None,
        }
    }

    /// A client with the cache-backed owners installed the way the runtime
    /// installs them: config stores, fonts and sprites, player stats and
    /// quest types.
    fn installed(pack: &crate::cache::Pack) -> Self {
        let mut w = Self::new();
        w.engine.load_cache(pack);
        let mut props = super::Runtime::load_state(pack).expect("static client state");
        props.objs = w.engine.configs.objs.clone();
        props.npcs = w.engine.configs.npcs.clone();
        w.props = props;
        w.vars = crate::ui_vars::State::with_client(pack).expect("player state");
        w
    }

    fn com(&self, id: i32, k: i32) -> Ref {
        self.store.interfaces[&id].borrow().components.borrow()[k as usize]
            .clone()
            .unwrap()
    }

    /// Scene local player (index 17) whose appearance model is `female`.
    fn local_player(&mut self, female: bool) {
        let mut player = crate::entities910::Player::default();
        player.appearance.model = Some(crate::entities910::appearance::Model {
            bas: -1,
            kits: vec![0; 12],
            custom: vec![None; 12],
            colours: [0; 10],
            textures: [0; 10],
            female,
            npc: -1,
            hash: 0,
        });
        self.players.players.resize(18, None);
        self.players.players[17] = Some(player);
        Set::Uid(17).apply(self, false);
    }

    fn outcome_int(&self) -> Result<i32, String> {
        let o = self.outcome.as_ref().unwrap();
        match o.ints[..] {
            [SENTINEL, v] if o.error.is_none() => Ok(v),
            _ => Err(format!("{o:?}")),
        }
    }

    fn outcome_str(&self) -> Result<String, String> {
        let o = self.outcome.as_ref().unwrap();
        match &o.objs[..] {
            [Some(s), Some(v)] if s == SENTINEL_STR && o.error.is_none() => Ok(v.clone()),
            _ => Err(format!("{o:?}")),
        }
    }

    fn run(&mut self, case: &Case) {
        let push = |command: &str, operand| Instruction {
            opcode: 0,
            command: command.into(),
            operand,
        };
        let mut code = vec![
            push("push_constant_int", Operand::Int(SENTINEL)),
            push("push_constant_string", Operand::Str(SENTINEL_STR.into())),
            push("push_long_constant", Operand::Long(i64::from(SENTINEL))),
        ];
        code.extend(
            case.ints
                .iter()
                .map(|v| push("push_constant_int", Operand::Int(*v))),
        );
        code.extend(
            case.objs
                .iter()
                .map(|v| push("push_constant_string", Operand::Str(v.clone()))),
        );
        code.extend(
            case.longs
                .iter()
                .map(|v| push("push_long_constant", Operand::Long(*v))),
        );
        code.push(push(case.cmd, Operand::Byte(u8::from(case.sec))));
        code.push(push("return", Operand::Byte(0)));
        let script = CompiledScript {
            name: Some(format!("spec:{}", case.cmd)),
            locals: Counts::default(),
            args: Counts::default(),
            code,
        };
        let request = Request {
            args: Some(vec![Arg::Int(1)]),
            ..Request::default()
        };
        self.pool.acquire();
        self.pool.contexts[0].active = std::mem::take(&mut self.active);
        self.pool.used = 0;
        let mut now = || 0i64;
        let mut vars = Variables {
            cycle: 0,
            definitions: &self.defs,
            state: &mut self.vars,
            player: None,
            active_player: None,
            active_npc: None,
            now: &mut now,
            probe: None,
            varp_transmit: crate::ui_loop::Counter::default(),
            scene: crate::ui_cam2::SceneInput {
                players: Some(&self.players),
                local_player: self.local,
                ..Default::default()
            },
        };
        let execution = self
            .pool
            .execute(
                &mut self.store,
                &mut self.props,
                ScriptRun {
                    id: 1,
                    script: &script,
                    request: &request,
                    limit: 1000,
                },
                Domains::Game(&mut vars),
                &mut self.engine,
                &(),
            )
            .expect("script prepared");
        let s = execution.snapshot;
        self.outcome = Some(Outcome {
            ints: s.ints,
            objs: s.strings,
            longs: s.longs,
            error: execution.result.err().map(|e| match e {
                VmError::TrapFailed { reason, .. } => format!("trap: {reason}"),
                other => format!("vm: {other:?}"),
            }),
        });
    }
}

/// The recorded failure names of each [`Fault`] class
/// (`fixtures/cs2-commands/fault-classes.tsv`, `label<TAB>recorded name`; a
/// class may list several names).
fn fault_names() -> Vec<(Fault, String)> {
    let table = std::fs::read_to_string(fixture().join("fault-classes.tsv"))
        .expect("fixtures/cs2-commands/fault-classes.tsv");
    table
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(label, name)| {
            let fault = Fault::ALL.into_iter().find(|f| f.label() == label)?;
            Some((fault, name.to_owned()))
        })
        .collect()
}

/// The [`Fault`] class a recorded failure name stands for.
fn recorded_fault(name: &str) -> Fault {
    fault_names()
        .into_iter()
        .find(|(_, recorded)| recorded == name)
        .map(|(fault, _)| fault)
        .unwrap_or_else(|| panic!("failure name {name} is not in fault-classes.tsv"))
}

/// The [`Fault`] class a trap error names: its label prefix, or a recorded
/// failure name found in the reason. `None` for a failure with no such class.
fn error_fault(error: &str) -> Option<Fault> {
    let reason = error.strip_prefix("trap: ")?;
    Fault::in_message(reason).or_else(|| {
        fault_names().into_iter().find_map(|(fault, name)| {
            reason
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == name)
                .then_some(fault)
        })
    })
}

struct Golden {
    ints: Vec<i32>,
    objs: Vec<String>,
    longs: Vec<i64>,
    thrown: Option<Fault>,
    obs: Vec<String>,
}

fn fixture() -> PathBuf {
    rs910_core::test_support::client_dir().join(FIXTURE)
}

/// Command names the recording still carries as the original book spelled
/// them, mapped to the names of our opcode book.
const RECORDED_COMMAND_NAMES: [(&str, &str); 2] = [
    ("field5245", "disabled_command_1144"), // provenance: recorded-data (command name in fixtures/cs2-commands/recorded.tsv)
    ("can_run_java_client", "can_run_classic_client"), // provenance: recorded-data (command name in fixtures/cs2-commands/recorded.tsv)
];

fn goldens() -> BTreeMap<String, Golden> {
    let text = std::fs::read_to_string(fixture().join("recorded.tsv"))
        .expect("fixtures/cs2-commands/recorded.tsv");
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let mut c: Vec<&str> = line.split('\t').collect();
            c.resize(6, "");
            fn list(s: &str) -> Vec<&str> {
                if s.is_empty() {
                    vec![]
                } else {
                    s.split(',').collect()
                }
            }
            let name = RECORDED_COMMAND_NAMES
                .iter()
                .find_map(|(recorded, ours)| {
                    c[0].strip_prefix(recorded)
                        .map(|rest| format!("{ours}{rest}"))
                })
                .unwrap_or_else(|| c[0].to_owned());
            (
                name,
                Golden {
                    ints: list(c[1]).iter().map(|v| v.parse().unwrap()).collect(),
                    objs: list(c[2]).iter().map(|v| unhex(v)).collect(),
                    longs: list(c[3]).iter().map(|v| v.parse().unwrap()).collect(),
                    thrown: (c[4] != "-").then(|| recorded_fault(c[4])),
                    obs: if c[5].is_empty() {
                        vec![]
                    } else {
                        c[5].split(';').map(str::to_owned).collect()
                    },
                },
            )
        })
        .collect()
}

/// Case ids: the command name, `#n` for the n-th repeat.
fn ids(cases: &[Case]) -> Vec<String> {
    let mut seen = BTreeMap::<&str, usize>::new();
    cases
        .iter()
        .map(|case| {
            let n = seen.entry(case.cmd).or_default();
            *n += 1;
            if *n == 1 {
                case.cmd.to_owned()
            } else {
                format!("{}#{n}", case.cmd)
            }
        })
        .collect()
}

/// Expected (above the sentinels) and observed outcome of one case.
fn verify(case: &Case, golden: Option<&Golden>, w: &World) -> Result<(), String> {
    let o = w.outcome.as_ref().unwrap();
    let observed: Vec<String> = case.obs.iter().map(|obs| obs.rust(w)).collect();
    let (ints, objs, longs, thrown, obs) = match (&case.want, golden) {
        (Want::Recorded, Some(g)) => (
            g.ints.clone(),
            g.objs.clone(),
            g.longs.clone(),
            g.thrown,
            g.obs.clone(),
        ),
        (Want::Recorded, None) => return Err("no recorded row for this command".into()),
        (
            Want::Hand(Hand {
                ints,
                objs,
                longs,
                throws,
                obs,
            }),
            _,
        ) => (
            [&[SENTINEL][..], ints].concat(),
            [&[SENTINEL_STR.to_owned()][..], objs].concat(),
            [&[i64::from(SENTINEL)][..], longs].concat(),
            *throws,
            obs.clone(),
        ),
    };
    if observed != obs {
        return Err(format!("observations {observed:?}, want {obs:?}"));
    }
    match (thrown, &o.error) {
        (Some(kind), Some(error)) => {
            if error_fault(error) != Some(kind) {
                return Err(format!(
                    "failed with {error}, the original fails with {}",
                    kind.label()
                ));
            }
            return Ok(());
        }
        (Some(kind), None) => {
            return Err(format!(
                "succeeded ({o:?}), the original fails with {}",
                kind.label()
            ))
        }
        (None, Some(error)) => return Err(format!("failed with {error}")),
        (None, None) => {}
    }
    let stacks_match = if case.free {
        o.ints.first() == Some(&SENTINEL)
            && o.objs.first().and_then(Option::as_deref) == Some(SENTINEL_STR)
            && o.longs == [i64::from(SENTINEL)]
    } else {
        (&o.ints, &o.objs, &o.longs) == (&ints, &objs.iter().cloned().map(Some).collect(), &longs)
    };
    if !stacks_match {
        return Err(format!(
            "stacks {:?} {:?} {:?}, want {ints:?} {objs:?} {longs:?}",
            o.ints, o.objs, o.longs
        ));
    }
    if let Some(check) = case.check {
        check(w)?;
    }
    // The fixed stack contract table.
    let operand = Operand::Byte(u8::from(case.sec));
    if let native910::semantics::Effect::Fixed { pops, pushes } =
        native910::semantics::effect(case.cmd, &operand)
    {
        let before = [
            1 + case.ints.len(),
            1 + case.objs.len(),
            1 + case.longs.len(),
        ];
        let after = [o.ints.len(), o.objs.len(), o.longs.len()];
        for lane in 0..3 {
            if after[lane] + usize::from(pops[lane]) != before[lane] + usize::from(pushes[lane]) {
                return Err(format!(
                    "lane {lane}: {} -> {} breaks the contract pops {pops:?} pushes {pushes:?}",
                    before[lane], after[lane]
                ));
            }
        }
    }
    Ok(())
}

/// Every command of both partitions, run through the production host chain
/// and compared with its recorded or hand-derived expectation.
#[test]
fn every_partition_command_matches_its_recorded_behaviour() {
    let cases = cases();
    let ids = ids(&cases);
    let goldens = goldens();
    let mut failures = vec![];
    for (case, id) in cases.iter().zip(&ids) {
        let mut w = World::new();
        for s in &case.set {
            s.apply(&mut w, case.sec);
        }
        if let Some(with) = case.with {
            with(&mut w);
        }
        w.run(case);
        if let Err(e) = verify(case, goldens.get(id), &w) {
            failures.push(format!("{id} ({}): {e}", case.note));
        }
    }
    // Coverage: every partition command has at least one value-level row.
    let covered: std::collections::BTreeSet<&str> = cases.iter().map(|c| c.cmd).collect();
    assert_eq!(PARTITION.len(), 322);
    let missing: Vec<&&str> = PARTITION
        .iter()
        .chain(ENGINE_PARTITION)
        .filter(|n| !covered.contains(**n))
        .collect();
    assert!(
        missing.is_empty(),
        "commands without a behaviour row: {missing:?}"
    );
    let divergent: Vec<&str> = failures
        .iter()
        .map(|f| f.split(' ').next().unwrap())
        .collect();
    assert_eq!(
        divergent,
        KNOWN_DIVERGENCES,
        "behaviour differs from the recordings:\n{}",
        failures.join("\n")
    );
}

/// The streaming probe settles on "unsupported" and stays there: a script
/// that polls the library state after requesting the platform never waits for
/// a download this client cannot perform.
#[test]
fn streaming_platform_probe_settles_on_unsupported() {
    let mut w = World::new();
    w.run(&c("ttv_library_getstate"));
    assert_eq!(w.outcome_int(), Ok(0), "before the probe the state is idle");
    for _ in 0..2 {
        w.run(&c("ttv_library_request"));
        assert_eq!(w.outcome_int(), Ok(-1));
        w.run(&c("ttv_library_getstate"));
        assert_eq!(w.outcome_int(), Ok(3));
    }
}

/// `login_accountappeal` reaches the accounts service only through the
/// engine; with no service behind it the appeal reports the failure code and
/// consumes its argument, whatever the login state.
#[test]
fn account_appeal_reports_an_unreachable_service() {
    let mut w = World::new();
    for ready in [false, true] {
        w.engine.login.ready = ready;
        w.run(&c("login_accountappeal").s(&["password"]));
        assert_eq!(w.outcome_int(), Ok(5));
        assert_eq!(
            w.outcome.as_ref().unwrap().objs,
            [Some(SENTINEL_STR.to_owned())],
            "the password is consumed"
        );
    }
}

/// Commands that stop at a named gap instead of an owner (`TODO(#...)`),
/// with the lane that closes each. The list is a ratchet like
/// [`KNOWN_DIVERGENCES`]: closing a gap or adding one changes it.
const NAMED_GAPS: &[&str] = &[];

/// Every command the retail client dispatches through its host reaches an
/// owner in an installed client (cache configs, fonts, player state), never
/// the unknown-command fallback, and only [`NAMED_GAPS`] stop at a named gap.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn every_retail_command_reaches_an_owner_in_an_installed_client() {
    use native910::semantics::{contract_for_opcode, ExecutionFamily};
    let pack = crate::test_support::require_pack("client.obj.config.js5");
    let book = native910::opcode::OpcodeBook::embedded().unwrap();
    let mut unowned = vec![];
    let mut gaps = std::collections::BTreeSet::new();
    let mut checked = 0;
    // One installed client per login state, reused: building it is the
    // expensive part.
    let mut worlds = [(false, 0), (true, 1)].map(|(ready, fill)| {
        let mut w = World::installed(&pack);
        for s in [Set::Iface(IFACE, 2), Set::Active(IFACE, 0)] {
            s.apply(&mut w, false);
        }
        (w, ready, fill)
    });
    for (id, name) in book.entries() {
        let contract = contract_for_opcode(&book, id, &Operand::Byte(0)).unwrap();
        if !contract.retail_dispatch || contract.execution != ExecutionFamily::HostRequired {
            continue;
        }
        let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
        for (w, ready, fill) in &mut worlds {
            // Earlier commands may end the login; each run starts logged in
            // or not as the world says.
            w.engine.login.ready = *ready;
            w.pool = Pool::default();
            let case = c(name)
                .i(&[*fill; 12])
                .s(&["a", "b", "c", "d"])
                .l(&[i64::from(*fill); 3]);
            w.run(&case);
            let error = w
                .outcome
                .as_ref()
                .unwrap()
                .error
                .clone()
                .unwrap_or_default();
            if error.contains("UnknownCommand") {
                unowned.push(name);
            }
            if error.contains("TODO(#") {
                gaps.insert(name);
            }
        }
        checked += 1;
    }
    assert!(checked > 1000, "only {checked} commands were exercised");
    assert!(unowned.is_empty(), "commands without an owner: {unowned:?}");
    assert_eq!(gaps.into_iter().collect::<Vec<_>>(), NAMED_GAPS);
}

/// CS2 sound commands through the production host into the audio owner:
/// the engine retains `SoundRequest`s (sound_song_volume
///  pops three ints, sound_song_stop), the app drains
/// them into
/// `AudioRuntime::submit`, and the next `update` queues
/// SOUND_SONGPRELOADED p4(song) (playSong) and then SOUND_SONGEND
/// p4(song) for the stopped song.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sound_commands_drive_the_audio_runtime() {
    let pack = crate::test_support::require_pack("client.vorbis.js5");
    let mut runtime = crate::audio_runtime::AudioRuntime::new(pack);
    let mut w = World::new();
    let mut out = Vec::new();
    let t0 = 1_000;
    runtime.tick(t0, true, &mut out);
    let mut step = |w: &mut World, case: Case, now: i64| {
        w.run(&case);
        let o = w.outcome.as_ref().unwrap();
        assert!(o.error.is_none(), "{}: {o:?}", case.cmd);
        assert_eq!(o.ints, [SENTINEL], "{} pops its arguments", case.cmd);
        for sound in w.engine.effects.sounds.drain(..) {
            runtime.submit(&sound);
        }
        let mut out = Vec::new();
        runtime.tick(now, true, &mut out);
        (out, runtime.current_song())
    };
    let song = |opcode: u8| [opcode, 0, 0, 0, 2];
    assert_eq!(
        step(&mut w, c("sound_song_volume").i(&[2, 200, 0]), t0 + 20),
        (song(crate::proto::client::SOUND_SONGPRELOADED).to_vec(), 2)
    );
    assert_eq!(
        step(&mut w, c("sound_song_stop"), t0 + 40),
        (song(crate::proto::client::SOUND_SONGEND).to_vec(), -1)
    );
}

// ---------------------------------------------------------------------------
// Small helpers and independent platform facts.
// ---------------------------------------------------------------------------

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn hex(s: &str) -> String {
    let mut out = String::from("x");
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04x}"));
    }
    out
}

fn unhex(s: &str) -> String {
    let body = s.strip_prefix('x').expect("hex string");
    let units: Vec<u16> = (0..body.len())
        .step_by(4)
        .map(|k| u16::from_str_radix(&body[k..k + 4], 16).unwrap())
        .collect();
    String::from_utf16(&units).unwrap()
}

fn join<T: ToString>(v: &[T]) -> String {
    v.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The lowercased OS name the handlers report for this build target.
fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac os x"
    } else if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

fn soundflower_running() -> bool {
    std::process::Command::new("ps")
        .arg("-few")
        .output()
        .is_ok_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains("soundflowerbed")
        })
}

fn wall_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// `date` in the default zone: the OS's own formatter, not the port's.
fn date_command(seconds: i64, format: &str) -> String {
    let output = if cfg!(target_os = "macos") {
        std::process::Command::new("date")
            .env("LC_ALL", "C")
            .args(["-r", &seconds.to_string(), format])
            .output()
    } else {
        std::process::Command::new("date")
            .env("LC_ALL", "C")
            .args(["-d", &format!("@{seconds}"), format])
            .output()
    }
    .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn local_year() -> i32 {
    date_command(wall_millis() / 1000, "+%Y").parse().unwrap()
}

fn local_date(seconds: i64) -> String {
    date_command(seconds, "+%d-%b-%Y")
}

fn system_clipboard() -> String {
    let output = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/pbpaste")
            .env("LANG", "en_US.UTF-8")
            .output()
    } else {
        std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
    };
    output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}
