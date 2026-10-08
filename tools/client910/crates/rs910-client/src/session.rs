//! Live post-login session: `REBUILD_NORMAL` parsing + the pre-render drain loop.
//!
//! Wire truth (server authority, read before coding):
//! - the server packet table under `server/src/formats/network/protocol/`
//!   `REBUILD_NORMAL.encode`: optional high-res player bit-block first (only
//!   when `nearbyPlayers=true`, which `Player.ts:88` `login()` uses), then the
//!   fixed 8-byte tail `p2_alt2(zoneX) | p1(npc bits) | p1_alt3(count) |
//!   p1(buildArea id) | p2_alt2(zoneZ) | p1_alt3(force)`.
//! - `server/src/formats/bytepacking/Packet.ts`: `_alt2`/`_alt3` are NOT
//!   plain big-endian on the wire — `p2_alt2(v)` emits `[(v >> 8) & 0xFF,
//!   (v + 128) & 0xFF]` and `p1_alt3(v)` emits `(128 - v) & 0xFF`. Decoding
//!   inverts exactly those transforms (there is no `p2_alt2` sender in
//!   `net.rs` to cross-check; `Packet.ts` is the authority).
//! - `World.ts` re-sends `REBUILD_NORMAL` without the bit-block (e.g. the
//!   `tele` cheat), so the tail is parsed from the LAST 8 payload bytes and
//!   any leading bytes are treated as the optional high-res block and skipped.
//!   Exact login-time block size for reference: 30 bits + 2047 × 18 bits =
//!   36876 bits → `ceil(36876 / 8)` = 4610 bytes, but parsing accepts any
//!   prefix length ≥ 0 (assumption: the fixed tail is always the final 8
//!   bytes, which holds for both `encode` call sites today).
//! - After `REBUILD_NORMAL` the client must reply `MAP_BUILD_COMPLETE` (79/4)
//!   or the server gates zones; `NO_TIMEOUT` (83/0) is not answered (the
//!   client's own keepalive is a timer, `loading_connection`);
//!   `PLAYER_INFO` (122/-2) is bit-packed and is only sighted here, never
//!   bit-decoded.
//!
//! Zone/group math: `zone = abs >> 3` (8×8 tiles), `mapsquare = zone >> 3`
//! (64×64 tiles), `group = mx | mz << 7` (same as `map::group_base`, which
//! inverts it). The drain covers the 3×3 mapsquare block around the player.
//!
//! IO seam (programme Phase 4; since Phase 5 the live transport runs
//! through `client_core::Io`, whose implementations call the transport
//! functions below, the only ones that touch a socket or a clock):
//!
//! | layer | startup drain | live poll |
//! |---|---|---|
//! | framing (pure, rs910-protocol) | `net::decode_frame` | `net::decode_frame`/`decode_frame_game`/`decode_frame_at` |
//! | dispatch (pure: frame in, replies/events/entity packets out) | [`StartupDrain::on_frame`] | [`handle_sync_frame`], [`drain_pending_sync`], [`drain_pending_entities`] |
//! | transport | [`drain_frames_mode`] (tokio stream + deadline) | [`flush_nonblocking`], [`read_nonblocking`] (any `Read`/`Write`) |
//!
//! Replies are queued bytes ([`StartupReply`], `pending_writes`), never
//! written from the dispatch half, so a recorded or replayed stream can
//! stand in for the socket without touching dispatch.

use std::time::{Duration, Instant};

use anyhow::Context;
#[cfg(test)]
use rs910_core::reader::Reader as CoreReader;

/// Phase 2.2: the parse half moved to rs910-protocol; every item stays
/// reachable as `session::...` (programme R1/R2 rewrite and remove this).
pub use rs910_protocol::server_prot::*;

/// Post-login drain budget: exit with whatever was collected after this long.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of [`drain_frames`]: what the pre-render frame loop collected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrainOutcome {
    /// Ordered entity packets, including the initial rebuild block.
    pub entities: crate::protocol910::live::Feed,
    /// Bytes read beyond the final startup frame; transferred to live polling.
    pub pending: Vec<u8>,
    /// First `REBUILD_NORMAL` seen, if any.
    pub rebuild: Option<Rebuild>,
    /// True once a non-empty `PLAYER_INFO` payload was sighted.
    pub player_seen: bool,
    /// Total `PLAYER_INFO` payload bytes sighted (never bit-decoded).
    pub player_info_bytes: usize,
    /// Live interface / script frames parsed during the drain, in arrival
    /// order (`IF_OPENTOP`/`IF_OPENSUB`/`IF_OPENSUB_ACTIVE_*`/`IF_SETTEXT`/
    /// `IF_SETHIDE`/`IF_SETPOSITION`/`IF_SETSCROLLPOS`/`RUNCLIENTSCRIPT`).
    /// `app.rs` applies them to `iface::OpenInterfaces` and executes scripts
    /// through `iface::VmAdapter` (the real `native910::vm::Vm` path).
    pub ui_events: Vec<UiEvent>,
}

/// Live world state handed back to `app.rs` after validation: the login
/// reply, what the startup drain collected and the open world connection.
pub struct LiveState {
    /// The `GAMELOGIN_CONTINUE` reply: player index, session token, server
    /// varcs and the account profile.
    pub login: crate::net::LoginOk,
    /// The startup drain's entity packets, first `REBUILD_NORMAL`,
    /// `PLAYER_INFO` sighting and interface frames in arrival order (`app.rs`
    /// applies them before the first frame so lobby 906 / world 1477 open
    /// immediately), plus the leftover frame bytes carried into the live
    /// poll (a `REBUILD_NORMAL` can arrive split across TCP segments).
    pub drain: DrainOutcome,
    /// Open world connection (held so the server keeps the session).
    pub stream: crate::wire_stream::WireStream<tokio::net::TcpStream>,
}

