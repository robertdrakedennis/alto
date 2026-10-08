//! Interface property and client-script payload decoding.

/// `PayloadReader` and `cp1252_byte` (split out in Phase 2.2).
pub use crate::payload_reader::*;

use super::{decode_p2_alt2, RunClientScript, ScriptArg};
#[cfg(any(test, feature = "test-hooks"))]
use super::{parse_ui_event, UiEvent};

/// Decode `g1_alt1`: wire `value + 128` (`Packet.ts:201-203`).
pub(super) fn decode_g1_alt1(raw: u8) -> u8 {
    raw.wrapping_sub(128)
}

/// Decode `g1_alt2`: wire `-value` (`Packet.ts:205-207`).
pub(super) fn decode_g1_alt2(raw: u8) -> u8 {
    raw.wrapping_neg()
}

/// Decode `g1_alt3`: wire `128 - value` (`Packet.ts:209-211`).
pub(super) fn decode_g1_alt3(raw: u8) -> u8 {
    128u8.wrapping_sub(raw)
}

/// Decode `g2_alt2` for scroll values (unsigned `p2_alt2`).
pub(super) fn decode_g2_alt2_u16(hi: u8, lo_plus: u8) -> u16 {
    decode_p2_alt2(hi, lo_plus)
}

/// Decode `g4_alt1`: little-endian (`Packet.ts:243-247`).
pub(super) fn decode_g4_alt1(bytes: [u8; 4]) -> i32 {
    i32::from_le_bytes(bytes)
}

/// Parse `IF_OPENSUB` (38, 23 bytes): key, `g4_alt1` parent, `g1_alt2` type,
/// `g4s` key, `g2` sub, two keys
/// (server table `:312-322`).
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_opensub(payload: &[u8]) -> anyhow::Result<(u32, u32, u8)> {
    match parse_ui_event(crate::proto::server::IF_OPENSUB, payload)? {
        Some(UiEvent::OpenSub {
            parent_packed,
            sub_id,
            kind,
            ..
        }) => Ok((parent_packed, sub_id, kind)),
        _ => unreachable!(),
    }
}

/// Parse `IF_OPENSUB_ACTIVE_LOC` (26, 32 bytes): `g4_alt1` parent, coord
/// `g4_alt3`, `g2` sub, `g1_alt3` type, four keys, `g1` + `g4s` loc tail.
/// This compatibility view returns parent/sub/type;
/// `parse_ui_event` retains the complete keys and binding.
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_opensub_active_loc(payload: &[u8]) -> anyhow::Result<(u32, u32, u8)> {
    match parse_ui_event(crate::proto::server::IF_OPENSUB_ACTIVE_LOC, payload)? {
        Some(UiEvent::OpenSubActive {
            parent_packed,
            sub_id,
            kind,
            ..
        }) => Ok((parent_packed, sub_id, kind)),
        _ => unreachable!(),
    }
}

/// Parse `IF_OPENSUB_ACTIVE_PLAYER` (61, 25 bytes): keys, `g4_alt2` parent,
/// key, `g1` type, `g2` player, key, `g2_alt3` sub.
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_opensub_active_player(payload: &[u8]) -> anyhow::Result<(u32, u32, u8)> {
    match parse_ui_event(crate::proto::server::IF_OPENSUB_ACTIVE_PLAYER, payload)? {
        Some(UiEvent::OpenSubActive {
            parent_packed,
            sub_id,
            kind,
            ..
        }) => Ok((parent_packed, sub_id, kind)),
        _ => unreachable!(),
    }
}

/// Parse `IF_OPENSUB_ACTIVE_NPC` (102, 25 bytes): `g2_alt3` sub, three keys,
/// `g2_alt1` npc, `g1` type, key, `g4_alt1` parent.
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_opensub_active_npc(payload: &[u8]) -> anyhow::Result<(u32, u32, u8)> {
    match parse_ui_event(crate::proto::server::IF_OPENSUB_ACTIVE_NPC, payload)? {
        Some(UiEvent::OpenSubActive {
            parent_packed,
            sub_id,
            kind,
            ..
        }) => Ok((parent_packed, sub_id, kind)),
        _ => unreachable!(),
    }
}

