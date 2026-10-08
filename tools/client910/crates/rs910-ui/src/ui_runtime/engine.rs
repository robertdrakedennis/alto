//! The retained engine state (`ui_runtime::Engine`) and its owned
//! sub-states (code-quality programme Phase 4.2). The groups follow the
//! subsystems that own the data (login, minimenu, chat history, window
//! shell, ...); command handlers and packet appliers borrow them disjointly.
use super::{
    crown_image, host_builtins, host_game, ActiveChatPhrase, ActiveEntity, CoverMarker, Cross,
    CrownedLine, FullscreenMode, HintTrail, LobbyEnterGameRequest, LoginRequest, OverheadChat,
    PlayerOp, SoundRequest, StockmarketSlot, WorldSwitchRequest,
};
use crate::ui_chat::NewChatLine;
use std::collections::BTreeMap;

/// Unsupported engine calls remain terminal for the current hook. Initial
/// state queries must be installed from their real owners, never preview zeros.
pub struct Engine {
    /// Scene-facing state: the active entity and pick frame of the scene
    /// menu, route targets, `activeTarget`/`drawOrder`, the server roof
    /// and the retained hint trails.
    pub scene: SceneState,
    /// `MiniMenu` state owned by the engine: attack priorities, server player
    /// ops, the move/face-here options, the cross, cursor defaults and the
    /// entry snapshots direct engine scripts read.
    pub menu: MenuState,
    /// The minimap flag/toggle and the anticheat camera values sent with
    /// minimap clicks.
    pub minimap: MinimapState,
    /// The config/type stores the engine commands read (`Loading`
    /// `SETUP_CONFIG_DECODERS`), installed by `load_cache`.
    pub configs: Configs,
    /// `ChatHistory`, the chat filters, the active quick-chat phrase and the
    /// `messageIds` deduplication window.
    pub messages: ChatState,
    /// Requests the engine queues for other owners (audio, renderer,
    /// browser/window, minimap, actors, console), drained after the tick or
    /// packet that produced them.
    pub effects: Effects,
    /// Login and world-switch state the login and lobby scripts read and
    /// the requests the app session owner consumes after the tick.
    pub login: LoginState,
    /// The lobby login profile.
    pub lobby: LobbyProfile,
    /// Account-creation replies and the create-connect request.
    pub creation: AccountCreation,
    /// Account flags from the login replies.
    pub account: AccountFlags,
    /// Window, input and device state the window owner installs before each
    /// tick (window shell, fullscreen, mouse and keyboard snapshots).
    pub platform: Platform,
    /// `cam2`, the staff free camera and the UI-side cutscene statics.
    pub camera: CameraState,
    /// `stockmarketSlots[3][8]` used by stockmarket CS2 hosts.
    pub stockmarket_slots: [[StockmarketSlot; 8]; 3],
    pub unsupported: BTreeMap<String, usize>,
    /// original friends/ignores and friendsListState, fed by the normal
    /// UPDATE_FRIENDLIST/UPDATE_IGNORELIST packets.
    pub social: crate::ui_social::State,
    pub outgoing: Vec<u8>,
    /// The `ClientWorldMap`/`WorldMap` statics (`world_map_client.rs`),
    /// shared with the draw owner (`ui_backend::Target::world_map`) and the
    /// component input walk (`ui_loop::WorldMapInteraction`).
    pub world_map: std::rc::Rc<std::cell::RefCell<crate::world_map_client::ClientWorldMap>>,
    /// `recentUse` (server inventory packets feed it).
    pub inv_cache: crate::ui_inv::InvCache,
    /// `rebootTimer` (UPDATE_REBOOT_TIMER sets
    /// it, updateGame counts it down). The current server never sends it.
    pub reboot_timer: i32,
    /// The `random` / `randominc` generator: a 48-bit LCG seeded from the
    /// clock, so only the generator is faithful, never the sequence.
    pub random_seed: u64,
    pub world_list: crate::ui_world_list::State,
    /// Client singletons behind the `host_builtins` CS2 commands.
    pub builtins: host_builtins::State,
    /// Game/UI-coupled command state (ui_host_game.rs).
    pub game_host: host_game::State,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            scene: SceneState::default(),
            menu: MenuState::default(),
            minimap: MinimapState::default(),
            configs: Configs::default(),
            messages: ChatState::default(),
            effects: Effects::default(),
            login: LoginState::default(),
            lobby: LobbyProfile::default(),
            creation: AccountCreation::default(),
            account: AccountFlags::default(),
            platform: Platform::default(),
            camera: CameraState::default(),
            stockmarket_slots: [[StockmarketSlot::default(); 8]; 3],
            unsupported: BTreeMap::new(),
            social: Default::default(),
            outgoing: Vec::new(),
            world_map: Default::default(),
            inv_cache: crate::ui_inv::InvCache::default(),
            reboot_timer: 0,
            random_seed: ((crate::logic_clock::monotonic_millis() as u64) ^ 0x5DEE_CE66D)
                & ((1 << 48) - 1),
            world_list: Default::default(),
            builtins: host_builtins::State::default(),
            game_host: host_game::State::default(),
        }
    }
}