/// True when `groups` is exactly the cached 3×3 Lumbridge block
/// (`map::LUMBRIDGE_GROUPS`, base 3136, extent 192).
#[must_use]
pub fn is_cached_lumbridge_block(groups: &[u16]) -> bool {
    if groups.len() != crate::map::LUMBRIDGE_GROUPS.len() {
        return false;
    }
    let mut sorted: Vec<u16> = groups.to_vec();
    sorted.sort_unstable();
    sorted
        .iter()
        .zip(crate::map::LUMBRIDGE_GROUPS.iter())
        .all(|(got, want)| u32::from(*got) == *want)
}

/// Read one server frame from any async stream, reusing the tested
/// [`crate::net::decode_frame`] framing (`opcode 1-or-2 bytes`, then `u8`/`u16 BE`
/// length for `-1`/`-2` sizes, then payload). `pending` carries over bytes
/// from a previous partial read.
async fn read_frame_stream<S>(
    stream: &mut S,
    pending: &mut Vec<u8>,
) -> anyhow::Result<crate::net::Frame>
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut tmp = [0u8; 4096];
    loop {
        if let Some((frame, used)) = crate::net::decode_frame(pending)? {
            pending.drain(..used);
            return Ok(frame);
        }
        let n = stream
            .read(&mut tmp)
            .await
            .context("live drain: read server bytes")?;
        if n == 0 {
            anyhow::bail!("live drain: server closed the connection");
        }
        pending.extend_from_slice(&tmp[..n]);
    }
}

/// Post-login drain loop over an already-authenticated stream.
///
/// Reads until both a `REBUILD_NORMAL` and a
/// following `PLAYER_INFO` have been seen (or `timeout` elapses):
/// - 88 `REBUILD_NORMAL`: store the parse, reply `MAP_BUILD_COMPLETE`, log a
///   reload hint for the render layer.
/// - 122 `PLAYER_INFO`: validate non-empty, add its byte length to
///   `player_info_bytes`, mark `player_seen` (the bit-stream itself is never
///   decoded here).
/// - 83 `NO_TIMEOUT`: nothing.
/// - 129 `SERVER_TICK_END`: continue.
/// - Live interface / script frames (every [`is_ui_frame`] opcode):
///   parsed with [`parse_ui_event`] and collected into
///   [`DrainOutcome::ui_events`] for `app.rs` to apply through
///   `iface::OpenInterfaces` + `iface::VmAdapter`. A payload that does not
///   decode is collected as a [`UiEvent::MalformedPacket`], which logs the
///   player out when it is applied, in its place in the order.
/// - Zone/varcache frames: logged and ignored.
///
/// Unknown opcodes are ignored (never fatal); short `REBUILD_NORMAL`
/// payloads are hard errors; timeout/EOF-after-progress returns whatever was
/// collected.
#[cfg(test)] // test-only drain loop
pub async fn drain_frames<S>(stream: &mut S, timeout: Duration) -> anyhow::Result<DrainOutcome>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    drain_frames_mode(stream, timeout, false).await
}

/// The transport half of the startup drain: read frames from `stream` until
/// [`StartupDrain`] is done or `timeout` passes, and write each reply it
/// asks for before the next read.
pub async fn drain_frames_mode<S>(
    stream: &mut S,
    timeout: Duration,
    strict_entities: bool,
) -> anyhow::Result<DrainOutcome>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let started = Instant::now();
    let deadline = started + timeout;

    // The native canvas does not exist during this headless startup drain.
    // Its owner sends WINDOW_STATUS after creating the real window; mode 0
    // and guessed 800x600 dimensions are not a valid window state.

    let mut drain = StartupDrain::new(strict_entities);
    let mut pending = Vec::new();
    while !drain.finished {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let frame =
            match tokio::time::timeout(remaining, read_frame_stream(stream, &mut pending)).await {
                Ok(frame) => frame?,
                Err(_) => break,
            };
        let elapsed_ms = || u128::min(started.elapsed().as_millis(), i32::MAX as u128) as i32;
        let Some(reply) = drain.on_frame(frame, elapsed_ms)? else {
            continue;
        };
        stream
            .write_all(&reply.bytes())
            .await
            .context(reply.context())?;
        if let (StartupReply::MapBuildComplete(elapsed_ms), Some(rebuild)) =
            (reply, &drain.outcome.rebuild)
        {
            log::info!(
                "[client910] live MAP_BUILD_COMPLETE sent ({elapsed_ms} ms); render reload hint: {:?}",
                rebuild_to_groups(rebuild),
            );
        }
    }
    drain.outcome.pending = pending;
    Ok(drain.outcome)
}

/// A reply the startup drain sends before its next read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupReply {
    /// `MAP_BUILD_COMPLETE` with the drain's elapsed milliseconds.
    MapBuildComplete(i32),
    /// `SEND_PING_REPLY`, already encoded (it reads the current frame rate).
    Ping(Vec<u8>),
}

impl StartupReply {
    /// The bytes on the wire.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            StartupReply::MapBuildComplete(elapsed_ms) => {
                crate::net::encode_map_build_complete(*elapsed_ms)
            }
            StartupReply::Ping(bytes) => bytes.clone(),
        }
    }

    /// The error context of a failed write.
    #[must_use]
    pub fn context(&self) -> &'static str {
        match self {
            StartupReply::MapBuildComplete(_) => "live drain: send MAP_BUILD_COMPLETE",
            StartupReply::Ping(_) => "live drain: reply SEND_PING",
        }
    }
}

