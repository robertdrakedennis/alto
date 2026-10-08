//! Async login + frame layer for the 910 Rust client (rev 910).
//!
//! Wire reference:
//! - Handshake and login blocks: login steps 14/35/84/98/256/268/138/141/157
//!   (connect, handshake token, game login, reply classification, var block,
//!   continuation); the RSA stub and TOTP detail blocks.
//! - Server side: `server/src/lostcity/engine/Lobby.ts` (`loginDecode`,
//!   `INIT_GAME_CONNECTION` -> 9-byte reply, `LOBBYLOGIN` parse lines 77-226)
//!   and `server/src/lostcity/engine/World.ts` (`loginDecode` lines 31-105).
//! - Frame loop: opcode 1-or-2 bytes, then size `u8` if `-1` / `u16` BE if
//!   `-2`, then payload.
//!
//! Login cryptography ([`crate::login_crypto`]): with a key configured the
//! login blocks carry an RSA-encrypted block of session seeds and credentials
//! and the rest of the body under the tiny cipher keyed by those seeds, and
//! the connection's opcodes are masked with ISAAC generators seeded from them
//! ([`crate::wire_stream`]). A [`LoginParams`] built with
//! `LoginCrypto::Plain` (tests and recorded replays) sends zero seeds and
//! unencrypted blocks and masks nothing.
//!
//! Entry points, one per login kind, each taking one [`LoginParams`]:
//! [`login_lobby`] and [`login_world`] -> `(TcpStream, LoginOk)`
//! (`session::login_world_and_drain` adds the startup drain), and
//! [`create_account_connect_at`] for `CREATE_ACCOUNT_CONNECT`. The packet
//! builders [`build_lobby_login_packet`] / [`build_game_login_packet`] take
//! the same params and the handshake token.
//!
//! IO seam (programme Phase 4; the Phase 5 `Io` trait plugs in here): the
//! login opens its socket in one place, `open_connection`'s
//! `TcpStream::connect`; the handshake and both login kinds' steps
//! (`handshake`, `lobby_login_steps`, `world_login_steps`) run over any
//! `AsyncRead + AsyncWrite` stream, and the blocks they write and parse are
//! pure functions of [`LoginParams`] and the bytes read.
//!
//! The faithful live path uses strict framing and retains partial bytes. An
//! unknown decoded opcode fails with its cursor and last-good packet; the
//! complete table covers IDs 0-194.
//! Legacy viewer helpers `decode_frame_game` / `read_frame` still expose their
//! old skip-and-continue recovery. This cannot recover delta state reliably and
//! is not used by the installed entity runtime.
//!
//! Login robustness (timeouts + reply codes):
//! - The login counts 20 ms logic updates: 500 before step 98 on the first
//!   attempt, 2000 afterwards, 6000 for social login. The headless transport
//!   uses cumulative wall-clock budgets with strict `>` expiry and excludes the
//!   parked reply states. Up to [`LOGIN_MAX_ATTEMPTS`] (4, initial plus three
//!   retries); exhaustion reports reply -5 semantics, and transport failures
//!   take the reconnect arm (final give-up is reply -4).
//! - Every step-98 reply byte is classified by [`classify_login_code`]:
//!   2 proceeds; 23 can reconnect; 1/15 require
//!   unimplemented continuation owners and fail explicitly; 21 ends login.
//!   42/49/52-lobby consume their follow-up bytes and keep reading on
//!   the same connection; 53 and anything unexpected (including 49/52 on a
//!   world connection) fail fast with the stream offset attached.

#[cfg(test)]
use login_packets::push_hardware_block;
#[cfg(test)]
use login_transport::{
    parse_continue_block, read_byte_clock, read_exact_clock, read_login_decision, Decision,
    ReplyPolicy,
};
#[cfg(test)]
use native910::packet::ByteWriter;

mod login_packets;
#[cfg(test)]
use login_packets::create_account_connect_packet;
use login_packets::seal_create_account_connect_packet;
use login_packets::seal_game_login_packet;
use login_packets::seal_lobby_login_packet;
use login_packets::seal_social_connection_packet;
use login_packets::seal_social_login_packet;
pub use login_packets::{
    build_game_login_packet, build_lobby_login_packet, build_social_connection_packet,
    build_social_login_packet, game_login_continue_packet, game_login_payload,
    init_game_connection_packet, lobby_login_payload, social_connection_payload,
    social_login_payload, ConnectionKind, CreateConnectInfo,
};
mod login_transport;
pub use login_transport::{create_account_connect_at, login_lobby, login_world, ContinueBlock};