/// Parse `IF_OPENSUB_ACTIVE_OBJ` (121, 29 bytes): `g2` sub, key, coord
/// `g4_alt2`, `g4_alt1` parent, key, `g2_alt1` obj, key, `g1_alt1` type, key.
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_opensub_active_obj(payload: &[u8]) -> anyhow::Result<(u32, u32, u8)> {
    match parse_ui_event(crate::proto::server::IF_OPENSUB_ACTIVE_OBJ, payload)? {
        Some(UiEvent::OpenSubActive {
            parent_packed,
            sub_id,
            kind,
            ..
        }) => Ok((parent_packed, sub_id, kind)),
        _ => unreachable!(),
    }
}

/// Parse `IF_SETTEXT` (181, `-2`): `g4_alt1` packed + `gjstr` text
/// (server table `:191`, size `-2`).
pub fn parse_if_settext(payload: &[u8]) -> anyhow::Result<(u32, String)> {
    let mut reader = PayloadReader::new(payload);
    let packed = reader.g4_alt1()?;
    let text = reader.gjstr()?;
    reader.finish("IF_SETTEXT")?;
    Ok((packed as u32, text))
}

/// Parse `IF_SETHIDE` (109, 5 bytes): `g4s` packed + `g1_alt2` flag
/// (server table `:119`).
#[cfg(any(test, feature = "test-hooks"))] // test-only parser
pub fn parse_if_sethide(payload: &[u8]) -> anyhow::Result<(u32, bool)> {
    if payload.len() != 5 {
        anyhow::bail!("IF_SETHIDE: need 5 bytes, got {}", payload.len());
    }
    let mut reader = PayloadReader::new(payload);
    let packed = reader.g4s()?;
    let flag = reader.g1_alt2()?;
    reader.finish("IF_SETHIDE")?;
    Ok((packed as u32, flag == 1))
}

/// Parse `IF_SETPOSITION` (72, 8 bytes): `g2s_alt1` x, `g2s_alt2` y,
/// `g4_alt1` packed (server table `:82`).
pub fn parse_if_setposition(payload: &[u8]) -> anyhow::Result<(u32, i16, i16)> {
    if payload.len() != 8 {
        anyhow::bail!("IF_SETPOSITION: need 8 bytes, got {}", payload.len());
    }
    let mut reader = PayloadReader::new(payload);
    let x = reader.g2s_alt1()?;
    let y = reader.g2s_alt2()?;
    let packed = reader.g4_alt1()?;
    reader.finish("IF_SETPOSITION")?;
    Ok((packed as u32, x, y))
}

/// Parse `IF_SETSCROLLPOS` (79, 6 bytes): `g4s` packed + `g2_alt2` scroll
/// (server table `:89`).
pub fn parse_if_setscrollpos(payload: &[u8]) -> anyhow::Result<(u32, u16)> {
    if payload.len() != 6 {
        anyhow::bail!("IF_SETSCROLLPOS: need 6 bytes, got {}", payload.len());
    }
    let mut reader = PayloadReader::new(payload);
    let packed = reader.g4s()?;
    let scroll = reader.g2_alt2_u16()?;
    reader.finish("IF_SETSCROLLPOS")?;
    Ok((packed as u32, scroll))
}

/// Parse `RUNCLIENTSCRIPT` (156, `-2`): `gjstr` descriptor, then one value
/// per descriptor char in reverse-index order (`'s'` → `gjstr`, else `g4s`),
/// then `g4s` script id (server table `:333-356`).
/// The returned args are in stored order (reverse of the wire).
pub fn parse_runclientscript(payload: &[u8]) -> anyhow::Result<RunClientScript> {
    let mut reader = PayloadReader::new(payload);
    let descriptor = reader.gjstr()?;
    let chars: Vec<char> = descriptor.chars().collect();
    // Reverse-index reads: first wire value uses the last descriptor char.
    let mut stored: Vec<Option<ScriptArg>> = vec![None; chars.len()];
    for index in (0..chars.len()).rev() {
        let is_str = chars[index] == 's';
        let arg = if is_str {
            ScriptArg::Str(reader.gjstr()?)
        } else {
            ScriptArg::Int(reader.g4s()?)
        };
        stored[index] = Some(arg);
    }
    let script_id = reader.g4s()?;
    reader.finish("RUNCLIENTSCRIPT")?;
    let mut args = Vec::with_capacity(chars.len());
    for slot in stored {
        match slot {
            Some(arg) => args.push(arg),
            None => anyhow::bail!("RUNCLIENTSCRIPT: missing arg slot"),
        }
    }
    Ok(RunClientScript { script_id, args })
}