/// The dispatch half of the startup drain ([`drain_frames_mode`] without
/// its socket): what it collected so far and whether it is done. It reads
/// no clock and writes no socket; the transport passes the elapsed time in
/// and writes the [`StartupReply`] it returns.
#[derive(Debug, Default)]
pub struct StartupDrain {
    pub outcome: DrainOutcome,
    strict_entities: bool,
    /// A `PLAYER_INFO` followed the first `REBUILD_NORMAL`, or (strict
    /// entities) the rebuild must commit before `MAP_BUILD_COMPLETE`.
    pub finished: bool,
}

impl StartupDrain {
    #[must_use]
    pub fn new(strict_entities: bool) -> Self {
        Self {
            strict_entities,
            ..Self::default()
        }
    }

    /// Dispatch one frame; `elapsed_ms` is sampled only for the
    /// `MAP_BUILD_COMPLETE` reply.
    pub fn on_frame(
        &mut self,
        frame: crate::net::Frame,
        elapsed_ms: impl FnOnce() -> i32,
    ) -> anyhow::Result<Option<StartupReply>> {
        let outcome = &mut self.outcome;
        let entity_frame = outcome.entities.enqueue(frame.opcode, &frame.payload);
        if entity_frame
            && !matches!(
                frame.opcode,
                crate::proto::server::REBUILD_NORMAL
                    | crate::proto::server::PLAYER_INFO
                    | crate::proto::server::SERVER_TICK_END
            )
        {
            return Ok(None);
        }
        match frame.opcode {
            crate::proto::server::REBUILD_NORMAL => {
                let rebuild = parse_logged_rebuild(&frame.payload)?;
                if self.strict_entities {
                    // Bootstrap/rebase must commit before MAP_BUILD_COMPLETE.
                    // Preserve every byte already read and return to the CPU adapter.
                    outcome.rebuild = Some(rebuild);
                    self.finished = true;
                    return Ok(None);
                }
                let elapsed_ms = elapsed_ms();
                outcome.rebuild = Some(rebuild);
                return Ok(Some(StartupReply::MapBuildComplete(elapsed_ms)));
            }
            crate::proto::server::PLAYER_INFO => {
                if frame.payload.is_empty() {
                    log::debug!("[client910] live PLAYER_INFO: empty payload, waiting for next");
                    return Ok(None);
                }
                outcome.player_info_bytes = outcome
                    .player_info_bytes
                    .saturating_add(frame.payload.len());
                outcome.player_seen = true;
                if outcome.rebuild.is_some() {
                    self.finished = true;
                }
            }
            // The server's keepalive: the client sends nothing for it.
            crate::proto::server::NO_TIMEOUT => {}
            crate::proto::server::SEND_PING => match send_ping_reply(&frame.payload) {
                Ok(reply) => return Ok(Some(StartupReply::Ping(reply))),
                Err(error) => outcome.ui_events.push(malformed(&frame, &error)),
            },
            crate::proto::server::SERVER_TICK_END => {}
            opcode if is_ui_frame(opcode) => {
                collect_ui_event(FrameSite::Startup, &frame, &mut outcome.ui_events);
            }
            opcode => log_ignored(FrameSite::Startup, opcode, frame.payload.len()),
        }
        Ok(None)
    }
}

/// Which loop dispatches a frame: the startup drain or the live poll. They
/// share the handlers below and differ in log wording.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameSite {
    Startup,
    Poll,
}