use std::fmt;

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use std::sync::{Arc, Mutex};

use std::time::Duration;

use anyhow::Result;

use rs910_core::hardware::Hardware;

use crate::login_crypto::LoginCrypto;

use rs910_protocol::wire_cipher::WireCipher;

/// Phase 2.2: framing and the pure encoders moved to rs910-protocol;
/// they stay reachable as `net::...` (programme R1/R2).
pub use rs910_protocol::framing::*;

/// Outcome of a successful login handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOk {
    /// Typed var values from login steps 256/268 (without block flags).
    pub server_varcs: Vec<u8>,
    /// Player index. `Some` for world login (parsed from the
    /// `GAMELOGIN_CONTINUE` reply block); `None` for lobby login.
    pub pid: Option<u16>,
    /// The 8-byte session token from the `INIT_GAME_CONNECTION` (14) reply.
    /// It is echoed back inside the RSA-stub block of the login payload.
    pub server_token: i64,
    /// The account flags and identity of either reply.
    pub profile: LoginProfile,
    /// Lobby profile values read after the lobby login decision. World
    /// logins leave these at their
    /// defaults because the world continuation block does not carry them.
    pub lobby: LobbyProfile,
    /// The player-positions block of an in-place reconnect (login reply 15):
    /// the server kept the character in its world, so the session carries on
    /// from the client's own state and only re-initialises the player list from
    /// this block. `None` for every other login.
    pub resume: Option<Vec<u8>>,
}

/// The account flags and identity both login replies carry (world and
/// lobby), which the retained interface runtime starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginProfile {
    /// Whether the login is a members world; false for the lobby.
    pub logged_in_members: bool,
    /// Whether the account has members status.
    pub player_is_members: bool,
    /// Whether the account is quick-chat only.
    pub player_is_quickchat: bool,
    /// Whether this login is quick-chat only.
    pub logged_in_quickchat: bool,
    /// Whether the date of birth is verified.
    pub dob_verified: bool,
    /// Packed date of birth from the lobby profile.
    pub lobby_dob: i32,
    /// Staff moderator level, 0 for the lobby stub.
    pub staff_mod_level: i32,
    /// Player moderator level.
    pub player_mod_level: i32,
    /// Account owner name from a world login; the lobby profile leaves it unset.
    pub owner: Option<String>,
    /// World login `g6` server clock; the client keeps its difference from
    /// the local monotonic clock. The lobby profile derives that offset from
    /// its membership fields instead.
    pub server_clock: Option<i64>,
}

/// The lobby login's profile block past the account flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LobbyProfile {
    pub membership: i64,
    pub membership_delay: i64,
    pub membership_flag: bool,
    pub unread_messages: i32,
    /// Days since the recovery questions were last set.
    pub recovery_day: i32,
    pub jcoins_balance: i32,
    pub loyalty_balance: i32,
    pub last_login_day: i32,
    /// Host name (numeric id) the account last connected from.
    pub player_host: i32,
    pub email_status: i32,
    /// Remaining lobby profile fields consumed by login scripts.
    pub cc_expiry: i32,
    pub grace_expiry: i32,
    pub dob_requested: bool,
    pub members_stats: i32,
    pub play_age: i32,
    /// The local player's display name.
    pub player_name: String,
    pub world_id: Option<u16>,
    pub world_host: String,
    pub world_port: u16,
    pub world_port2: u16,
}

/// The world-transfer failure fields (login state 19)
/// carried through the worker so `login_last_transfer_reply` can report the
/// real reply, disallow result and trigger instead of the retained defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginTransferFailure {
    pub reply: i32,
    pub disallow_result: i32,
    pub disallow_trigger: i32,
}

impl fmt::Display for LoginTransferFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "world transfer rejected with reply {} (result {}, trigger {})",
            self.reply, self.disallow_result, self.disallow_trigger
        )
    }
}

impl std::error::Error for LoginTransferFailure {}