/// Scene-facing state: the active entity and pick frame of the scene
/// menu, route targets, `activeTarget`/`drawOrder`, the server roof
/// and the retained hint trails.
pub struct SceneState {
    /// `activeEntity` for an active-sub trigger. The
    /// scene owner supplies this only for the duration of that script.
    pub active_entity: Option<ActiveEntity>,
    pub player_picks: Option<crate::player_picking::Frame>,
    /// The matrices of the last scene draw that was not blacked out (`None`
    /// before the first draw); the ground pick unprojects through them.
    pub drawn_view: Option<crate::ui_scene_options::DrawnView>,
    pub player_routes: BTreeMap<usize, [i32; 2]>,
    /// NPC membership with each NPC's first route waypoint, the minimap flag
    /// target of OPNPC/OPNPCT.
    pub npc_routes: BTreeMap<usize, [i32; 2]>,
    /// Cover-marker rectangles from the previous scene draw, consumed by the
    /// next shaped like the original `addSceneOptions` pass.
    pub cover_markers: Vec<CoverMarker>,
    /// `activeTarget`, used by the player
    /// scene owner when prioritising the server-targeted actor.
    pub active_target: i32,
    /// `drawOrder`, used by player overlap
    /// ordering in the scene owner.
    pub draw_order: i32,
    /// `serverRoofX/Z` selected by CAM_REMOVEROOF and consumed by the
    /// live roof flood-fill owner during the next scene draw.
    pub server_roof: [i32; 2],
    /// `hintTrails[8]`; the app scene owner turns each retained
    /// path into ordinary transient model entities on the next frame.
    pub hint_trails: [Option<HintTrail>; 8],
}

impl Default for SceneState {
    fn default() -> Self {
        Self {
            active_entity: None,
            player_picks: None,
            drawn_view: None,
            player_routes: BTreeMap::new(),
            npc_routes: BTreeMap::new(),
            cover_markers: Vec::new(),
            active_target: 0,
            draw_order: 0,
            server_roof: [-1; 2],
            hint_trails: std::array::from_fn(|_| None),
        }
    }
}

