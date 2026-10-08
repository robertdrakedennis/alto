//! Game-stream framing (opcode + size prefix) and the pure client packet
//! encoders: the byte
//! half of `net.rs`. Sockets, login and timing stay in client910's `net.rs`,
//! which re-exports this module (`pub use rs910_protocol::framing::*`).
//!
//! Moved from `net.rs` by the Phase 2.2 codemod
//! (`tools/refactor/steps/p2-proto-move.py`).

use anyhow::{bail, Context, Result};

use crate::proto;

/// One framed server-to-client packet: opcode + payload (size prefix consumed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Cursor for frame-stream decoding: absolute byte offset plus the last
/// successfully decoded server opcode. Mirrors the diagnostics the reference
/// client keeps when a read fails: failing packet id, the two previous packet
/// types, size, position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResyncState {
    /// Absolute offset of the next unread byte in the frame stream.
    pub offset: u64,
    /// Last successfully decoded server opcode, if any.
    pub last_good: Option<u8>,
}

impl ResyncState {
    /// Human-readable name of [`Self::last_good`] for error messages.
    #[must_use]
    pub fn last_good_name(&self) -> &'static str {
        match self.last_good {
            Some(opcode) => proto::server::name(opcode),
            None => "none",
        }
    }

    /// Advance past one decoded frame. (A method, not inline `+=`, so the
    /// success-path bookkeeping in [`read_frame`] never trips
    /// `unused_assignments`: the cursor is only *observed* on later skips.)
    pub fn advance(&mut self, used: u64, opcode: u8) {
        self.offset += used;
        self.last_good = Some(opcode);
    }
}

/// Legacy viewer decode step: a full frame or a skipped invalid opcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameDecode {
    /// A complete frame (length prefix already consumed).
    Frame(Frame),
    /// An unknown opcode skipped during legacy resync (no frame yet).
    Skipped {
        /// The decoded 1-or-2-byte opcode value.
        opcode: u16,
        /// Absolute stream offset the byte was found at.
        offset: u64,
    },
}

// ---------------------------------------------------------------------------
// Pure client->server packet encoders (exact bytes the senders write).
// ---------------------------------------------------------------------------

/// `NO_TIMEOUT` (103, size 0): bare opcode. No ISAAC obfuscation (disabled).
#[must_use]
pub fn encode_no_timeout() -> Vec<u8> {
    vec![proto::client::NO_TIMEOUT]
}

/// `UID_PASSPORT_RESEND_REQUEST` (82, size 0): bare opcode. The device check
/// asks the server to email its link again.
#[must_use]
pub fn encode_uid_passport_resend_request() -> Vec<u8> {
    vec![proto::client::UID_PASSPORT_RESEND_REQUEST]
}

/// `MAP_BUILD_COMPLETE` (79, size 4): opcode + `p4(elapsed_ms)`.
/// The client sends the map-build time in ms.
#[must_use]
pub fn encode_map_build_complete(elapsed_ms: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(proto::client::MAP_BUILD_COMPLETE);
    out.extend_from_slice(&elapsed_ms.to_be_bytes());
    out
}

/// `SEND_PING_REPLY` (100, size 9): the reply to `SEND_PING`
/// echoes both ints as `p4_alt1`/`p4_alt3`
/// followed by `p1_alt2(fps)`.
#[must_use]
pub fn encode_send_ping_reply(first: i32, second: i32, fps: i32) -> Vec<u8> {
    let [a0, a1, a2, a3] = first.to_le_bytes();
    let [b0, b1, b2, b3] = second.to_le_bytes();
    vec![
        proto::client::SEND_PING_REPLY,
        a0,
        a1,
        a2,
        a3,
        b2,
        b3,
        b0,
        b1,
        (fps as u8).wrapping_neg(),
    ]
}

/// `WINDOW_STATUS` (123, size 6): `p1(mode) p2(width) p2(height) p1(aa)`.
/// Sent when the window mode or size changes.
#[must_use]
pub fn encode_window_status(mode: u8, width: u16, height: u16, antialiasing: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(7);
    out.push(proto::client::WINDOW_STATUS);
    out.push(mode);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.push(antialiasing);
    out
}

/// `WORLDLIST_FETCH` (77, size 4): opcode + `p4` token. The server
/// (`Lobby.ts` `lobbyDecode`) ignores the payload and replies
/// `WORLDLIST_FETCH_REPLY`.
#[must_use]
pub fn encode_worldlist_fetch(token: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(proto::client::WORLDLIST_FETCH);
    out.extend_from_slice(&token.to_be_bytes());
    out
}

/// `TRANSMITVAR_VERIFYID` (22, size 4): opcode + `p4(counter)`, the number of
/// server packets that changed interface state since the session began.
#[must_use]
pub fn encode_transmitvar_verifyid(counter: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(proto::client::TRANSMITVAR_VERIFYID);
    out.extend_from_slice(&counter.to_be_bytes());
    out
}

/// What `PING_STATISTICS` reports of the collector when there is none.
pub const NO_COLLECTOR_PERCENT: i32 = -1;