/// Context of a world login that failed after `GAMELOGIN_CONTINUE`: the world
/// had accepted the login, so a login from the lobby returns to the lobby
/// through a logout instead of staying where it was.
#[derive(Debug)]
pub struct LateLoginFailure;

impl fmt::Display for LateLoginFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the login failed after the world had accepted it")
    }
}

/// The terminal reply state for hop-wait and banned-login responses.
/// These values are decoded by the normal login stream owner and
/// must remain visible to the retained login CS2 tree after the worker stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginReplyState {
    pub reply: i32,
    pub hoptime: i32,
    pub ban_duration: i32,
}

impl fmt::Display for LoginReplyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reply {
            21 => write!(f, "login reply 21 (hop time {} ms)", self.hoptime),
            53 => write!(f, "login reply 53 (ban duration {})", self.ban_duration),
            reply => write!(f, "login reply {reply}"),
        }
    }
}

impl std::error::Error for LoginReplyState {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthOptions {
    pub new_auth_preference: String,
    pub auth_dont_trust: bool,
    /// Set for a reconnect (the client's connection-lost state) while the
    /// GAMELOGIN is built: the header byte, the previous connection's seeds
    /// after the server token, and no credentials in the RSA block.
    pub reconnect: bool,
}

/// A sign-on through a social network account instead of a username and
/// password.
///
/// The first login of a session has no key from the server yet and negotiates
/// one: the server answers with a page for the player to sign in on, then with
/// the account's key and name. Later logins of the session (the lobby, then
/// the world; a reconnect) present the key and name they were given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsoLogin {
    /// The network the player picked (the login screen's id for it).
    pub network: i32,
    /// The account key the server gave, once known.
    pub social_key: Option<i64>,
    /// The account name word the server gave with the key.
    pub social_name: i64,
}

/// What a login worker reports while it runs and what the app hands it in
/// return. The worker's thread and the app's logic loop share one per login
/// (the logic loop polls it every cycle; the worker never waits on the
/// loop except while a login is parked).
#[derive(Debug)]
pub struct LoginProgress {
    /// The reply 42 queue position, -1 outside a queue.
    queue_position: AtomicI32,
    /// The reply the server sent that keeps the login running (queue 42,
    /// device checks 49/52, the parked reply 1); [`LoginProgress::NO_REPLY`]
    /// when none.
    interim_reply: AtomicI32,
    /// `login_continue` was called.
    resume_requested: AtomicBool,
    /// Pages the server asked the player to open, in order.
    urls: Mutex<Vec<String>>,
    /// The key and name word a social sign-on negotiated.
    social: Mutex<Option<(i64, i64)>>,
    /// Client packets queued for the login connection while it waits (the
    /// device check's "resend the email" request), already framed.
    outgoing: Mutex<Vec<Vec<u8>>>,
    /// The connection's opcode masking from the moment the login block is
    /// sent: it covers what the login itself reads and writes afterwards,
    /// then goes with the stream ([`LoginProgress::take_cipher`]).
    cipher: Mutex<Option<WireCipher>>,
}