/// `MiniMenu` state owned by the engine: attack priorities, server player
/// ops, the move/face-here options, the cross, cursor defaults and the
/// entry snapshots direct engine scripts read.
pub struct MenuState {
    pub player_attack_priority: crate::ui_player_options::AttackPriority,
    pub npc_attack_priority: crate::ui_player_options::AttackPriority,
    pub player_ops: [Option<PlayerOp>; 8],
    pub show_single_option_menu: bool,
    /// Snapshot consumed by direct Engine-host scripts outside the component
    /// HookHost. Runtime refreshes it after the original client's per-cycle update.
    pub active: crate::ui_minimenu::EntryView,
    pub secondary: crate::ui_minimenu::EntryView,
    pub counts: [i32; 2],
    /// `walkHereText` / `defaultWalkAction`
    /// `SET_MOVEACTION` overrides them).
    pub walk_here_text: String,
    pub default_walk_action: i32,
    /// `showFaceHere`, controlled by server packet76.
    pub show_face_here: bool,
    pub cross: Cross,
    /// The default menu cursors, set by `setdefaultcursors`.
    pub default_cursors: [i32; 2],
    /// Once-logged gaps of the menu click path.
    pub gaps: BTreeMap<String, usize>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            player_attack_priority: crate::ui_player_options::AttackPriority::HigherLevelRight,
            npc_attack_priority: crate::ui_player_options::AttackPriority::HigherLevelRight,
            player_ops: Default::default(),
            show_single_option_menu: true,
            active: crate::ui_minimenu::MiniMenu::default().view(None),
            secondary: crate::ui_minimenu::MiniMenu::default().view(None),
            counts: [0; 2],
            walk_here_text: rs910_core::texts::Msg::WalkHere.get().to_string(),
            default_walk_action: -1,
            show_face_here: false,
            cross: Cross::default(),
            default_cursors: [-1, -1],
            gaps: BTreeMap::new(),
        }
    }
}

/// The minimap flag/toggle and the anticheat camera values sent with
/// minimap clicks.
#[derive(Default)]
pub struct MinimapState {
    /// `flagSceneTileX/Z` set by `createMoveMessage` / `setMinimapFlag`,
    /// taken by the minimap owner (app) each frame.
    pub flag: Option<[i32; 2]>,
    /// The minimap toggle, copied to the app minimap owner during the render
    /// frame.
    pub toggle: i32,
    /// The orbit camera yaw as sent in the `MOVE_MINIMAPCLICK` tail. The
    /// original client seeds it randomly (sequence unreproducible here); live
    /// orbit updates and the anticheat drift below have no owner yet, so this
    /// stays 0 until then.
    pub orbit_yaw: i32,
    /// The minimap anticheat angle (randomly initialised in the original;
    /// per-tick drift open), sent as `p1` in the click tail.
    pub angle: i32,
    /// The minimap zoom (randomly initialised in the original; drift open),
    /// sent as `p1` in the click tail.
    pub zoom: i32,
}

/// The config/type stores the engine commands read (`Loading`
/// `SETUP_CONFIG_DECODERS`), installed by `load_cache`.
#[derive(Default)]
pub struct Configs {
    /// Inventory sizes from `client.config.js5` group 5 (`Js5ConfigGroup.INVTYPE`).
    pub inv_sizes: BTreeMap<i32, i32>,
    /// Param defaults from `client.config.js5` group 11. NPC/custom overrides
    /// remain a future service; fresh-login NPCs carry none.
    pub params: BTreeMap<i32, native910::config::ParamConfig>,
    /// `objTypeList`, including derived templates.
    pub objs: Option<std::rc::Rc<crate::config::ObjStore>>,
    /// `locTypeList`, used by ordinary scene-menu
    /// construction for locations captured from the installed scene graph.
    pub locs: Option<std::rc::Rc<crate::config::LocStore>>,
    /// `npcTypeList`, used by the retained NPC
    /// menu owner for operation names and active-state filtering.
    pub npcs: Option<std::rc::Rc<crate::config::NpcStore>>,
    /// `seqTypeList`, used by the `seq_param` CS2 host.
    pub seqs: Option<std::rc::Rc<crate::config::SeqStore>>,
    /// `basTypeList`, config archive group 32.
    pub bas: Option<BTreeMap<i32, crate::protocol910::bas_types::Bas>>,
    /// `miniMenuDefaults` (defaults archive group 7).
    pub minimenu: Option<crate::ui_defaults::MiniMenuDefaults>,
    pub db: crate::ui_db::Tables,
    pub imported_enums:
        BTreeMap<native910::execution::ResourceDigest, rs910_config::ui_enum_resource::Library>,
    pub imported_databases:
        BTreeMap<native910::execution::ResourceDigest, rs910_config::ui_db_schema::Database>,
    pub map_element_types: Option<crate::minimap::MapElementStore>,
    pub inv_varbits: Option<std::sync::Arc<crate::scenery_varbits::Inputs>>,
    /// The Huffman coder for chat text, loaded from the binary JS5 archive.
    pub wordpack: Option<crate::wordpack::Huffman>,
    /// The quick-chat phrase types, loaded from the two JS5 quick-chat archives.
    pub quickchat: Option<crate::ui_social::QuickChatStore>,
    /// The skill definitions (experience tables), which turn a group member's
    /// experience into a level.
    pub skills: Option<crate::ui_stats::SkillDefaults>,
}