/// The server frames both loops parse with [`parse_ui_event`]: interface,
/// script, var, social, audio, camera, telemetry and session packets.
#[must_use]
pub fn is_ui_frame(opcode: u8) -> bool {
    matches!(
        opcode,
        crate::proto::server::IF_OPENTOP
            | crate::proto::server::IF_OPENSUB
            | crate::proto::server::IF_CLOSESUB
            | crate::proto::server::IF_MOVESUB
            | crate::proto::server::IF_OPENSUB_ACTIVE_LOC
            | crate::proto::server::IF_OPENSUB_ACTIVE_PLAYER
            | crate::proto::server::IF_OPENSUB_ACTIVE_NPC
            | crate::proto::server::IF_OPENSUB_ACTIVE_OBJ
            | crate::proto::server::IF_SETTEXT
            | crate::proto::server::UPDATE_SITESETTINGS
            | crate::proto::server::UPDATE_UID192
            | crate::proto::server::LAST_LOGIN_INFO
            | crate::proto::server::WORLDLIST_FETCH_REPLY
            | crate::proto::server::STORE_SERVERPERM_VARCS_ACK
            | crate::proto::server::IF_SETTARGETPARAM
            | crate::proto::server::CAM2_ENABLE
            | crate::proto::server::CAMERA_UPDATE
            | crate::proto::server::CAM_RESET
            | crate::proto::server::CAM_SMOOTHRESET
            | crate::proto::server::CAM_REMOVEROOF
            | crate::proto::server::ENVIRONMENT_OVERRIDE
            | crate::proto::server::URL_OPEN
            | crate::proto::server::SOCIAL_NETWORK_LOGOUT
            | crate::proto::server::POINTLIGHT_COLOUR
            | crate::proto::server::POINTLIGHT_INTENSITY
            | crate::proto::server::UPDATE_INV_FULL
            | crate::proto::server::UPDATE_INV_PARTIAL
            | crate::proto::server::UPDATE_INV_STOP_TRANSMIT
            | crate::proto::server::UPDATE_STAT
            | crate::proto::server::UPDATE_RUNENERGY
            | crate::proto::server::UPDATE_RUNWEIGHT
            | crate::proto::server::UPDATE_REBOOT_TIMER
            | crate::proto::server::SET_TARGET
            | crate::proto::server::SETDRAWORDER
            | crate::proto::server::CHAT_FILTER_SETTINGS
            | crate::proto::server::CHAT_FILTER_SETTINGS_PRIVATECHAT
            | crate::proto::server::UPDATE_DOB
            | crate::proto::server::LOYALTY_UPDATE
            | crate::proto::server::JCOINS_UPDATE
            | crate::proto::server::TRIGGER_ONDIALOGABORT
            | crate::proto::server::MESSAGE_GAME
            | crate::proto::server::UPDATE_FRIENDLIST
            | crate::proto::server::FRIENDLIST_LOADED
            | crate::proto::server::UPDATE_IGNORELIST
            | crate::proto::server::UPDATE_FRIENDCHAT_CHANNEL_FULL
            | crate::proto::server::UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER
            | crate::proto::server::MESSAGE_FRIENDCHANNEL
            | crate::proto::server::CLANCHANNEL_FULL
            | crate::proto::server::CLANCHANNEL_DELTA
            | crate::proto::server::CLANSETTINGS_FULL
            | crate::proto::server::CLANSETTINGS_DELTA
            | crate::proto::server::MESSAGE_CLANCHANNEL
            | crate::proto::server::MESSAGE_CLANCHANNEL_SYSTEM
            | crate::proto::server::PLAYER_GROUP_FULL
            | crate::proto::server::PLAYER_GROUP_DELTA
            | crate::proto::server::PLAYER_GROUP_VARPS
            | crate::proto::server::MESSAGE_PLAYER_GROUP
            | crate::proto::server::MESSAGE_QUICKCHAT_PRIVATE
            | crate::proto::server::MESSAGE_QUICKCHAT_CLANCHANNEL
            | crate::proto::server::MESSAGE_QUICKCHAT_PRIVATE_ECHO
            | crate::proto::server::MESSAGE_QUICKCHAT_FRIENDCHAT
            | crate::proto::server::MESSAGE_QUICKCHAT_PLAYER_GROUP
            | crate::proto::server::MESSAGE_PUBLIC
            | crate::proto::server::MESSAGE_PRIVATE_ECHO
            | crate::proto::server::MESSAGE_PRIVATE
            | crate::proto::server::CHANGE_LOBBY
            | crate::proto::server::LOGOUT_TRANSFER
            | crate::proto::server::LOGOUT
            | crate::proto::server::LOGOUT_FULL
            | crate::proto::server::IF_SETANIM
            | crate::proto::server::IF_SETMODEL
            | crate::proto::server::IF_SETOBJECT
            | crate::proto::server::IF_SET_HTTP_IMAGE
            | crate::proto::server::SET_MAP_FLAG
            | crate::proto::server::MINIMAP_TOGGLE
            | crate::proto::server::IF_SETPLAYERHEAD
            | crate::proto::server::IF_SETPLAYERHEAD_OTHER
            | crate::proto::server::IF_SETPLAYERHEAD_IGNOREWORN
            | crate::proto::server::IF_SETPLAYERMODEL_SELF
            | crate::proto::server::IF_SETPLAYERMODEL_OTHER
            | crate::proto::server::IF_SETPLAYERMODEL_SNAPSHOT
            | crate::proto::server::PLAYER_SNAPSHOT
            | crate::proto::server::CLEAR_PLAYER_SNAPSHOT
            | crate::proto::server::LOBBY_APPEARANCE
            | crate::proto::server::UPDATE_STOCKMARKET_SLOT
            | crate::proto::server::CUTSCENE
            | crate::proto::server::IF_SETNPCHEAD
            | crate::proto::server::IF_SETCOLOUR
            | crate::proto::server::IF_SETANGLE
            | crate::proto::server::IF_SETGRAPHIC
            | crate::proto::server::IF_SETTEXTANTIMACRO
            | crate::proto::server::IF_SETTEXTFONT
            | crate::proto::server::IF_SETCLICKMASK
            | crate::proto::server::IF_SETRECOL
            | crate::proto::server::IF_SETRETEX
            | crate::proto::server::CLIENT_SETVARC_SMALL
            | crate::proto::server::CLIENT_SETVARC_LARGE
            | crate::proto::server::CLIENT_SETVARCBIT_SMALL
            | crate::proto::server::CLIENT_SETVARCBIT_LARGE
            | crate::proto::server::CLIENT_SETVARCSTR_SMALL
            | crate::proto::server::CLIENT_SETVARCSTR_LARGE
            | crate::proto::server::VARCLAN_ENABLE
            | crate::proto::server::VARCLAN_DISABLE
            | crate::proto::server::VARCLAN
            | crate::proto::server::SET_PLAYER_OP
            | crate::proto::server::SET_MOVEACTION
            | crate::proto::server::SHOW_FACE_HERE
            | crate::proto::server::REDUCE_PLAYER_ATTACK_PRIORITY
            | crate::proto::server::REDUCE_NPC_ATTACK_PRIORITY
            | crate::proto::server::IF_SETEVENTS
            | crate::proto::server::IF_SETHIDE
            | crate::proto::server::IF_SETPOSITION
            | crate::proto::server::IF_SETSCROLLPOS
            | crate::proto::server::MIDI_JINGLE
            | crate::proto::server::MIDI_SONG
            | crate::proto::server::MIDI_SONG_STOP
            | crate::proto::server::MIDI_SONG_LOCATION
            | crate::proto::server::SYNTH_SOUND
            | crate::proto::server::VORBIS_SOUND
            | crate::proto::server::VORBIS_SPEECH_SOUND
            | crate::proto::server::VORBIS_SPEECH_STOP
            | crate::proto::server::SONG_PRELOAD
            | crate::proto::server::VORBIS_PRELOAD_SOUNDS
            | crate::proto::server::VORBIS_PRELOAD_SOUND_GROUP
            | crate::proto::server::VORBIS_SOUND_GROUP
            | crate::proto::server::VORBIS_SOUND_GROUP_START
            | crate::proto::server::VORBIS_SOUND_GROUP_STOP
            | crate::proto::server::SOUND_MIXBUSS_ADD
            | crate::proto::server::SOUND_MIXBUSS_SETLEVEL
            | crate::proto::server::TELEMETRY_GRID_FULL
            | crate::proto::server::TELEMETRY_CLEAR_GRID_VALUE
            | crate::proto::server::TELEMETRY_GRID_ADD_ROW
            | crate::proto::server::TELEMETRY_GRID_REMOVE_COLUMN
            | crate::proto::server::TELEMETRY_GRID_REMOVE_GROUP
            | crate::proto::server::TELEMETRY_GRID_ADD_GROUP
            | crate::proto::server::TELEMETRY_GRID_MOVE_ROW
            | crate::proto::server::TELEMETRY_GRID_REMOVE_ROW
            | crate::proto::server::TELEMETRY_GRID_VALUES_DELTA
            | crate::proto::server::TELEMETRY_GRID_SET_ROW_PINNED
            | crate::proto::server::TELEMETRY_GRID_MOVE_COLUMN
            | crate::proto::server::TELEMETRY_GRID_ADD_COLUMN
            | crate::proto::server::CREATE_CHECK_EMAIL_REPLY
            | crate::proto::server::CREATE_ACCOUNT_REPLY
            | crate::proto::server::CREATE_CHECK_NAME_REPLY
            | crate::proto::server::CREATE_SUGGEST_NAME_ERROR
            | crate::proto::server::CREATE_SUGGEST_NAME_REPLY
            | crate::proto::server::CAM_LOOKAT
            | crate::proto::server::CAM_MOVETO
            | crate::proto::server::CAM_FORCEANGLE
            | crate::proto::server::CAM_SHAKE
            | crate::proto::server::HINT_ARROW
            | crate::proto::server::HINT_TRAIL
            | crate::proto::server::REFLECTION_CHECKER
            | crate::proto::server::JS5_RELOAD
            | crate::proto::server::EXECUTE_CLIENT_CHEAT
            | crate::proto::server::DO_CHEAT
            | crate::proto::server::DEBUG_SERVER_TRIGGERS
            | crate::proto::server::UNHANDLED_51
            | crate::proto::server::UNHANDLED_170
            | crate::proto::server::RUNCLIENTSCRIPT
    )
}