impl LoginProgress {
    /// [`LoginProgress::interim_reply`] before the server sent one.
    pub const NO_REPLY: i32 = i32::MIN;

    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue_position: AtomicI32::new(-1),
            interim_reply: AtomicI32::new(Self::NO_REPLY),
            resume_requested: AtomicBool::new(false),
            urls: Mutex::new(Vec::new()),
            social: Mutex::new(None),
            outgoing: Mutex::new(Vec::new()),
            cipher: Mutex::new(None),
        })
    }

    /// Forget everything the previous login left (a new login starts).
    pub fn reset(&self) {
        self.queue_position.store(-1, Ordering::Release);
        self.interim_reply.store(Self::NO_REPLY, Ordering::Release);
        self.resume_requested.store(false, Ordering::Release);
        lock(&self.urls).clear();
        lock(&self.outgoing).clear();
        *lock(&self.cipher) = None;
    }

    /// The queue position of reply 42, -1 when not queued.
    #[must_use]
    pub fn queue_position(&self) -> i32 {
        self.queue_position.load(Ordering::Acquire)
    }

    /// The reply that keeps the login going, if the server sent one.
    #[must_use]
    pub fn interim_reply(&self) -> Option<i32> {
        match self.interim_reply.load(Ordering::Acquire) {
            Self::NO_REPLY => None,
            reply => Some(reply),
        }
    }

    /// `login_continue`: resume a login parked on reply 1. A login that is
    /// not parked ignores it.
    pub fn request_resume(&self) {
        if self.interim_reply() == Some(1) {
            self.resume_requested.store(true, Ordering::Release);
        }
    }

    /// The pages queued to open since the last call.
    #[must_use]
    pub fn take_urls(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.urls))
    }

    /// The key and name word the server gave a social sign-on, if it did.
    #[must_use]
    pub fn social(&self) -> Option<(i64, i64)> {
        *lock(&self.social)
    }

    /// Queue a framed client packet for the login connection; the worker
    /// writes it the next time it waits on the server.
    pub fn queue_packet(&self, bytes: Vec<u8>) {
        lock(&self.outgoing).push(bytes);
    }

    pub(crate) fn set_queue_position(&self, position: i32) {
        self.queue_position.store(position, Ordering::Release);
    }

    pub(crate) fn set_interim_reply(&self, reply: i32) {
        self.interim_reply.store(reply, Ordering::Release);
    }

    pub(crate) fn push_url(&self, url: String) {
        lock(&self.urls).push(url);
    }

    pub(crate) fn set_social(&self, key: i64, name: i64) {
        *lock(&self.social) = Some((key, name));
    }

    fn take_outgoing(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *lock(&self.outgoing))
    }

    /// Install the masking of the connection whose login block was just sent.
    fn install_cipher(&self, cipher: Option<WireCipher>) {
        *lock(&self.cipher) = cipher;
    }

    /// The connection's masking, handed to its stream once the login is done.
    fn take_cipher(&self) -> Option<WireCipher> {
        lock(&self.cipher).take()
    }

    /// The wire form of client frames written while the login runs.
    fn seal_outgoing(&self, plain: &[u8]) -> Result<Vec<u8>> {
        match lock(&self.cipher).as_mut() {
            Some(cipher) => {
                let mut wire = Vec::with_capacity(plain.len());
                cipher.outbound.seal(plain, &mut wire)?;
                Ok(wire)
            }
            None => Ok(plain.to_vec()),
        }
    }

    /// Unmask a block of the login reply that is masked outside any frame.
    fn unmask_reply_block(&self, block: &mut [u8]) {
        if let Some(cipher) = lock(&self.cipher).as_mut() {
            cipher.inbound.unmask_block(block);
        }
    }

    /// The reply block's leading authenticator key, one masked byte per
    /// value: when the flag is 1 the four bytes that follow it are masked.
    fn unmask_auth_key(&self, block: &mut [u8]) {
        if block.first() == Some(&1) {
            if let Some(cipher) = lock(&self.cipher).as_mut() {
                let end = block.len().min(5);
                cipher.inbound.unmask_block(&mut block[1..end]);
            }
        }
    }

    fn take_resume(&self) -> bool {
        self.resume_requested.swap(false, Ordering::AcqRel)
    }
}

/// The guarded value, whatever a panicking holder left (every value here is
/// plain data that is valid in any state).
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a login reports of the client's window and saved options: the window
/// mode, the canvas size, the anti-aliasing level and the options block. The
/// default is the unreported client the packet builders were written against
/// (no window mode, an 800x600 canvas, no anti-aliasing, a zeroed block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientReport {
    pub window_mode: u8,
    pub canvas: [u16; 2],
    pub anti_aliasing: u8,
    /// The encoded client options (58 bytes when they come from
    /// `ClientOptions::encode`).
    pub preferences: Vec<u8>,
}

impl Default for ClientReport {
    fn default() -> Self {
        Self {
            window_mode: 0,
            canvas: [800, 600],
            anti_aliasing: 0,
            preferences: vec![0; 58],
        }
    }
}

