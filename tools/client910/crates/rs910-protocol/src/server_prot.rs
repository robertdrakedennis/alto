//! Server-to-client game packets parsed into [`UiEvent`]s, and
//! the `REBUILD_NORMAL` tail: the packets
//! whose payload decode is pure. Applying the events (interfaces, vars,
//! camera, audio, entity state) and every socket read stay with their owners
//! in client910 (`session.rs` drains and routes the frames).
//!
//! Moved from `session.rs` by the Phase 2.2 codemod
//! (`tools/refactor/steps/p2-proto-move.py`); item docs keep their original
//! wire-truth notes. `session.rs` re-exports this module
//! (`pub use rs910_protocol::server_prot::*`).

mod rebuild;
use rebuild::decode_p2_alt2;
pub use rebuild::{
    parse_rebuild_normal, rebuild_to_groups, should_reload, Rebuild, RebuildEvent, REBUILD_TAIL_LEN,
};
mod interface_packets;
use interface_packets::decode_g1_alt1;
use interface_packets::decode_g1_alt2;
use interface_packets::decode_g1_alt3;
use interface_packets::decode_g2_alt2_u16;
use interface_packets::decode_g4_alt1;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_opensub;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_opensub_active_loc;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_opensub_active_npc;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_opensub_active_obj;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_opensub_active_player;
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub use interface_packets::parse_if_sethide;
pub use interface_packets::{
    parse_if_setposition, parse_if_setscrollpos, parse_if_settext, parse_runclientscript,
};
mod environment;
pub use environment::parse_environment_override;
mod audio_packets;
use audio_packets::parse_audio_event;
mod event_decode;
pub use event_decode::parse_ui_event;

/// `PayloadReader` and `cp1252_byte` (split out in Phase 2.2).
pub use crate::payload_reader::*;

// ---------------------------------------------------------------------------
// Live interface / CS2 frames (P0).
//
// Wire truth (server authority):
// - Sizes: the server packet table (`server/src/formats/network/protocol/`) line
//   36 (26/32),
//   `:45` (35/19), `:48` (38/23), `:71` (61/25), `:82` (72/8), `:89` (79/6),
//   `:112` (102/25), `:119` (109/5), `:131` (121/29), `:166` (156/-2),
//   `:191` (181/-2).
// - Encoders: the same table `:301-310` (`IF_OPENTOP.encode`: `p4_alt2/p4_alt1/
//   p4_alt2/p4` keys, `p1` type, `p2_alt3` top id), `:312-322`
//   (`IF_OPENSUB.encode`: `p4_alt2` key, `p4_alt1` parent packed,
//   `p1_alt2` type, `p4` key, `p2` sub id, two `p4_alt2` keys), `:333-356`
//   (`RUNCLIENTSCRIPT.encode`: `pjstr` descriptor built reverse over args,
//   then args forward (`pjstr`/`p4`), then `p4` script id).
// - Dispatch (read order, the truth for payloads without a TS encoder):
//   `IF_OPENTOP`: `g4_alt2/g4_alt1/g4_alt2/g4s/g1/g2_alt3`; `IF_OPENSUB`:
//   `g4_alt2/g4_alt1/g1_alt2/g4s/g2/g4_alt2/g4_alt2`; the four
//   `IF_OPENSUB_ACTIVE_*` variants read their own fixed layouts;
//   `IF_SETTEXT`: `g4_alt1/gjstr`; `IF_SETHIDE`: `g4s/g1_alt2`;
//   `IF_SETPOSITION`: `g2s_alt1/g2s_alt2/g4_alt1`; `IF_SETSCROLLPOS`:
//   `g4s/g2_alt2`; `RUNCLIENTSCRIPT`: `gjstr` descriptor, then reverse-index
//   reads, then `g4s` script id.
// - Alt transforms (`g1_alt1/2/3`, `g2_alt1/2/3`, `g2s_alt1/2`,
//   `g4_alt1/2/3`) are the inverses of the server's
//   `server/src/formats/bytepacking/Packet.ts:201-254` writes
//   (`p1_alt1/2/3`, `p2_alt1/2/3`, `p4_alt1/2/3`).
// - Login UI: `server/src/lostcity/systems/session/LoginLayout.ts` opens the
//   lobby window (906) and its nine panes in the lobby, and the game window
//   (1477) with the world view, minimap, compass and clock (1482/1465/1919/635)
//   in the world.
//
// Parsers below are pure (no pack/VM): they decode the `_alt*` shuffles to
// logical ids and hand them to the client session (`parse_ui_event`), which
// applies them to the interface state and runs their scripts in the CS2 host.
// Uncitable behaviour is `TODO(#gap-N)`, never silent.