/// Parse `REBUILD_NORMAL` and log it (both loops).
fn parse_logged_rebuild(payload: &[u8]) -> anyhow::Result<Rebuild> {
    let rebuild = parse_rebuild_normal(payload)?;
    log::info!(
        "[client910] live REBUILD_NORMAL: zone=({},{}) npc_bits={} count={} build_area={} force={} (+{} high-res bytes)",
        rebuild.zone_x,
        rebuild.zone_z,
        rebuild.npc_bits,
        rebuild.map_count,
        rebuild.build_area_id,
        rebuild.force,
        rebuild.high_res_bytes,
    );
    Ok(rebuild)
}

/// The event a packet that does not decode becomes: the read logs the player
/// out when it reaches it.
fn malformed(frame: &crate::net::Frame, error: &anyhow::Error) -> UiEvent {
    log::warn!(
        "[client910] {} (opcode {}) malformed ({error:#}); logging out",
        crate::proto::server::name(frame.opcode),
        frame.opcode,
    );
    UiEvent::MalformedPacket {
        opcode: frame.opcode,
        size: frame.payload.len(),
        reason: format!("{error:#}"),
    }
}

/// Parse an [`is_ui_frame`] frame into `ui_out`; a payload that does not
/// decode becomes a [`UiEvent::MalformedPacket`] in its place.
fn collect_ui_event(site: FrameSite, frame: &crate::net::Frame, ui_out: &mut Vec<UiEvent>) {
    match parse_ui_event(frame.opcode, &frame.payload) {
        Ok(Some(event)) => {
            match site {
                FrameSite::Startup => log::debug!(
                    "[client910] live {} collected ({:?})",
                    crate::proto::server::name(frame.opcode),
                    event_kind(&event),
                ),
                FrameSite::Poll => log::debug!(
                    "[client910] live poll {} collected ({})",
                    crate::proto::server::name(frame.opcode),
                    event_kind(&event),
                ),
            }
            ui_out.push(event);
        }
        Ok(None) => {}
        Err(err) => ui_out.push(malformed(frame, &err)),
    }
}

/// Zone frames (debug) and anything else (info) neither loop handles.
fn log_ignored(site: FrameSite, opcode: u8, len: usize) {
    let prefix = match site {
        FrameSite::Startup => "live",
        FrameSite::Poll => "live poll",
    };
    match opcode {
        crate::proto::server::UPDATE_ZONE_PARTIAL_FOLLOWS
        | crate::proto::server::UPDATE_ZONE_PARTIAL_ENCLOSED
        | crate::proto::server::UPDATE_ZONE_FULL_FOLLOWS => log::debug!(
            "[client910] {prefix} {} (opcode {opcode}, {len} bytes): ignored",
            crate::proto::server::name(opcode),
        ),
        _ => log::info!(
            "[client910] {prefix} {} (opcode {opcode}, {len} bytes): ignored",
            crate::proto::server::name(opcode),
        ),
    }
}