/// What the launcher told the client about its player and installation, as
/// every login reports it: the language and game, the affiliate, the two
/// user-flow words and two flag words, a launcher label and an optional note
/// from the account-creation page, whether the page has JavaScript and
/// Chrome, the client type and a build word, and the game pack's name.
///
/// The default is the launcher-less client: every word zero, no strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherReport {
    /// The language id.
    pub language: u8,
    /// The game id (0 is RuneScape).
    pub game: u8,
    pub affiliate: i32,
    /// The user-flow words, second then first.
    pub user_flow: [i32; 2],
    /// The two flag words the automated-test commands read.
    pub flags: [i32; 2],
    /// The launcher's label for this client.
    pub label: String,
    /// The note an account-creation page passed along.
    pub additional_info: Option<String>,
    pub javascript: bool,
    pub chrome: bool,
    pub client_type: i32,
    /// The launcher's build word.
    pub build: i32,
    pub gamepack: String,
}

/// The archive checksum words of a login block: every archive but the loading
/// sprites, in the order of the archive list.
pub const ARCHIVE_CHECKSUM_WORDS: usize = 41;

/// Everything one login sends and where it connects (the login inputs for the
/// current world or lobby): the one parameter of [`login_lobby`], [`login_world`],
/// `session::login_world_and_drain` and the login packet builders. The login
/// workers own it for their thread's lifetime.
#[derive(Clone, Debug)]
pub struct LoginParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// The site settings string (`UPDATE_SITESETTINGS`), sent as a `pjstr`.
    pub site_settings: String,
    /// The uid.dat bytes the login sends (all zero = absent).
    pub uid192: [u8; 24],
    pub auth: AuthOptions,
    /// The machine the hardware block describes. The client always sends
    /// [`Hardware::default`], the all-unknown machine: the local probe feeds
    /// auto-setup and the settings panel but is never reported to a server.
    pub hardware: Hardware,
    /// The window and options the login reports.
    pub client: ClientReport,
    /// The launcher's parameters.
    pub launcher: LauncherReport,
    /// How many server packets changed interface state so far (a reconnect
    /// reports the running count; a fresh login is zero).
    pub verify_id: i32,
    /// The checksum of each loaded archive index, zero for an archive that is
    /// not loaded.
    pub archive_checksums: [i32; ARCHIVE_CHECKSUM_WORDS],
    /// A social network sign-on; `None` for a username and password login.
    pub sso: Option<SsoLogin>,
    /// The game login enters a different world from the one the lobby
    /// advertised (a world switch), which the login block flags; false when it
    /// enters the advertised world.
    pub switched_world: bool,
    /// The lobby login is the relogin after a logout (client state 9), where
    /// the device-check replies 49 and 52 end the login instead of waiting.
    pub relogin: bool,
    /// What the worker reports and what the app hands it while it runs.
    pub progress: Arc<LoginProgress>,
    /// How the blocks are protected; [`LoginCrypto::Plain`] sends them
    /// unencrypted (tests and recorded replays).
    pub crypto: LoginCrypto,
}

impl LoginParams {
    /// `username`/`password` against `host:port` with no site settings, an
    /// absent uid.dat, default [`AuthOptions`], no social sign-on, a world
    /// switch and a progress record no one else reads.
    #[must_use]
    pub fn new(host: &str, port: u16, username: &str, password: &str) -> Self {
        Self {
            host: host.to_owned(),
            port,
            username: username.to_owned(),
            password: password.to_owned(),
            site_settings: String::new(),
            uid192: [0; 24],
            auth: AuthOptions::default(),
            hardware: Hardware::default(),
            client: ClientReport::default(),
            launcher: LauncherReport::default(),
            verify_id: 0,
            archive_checksums: [0; ARCHIVE_CHECKSUM_WORDS],
            sso: None,
            switched_world: true,
            relogin: false,
            progress: LoginProgress::new(),
            crypto: LoginCrypto::Plain,
        }
    }
}

// ---------------------------------------------------------------------------
// Login timing + reply-code policy.
// ---------------------------------------------------------------------------

/// Maximum connection attempts per login call, including the initial attempt.
/// The attempt counter starts at zero and recovery is permitted while it is
/// below three, yielding the initial attempt plus three retries.
pub const LOGIN_MAX_ATTEMPTS: u32 = 4;

/// The login state machine is updated about every 20 ms. `loginwait` is an
/// update-call counter, despite the timeout constants being expressed as
/// values such as 500 and 2000.
pub const LOGIN_UPDATE_INTERVAL_MS: u64 = 20;