/// One `RUNCLIENTSCRIPT` argument, in stored order
/// (descriptor reverse-index reads; see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptArg {
    /// Integer argument (`p4` / `g4s`).
    Int(i32),
    /// String argument (`pjstr` / `gjstr`, Windows-1252).
    Str(String),
}

/// A synthetic top-interface packet with zero keys for client authoring replays.
#[cfg(any(test, feature = "test-hooks"))]
pub fn authoring_open_top_payload(interface: u16) -> Vec<u8> {
    const KEY_COUNT: usize = 4;
    const TOP_KIND_WIDTH: usize = 1;
    const ALT_BYTE_BIAS: u8 = 128;
    let mut payload = vec![u8::default(); KEY_COUNT * std::mem::size_of::<i32>() + TOP_KIND_WIDTH];
    let [low, high] = interface.to_le_bytes();
    payload.extend([low.wrapping_add(ALT_BYTE_BIAS), high]);
    payload
}

/// Parsed `RUNCLIENTSCRIPT` (opcode 156, `-2` variable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunClientScript {
    /// Client-script id (`clientscripts` group, file 0).
    pub script_id: i32,
    /// Arguments in stored order (see [`ScriptArg`]).
    pub args: Vec<ScriptArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentOverrideEvent {
    pub sun_colour: Option<i32>,
    pub sun_ambient: Option<f32>,
    pub sun_diffuse: Option<f32>,
    pub sun_shadow: Option<f32>,
    pub sun_dir: Option<[f32; 3]>,
    pub fog_colour: Option<i32>,
    pub fog_depth: Option<i32>,
    /// The skybox override, applied as a new skybox.
    pub skybox: Option<SkyboxRef>,
    /// Bloom intensity, threshold and white point squared.
    pub bloom_intensity: Option<f32>,
    pub bloom_threshold: Option<f32>,
    pub bloom_white_point_sq: Option<f32>,
    /// The environment sampler material.
    pub sampler: Option<i32>,
    /// Colour remapping map and weight per slot.
    pub colour_remap: [Option<(i32, f32)>; 3],
    pub duration_ms: u16,
    pub clear: bool,
}

impl Eq for EnvironmentOverrideEvent {}

/// Which `IF_OPENSUB_ACTIVE_*` packet supplied the retained binding.
/// Active-trigger execution is still an integration gap in the live UI host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveVariant {
    /// Opcode 26 (`IF_OPENSUB_ACTIVE_LOC`, 32 bytes).
    Loc,
    /// Opcode 61 (`IF_OPENSUB_ACTIVE_PLAYER`, 25 bytes).
    Player,
    /// Opcode 102 (`IF_OPENSUB_ACTIVE_NPC`, 25 bytes).
    Npc,
    /// Opcode 121 (`IF_OPENSUB_ACTIVE_OBJ`, 29 bytes).
    Obj,
}

/// A packed coordinate (level, x, z). The -1 sentinel leaves x/z at their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceCoord {
    pub level: i32,
    pub x: i32,
    pub z: i32,
}