/// `ChatHistory`, the chat filters, the active quick-chat phrase and the
/// `messageIds` deduplication window.
pub struct ChatState {
    pub history: crate::ui_chat::ChatHistory,
    pub changed: bool,
    /// Client. A fresh session has public/trade zero,
    /// private null; filter packets and user mutations are separate services.
    pub filters: [Option<i32>; 3],
    /// `activeChatPhrase`, retained between prepare,
    /// dynamic setters and the outgoing quick-chat command.
    pub active_chat_phrase: Option<ActiveChatPhrase>,
    /// `chatPhraseFindResults` and its cursor.
    pub phrase_find_results: Option<Vec<u16>>,
    pub phrase_find_index: usize,
    /// messageIds/messageCount: private-message deduplication window.
    /// `messageIds`/`messageCount`: one zero-initialised 100-slot
    /// ring shared by every uid-carrying message packet.
    pub message_ids: [i64; 100],
    pub message_count: usize,
}

impl ChatState {
    /// A chat line added by an owner outside `Runtime::packet` still takes
    /// the `ChatHistory` redraw (`lastOnChatTransmitRedrawCycle`) path.
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }

    /// The `messageIds` scan of every uid-carrying message (e.g.
    pub fn message_seen(&self, id: i64) -> bool {
        self.message_ids.contains(&id)
    }

    pub fn record_message(&mut self, id: i64) {
        self.message_ids[self.message_count] = id;
        self.message_count = (self.message_count + 1) % 100;
    }

    /// Adds a chat line with a crown: names take the crown image tag prefix
    /// and the line keeps the crown id.
    pub(super) fn add_crowned_line(&mut self, line: CrownedLine<'_>) {
        let CrownedLine {
            chat_type,
            name,
            name_unfiltered,
            name_simple,
            clan,
            phrase,
            message,
            crown,
        } = line;
        let image = crown_image(i32::from(crown));
        let tag = |n: &str| image.map_or_else(|| n.to_string(), |img| format!("<img={img}>{n}"));
        self.history.add_message(NewChatLine {
            name: tag(name),
            name_unfiltered: tag(name_unfiltered),
            name_simple: name_simple.to_owned(),
            clan,
            phrase,
            crown: Some(i32::from(crown)),
            ..NewChatLine::system(chat_type, message)
        });
        self.changed = true;
    }
    /// A type-0 system line from a client-owned path, with the same chat
    /// redraw as `mes`.
    pub fn system_message(&mut self, message: impl Into<String>) {
        self.history.mes(message);
        self.changed = true;
    }
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            history: Default::default(),
            changed: false,
            filters: [Some(0), None, Some(0)],
            active_chat_phrase: None,
            phrase_find_results: None,
            phrase_find_index: 0,
            message_ids: [0; 100],
            message_count: 0,
        }
    }
}