/// Short kind label for drain/poll logs (avoids dumping full text payloads).
fn event_kind(event: &UiEvent) -> &'static str {
    match event {
        UiEvent::Camera { .. } => "Camera",
        UiEvent::CameraForceAngle { .. } => "CameraForceAngle",
        UiEvent::CameraShake { .. } => "CameraShake",
        UiEvent::CameraMoveTo { .. } => "CameraMoveTo",
        UiEvent::CameraLookAt { .. } => "CameraLookAt",
        UiEvent::CameraReset => "CameraReset",
        UiEvent::CameraSmoothReset => "CameraSmoothReset",
        UiEvent::CameraRemoveRoof { .. } => "CameraRemoveRoof",
        UiEvent::PointLightColour { .. } => "PointLightColour",
        UiEvent::PointLightIntensity { .. } => "PointLightIntensity",
        UiEvent::Audio { .. } => "Audio",
        UiEvent::PlayerSnapshot { .. } => "PlayerSnapshot",
        UiEvent::ClearPlayerSnapshot { .. } => "ClearPlayerSnapshot",
        UiEvent::LobbyAppearance { .. } => "LobbyAppearance",
        UiEvent::StockmarketSlot { .. } => "StockmarketSlot",
        UiEvent::Cutscene { .. } => "Cutscene",
        UiEvent::OverrideEnvironment(_) => "OverrideEnvironment",
        UiEvent::SiteSettings { .. } => "SiteSettings",
        UiEvent::Uid192 { .. } => "Uid192",
        UiEvent::LastLoginInfo { .. } => "LastLoginInfo",
        UiEvent::Telemetry { .. } => "Telemetry",
        UiEvent::UrlOpen { .. } => "UrlOpen",
        UiEvent::SocialNetworkLogout { .. } => "SocialNetworkLogout",
        UiEvent::WorldList { .. } => "WorldList",
        UiEvent::Inventory { .. } => "Inventory",
        UiEvent::Stat { .. } => "Stat",
        UiEvent::RunEnergy { .. } => "RunEnergy",
        UiEvent::RunWeight { .. } => "RunWeight",
        UiEvent::RebootTimer { .. } => "RebootTimer",
        UiEvent::SetTarget { .. } => "SetTarget",
        UiEvent::SetDrawOrder { .. } => "SetDrawOrder",
        UiEvent::ChatFilters { .. } => "ChatFilters",
        UiEvent::ChatPrivateFilter { .. } => "ChatPrivateFilter",
        UiEvent::UpdateDob { .. } => "UpdateDob",
        UiEvent::LoyaltyUpdate { .. } => "LoyaltyUpdate",
        UiEvent::JCoinsUpdate { .. } => "JCoinsUpdate",
        UiEvent::CreateEmailReply { .. } => "CreateEmailReply",
        UiEvent::AccountCreationResult { .. } => "AccountCreationResult",
        UiEvent::CreateNameReply { .. } => "CreateNameReply",
        UiEvent::CreateSuggestNameError { .. } => "CreateSuggestNameError",
        UiEvent::CreateSuggestName { .. } => "CreateSuggestName",
        UiEvent::TriggerDialogAbort => "TriggerDialogAbort",
        UiEvent::GameMessage { .. } => "GameMessage",
        UiEvent::FriendList { .. } => "FriendList",
        UiEvent::FriendListLoaded => "FriendListLoaded",
        UiEvent::IgnoreList { .. } => "IgnoreList",
        UiEvent::FriendChatFull { .. } => "FriendChatFull",
        UiEvent::FriendChatSingle { .. } => "FriendChatSingle",
        UiEvent::FriendChannelMessage { .. } => "FriendChannelMessage",
        UiEvent::ClanChannelFull { .. } => "ClanChannelFull",
        UiEvent::ClanRosterDelta { .. } => "ClanRosterDelta",
        UiEvent::ClanSettingsFull { .. } => "ClanSettingsFull",
        UiEvent::ClanSettingsUpdate { .. } => "ClanSettingsUpdate",
        UiEvent::ClanChannelMessage { .. } => "ClanChannelMessage",
        UiEvent::ClanChannelSystemMessage { .. } => "ClanChannelSystemMessage",
        UiEvent::PlayerGroupFull { .. } => "PlayerGroupFull",
        UiEvent::GroupRosterDelta { .. } => "GroupRosterDelta",
        UiEvent::PlayerGroupVars { .. } => "PlayerGroupVars",
        UiEvent::PlayerGroupMessage { .. } => "PlayerGroupMessage",
        UiEvent::QuickChat { .. } => "QuickChat",
        UiEvent::PublicMessage { .. } => "PublicMessage",
        UiEvent::PrivateMessageEcho { .. } => "PrivateMessageEcho",
        UiEvent::PrivateMessage { .. } => "PrivateMessage",
        UiEvent::ChangeLobby { .. } => "ChangeLobby",
        UiEvent::LogoutTransfer { .. } => "LogoutTransfer",
        UiEvent::Logout { .. } => "Logout",
        UiEvent::VarcAck => "VarcAck",
        UiEvent::SetVarc { .. } => "SetVarc",
        UiEvent::SetVarcBit { .. } => "SetVarcBit",
        UiEvent::SetVarcString { .. } => "SetVarcString",
        UiEvent::VarClanEnable => "VarClanEnable",
        UiEvent::VarClanDisable => "VarClanDisable",
        UiEvent::VarClan { .. } => "VarClan",
        UiEvent::SetTargetParam { .. } => "SetTargetParam",
        UiEvent::SetEvents { .. } => "SetEvents",
        UiEvent::OpenTop { .. } => "OpenTop",
        UiEvent::OpenSub { .. } => "OpenSub",
        UiEvent::OpenSubActive { variant, .. } => match variant {
            ActiveVariant::Loc => "OpenSubActive(Loc)",
            ActiveVariant::Player => "OpenSubActive(Player)",
            ActiveVariant::Npc => "OpenSubActive(Npc)",
            ActiveVariant::Obj => "OpenSubActive(Obj)",
        },
        UiEvent::CloseSub { .. } => "CloseSub",
        UiEvent::MoveSub { .. } => "MoveSub",
        UiEvent::SetText { .. } => "SetText",
        UiEvent::SetHide { .. } => "SetHide",
        UiEvent::SetPosition { .. } => "SetPosition",
        UiEvent::SetScrollPos { .. } => "SetScrollPos",
        UiEvent::SetInterfaceAnim { .. } => "SetInterfaceAnim",
        UiEvent::SetInterfaceModel { .. } => "SetInterfaceModel",
        UiEvent::SetInterfaceObject { .. } => "SetInterfaceObject",
        UiEvent::SetInterfaceHttpImage { .. } => "SetInterfaceHttpImage",
        UiEvent::SetMapFlag { .. } => "SetMapFlag",
        UiEvent::MinimapToggle { .. } => "MinimapToggle",
        UiEvent::HintArrow { .. } => "HintArrow",
        UiEvent::HintTrail { .. } => "HintTrail",
        UiEvent::SetInterfaceColour { .. } => "SetInterfaceColour",
        UiEvent::SetInterfaceAngle { .. } => "SetInterfaceAngle",
        UiEvent::SetInterfaceGraphic { .. } => "SetInterfaceGraphic",
        UiEvent::SetInterfaceTextAntiMacro { .. } => "SetInterfaceTextAntiMacro",
        UiEvent::SetInterfaceTextFont { .. } => "SetInterfaceTextFont",
        UiEvent::SetInterfaceClickMask { .. } => "SetInterfaceClickMask",
        UiEvent::SetInterfaceRecolour { .. } => "SetInterfaceRecolour",
        UiEvent::SetInterfaceRetexture { .. } => "SetInterfaceRetexture",
        UiEvent::RunScript(_) => "RunScript",
        UiEvent::ReflectionProbe(_) => "ReflectionProbe",
        UiEvent::Js5Reload => "Js5Reload",
        UiEvent::ExecuteClientCheat { .. } => "ExecuteClientCheat",
        UiEvent::DoCheat { .. } => "DoCheat",
        UiEvent::DebugServerTriggers { .. } => "DebugServerTriggers",
        UiEvent::UnhandledPacket { .. } => "UnhandledPacket",
        UiEvent::MalformedPacket { .. } => "MalformedPacket",
        UiEvent::SetMoveAction { .. } => "SetMoveAction",
        UiEvent::ShowFaceHere { .. } => "ShowFaceHere",
        UiEvent::SetPlayerOp { .. } => "SetPlayerOp",
        UiEvent::PlayerAttackPriority { .. } => "PlayerAttackPriority",
        UiEvent::NpcAttackPriority { .. } => "NpcAttackPriority",
    }
}