/// Cumulative `loginwait` clock for one connection attempt. It advances once per
/// login update and times out only when the counter is strictly greater than
/// its threshold.
struct LoginClock {
    started: std::time::Instant,
    accumulated: Duration,
    limit_ticks: u64,
    paused: bool,
    /// A social sign-on is under way: from its browser step on the budget is
    /// [`LOGIN_TIMEOUT_SOCIAL_TICKS`] for the rest of the login.
    social: bool,
}

impl LoginClock {
    fn new(first_attempt: bool) -> Self {
        Self {
            started: std::time::Instant::now(),
            accumulated: Duration::ZERO,
            limit_ticks: stage_timeout_ms(first_attempt, false),
            paused: false,
            social: false,
        }
    }

    /// Step 98 changes the threshold but does not reset `loginwait`.
    fn enter_step98(&mut self) {
        self.limit_ticks = stage_timeout_ms(false, self.social);
    }

    /// The social sign-on's browser step (login step 64) on: the player needs
    /// time to sign in, so the budget stays at the social threshold.
    fn enter_social(&mut self) {
        self.social = true;
        self.limit_ticks = LOGIN_TIMEOUT_SOCIAL_TICKS;
    }

    fn pause(&mut self) {
        if !self.paused {
            self.accumulated += self.started.elapsed();
        }
        self.paused = true;
    }

    fn resume(&mut self) {
        if self.paused {
            self.started = std::time::Instant::now();
        }
        self.paused = false;
    }

    fn remaining(&self) -> Option<Duration> {
        if self.paused {
            return None;
        }
        // Strict `>` means the 501st/2001st update call expires.
        Some(self.budget_remaining(self.accumulated + self.started.elapsed()))
    }

    fn budget_remaining(&self, elapsed: Duration) -> Duration {
        let budget = Duration::from_millis((self.limit_ticks + 1) * LOGIN_UPDATE_INTERVAL_MS);
        budget.saturating_sub(elapsed)
    }
}

/// First-attempt wait budget in update calls: 500 when
/// `loginAttempts == 0 && loginStep < 98` (the connect + handshake stages
/// 14/35), 2000 otherwise.
pub const LOGIN_TIMEOUT_FIRST_TICKS: u64 = 500;

/// Retry-attempt wait budget in update calls.
pub const LOGIN_TIMEOUT_RETRY_TICKS: u64 = 2000;

/// Social sign-on budget in update calls from the browser step on
/// (`isSocialLogin && loginStep >= 64`).
pub const LOGIN_TIMEOUT_SOCIAL_TICKS: u64 = 6000;

/// Stage timeout in update calls:
/// 500 on the first attempt before step 98, 2000 afterwards, 6000 for
/// social-login late stages. Use [`stage_timeout_duration`] for wall-clock IO.
#[must_use]
pub const fn stage_timeout_ms(first_attempt: bool, social_late: bool) -> u64 {
    if social_late {
        LOGIN_TIMEOUT_SOCIAL_TICKS
    } else if first_attempt {
        LOGIN_TIMEOUT_FIRST_TICKS
    } else {
        LOGIN_TIMEOUT_RETRY_TICKS
    }
}

/// Wall-clock budget corresponding to the update-call counter.
#[must_use]
pub const fn stage_timeout_duration(first_attempt: bool, social_late: bool) -> Duration {
    Duration::from_millis(stage_timeout_ms(first_attempt, social_late) * LOGIN_UPDATE_INTERVAL_MS)
}

/// Retry-after delay for reply `21`: `hoptime = byte * 50` (login step 126).
#[must_use]
pub const fn hop_retry_after_ms(hop_byte: u8) -> u64 {
    hop_byte as u64 * 50
}

/// Step-98 login reply classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginCode {
    /// `2`: proceed with the login flow.
    Proceed,
    /// `1`: the server shows an advertisement first; the login parks (login
    /// step 103) until the login screen calls `login_continue`.
    ConfirmRetry,
    /// `15`: an in-place reconnect succeeded. The character never left the
    /// world, so a `u16`-sized player-positions block follows and the client
    /// carries on with the state it has (step 204).
    AltSuccess,
    /// `21`: wait timer; the next byte sets `hoptime`.
    /// The login ends and the timer is exposed to the login UI.
    HopWait,
    /// `23`: reconnect immediately while attempts remain.
    WorldFull,
    /// `42`: a `u16` queue position follows, then stay in step 98 (step 215).
    Queued,
    /// `49` on a lobby login that waits for the player: the account guardian
    /// is holding the login while the player validates the device by email;
    /// the login stays in step 98 for the next reply.
    LobbyHold,
    /// `52`: a `u16`-sized block follows holding the address of the page that
    /// validates this device; the client opens it, then a waiting lobby login
    /// returns to step 98 and any other login ends (steps 225/235).
    UrlBlock,
    /// `53`: a `u32` ban duration follows, then login ends.
    Banned,
    /// Anything else (including `49` on a login that does not wait): end login.
    Fatal,
}