/// Requests the engine queues for other owners (audio, renderer,
/// browser/window, minimap, actors, console), drained after the tick or
/// packet that produced them.
#[derive(Default)]
pub struct Effects {
    /// `sound_*` commands. Requests are retained for the audio
    /// service; UI execution does not depend on decoder/device readiness.
    pub sounds: Vec<SoundRequest>,
    /// Sequence sounds the game raised for animated entities, played by the
    /// audio owner after the tick.
    pub sequence_sounds: Vec<rs910_game::sequence_sound::SequenceSound>,
    /// Scene-owned point-light mutations are decoded with the normal server
    /// frame path, then consumed by the renderer owner after packet dispatch.
    pub point_light_updates: Vec<crate::server_prot::UiEvent>,
    /// `URL_OPEN` requests retained until the platform/browser owner
    /// consumes them after the live packet tick.
    pub browser_urls: Vec<crate::server_prot::UiEvent>,
    /// A queued URL was opened by URL_OPEN or
    /// `openurl_nologin`, which first leave
    /// fullscreen when `Fullscreen.allowed`; the window owner applies it
    /// before the browser opens.
    pub browser_fullscreen_exit: bool,
    /// Environment overrides decoded from the live server stream and applied
    /// by the renderer owner after packet dispatch.
    pub environment_overrides: Vec<crate::server_prot::EnvironmentOverrideEvent>,
    /// `hintArrows`: packet updates are consumed by the app's
    /// retained minimap owner after the normal UI dispatch.
    pub hint_arrow_updates: Vec<Vec<u8>>,
    /// `addMessage` requests from `MESSAGE_PUBLIC`
    /// applied by the app owner to the live actor table
    /// with `getLogicRate() * graphicsDefaults.playerChatTimeout`
    pub overhead_chat: Vec<OverheadChat>,
    /// Developer-console lines received through MESSAGE_GAME types 98/99;
    /// the window owner drains these into its existing Console owner.
    pub console_messages: Vec<String>,
}

/// Login and world-switch state the login and lobby scripts read and the
/// requests the app session owner consumes after the tick.
pub struct LoginState {
    /// `state == 13`, installed by the app's `login_state::Machine`
    /// owner before each tick (the original client's lobby-only command gates).
    pub lobby_login: bool,
    /// `state == 0` (the account-creation lobby connection, entered on
    /// a successful CREATE_ACCOUNT_CONNECT reply), installed by the same
    /// owner. Every create_* lobby request is a no-op outside it.
    pub account_creation_connected: bool,
    /// `isStateGame`, installed by the same
    /// owner: the active game connection may also fetch the world list.
    pub world_list_game: bool,
    /// `state == 4 && !isInProgress()` plus the account
    /// creation gate: `isLoginReady`,
    /// which `requestLogin`/`enterLobby` check before accepting credentials.
    pub ready: bool,
    /// Pending `setWorld` request. The app session owner
    /// consumes this after the VM tick and updates `currentWorld`; the next
    /// `lobby_entergame` logs into it.
    pub world_switch: Option<WorldSwitchRequest>,
    /// `targetWorld`, retained from the lobby profile so
    /// `worldlist_autoworld` can restore the advertised world before the
    /// normal lobby-to-game transition.
    pub target_world: Option<WorldSwitchRequest>,
    /// Pending original login/lobby request emitted by the retained VM.
    pub request: Option<LoginRequest>,
    /// Pending `enterGame` transition from lobby state 13.
    pub lobby_enter_game: Option<LobbyEnterGameRequest>,
    /// `cancelLogin` is consumed by the app session owner
    /// after the retained VM tick so it can stop the in-flight worker.
    pub cancel_requested: bool,
    /// `logout(false)` request emitted by `lobby_leavelobby`.
    pub logout_requested: bool,
    /// `login_continue` was called; consumed by the app session owner, which
    /// resumes a login parked on reply 1.
    pub continue_requested: bool,
    /// `resend_uid_passport_request` while a lobby login waits on the device
    /// check (`state == 17`): the app session owner sends the request on the
    /// waiting login connection.
    pub resend_uid_passport_requested: bool,
    /// `state == 17`, installed by the session owner: a lobby login is running.
    pub lobby_logging_in: bool,
    /// Login state exposed to login scripts while the app worker runs.
    pub in_progress: bool,
    /// `enterGameReply`: world logins
    /// (`requestState == 211`), read by `login_reply`/`lobby_entergamereply`.
    pub reply: i32,
    /// `localPlayerEntity.name`/`nameUnfiltered` from the lobby login
    /// reply while no game local player exists.
    pub lobby_player_name: String,
    /// `enterLobbyReply`: lobby logins
    /// (`requestState == 132`), read by `lobby_enterlobbyreply`.
    pub lobby_reply: i32,
    /// `hoptime` / `banDuration` terminal reply values.
    pub hoptime: i32,
    pub ban_duration: i32,
    pub queue_position: i32,
    /// World-transfer outcome fields consumed once by
    /// `login_last_transfer_reply` after a state-19 world handoff.
    pub last_transfer_reply: i32,
    pub last_transfer_disallow_result: i32,
    pub last_transfer_disallow_trigger: i32,
    /// `disallowResult` / `disallowTrigger` values.
    pub disallow_result: i32,
    pub disallow_trigger: i32,
    /// The user-flow words (second, first) and the automated-test flag
    /// words, from the launcher parameters. They are retained by the login
    /// owner, exposed to the ordinary login scripts and sent in every login.
    pub user_flow: [i32; 2],
    pub automated_test_flags: [i32; 2],
    /// `siteSettings`, sent in the next login payload.
    pub site_settings: String,
    /// The original client's persisted `uid192`, updated only after CRC validation.
    pub uid192: [u8; 24],
    /// The host the account last connected from (`lastlogin`), resolving.
    pub last_login: Option<crate::host_name::HostName>,
    /// The client state, published each cycle for the packets whose reading
    /// depends on it.
    pub client_state: i32,
    /// `currentWorld.node`: this client logs
    /// into world 1 (`net::login_world`).
    pub world: i32,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            lobby_login: false,
            account_creation_connected: false,
            world_list_game: false,
            ready: false,
            world_switch: None,
            target_world: None,
            request: None,
            lobby_enter_game: None,
            cancel_requested: false,
            logout_requested: false,
            continue_requested: false,
            resend_uid_passport_requested: false,
            lobby_logging_in: false,
            in_progress: false,
            reply: -2,
            lobby_player_name: String::new(),
            lobby_reply: -2,
            hoptime: 0,
            ban_duration: 0,
            queue_position: -1,
            last_transfer_reply: -2,
            last_transfer_disallow_result: -1,
            last_transfer_disallow_trigger: -1,
            disallow_result: -1,
            disallow_trigger: -1,
            user_flow: crate::applet_params::get().user_flow(),
            automated_test_flags: crate::applet_params::get().automated_test_flags(),
            site_settings: crate::applet_params::get().site_settings(),
            uid192: [0u8; 24],
            last_login: None,
            client_state: 0,
            world: 1,
        }
    }
}