/// The game login plus the startup drain: [`crate::net::login_world`]
/// with `params`, then (unless the server resumed the session in place) [`drain_frames_mode`] over the world socket (through
/// the RTR1 recorder when `CLIENT910_RECORD` is on).
pub async fn login_world_and_drain(
    params: &crate::net::LoginParams,
    strict_entities: bool,
) -> anyhow::Result<LiveState> {
    let (mut stream, ok) = crate::net::login_world(params).await?;
    // An in-place reconnect (reply 15) carries no map: the client keeps its
    // own, and the server's ordinary packets follow on the socket.
    if ok.resume.is_some() {
        return Ok(LiveState {
            login: ok,
            drain: DrainOutcome::default(),
            stream,
        });
    }
    let outcome = if crate::session_record::active() {
        let mut recorded = crate::session_record::RecordedStream(&mut stream);
        drain_frames_mode(&mut recorded, DRAIN_TIMEOUT, strict_entities).await?
    } else {
        drain_frames_mode(&mut stream, DRAIN_TIMEOUT, strict_entities).await?
    };
    Ok(LiveState {
        login: ok,
        drain: outcome,
        stream,
    })
}

/// `SEND_PING` (48/8): `g4s` twice, answered immediately with
/// `SEND_PING_REPLY` and the current frame rate.
pub fn send_ping_reply(payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut r = PayloadReader::new(payload);
    let first = r.g4s()?;
    let second = r.g4s()?;
    r.finish("SEND_PING")?;
    Ok(crate::net::encode_send_ping_reply(
        first,
        second,
        crate::ui_runtime::host_builtins::game_shell_fps(),
    ))
}