/// Classify one step-98 reply byte. `holds` is a lobby login that waits for
/// the player (not the relogin after a logout).
#[must_use]
pub const fn classify_login_code(code: u8, holds: bool) -> LoginCode {
    match code {
        2 => LoginCode::Proceed,
        1 => LoginCode::ConfirmRetry,
        15 => LoginCode::AltSuccess,
        21 => LoginCode::HopWait,
        23 => LoginCode::WorldFull,
        42 => LoginCode::Queued,
        49 if holds => LoginCode::LobbyHold,
        52 => LoginCode::UrlBlock,
        53 => LoginCode::Banned,
        _ => LoginCode::Fatal,
    }
}

/// Outcome of one login attempt: success, or a retryable reconnect. Fatal
/// replies are `Err` (returned to the caller immediately); every
/// transport/timeout failure maps to `RetryAfter`, mirroring the I/O error
/// arm, which reconnects while attempts remain and gives up with -4.
#[derive(Debug)]
enum Attempt<T> {
    Done(T),
    /// `timeout`: this attempt ended because `loginwait` passed its limit
    /// rather than an I/O error.
    RetryAfter {
        after_ms: u64,
        why: String,
        timeout: bool,
    },
    Disallowed {
        reply: u8,
        result: u8,
        trigger: u16,
    },
    /// The server did not know the social key presented (reply 35): connect
    /// again and negotiate a new one. Not a retry: the attempt count stands.
    Renegotiate,
}

impl<T> Attempt<T> {
    /// `Done(value)` as `Ok(value)`; a retry or transfer outcome carried,
    /// unchanged, to the caller's attempt type.
    fn done<U>(self) -> std::result::Result<T, Attempt<U>> {
        match self {
            Attempt::Done(value) => Ok(value),
            Attempt::RetryAfter {
                after_ms,
                why,
                timeout,
            } => Err(Attempt::RetryAfter {
                after_ms,
                why,
                timeout,
            }),
            Attempt::Disallowed {
                reply,
                result,
                trigger,
            } => Err(Attempt::Disallowed {
                reply,
                result,
                trigger,
            }),
            Attempt::Renegotiate => Err(Attempt::Renegotiate),
        }
    }
}

fn retry_after<T>(after_ms: u64, why: String) -> Attempt<T> {
    Attempt::RetryAfter {
        after_ms,
        why,
        timeout: false,
    }
}

/// A failed read: [`LoginWaitExpired`] is the `loginwait` timeout, any
/// other error the I/O error path.
fn retry_read<T>(error: &anyhow::Error, why: String) -> Attempt<T> {
    Attempt::RetryAfter {
        after_ms: 0,
        why,
        timeout: error.chain().any(|e| e.is::<LoginWaitExpired>()),
    }
}

/// `loginwait > limit`: the attempt timed out.
#[derive(Debug)]
pub struct LoginWaitExpired(String);

impl fmt::Display for LoginWaitExpired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: loginwait exceeded its strict threshold", self.0)
    }
}

impl std::error::Error for LoginWaitExpired {}

/// Retries exhausted: reply -5 when the last attempt timed out, reply -4 when
/// it raised an I/O error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginAttemptsExhausted {
    pub reply: i32,
}

impl LoginAttemptsExhausted {
    #[must_use]
    pub fn from_last_attempt(timeout: bool) -> Self {
        Self {
            reply: if timeout { -5 } else { -4 },
        }
    }
}

impl fmt::Display for LoginAttemptsExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "login attempts exhausted: reply {}", self.reply)
    }
}

impl std::error::Error for LoginAttemptsExhausted {}

#[cfg(test)]
mod tests;