/// The lobby login profile (`lobby*`,).
#[derive(Default)]
pub struct LobbyProfile {
    /// The lobby membership fields, installed from the parsed lobby login
    /// profile.
    pub membership: i64,
    pub membership_offset: i64,
    pub membership_flag: bool,
    /// `lobbyUnreadMessages`, same gap.
    pub unread_messages: i32,
    /// Remaining lobby profile values used by the login/account scripts.
    pub recovery_day: i32,
    pub cc_expiry: i32,
    pub grace_expiry: i32,
    pub dob_requested: bool,
    pub members_stats: i32,
    pub play_age: i32,
    /// `lobbyLoyaltyBalance`, updated by
    /// `LOYALTY_UPDATE` and exposed to lobby scripts.
    pub loyalty_balance: i32,
    /// `lobbyJCoinsBalance`, updated by
    /// `JCOINS_UPDATE` and exposed to lobby scripts.
    pub jcoins_balance: i32,
    /// `lobbyEmailStatus`, same gap.
    pub email_status: i32,
    /// `lobbyLastLoginDay`, same gap.
    pub last_login_day: i32,
}

/// Account-creation replies and the create-connect request.
pub struct AccountCreation {
    /// The email reply, exposed by
    /// `create_email_validate_reply` to the lobby creation scripts.
    pub email_reply: i32,
    /// The account reply, exposed by `create_reply`.
    pub account_reply: i32,
    /// The name reply, exposed by
    /// `create_name_validate_reply` to the lobby creation scripts.
    pub name_reply: i32,
    /// The suggestion reply and the suggested name.
    pub suggest_reply: i32,
    pub suggested_name: Option<String>,
    /// `isUnder13`, retained for account-creation CS2 queries and
    /// set by the original account stage/under-13 command path.
    pub is_under13: bool,
    /// Account-creation socket owner. The app consumes the request after the
    /// VM tick and installs the returned lobby stream before create packets
    /// are emitted.
    pub connect_requested: bool,
    pub connect_in_progress: bool,
    pub connect_reply: i32,
}