/// Handle one post-login frame for the winit render loop (P2), without a
/// runtime: instead of `write_all` it queues reply bytes into
/// `pending_writes` (flushed with `try_write` by `app.rs`), and pushes live
/// interface frames into `ui_out`. Pure apart from the queue appends, so
/// pack-free tests cover it without a socket or runtime.
pub fn handle_sync_frame(
    frame: crate::net::Frame,
    pending_writes: &mut Vec<u8>,
    ui_out: &mut Vec<UiEvent>,
) -> anyhow::Result<Option<RebuildEvent>> {
    match frame.opcode {
        crate::proto::server::REBUILD_NORMAL => match parse_logged_rebuild(&frame.payload) {
            Ok(rebuild) => {
                pending_writes.extend_from_slice(&crate::net::encode_map_build_complete(0));
                Ok(Some(RebuildEvent::new(rebuild)))
            }
            Err(error) => {
                ui_out.push(malformed(&frame, &error));
                Ok(None)
            }
        },
        crate::proto::server::PLAYER_INFO => {
            if frame.payload.is_empty() {
                log::debug!("[client910] live poll PLAYER_INFO: empty payload, ignoring");
            }
            Ok(None)
        }
        // The server's keepalive: the client sends nothing for it.
        crate::proto::server::NO_TIMEOUT => Ok(None),
        crate::proto::server::SEND_PING => {
            match send_ping_reply(&frame.payload) {
                Ok(reply) => pending_writes.extend(reply),
                Err(error) => ui_out.push(malformed(&frame, &error)),
            }
            Ok(None)
        }
        crate::proto::server::SERVER_TICK_END => Ok(None),
        opcode if is_ui_frame(opcode) => {
            collect_ui_event(FrameSite::Poll, &frame, ui_out);
            Ok(None)
        }
        opcode => {
            log_ignored(FrameSite::Poll, opcode, frame.payload.len());
            Ok(None)
        }
    }
}

/// Drain fully-buffered frames from `pending` without touching the socket
/// (runtime-free, for the winit thread): game-phase resync via
/// [`crate::net::decode_frame_game`] (unknown bytes skipped with a warn, never
/// fatal), replies queued to `pending_writes`, interface frames to `ui_out`.
/// Returns the first `REBUILD_NORMAL` found, if any. Returns immediately when
/// no complete frame is buffered (the P2 no-block guarantee).
pub fn drain_pending_sync(
    pending: &mut Vec<u8>,
    state: &mut crate::net::ResyncState,
    pending_writes: &mut Vec<u8>,
    ui_out: &mut Vec<UiEvent>,
) -> anyhow::Result<Option<RebuildEvent>> {
    loop {
        let decoded = crate::net::decode_frame_game(pending, state)?;
        let Some((kind, used)) = decoded else {
            return Ok(None);
        };
        pending.drain(..used);
        match kind {
            crate::net::GameDecode::Frame(frame) => {
                if let Some(event) = handle_sync_frame(frame, pending_writes, ui_out)? {
                    return Ok(Some(event));
                }
            }
            crate::net::GameDecode::Skipped { .. } => {}
        }
    }
}

/// Entity-aware polling. Strict framing is required: byte resynchronization
/// cannot reconstruct PLAYER/NPC delta state. Frames remain owned by `entities`
/// until the CPU context adapter accepts them. Existing UI/keepalive processing
/// uses the same handlers as before. The app stops polling while a front entity
/// packet needs context, preserving the rest of `pending` without further reads.
pub fn drain_pending_entities(
    pending: &mut Vec<u8>,
    pending_writes: &mut Vec<u8>,
    ui_out: &mut Vec<UiEvent>,
    entities: &mut crate::protocol910::live::Feed,
    contexts: &crate::protocol910::live::Contexts,
) -> anyhow::Result<Option<RebuildEvent>> {
    loop {
        while entities.front().is_some() {
            let applied = entities.apply_next(contexts).map_err(|error| {
                anyhow::anyhow!(
                    "entity stream paused at opcode {:?}: {error:?}",
                    entities.front().map(|f| f.opcode)
                )
            })?;
            // Yield after one entity operation: caller must refresh context and
            // consume the retained RNG/refresh receipt before the next delta.
            if applied.is_some() {
                return Ok(None);
            }
        }
        let Some((frame, used)) = crate::net::decode_frame(pending)? else {
            return Ok(None);
        };
        if entities.enqueue(frame.opcode, &frame.payload) {
            pending.drain(..used);
            continue;
        }
        let event = handle_sync_frame(frame, pending_writes, ui_out)?;
        pending.drain(..used);
        if event.is_some() {
            return Ok(event);
        }
    }
}

/// The live poll's write half (the connection flush): write as much of
/// `pending_writes` as the non-blocking `stream` takes, handing each written
/// chunk to `wrote` before it leaves the queue. Returns at a zero write or
/// `WouldBlock`; any other error is the caller's I/O error.
pub fn flush_nonblocking<W: std::io::Write>(
    stream: &mut W,
    pending_writes: &mut Vec<u8>,
    mut wrote: impl FnMut(&[u8]),
) -> std::io::Result<()> {
    while !pending_writes.is_empty() {
        match stream.write(pending_writes) {
            Ok(0) => break,
            Ok(consumed) => {
                wrote(&pending_writes[..consumed]);
                pending_writes.drain(..consumed);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(err),
        }
    }
    // A masked stream may hold bytes the socket had no room for.
    match stream.flush() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        other => other,
    }
}

/// The live poll's read half: append everything the non-blocking `stream`
/// has buffered to `pending` (4 KB at a time), handing each chunk to `read`
/// first. `Ok(true)` when the peer closed the stream; the bytes read before
/// the end of stream stay in `pending`, in order.
pub fn read_nonblocking<R: std::io::Read>(
    stream: &mut R,
    pending: &mut Vec<u8>,
    mut read: impl FnMut(&[u8]),
) -> std::io::Result<bool> {
    let mut scratch = [0u8; 4096];
    loop {
        match stream.read(&mut scratch) {
            Ok(0) => return Ok(true),
            Ok(consumed) => {
                read(&scratch[..consumed]);
                pending.extend_from_slice(&scratch[..consumed]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../tests/phase-g/session-checks.rs"]
mod phase_g_wiring;

#[cfg(test)]
mod client_debug_packet_tests;