/// `PING_STATISTICS` (74, size 4): the round trip to the game host in
/// milliseconds (`p2_alt1`), the frame rate (`p1_alt3`) and the percentage of
/// time spent collecting garbage, or [`NO_COLLECTOR_PERCENT`] (`p1`).
#[must_use]
pub fn encode_ping_statistics(ping_ms: i32, fps: i32, collector_percent: i32) -> Vec<u8> {
    let [lo, hi, ..] = ping_ms.to_le_bytes();
    vec![
        proto::client::PING_STATISTICS,
        lo,
        hi,
        128_i32.wrapping_sub(fps) as u8,
        collector_percent as u8,
    ]
}

/// The state of the JS5 connection a [`encode_map_build_stuck`] report carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Js5Stall {
    pub connect_state: i32,
    pub error_count: i32,
    pub js5_state: i32,
    pub urgents_full: bool,
    pub prefetches_full: bool,
    pub pending_requests: i32,
}

/// `MAP_BUILD_STUCK` (0, size 15): the map build waited for the same
/// non-zero number of loc models for a thousand cycles. Carries the last
/// loc and model waited on, the count (at most 65535) and the JS5 state.
#[must_use]
pub fn encode_map_build_stuck(loc: i32, model: i32, waiting: i32, js5: &Js5Stall) -> Vec<u8> {
    let waiting = waiting.min(65535);
    let errors = js5.error_count.min(255);
    let pending = js5.pending_requests.min(255);
    let flags = (i32::from(js5.prefetches_full) << 1) | i32::from(js5.urgents_full);
    let [l0, l1, l2, l3] = loc.to_le_bytes();
    let [m0, m1, m2, m3] = model.to_le_bytes();
    vec![
        proto::client::MAP_BUILD_STUCK,
        128_i32.wrapping_sub(js5.js5_state) as u8,
        (flags as u8).wrapping_neg(),
        128_i32.wrapping_sub(errors) as u8,
        waiting as u8,
        (waiting >> 8) as u8,
        js5.connect_state as u8,
        l2,
        l3,
        l0,
        l1,
        m0,
        m1,
        m2,
        m3,
        128_i32.wrapping_sub(pending) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Pure frame decoder (shared by the socket reader and unit tests).
// ---------------------------------------------------------------------------

/// Disabling ISAAC removes the cipher, not the two-byte
/// opcode encoding. Wait for both bytes before interpreting opcodes >= 128.
fn opcode_header(buf: &[u8]) -> Option<(u16, usize)> {
    let first = *buf.first()?;
    if first < 128 {
        Some((u16::from(first), 1))
    } else {
        Some(((u16::from(first) - 128) * 256 + u16::from(*buf.get(1)?), 2))
    }
}
fn known_opcode(op: u16) -> Option<(u8, i32)> {
    let opcode = u8::try_from(op).ok()?;
    Some((opcode, proto::server::size(opcode)?))
}
/// Strict 1-or-2-byte opcode, optional byte/short length,
/// then payload. A partial header or body consumes nothing in this buffered API.
pub fn decode_frame_at(buf: &[u8], state: &ResyncState) -> Result<Option<(Frame, usize)>> {
    let Some((op, mut header)) = opcode_header(buf) else {
        return Ok(None);
    };
    let Some((opcode, fixed)) = known_opcode(op) else {
        bail!(
            "unknown server opcode {op} at stream offset {} (last good: {})",
            state.offset,
            state.last_good_name()
        );
    };
    let len = match fixed {
        -2 => {
            let Some(size) = buf.get(header..header + 2) else {
                return Ok(None);
            };
            header += 2;
            u16::from_be_bytes([size[0], size[1]]) as usize
        }
        -1 => {
            let Some(&size) = buf.get(header) else {
                return Ok(None);
            };
            header += 1;
            size as usize
        }
        n => usize::try_from(n).context("negative fixed frame size")?,
    };
    let Some(payload) = buf.get(header..header + len) else {
        return Ok(None);
    };
    Ok(Some((
        Frame {
            opcode,
            payload: payload.to_vec(),
        },
        header + len,
    )))
}
/// Strict buffered decoder with an implicit zero cursor.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
    decode_frame_at(buf, &ResyncState::default())
}
/// Legacy viewer recovery only; the faithful entity path uses decode_frame_at.
/// Skipping an invalid opcode cannot recover a misaligned delta stream reliably.
pub fn decode_frame_game(
    buf: &[u8],
    state: &mut ResyncState,
) -> Result<Option<(GameDecode, usize)>> {
    let Some((opcode, header)) = opcode_header(buf) else {
        return Ok(None);
    };
    if known_opcode(opcode).is_none() {
        log::warn!("[client910] WARN game frame: unknown server opcode {opcode} at stream offset {} (last good: {}); skipping {header} opcode bytes",state.offset,state.last_good_name());
        let skipped = GameDecode::Skipped {
            opcode,
            offset: state.offset,
        };
        state.offset += header as u64;
        return Ok(Some((skipped, header)));
    }
    let Some((frame, used)) = decode_frame_at(buf, state)? else {
        return Ok(None);
    };
    state.advance(used as u64, frame.opcode);
    Ok(Some((GameDecode::Frame(frame), used)))
}