impl Default for AccountCreation {
    fn default() -> Self {
        Self {
            email_reply: -2,
            account_reply: -2,
            name_reply: -2,
            suggest_reply: -2,
            suggested_name: None,
            is_under13: false,
            connect_requested: false,
            connect_in_progress: false,
            connect_reply: -2,
        }
    }
}

/// Account flags from the login replies.
#[derive(Default)]
pub struct AccountFlags {
    /// `loggedInMembers`, installed from the
    /// world login reply; false until then and in the lobby.
    pub logged_in_members: bool,
    /// `playerIsMembers`, distinct from the logged-in-world
    /// membership flag used by cache/object gates.
    pub player_is_members: bool,
    pub player_is_quickchat: bool,
    pub logged_in_quickchat: bool,
    pub dob_verified: bool,
    pub dob: i32,
    /// `staffModLevel` (set at).
    pub staff_mod_level: i32,
    /// `playerModLevel`, distinct from `staffModLevel`.
    pub player_mod_level: i32,
}

/// Window, input and device state the window owner installs before each
/// tick (window shell, fullscreen, mouse and keyboard snapshots).
pub struct Platform {
    /// getWindowMode: this application's window is resizable, windowed.
    pub window_mode: i32,
    /// `Fullscreen.allowed` and the display modes the
    /// window owner installs (`getFullscreenModes`).
    pub fullscreen_allowed: bool,
    /// The window shell remembers the last successfully entered exclusive mode.
    pub last_fullscreen_size: [i32; 2],
    pub safe_mode: bool,
    pub chose_safe_mode: bool,
    /// Installed physical memory in megabytes, as the machine probe reports
    /// it (0 until the shell installs the probe's figure).
    pub physical_memory_mb: i32,
    pub fullscreen_modes: Option<Vec<FullscreenMode>>,
    /// The mouse snapshots the cursor once per logic update.
    pub mouse: [i32; 2],
    pub pending_mouse: [i32; 2],
    /// `mouse.isLeft/Middle/RightButtonHeld` (`get_mousebuttons`).
    pub mouse_buttons: [bool; 3],
    /// `hasFocus`: updated from the native window owner before
    /// each retained VM tick.
    pub app_focused: bool,
    /// `textureFormat` of the native device
    /// (`client_watch::gl_compressed_texture_formats`), installed by the
    /// window owner.
    pub gl_texture_formats: Vec<i32>,
    /// Keyboard held state sampled before this cycle's scripts.
    pub held_keys: [bool; 112],
}

impl Default for Platform {
    fn default() -> Self {
        Self {
            window_mode: 2,
            fullscreen_allowed: true,
            last_fullscreen_size: [0; 2],
            safe_mode: false,
            chose_safe_mode: false,
            physical_memory_mb: 0,
            fullscreen_modes: None,
            mouse: [0; 2],
            pending_mouse: [0; 2],
            mouse_buttons: [false; 3],
            app_focused: true,
            gl_texture_formats: Vec::new(),
            held_keys: [false; 112],
        }
    }
}

/// `cam2`, the staff free camera and the UI-side cutscene statics.
pub struct CameraState {
    /// `cam2` (ui_cam2.rs); `cameraReset(getDefaultCameraState())`
    /// needs the graphics defaults, installed by `load_cache`.
    pub cam2: crate::ui_cam2::Cam2,
    /// `camera` while `cameraActive` (staff free camera).
    pub free_camera: Option<crate::ui_cam2::FreeCamera>,
    /// Cutscene statics owned by the UI side (`crate::cutscene::UiState`).
    pub cutscene: crate::cutscene::UiState,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            cam2: crate::ui_cam2::Cam2::new(false),
            free_camera: None,
            cutscene: Default::default(),
        }
    }
}