impl InterfaceCoord {
    pub fn unpack(v: i32) -> Self {
        if v == -1 {
            Self {
                level: -1,
                x: 0,
                z: 0,
            }
        } else {
            Self {
                level: (v >> 28) & 3,
                x: (v >> 14) & 16383,
                z: v & 16383,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveBinding {
    Loc {
        coord: InterfaceCoord,
        shape: i32,
        angle: i32,
        id: i32,
    },
    Player {
        index: i32,
    },
    Npc {
        index: i32,
    },
    Obj {
        coord: InterfaceCoord,
        id: i32,
    },
}

/// Skybox wire values: the skybox type
/// id, three `g2s`, and the `g2` yaw offset of the 2D skybox path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkyboxRef {
    pub kind: i32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub yaw_offset: i32,
}

/// One live interface / script frame, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    WorldList {
        bytes: Vec<u8>,
    },
    Camera {
        opcode: u8,
        bytes: Vec<u8>,
    },
    CameraForceAngle {
        pitch: u16,
        yaw: u16,
    },
    CameraShake {
        channel: u8,
        jitter: u8,
        wobble_scale: u8,
        cycle: u8,
        wobble_speed: u16,
    },
    CameraMoveTo {
        x: u8,
        z: u8,
        source_height: u16,
        acceleration: u8,
        speed: u8,
    },
    CameraLookAt {
        x: u8,
        z: u8,
        height: u16,
        acceleration: u8,
        speed: u8,
    },
    CameraReset,
    /// `CAM_SMOOTHRESET` (87/0): clear camera modifiers and restore state 1.
    CameraSmoothReset,
    /// `CAM_REMOVEROOF` (64/4): server-selected roof target, or -1 to clear.
    CameraRemoveRoof {
        packed: i32,
    },
    /// `POINTLIGHT_COLOUR` (107/8): `g4_alt3`
    /// packed colour (-1 restores the base), `g2_alt1` duration and
    /// `g2_alt3` light group: fade the group's colour over the duration.
    PointLightColour {
        duration: u16,
        colour: i32,
        id: u16,
    },
    /// `POINTLIGHT_INTENSITY` (144/5): `g2_alt2`
    /// duration, `g2_alt2` light group and `g1` percentage (255 -> -1,
    /// restoring 1.0): fade the group's intensity over the duration.
    PointLightIntensity {
        duration: u16,
        intensity: i32,
        id: u16,
    },
    /// Server-side sound/song/group request.
    /// `ui_runtime` retains this beside CS2-originated `SoundRequest`s so
    /// `audio_runtime` routes one ordered stream into `AudioApi`.
    Audio {
        command: String,
        args: Vec<i32>,
    },
    /// `PLAYER_SNAPSHOT` (67/-2): identity-kit bytes for an interface-only
    /// player model. Interface components index these as `-snapshot - 2`.
    PlayerSnapshot {
        id: i32,
        gender: i8,
        bytes: Vec<u8>,
    },
    /// `CLEAR_PLAYER_SNAPSHOT` (27/1): remove one interface-only appearance.
    ClearPlayerSnapshot {
        id: i32,
    },
    /// `LOBBY_APPEARANCE` (105/-2): the local identity-kit body used by the
    /// lobby's self-player model.
    LobbyAppearance {
        gender: i8,
        bytes: Vec<u8>,
    },
    /// `UPDATE_STOCKMARKET_SLOT` (28/21): one of the three-by-eight offer
    /// slots, retained for the stockmarket CS2 query family.
    StockmarketSlot {
        market: usize,
        slot: usize,
        state: u8,
        object: i32,
        price: i32,
        count: i32,
        completed_count: i32,
        completed_gold: i32,
    },
    /// `CUTSCENE` (136/-2): enter the cutscene rebuild mode. The retained
    /// game owner uses this to suppress ordinary-world transient effects.
    Cutscene {
        id: u16,
        parameter: u16,
        appearance: Vec<u8>,
    },
    /// `ENVIRONMENT_OVERRIDE` (1/-1): property-mask environment override
    /// applied to the environment manager.
    OverrideEnvironment(EnvironmentOverrideEvent),
    /// `UPDATE_SITESETTINGS` (10/-1): the client retains this value for the next
    /// lobby/world login payload. Application mode has no browser cookie.
    SiteSettings {
        value: String,
    },
    /// `UPDATE_UID192` (53/28): the client stores the 24-byte UID only when the
    /// trailing CRC validates, then reuses it in later login payloads.
    Uid192 {
        value: Option<[u8; 24]>,
    },
    /// `LAST_LOGIN_INFO` (110/4): prior-login IPv4 address for lobby scripts.
    LastLoginInfo {
        address: i32,
    },
    /// `TELEMETRY_GRID_*` / `TELEMETRY_CLEAR_GRID_VALUE`: raw payload for
    /// the telemetry owner (ui_host_builtins), which does the
    /// per-packet reads and the error latch.
    Telemetry {
        opcode: u8,
        bytes: Vec<u8>,
    },
    /// `URL_OPEN` (60/-2): the client opens the primary URL, or the fallback URL
    /// when the JavaScript/browser path is unavailable.
    UrlOpen {
        primary: String,
        fallback: Option<String>,
        javascript: bool,
    },
    /// `SOCIAL_NETWORK_LOGOUT` (77/-2): the client opens the CP1252 social URL.
    SocialNetworkLogout {
        url: String,
    },
    Inventory {
        opcode: u8,
        bytes: Vec<u8>,
    },
    /// `UPDATE_STAT` (33/6): current displayed level, XP, skill id.
    /// Read as `g1_alt2`, `g4_alt1`, `g1_alt3`.
    Stat {
        current_level: u8,
        xp: i32,
        skill: u8,
    },
    VarcAck,
    /// CLIENT_SETVARC_SMALL/LARGE: server-owned client integer varc.
    SetVarc {
        id: i32,
        value: i32,
    },
    /// CLIENT_SETVARCBIT_SMALL/LARGE: server-owned VarBitConfig value whose
    /// base variable is a client integer varc.
    SetVarcBit {
        id: i32,
        value: i32,
    },
    /// CLIENT_SETVARCSTR_SMALL/LARGE: server-owned client string varc.
    SetVarcString {
        id: i32,
        value: Vec<u16>,
    },
    /// `VARCLAN_ENABLE` / `VARCLAN_DISABLE`: install or clear the sparse clan
    /// variable domain used by the script runner's primary variable-domain map.
    VarClanEnable,
    VarClanDisable,
    /// `VARCLAN` (172/-1): one typed sparse clan variable update.
    VarClan {
        bytes: Vec<u8>,
    },
    SetTargetParam {
        packed: u32,
        from: i32,
        to: i32,
        param: i32,
    },
    /// `IF_SETEVENTS`: set the event mask over an inclusive child range.
    SetEvents {
        packed: u32,
        from: i32,
        to: i32,
        mask: i32,
    },
    /// `IF_OPENTOP` (35/19): open top-level interface.
    OpenTop {
        /// Logical top interface id (`g2_alt3`, e.g. 906 lobby / 1477 world).
        interface_id: u32,
        /// Keys in the order passed to Component.openInterface.
        keys: [i32; 4],
    },
    /// `IF_OPENSUB` (38/23): attach sub under a packed parent.
    OpenSub {
        /// Packed parent `(interface << 16) | component` (`g4_alt1`).
        parent_packed: u32,
        /// Sub interface id (`g2`).
        sub_id: u32,
        /// Sub type (`g1_alt2`; modal vs modeless layering).
        kind: u8,
        keys: [i32; 4],
    },
    /// `IF_OPENSUB_ACTIVE_*` (26/61/102/121): attach sub with an entity
    /// binding. The retained lifecycle owns this data; active-sub trigger
    /// lookup/execution is installed in the runtime, while entity query hosts
    /// remain a separate boundary.
    OpenSubActive {
        /// Packed parent.
        parent_packed: u32,
        /// Sub interface id.
        sub_id: u32,
        /// Sub type.
        kind: u8,
        /// Which active opcode carried it.
        variant: ActiveVariant,
        keys: [i32; 4],
        binding: ActiveBinding,
    },
    CloseSub {
        parent_packed: u32,
    },
    MoveSub {
        source_packed: u32,
        target_packed: u32,
    },
    /// `IF_SETTEXT` (181/-2): set component text.
    SetText {
        /// Packed `(interface << 16) | component` (`g4_alt1`).
        packed: u32,
        /// New text (`gjstr`).
        text: String,
    },
    /// `IF_SETHIDE` (109/5): show/hide a component.
    SetHide {
        /// Packed id (`g4s`).
        packed: u32,
        /// True when the `g1_alt2` flag equals one.
        hidden: bool,
        /// Original byte retained for the delayed state change's first integer.
        flag: u8,
    },
    /// `IF_SETPOSITION` (72/8): move a component.
    SetPosition {
        /// Packed id (`g4_alt1`).
        packed: u32,
        /// New X (`g2s_alt1`).
        x: i16,
        /// New Y (`g2s_alt2`).
        y: i16,
    },
    /// `IF_SETSCROLLPOS` (79/6): scroll a layer.
    SetScrollPos {
        /// Packed id (`g4s`).
        packed: u32,
        /// New scroll Y (`g2_alt2`).
        scroll_y: u16,
    },
    SetInterfaceAnim {
        packed: u32,
        animation: i32,
    },
    /// Delayed state change kind 4. `local_player` resolves the model id
    /// from the retained game scene at packet-dispatch time (the SELF packet
    /// does not carry an id on the wire).
    SetInterfaceModel {
        packed: u32,
        model_kind: i32,
        model: i32,
        model_name_hash: i32,
        local_player: bool,
    },
    SetInterfaceObject {
        packed: u32,
        object: i32,
        count: i32,
    },
    SetInterfaceHttpImage {
        packed: u32,
        image: i32,
    },
    SetMapFlag {
        x: i32,
        z: i32,
    },
    MinimapToggle {
        toggle: i32,
    },
    /// `HINT_ARROW` (78/14): retained by the minimap owner after the
    /// nine-slot marker update and target decoding.
    HintArrow {
        bytes: Vec<u8>,
    },
    /// `HINT_TRAIL` (119/-2): replace one of the eight stepped model
    /// trails. The app materializes the path through the transient scene
    /// owner, so the packet carries decoded points rather than raw bytes.
    HintTrail {
        slot: usize,
        model: i32,
        points: Vec<[i32; 2]>,
    },
    SetInterfaceColour {
        packed: u32,
        colour: u16,
    },
    SetInterfaceAngle {
        packed: u32,
        x: u16,
        y: u16,
        zoom: u16,
    },
    SetInterfaceGraphic {
        packed: u32,
        graphic: i32,
    },
    SetInterfaceTextAntiMacro {
        packed: u32,
        enabled: bool,
    },
    SetInterfaceTextFont {
        packed: u32,
        font: i32,
    },
    SetInterfaceClickMask {
        packed: u32,
        enabled: bool,
    },
    SetInterfaceRecolour {
        packed: u32,
        index: u8,
        source: u16,
        destination: u16,
    },
    SetInterfaceRetexture {
        packed: u32,
        index: u8,
        source: u16,
        destination: u16,
    },
    SetMoveAction {
        text: String,
        action: i32,
    },
    ShowFaceHere {
        enabled: bool,
    },
    PlayerAttackPriority {
        value: u8,
    },
    /// `REDUCE_NPC_ATTACK_PRIORITY` (141/1): NPC attack menu policy.
    NpcAttackPriority {
        value: u8,
    },
    SetPlayerOp {
        slot: u8,
        cursor: i32,
        name: Option<String>,
        deprioritised: bool,
    },
    RunEnergy {
        value: u8,
    },
    RunWeight {
        value: i16,
    },
    /// `UPDATE_REBOOT_TIMER` (177/2): server reboot countdown seed.
    /// The retained runtime applies the lobby/world scale, then decrements
    /// the timer once per game cycle.
    RebootTimer {
        ticks: u16,
    },
    /// `SET_TARGET` (150/2): the active target, used by player scene
    /// priority to keep the server-targeted actor on top.
    SetTarget {
        value: i16,
    },
    /// `SETDRAWORDER` (47/1): player overlap ordering mode.
    SetDrawOrder {
        value: u8,
    },
    /// `CHAT_FILTER_SETTINGS` (162/2): trade and public filter values.
    ChatFilters {
        trade: i32,
        public: i32,
    },
    /// `CHAT_FILTER_SETTINGS_PRIVATECHAT` (96/1): private filter enum value.
    ChatPrivateFilter {
        value: Option<i32>,
    },
    /// `UPDATE_DOB` (126/4): lobby date-of-birth value and verification bit.
    UpdateDob {
        dob: i32,
        verified: bool,
    },
    /// `LOYALTY_UPDATE` (188/4): lobby loyalty balance.
    LoyaltyUpdate {
        value: i32,
    },
    /// `JCOINS_UPDATE` (194/4): lobby JCoins balance.
    JCoinsUpdate {
        value: i32,
    },
    /// `CREATE_CHECK_EMAIL_REPLY` (3/1): account-creation email result.
    CreateEmailReply {
        value: i32,
    },
    /// `CREATE_ACCOUNT_REPLY` (131/1): account-creation result.
    AccountCreationResult {
        value: i32,
    },
    /// `CREATE_CHECK_NAME_REPLY` (163/1): account-creation name result.
    CreateNameReply {
        value: i32,
    },
    /// `CREATE_SUGGEST_NAME_ERROR` (19/1): failed name suggestion result.
    CreateSuggestNameError {
        value: i32,
    },
    /// `CREATE_SUGGEST_NAME_REPLY` (66/-1): suggested account name.
    CreateSuggestName {
        value: String,
    },
    /// `TRIGGER_ONDIALOGABORT` (133/0): run the opened-top dialog-abort hooks.
    TriggerDialogAbort,
    /// `MESSAGE_GAME` (120/-1): regular game chat-history message.
    GameMessage {
        chat_type: i32,
        flags: i32,
        name: String,
        name_unfiltered: String,
        message: String,
    },
    /// `UPDATE_FRIENDLIST` (46/-2): retained friend-list delta batch.
    FriendList {
        bytes: Vec<u8>,
    },
    /// `FRIENDLIST_LOADED` (25/0): friend/ignore list loading marker.
    FriendListLoaded,
    /// `UPDATE_IGNORELIST` (174/-2): retained ignore-list delta batch.
    IgnoreList {
        bytes: Vec<u8>,
    },
    /// `UPDATE_FRIENDCHAT_CHANNEL_FULL` (23/-2): retained friends-chat roster.
    FriendChatFull {
        bytes: Vec<u8>,
    },
    /// `UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER` (2/-1): roster delta.
    FriendChatSingle {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_FRIENDCHANNEL` (36/-1): packed friends-chat message.
    FriendChannelMessage {
        bytes: Vec<u8>,
    },
    /// `CLANCHANNEL_FULL` (59/-2): affined/listened clan-channel snapshot.
    ClanChannelFull {
        bytes: Vec<u8>,
    },
    /// `CLANCHANNEL_DELTA` (146/-2): clan-channel roster delta.
    ClanRosterDelta {
        bytes: Vec<u8>,
    },
    /// `CLANSETTINGS_FULL` (101/-2): affined/listened clan-settings snapshot.
    ClanSettingsFull {
        bytes: Vec<u8>,
    },
    /// `CLANSETTINGS_DELTA` (132/-2): ordered clan-settings mutation batch.
    ClanSettingsUpdate {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_CLANCHANNEL` (22/-1): packed clan-channel message.
    ClanChannelMessage {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_CLANCHANNEL_SYSTEM` (113/-1): system message for a channel.
    ClanChannelSystemMessage {
        bytes: Vec<u8>,
    },
    /// `PLAYER_GROUP_FULL` / `PLAYER_GROUP_DELTA`: retained group presence.
    PlayerGroupFull {
        bytes: Vec<u8>,
    },
    GroupRosterDelta {
        bytes: Vec<u8>,
    },
    /// `PLAYER_GROUP_VARPS` (159/-2): sparse variables for one group member.
    PlayerGroupVars {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_PLAYER_GROUP` (41/-1): group chat with a packed body.
    PlayerGroupMessage {
        bytes: Vec<u8>,
    },
    /// Server quick-chat families share one phrase-type renderer but
    /// carry different retained channel metadata.
    QuickChat {
        opcode: u8,
        bytes: Vec<u8>,
    },
    /// `MESSAGE_PUBLIC` (8/-1): player index, public-chat flags, crown and
    /// packed/quick-chat body; the retained UI resolves the player name.
    PublicMessage {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_PRIVATE_ECHO` (7/-2): sender name followed by a packed
    /// compressed message body.
    PrivateMessageEcho {
        bytes: Vec<u8>,
    },
    /// `MESSAGE_PRIVATE` (90/-2): deduplicated private chat with a packed
    /// body and chat crown metadata.
    PrivateMessage {
        bytes: Vec<u8>,
    },
    /// `LOGOUT`/`LOGOUT_FULL` (168/1, 86/1): server-directed session stop.
    Logout {
        reason: u8,
        full: bool,
    },
    /// `CHANGE_LOBBY`: the client updates the lobby endpoint while the
    /// current session remains alive.
    ChangeLobby {
        host: String,
        node: u16,
        port: u16,
        port2: u16,
    },
    /// `LOGOUT_TRANSFER`: the client schedules a world transfer and enters login
    /// state 19; the app owner consumes this transition and stops the stream.
    LogoutTransfer {
        world_id: u16,
        host: String,
        port: u16,
        port2: u16,
        cancellable: bool,
    },
    /// `REFLECTION_CHECKER` (111/-2): queues one check; the game update
    /// answers it after the read batch while in game.
    ReflectionProbe(crate::reflection_check::Check),
    /// `JS5_RELOAD` (153/0): reload the cache connection. Ends the read batch.
    Js5Reload,
    /// `EXECUTE_CLIENT_CHEAT` (24/2): run the developer-console command
    /// with this id.
    ExecuteClientCheat {
        id: u16,
    },
    /// `DO_CHEAT` (117/-1): run the developer-console command text (not
    /// scripted, no suggestion).
    DoCheat {
        command: String,
    },
    /// `DEBUG_SERVER_TRIGGERS` (52/-1): per-component `serverTriggers` for
    /// components `[start, end)` of a loaded interface.
    /// `values` holds every complete `g3` after the header; they are applied
    /// only when the interface is loaded.
    DebugServerTriggers {
        interface: u16,
        start: u16,
        end: u16,
        values: Vec<i32>,
    },
    /// `UNHANDLED_51` (51/-2) and `UNHANDLED_170` (170/-2) have no handler:
    /// the final branch reports the packet and logs out.
    UnhandledPacket {
        opcode: u8,
        size: usize,
    },
    /// A known packet whose payload does not decode: the read reports it and
    /// logs out.
    MalformedPacket {
        opcode: u8,
        size: usize,
        reason: String,
    },
    /// `RUNCLIENTSCRIPT` (156/-2): execute a client script outside `onLoad`.
    /// `app.rs` routes this through the same `VmAdapter` as `onLoad`s.
    RunScript(RunClientScript),
}

/// Server opcodes routed to [`State::telemetry_packet`].
pub fn is_telemetry_opcode(opcode: u8) -> bool {
    use crate::proto::server as p;
    matches!(
        opcode,
        p::TELEMETRY_GRID_FULL
            | p::TELEMETRY_CLEAR_GRID_VALUE
            | p::TELEMETRY_GRID_ADD_ROW
            | p::TELEMETRY_GRID_REMOVE_COLUMN
            | p::TELEMETRY_GRID_REMOVE_GROUP
            | p::TELEMETRY_GRID_ADD_GROUP
            | p::TELEMETRY_GRID_MOVE_ROW
            | p::TELEMETRY_GRID_REMOVE_ROW
            | p::TELEMETRY_GRID_VALUES_DELTA
            | p::TELEMETRY_GRID_SET_ROW_PINNED
            | p::TELEMETRY_GRID_MOVE_COLUMN
            | p::TELEMETRY_GRID_ADD_COLUMN
    )
}

/// Route one server frame to a [`UiEvent`]: `Ok(None)` for non-interface
/// frames (`REBUILD_NORMAL`/`PLAYER_INFO`/`NO_TIMEOUT`/`SERVER_TICK_END`/
/// zone/varcache), `Ok(Some(_))` for the UI operations above,
/// and `Err` for a malformed UI payload. The caller owns error policy.
pub fn crc32(data: &[u8]) -> u32 {
    // The IEEE CRC-32; one copy in rs910-core since Phase 2.1 (this bitwise
    // form computed the same value; client910's `core_goldens::checksum`).
    rs910_core::checksum::crc32(data)
}

/// Debug/transport packets that are not interface
/// updates: `REFLECTION_CHECKER`, `JS5_RELOAD`, `EXECUTE_CLIENT_CHEAT`,
/// `DO_CHEAT`, `DEBUG_SERVER_TRIGGERS` and the two table entries that are
/// never dispatched (`UNHANDLED_51`, `UNHANDLED_170`).
fn parse_client_debug_event(opcode: u8, payload: &[u8]) -> anyhow::Result<Option<UiEvent>> {
    use crate::proto::server as p;
    Ok(Some(match opcode {
        p::REFLECTION_CHECKER => {
            UiEvent::ReflectionProbe(crate::reflection_check::decode(payload)?)
        }
        p::JS5_RELOAD => {
            anyhow::ensure!(payload.is_empty(), "JS5_RELOAD length");
            UiEvent::Js5Reload
        }
        p::EXECUTE_CLIENT_CHEAT => {
            let mut r = PayloadReader::new(payload);
            let id = r.g2()?;
            r.finish("EXECUTE_CLIENT_CHEAT")?;
            UiEvent::ExecuteClientCheat { id }
        }
        p::DO_CHEAT => {
            let mut r = PayloadReader::new(payload);
            let command = r.gjstr()?;
            r.finish("DO_CHEAT")?;
            UiEvent::DoCheat { command }
        }
        p::DEBUG_SERVER_TRIGGERS => {
            let mut r = PayloadReader::new(payload);
            let interface = r.g2()?;
            let start = r.g2()?;
            let end = r.g2()?;
            let mut values = Vec::with_capacity(r.remaining() / 3);
            while r.remaining() >= 3 {
                // Packet.g3 (unsigned 24-bit).
                values.push(r.g3s()? & 0xff_ffff);
            }
            UiEvent::DebugServerTriggers {
                interface,
                start,
                end,
                values,
            }
        }
        p::UNHANDLED_51 | p::UNHANDLED_170 => UiEvent::UnhandledPacket {
            opcode,
            size: payload.len(),
        },
        _ => return Ok(None),
    }))
}
