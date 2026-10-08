//! Disk-backed reader for packed `client.<name>.js5` archives.
//!
//! Disk layout mirrors `server/src/formats/js5/Js5.ts` `load()` + constructor
//! exactly (`Js5.ts:29-47` master index, `Js5.ts:214-230` trailer):
//!
//! ```text
//! [master container][group 0 container]...[group N container][u32be group lengths x N]
//! ```
//!
//! * The file opens with one js5 container (the archive index). Its byte
//!   length is `5 + declared_len (+ 4 when compressed)`, read from its own
//!   5-byte header.
//! * The file closes with `group_count * 4` bytes: each group's on-disk
//!   container length as big-endian `u32`, in archive-index order. A length
//!   `< 5` means the group is absent (same rule as `Js5.ts:225`).
//! * Group `i` starts at `master_len + sum(lengths[..i])`.
//!
//! Groups are fetched with `File` + `Seek` so real multi-GB packs (e.g. the
//! 6.5 GB audiostreams archive) never get fully mapped or read: only the
//! master container, the ~`group_count * 4` trailer, and the requested group
//! container touch the disk. No patch merging and no whole-file reads.
//!
//! Group payloads are split with the existing
//! `native910::js5::{ArchiveIndex, decompress, unpack_group}` primitives —
//! decompression is never reimplemented here.
//!
//! XTEA: most map groups are open, but some regions are locked. The server
//! threads an optional key per group (`server/src/formats/js5/Js5.ts:458,478`
//! `key: number[] | null`, default `null`): `null` or all-zero means open
//! (`Js5.ts:514`), otherwise the packed container is decrypted in place with
//! `tinydec(key, 5, len)` (`Js5.ts:514-521`) BEFORE decompression. Lumbridge
//! loads pass `null` (`server/src/lostcity/engine/collision/CollisionManager.ts:38,42`),
//! which is why the 3x3 block streams keyless.
//!
//! Keys come from OpenRS2 `keys.json`
//! (`server/src/lostcity/util/OpenRS2.ts:124-126` `getKeys`, saved by hand
//! from `https://archive.openrs2.org/caches/runescape/1730/keys.json` to
//! `<pack_root>/keys.json`; id 1730 served `[]` when checked 2026-09-09):
//! [`load_keys`] parses it (missing file = empty map, so open groups keep
//! working), [`lookup_key`] resolves one group (absent or all-zero = open),
//! and [`decrypt_container`]/[`tinydec`] apply it before the existing
//! `native910::js5::{decompress, unpack_group}` primitives — which assume
//! plaintext containers and are never reimplemented here.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use native910::error::NativeError;
use native910::js5::{decompress, unpack_group, ArchiveIndex};
use thiserror::Error;

/// File id of the loc stream inside a map group (the loc file).
pub const LOC_FILE: u32 = 0;
/// File id of the NPC spawn list inside a map group.
pub const NPC_FILE: u32 = 2;
/// File id of the landscape stream inside a map group (the land file).
pub const LAND_FILE: u32 = 3;

/// On-disk name under the pack root. Mirrors `OpenRS2.ts:125`
/// (`data/cache/<id>/keys.json`), relocated beside the packs it unlocks so one
/// `Pack` root is self-contained.
pub const KEYS_FILENAME: &str = "keys.json";

/// One XTEA key: four words (`Js5.ts:514-521` `key: number[]`, indexed by
/// `Packet.ts:596,602` `key[(sum >>> 11) & 0x3]` / `key[sum & 0x3]`).
pub type XteaKey = [u32; 4];
/// `(archive id, group id) -> key`, parsed from `keys.json` (`archive` /
/// `group` / `key` per entry; see the live shape sampled in tests).
pub type KeyMap = BTreeMap<(u32, u32), XteaKey>;

/// XTEA block decrypt of a js5 container's payload, in place.
///
/// Mirrors `server/src/formats/bytepacking/Packet.ts:582-612`
/// `tinydec(key, offset, length)` as called from
/// `server/src/formats/js5/Js5.ts:519` (`buf.tinydec(key, 5, buf.length)`):
/// 32 rounds, delta `0x9E3779B9`, `sum` seeded at `delta * 32` (wrapping),
/// big-endian `u32` pairs (`Packet.ts:381-383` `g4` / `:239-240` `p4` use
/// `DataView.get/setInt32` with no little-endian flag, i.e. big-endian) from
/// `offset = 5` — the 5-byte container header is never touched — over
/// `blocks = (len - 5) / 8` whole pairs only, so a trailing partial block is
/// left as-is exactly like the TS `(length - offset) / 8 | 0` count (a
/// container shorter than the header decrypts to a no-op, matching the
/// negative-count loop skip). JS `>>>` / `| 0` wrapping maps to `u32`
/// `wrapping_*` here. Hand-rolled on purpose: no cipher crates.
pub fn tinydec(container: &mut [u8], key: XteaKey) {
    const OFFSET: usize = 5;
    const DELTA: u32 = 0x9E3779B9;
    if container.len() <= OFFSET {
        return;
    }
    let blocks = (container.len() - OFFSET) / 8;
    for block in 0..blocks {
        let base = OFFSET + block * 8;
        let mut v0 = u32::from_be_bytes([
            container[base],
            container[base + 1],
            container[base + 2],
            container[base + 3],
        ]);
        let mut v1 = u32::from_be_bytes([
            container[base + 4],
            container[base + 5],
            container[base + 6],
            container[base + 7],
        ]);
        // `Packet.ts:593`: `sum = delta * num_rounds` evaluated as f64 then
        // reduced mod 2^32 by the first `^`; `wrapping_mul(32)` is that value.
        let mut sum: u32 = DELTA.wrapping_mul(32);
        let mut rounds: u32 = 32;
        // `Packet.ts:595` `while (num_rounds-- > 0)`.
        while rounds > 0 {
            rounds -= 1;
            // `Packet.ts:596`.
            v1 = v1.wrapping_sub(
                (v0.wrapping_shl(4) ^ v0.wrapping_shr(5)).wrapping_add(v0)
                    ^ sum.wrapping_add(key[((sum >> 11) & 0x3) as usize]),
            );
            // `Packet.ts:599-600` (`>>> 0` after the subtract).
            sum = sum.wrapping_sub(DELTA);
            // `Packet.ts:602` (reads the already-updated `sum` and `v1`).
            v0 = v0.wrapping_sub(
                (v1.wrapping_shl(4) ^ v1.wrapping_shr(5)).wrapping_add(v1)
                    ^ sum.wrapping_add(key[(sum & 0x3) as usize]),
            );
        }
        container[base..base + 4].copy_from_slice(&v0.to_be_bytes());
        container[base + 4..base + 8].copy_from_slice(&v1.to_be_bytes());
    }
}

/// Test-only XTEA encrypt: the inverse block walk of [`tinydec`], mirroring
/// `server/src/formats/bytepacking/Packet.ts:557-580` `tinyenc(key, offset, length)`.
///
/// One deliberate deviation: `:571` masks the key index with `0xE8C00003`,
/// which cannot index a 4-word key; the mirror uses the standard `& 0x3`
/// (matching `tinydec` `:596`), otherwise encrypt/decrypt could never
/// round-trip. TODO(#gap-1): confirm the real `tinyenc` mask at `:571`.
#[cfg(test)]
pub(crate) fn tinyenc(container: &mut [u8], key: XteaKey) {
    const OFFSET: usize = 5;
    const DELTA: u32 = 0x9E3779B9;
    if container.len() <= OFFSET {
        return;
    }
    let blocks = (container.len() - OFFSET) / 8;
    for block in 0..blocks {
        let base = OFFSET + block * 8;
        let mut v0 = u32::from_be_bytes([
            container[base],
            container[base + 1],
            container[base + 2],
            container[base + 3],
        ]);
        let mut v1 = u32::from_be_bytes([
            container[base + 4],
            container[base + 5],
            container[base + 6],
            container[base + 7],
        ]);
        // `Packet.ts:565` `sum = 0`.
        let mut sum: u32 = 0;
        // `Packet.ts:567-568` 32 rounds.
        for _ in 0..32 {
            // `Packet.ts:569` (`+` binds tighter than `^` in JS).
            v0 = v0.wrapping_add(
                v1.wrapping_add(v1.wrapping_shl(4) ^ v1.wrapping_shr(5))
                    ^ sum.wrapping_add(key[(sum & 0x3) as usize]),
            );
            // `Packet.ts:570`.
            sum = sum.wrapping_add(DELTA);
            // `Packet.ts:571` with the `& 0x3` deviation noted above.
            v1 = v1.wrapping_add(
                (v0.wrapping_shr(5) ^ v0.wrapping_shl(4)).wrapping_add(v0)
                    ^ sum.wrapping_add(key[((sum >> 11) & 0x3) as usize]),
            );
        }
        container[base..base + 4].copy_from_slice(&v0.to_be_bytes());
        container[base + 4..base + 8].copy_from_slice(&v1.to_be_bytes());
    }
}

/// True when a key means "open" (no decryption).
///
/// `Js5.ts:514` treats `null` and `[0, 0, 0, 0]` identically; the Lumbridge
/// loads in `CollisionManager.ts:38,42` rely on the `null` side of that.
#[must_use]
pub fn key_is_open(key: &XteaKey) -> bool {
    key == &[0, 0, 0, 0]
}

/// On-disk path of the cached `keys.json` for a pack root.
#[must_use]
pub fn keys_path(pack_root: &Path) -> PathBuf {
    pack_root.join(KEYS_FILENAME)
}

/// Load `(archive, group) -> XTEA key` from `<pack_root>/keys.json`.
///
/// A missing file parses to an empty map (open regions stream keyless, the
/// `CollisionManager.ts:38,42` case); a present-but-corrupt file is a hard
/// [`CacheError::Corrupt`] — fail fast so operators notice instead of
/// silently mis-decoding locked groups.
pub fn load_keys(pack_root: &Path) -> Result<KeyMap, CacheError> {
    let path = keys_path(pack_root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(CacheError::Io(error)),
    };
    parse_keys_json(&text)
        .map_err(|detail| CacheError::Corrupt(format!("{}: {detail}", path.display())))
}

/// Resolve the key for one group: `None` when the map has no entry or the
/// entry is all-zero (open, per [`key_is_open`] / `Js5.ts:514`).
#[must_use]
pub fn lookup_key(keys: &KeyMap, archive: u32, group: u32) -> Option<XteaKey> {
    keys.get(&(archive, group))
        .copied()
        .filter(|key| !key_is_open(key))
}

/// Copy `container` with XTEA applied when `key` is `Some`.
///
/// Decryption covers bytes `5..len` ([`tinydec`], mirroring
/// `Js5.ts:517-520`: `unwrap`, then `tinydec`, then `decompress`); `None`
/// returns the bytes unchanged. Callers always run this BEFORE
/// `native910::js5::{decompress, unpack_group}`, which assume plaintext.
#[must_use]
pub fn decrypt_container(container: &[u8], key: Option<XteaKey>) -> Vec<u8> {
    let mut plain = container.to_vec();
    if let Some(key) = key {
        tinydec(&mut plain, key);
    }
    plain
}

/// The `(name hash, group id)` pairs of a decoded archive index
/// (`Js5Index` protocol 5-7 header up to the names block; the rest of the
/// index is decoded by `native910::js5::ArchiveIndex`). Empty when the index
/// carries no names (flag bit 1 clear).
pub fn group_name_hashes(raw: &[u8]) -> Result<Vec<(i32, u32)>, CacheError> {
    let mut packet = native910::packet::Packet::new(raw);
    let protocol = packet.g1()?;
    if !(5..=7).contains(&protocol) {
        return Err(CacheError::Native(NativeError::Invalid(format!(
            "unsupported archive index protocol: {protocol}"
        ))));
    }
    if protocol >= 6 {
        let _version = packet.g4s()?;
    }
    let flags = packet.g1()?;
    let group_count = if protocol >= 7 {
        packet.gsmart2or4null()? as u32
    } else {
        u32::from(packet.g2()?)
    };
    let mut ids = Vec::with_capacity(group_count as usize);
    let mut last = 0_u32;
    for _ in 0..group_count {
        let delta = if protocol >= 7 {
            packet.gsmart2or4null()? as u32
        } else {
            u32::from(packet.g2()?)
        };
        last = last.wrapping_add(delta);
        ids.push(last);
    }
    if flags & 1 == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push((packet.g4s()?, id));
    }
    Ok(out)
}

/// `Js5Archive` id of a pack file name (XTEA lookup, JS5 disk stores).
pub fn archive_id_for_name(archive_name: &str) -> Option<u32> {
    ARCHIVE_NAMES
        .iter()
        .find(|(_, name)| *name == archive_name)
        .map(|(id, _)| *id)
}

/// Pack name of a `Js5Archive` id (the inverse of [`archive_id_for_name`]).
#[must_use]
pub fn archive_name_for_id(archive: u32) -> Option<&'static str> {
    ARCHIVE_NAMES
        .iter()
        .find(|(id, _)| *id == archive)
        .map(|(_, name)| *name)
}

/// Archive ids and the pack file names the
/// server's `Js5Archive` gives them (`server/src/formats/config/Js5Archive.ts:64-105`).
const ARCHIVE_NAMES: [(u32, &str); 42] = [
    (0, "anims"),
    (1, "bases"),
    (2, "config"),
    (3, "interfaces"),
    (5, "mapsv2"),
    (7, "models"),
    (8, "sprites"),
    (10, "binary"),
    (12, "scripts"),
    (13, "fontmetrics"),
    (14, "vorbis"),
    (16, "loc.config"),
    (17, "enum.config"),
    (18, "npc.config"),
    (19, "obj.config"),
    (20, "seq.config"),
    (21, "spot.config"),
    (22, "struct.config"),
    (23, "worldmap"),
    (24, "quickchat"),
    (25, "global.quickchat"),
    (26, "materials"),
    (27, "particles"),
    (28, "defaults"),
    (29, "billboards"),
    (30, "dlls"),
    (31, "shaders"),
    (32, "loadingsprites"),
    (33, "loadingscreen"),
    (34, "loadingspritesraw"),
    (35, "cutscenes"),
    (40, "audiostreams"),
    (41, "worldmapareas"),
    (42, "worldmaplabels"),
    (47, "modelsrt7"),
    (48, "animsrt7"),
    (49, "dbtableindex"),
    (52, "textures.dxt"),
    (53, "textures.png"),
    (54, "textures.png.mipped"),
    (55, "textures.etc"),
    (56, "anims.keyframes"),
];

// ---------------------------------------------------------------------------
// Minimal `keys.json` reader (no serde for one small file)
// ---------------------------------------------------------------------------

/// Subset DOM: objects, arrays, strings, integers, and skipped literals.
/// (`Null`/`Bool`/`Str` payloads only ride through to their parents; the key
/// table reads `archive`/`group`/`key` and ignores the rest.)
#[derive(Debug)]
#[allow(dead_code)]
enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

/// Byte cursor over the document.
struct JsonReader<'a> {
    text: &'a [u8],
    pos: usize,
}

impl<'a> JsonReader<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text: text.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn eof(&self) -> bool {
        self.pos >= self.text.len()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn fail(&self, what: &str) -> String {
        format!("keys.json: {what} at byte {}", self.pos)
    }

    fn expect_byte(&mut self, want: u8, what: &str) -> Result<(), String> {
        if self.peek() == Some(want) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.fail(&format!("expected {what}")))
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(JsonValue::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", JsonValue::Bool(true)),
            Some(b'f') => self.parse_literal("false", JsonValue::Bool(false)),
            Some(b'n') => self.parse_literal("null", JsonValue::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => {
                Ok(JsonValue::Int(self.parse_int()?))
            }
            Some(_) => Err(self.fail("unexpected byte")),
            None => Err(self.fail("unexpected end of file")),
        }
    }

    fn parse_literal(&mut self, word: &str, value: JsonValue) -> Result<JsonValue, String> {
        if self.text.get(self.pos..self.pos + word.len()) == Some(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.fail("bad literal"))
        }
    }

    /// JSON integer only: fractions/exponents are rejected (the schema is `i32`s).
    fn parse_int(&mut self) -> Result<i64, String> {
        let mut value: i64 = 0;
        let mut negative = false;
        if self.peek() == Some(b'-') {
            negative = true;
            self.pos += 1;
        }
        let mut digits = 0_u32;
        while let Some(byte) = self.peek() {
            if !byte.is_ascii_digit() {
                break;
            }
            value = value
                .checked_mul(10)
                .and_then(|scaled| {
                    if negative {
                        scaled.checked_sub(i64::from(byte - b'0'))
                    } else {
                        scaled.checked_add(i64::from(byte - b'0'))
                    }
                })
                .ok_or_else(|| self.fail("integer overflow"))?;
            digits += 1;
            self.pos += 1;
        }
        if digits == 0 {
            return Err(self.fail("expected digits"));
        }
        match self.peek() {
            Some(b'.' | b'e' | b'E') => Err(self.fail("non-integer number")),
            _ => Ok(value),
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"', "`\"`")?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.fail("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{0008}'),
                        Some(b'f') => out.push('\u{000C}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => out.push(self.parse_unicode_escape()?),
                        _ => return Err(self.fail("bad escape")),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    let rest = &self.text[self.pos..];
                    let end = rest
                        .iter()
                        .position(|byte| *byte == b'"' || *byte == b'\\')
                        .unwrap_or(rest.len());
                    let chunk = rest.get(..end).ok_or_else(|| self.fail("bad string"))?;
                    let chunk =
                        std::str::from_utf8(chunk).map_err(|_| self.fail("non-UTF8 string"))?;
                    // Raw control bytes are invalid JSON; surface them instead
                    // of passing mojibake through.
                    if chunk.chars().any(|c| c < '\u{20}') {
                        return Err(self.fail("unescaped control in string"));
                    }
                    out.push_str(chunk);
                    self.pos += end;
                }
            }
        }
    }

    /// `\uXXXX` escape (one unit only; names in the wild are ASCII like `"l40_55"`).
    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        // `self.pos` sits on the `u`; the caller advances past the value.
        let digits = self
            .text
            .get(self.pos + 1..self.pos + 5)
            .ok_or_else(|| self.fail("truncated unicode escape"))?;
        let mut unit: u32 = 0;
        for byte in digits {
            let hex = (*byte as char)
                .to_digit(16)
                .ok_or_else(|| self.fail("bad hex"))?;
            unit = unit * 16 + hex;
        }
        let parsed = char::from_u32(unit).ok_or_else(|| self.fail("bad unicode scalar"))?;
        self.pos += 4;
        Ok(parsed)
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect_byte(b'[', "`[`")?;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(JsonValue::Array(items));
            }
            if !items.is_empty() {
                self.expect_byte(b',', "`,`")?;
                self.skip_ws();
                if self.peek() == Some(b']') {
                    return Err(self.fail("trailing comma"));
                }
            }
            items.push(self.parse_value()?);
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect_byte(b'{', "`{`")?;
        let mut fields = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(JsonValue::Object(fields));
            }
            if !fields.is_empty() {
                self.expect_byte(b',', "`,`")?;
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    return Err(self.fail("trailing comma"));
                }
            }
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.fail("expected string key"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':', "`:`")?;
            let value = self.parse_value()?;
            fields.push((key, value));
        }
    }
}

/// Parse `keys.json` text: a top-level array of
/// `{archive: u32, group: u32, key: [i32 x4], ...}` (extra fields ignored).
/// Entry shape verified live against
/// `https://archive.openrs2.org/caches/runescape/901/keys.json`
/// (`{archive: 5, group: 1, name_hash, name: "l40_55", mapsquare: 10295,
/// key: [88121581, -1749097894, 1831922082, -1885905985], ...}` — note the
/// signed 32-bit key words, hence the `i32`-range check before the bit-preserving
/// `as u32`).
fn parse_keys_json(text: &str) -> Result<KeyMap, String> {
    let mut reader = JsonReader::new(text);
    let root = reader.parse_value()?;
    reader.skip_ws();
    if !reader.eof() {
        return Err(reader.fail("trailing bytes"));
    }
    let JsonValue::Array(entries) = root else {
        return Err("keys.json: top level must be an array".to_string());
    };
    let mut map = KeyMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let JsonValue::Object(fields) = entry else {
            return Err(format!("keys.json: entry {index} must be an object"));
        };
        let field = |name: &str| fields.iter().find(|(key, _)| key == name).map(|(_, v)| v);
        let archive = json_id(field("archive"), index, "archive")?;
        let group = json_id(field("group"), index, "group")?;
        let key = match field("key") {
            Some(JsonValue::Array(words)) if words.len() == 4 => {
                let mut key = [0_u32; 4];
                for (slot, word) in words.iter().enumerate() {
                    let JsonValue::Int(signed) = word else {
                        return Err(format!(
                            "keys.json: entry {index} key[{slot}] must be an integer"
                        ));
                    };
                    let word = i32::try_from(*signed).map_err(|_| {
                        format!("keys.json: entry {index} key[{slot}] outside i32 range")
                    })?;
                    key[slot] = word as u32;
                }
                key
            }
            _ => return Err(format!("keys.json: entry {index} needs key: [i32 x4]")),
        };
        map.insert((archive, group), key);
    }
    Ok(map)
}

/// `archive`/`group` member: present, integer, `u32` range.
fn json_id(value: Option<&JsonValue>, index: usize, name: &str) -> Result<u32, String> {
    match value {
        Some(JsonValue::Int(signed)) => u32::try_from(*signed)
            .map_err(|_| format!("keys.json: entry {index} {name} outside u32 range")),
        _ => Err(format!("keys.json: entry {index} needs integer {name}")),
    }
}

/// Failures this layer can produce. Library code returns `Result<_, CacheError>`;
/// only a CLI boundary converts to `anyhow`.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Filesystem IO.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Decompression / index / group-split failure from `native910`.
    #[error(transparent)]
    Native(#[from] NativeError),
    /// The `.js5` bytes are not shaped like `Js5.ts` expects.
    #[error("corrupt pack file: {0}")]
    Corrupt(String),
    /// Group id is not listed in the archive index.
    #[error("group {group} not listed in archive {archive:?}")]
    UnknownGroup {
        /// Archive name, e.g. `"mapsv2"`.
        archive: String,
        /// Requested group id.
        group: u32,
    },
    /// Group slot is listed but has no bytes on disk (trailer length `< 5`,
    /// same rule as `Js5.ts:225`).
    #[error("group {group} has no data in archive {archive:?}")]
    GroupMissing {
        /// Archive name, e.g. `"mapsv2"`.
        archive: String,
        /// Requested group id.
        group: u32,
    },
}

/// A pack root: the directory holding `client.<name>.js5` files.
///
/// Archive indexes and the XTEA key map are decoded once per handle and
/// shared between clones: a rebuild reads thousands of model groups and
/// re-decoding the master container per read dominated the build time.
/// The client opens one handle per pack root at startup and passes clones
/// to every consumer (frame, tick and worker paths); `Pack::open` there
/// would start a fresh, empty index cache.
///
/// With a [`DiskOverlay`] (the client's writable JS5 disk store, see
/// [`install_disk_overlay`]) the pack is the pre-populated half of each
/// disk store and the overlay holds what the resource provider wrote
/// after a network fetch:
/// - the effective archive index is the overlay's `255/<archive>` copy when
///   the provider replaced the pack's index, else the pack's own;
/// - a group reads from the overlay first (the replacing write), else from
///   the pack while the pack's copy matches the effective index checksum
///   (the group-checksum check applied to a disk copy);
/// - a group absent from both is queued for the client's JS5 owner
///   ([`DiskOverlay::take_missing`]) and reads as [`CacheError::GroupMissing`],
///   the original's absent-file result until the urgent request completes.
#[derive(Clone)]
pub struct Pack {
    root: PathBuf,
    overlay: Option<Arc<DiskOverlay>>,
    indexes: Arc<Mutex<HashMap<String, Arc<ArchiveEntry>>>>,
    keys: Arc<Mutex<Option<Arc<KeyMap>>>>,
}

impl std::fmt::Debug for Pack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pack")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// One archive's decoded state: the effective index plus where the pack
/// file keeps its groups.
struct ArchiveEntry {
    /// Overlay `255/<archive>` when present, else the pack's own index.
    index: Arc<ArchiveIndex>,
    /// `index` came from the overlay.
    overlay_index: bool,
    /// The overlay's master-index write generation when `index` was decoded.
    generation: u64,
    pack: Option<PackLayout>,
}

/// The pack file's own index and group byte ranges (`Js5.ts:214-230`).
struct PackLayout {
    index: Arc<ArchiveIndex>,
    /// `(offset, stored length)` by group id; length `< 5` = absent.
    ranges: Vec<(u64, u64)>,
}

/// Queued overlay writes by `(archive, group)`, not yet on disk.
type PendingWrites = HashMap<(u32, u32), Arc<Vec<u8>>>;

/// The writable half of the disk stores (the cache data and index files):
/// `<dir>/<archive>/<group>.dat` holds a group container plus its 2-byte
/// version trailer exactly as the resource provider writes it;
/// `<dir>/255/<archive>.dat` holds an archive index container. Writes queued
/// on the disk-cache thread are visible to reads before they reach the file,
/// as the synchronous read scans the queue.
pub struct DiskOverlay {
    dir: PathBuf,
    pending: Mutex<PendingWrites>,
    /// Groups a `Pack` read found in neither store, for the JS5 owner.
    missing: Mutex<Vec<(u32, u32)>>,
    /// Bumped by every master-index (`255/<archive>`) write, queued or
    /// flushed: a shared `Pack` re-decodes an archive index cached before
    /// the write, as the archive re-reads the index its provider replaced.
    master_generation: AtomicU64,
}

impl std::fmt::Debug for DiskOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskOverlay")
            .field("dir", &self.dir)
            .finish()
    }
}

impl DiskOverlay {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.into(),
            pending: Mutex::new(HashMap::new()),
            missing: Mutex::new(Vec::new()),
            master_generation: AtomicU64::new(0),
        })
    }

    /// The directory the store keeps its files in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// File of one stored entry (`store` 255 = the master-index store).
    #[must_use]
    pub fn path(&self, store: u32, key: u32) -> PathBuf {
        self.dir.join(store.to_string()).join(format!("{key}.dat"))
    }

    /// `DiskStore.read`: the queued write, else the file.
    #[must_use]
    pub fn read(&self, store: u32, key: u32) -> Option<Vec<u8>> {
        if let Some(bytes) = self
            .pending
            .lock()
            .expect("overlay pending")
            .get(&(store, key))
        {
            return Some(bytes.as_ref().clone());
        }
        std::fs::read(self.path(store, key)).ok()
    }

    /// True when an entry exists (queued or on disk).
    #[must_use]
    pub fn has(&self, store: u32, key: u32) -> bool {
        self.pending
            .lock()
            .expect("overlay pending")
            .contains_key(&(store, key))
            || self.path(store, key).is_file()
    }

    /// A write was queued on the disk-cache thread.
    pub fn mark_pending(&self, store: u32, key: u32, bytes: Arc<Vec<u8>>) {
        self.pending
            .lock()
            .expect("overlay pending")
            .insert((store, key), bytes);
        self.note_write(store);
    }

    /// Current master-index write generation (see `master_generation`).
    fn master_generation(&self) -> u64 {
        self.master_generation.load(Ordering::Acquire)
    }

    fn note_write(&self, store: u32) {
        if store == ARCHIVE_SET {
            self.master_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The disk thread finished `bytes`; a newer queued write stays visible.
    pub fn clear_pending(&self, store: u32, key: u32, bytes: &Arc<Vec<u8>>) {
        let mut pending = self.pending.lock().expect("overlay pending");
        if pending
            .get(&(store, key))
            .is_some_and(|b| Arc::ptr_eq(b, bytes))
        {
            pending.remove(&(store, key));
        }
    }

    /// Groups a `Pack` read found in neither store since the last call:
    /// `(archive, group)` pairs for the client's JS5 owner to fetch urgently.
    pub fn take_missing(&self) -> Vec<(u32, u32)> {
        std::mem::take(&mut *self.missing.lock().expect("missing groups"))
    }

    fn note_missing(&self, archive: u32, group: u32) {
        let mut missing = self.missing.lock().expect("missing groups");
        if !missing.contains(&(archive, group)) {
            missing.push((archive, group));
        }
    }

    /// `DiskStore.write`: replace the entry (temp file + rename so a reader
    /// never sees a partial container).
    pub fn write(&self, store: u32, key: u32, bytes: &[u8]) -> std::io::Result<()> {
        let path = self.path(store, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.note_write(store);
        Ok(())
    }
}

static OVERLAY: OnceLock<Arc<DiskOverlay>> = OnceLock::new();

/// Install the process-wide disk overlay (a static too). Only the app installs one; without it `Pack` reads the pack alone.
pub fn install_disk_overlay(dir: impl Into<PathBuf>) -> Arc<DiskOverlay> {
    OVERLAY.get_or_init(|| DiskOverlay::new(dir)).clone()
}

/// The installed overlay, if any.
#[must_use]
pub fn disk_overlay() -> Option<Arc<DiskOverlay>> {
    OVERLAY.get().cloned()
}

/// What a disk store holds for one group, as the JS5 verify pass reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// The stored container plus its 2-byte version trailer.
    Bytes(Vec<u8>),
    /// A pack group: the pack format keeps no per-group trailer, so its
    /// index records the checksum, digest and version of the stored bytes
    /// (the pack builder derived them from those bytes). The verify pass
    /// compares these recorded values instead of rehashing the multi-GB
    /// pack at every start.
    Recorded {
        crc: i32,
        version: i32,
        digest: Option<[u8; 64]>,
    },
}

/// The disk store of one archive (`255` = the
/// master-index store): the pack's copy plus the overlay's replacements.
pub struct DiskStore {
    pub archive: u32,
    pack: Pack,
    overlay: Arc<DiskOverlay>,
}

impl std::fmt::Debug for DiskStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskStore")
            .field("archive", &self.archive)
            .finish_non_exhaustive()
    }
}

impl DiskStore {
    #[must_use]
    pub fn new(archive: u32, pack: Pack, overlay: Arc<DiskOverlay>) -> Self {
        Self {
            archive,
            pack,
            overlay,
        }
    }

    #[must_use]
    pub fn overlay(&self) -> &Arc<DiskOverlay> {
        &self.overlay
    }

    /// `DiskStore.read(key)`: the bytes as stored (index container for the
    /// master store, container + version trailer for a group store), or
    /// `None` when neither half holds the key.
    #[must_use]
    pub fn read(&self, key: u32) -> Option<Vec<u8>> {
        if let Some(bytes) = self.overlay.read(self.archive, key) {
            return Some(bytes);
        }
        if self.archive == ARCHIVE_SET {
            return self.pack.pack_master(archive_name_for_id(key)?);
        }
        match self
            .pack
            .pack_stored(archive_name_for_id(self.archive)?, key, true)?
        {
            Stored::Bytes(bytes) => Some(bytes),
            Stored::Recorded { .. } => None,
        }
    }

    /// The verify pass's disk read: overlay bytes, or a
    /// pack group's recorded checksum/version (see [`Stored::Recorded`]).
    #[must_use]
    pub fn read_recorded(&self, key: u32) -> Option<Stored> {
        if self.archive == ARCHIVE_SET {
            return self.read(key).map(Stored::Bytes);
        }
        if let Some(bytes) = self.overlay.read(self.archive, key) {
            return Some(Stored::Bytes(bytes));
        }
        self.pack
            .pack_stored(archive_name_for_id(self.archive)?, key, false)
    }

    /// `DiskStore.write(key, bytes)`.
    pub fn write(&self, key: u32, bytes: &[u8]) -> std::io::Result<()> {
        self.overlay.write(self.archive, key, bytes)
    }
}

/// The store/archive id of the master indexes.
pub const ARCHIVE_SET: u32 = 255;

/// Test hook (code-quality programme Phase 5): the archive indexes and
/// groups every `Pack` decodes from pack files while a log is on, so a
/// test can export exactly what a recorded session reads (the pack-free
/// replay overlay). Not compiled into the client.
#[cfg(any(test, feature = "test-hooks"))]
pub mod read_log {
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    /// `(archive name, group)`; `None` is the archive's index.
    pub type Reads = BTreeSet<(String, Option<u32>)>;

    static LOG: Mutex<Option<Reads>> = Mutex::new(None);

    /// Start (or restart) logging.
    pub fn start() {
        *LOG.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Reads::new());
    }

    /// Stop logging; what was read since [`start`].
    pub fn take() -> Reads {
        LOG.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_default()
    }

    pub(super) fn note(archive: &str, group: Option<u32>) {
        if let Some(log) = LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            log.insert((archive.to_string(), group));
        }
    }
}

// The app's one shared handle crosses into the prefetch and loading workers.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Pack>();
};

/// Process-wide cache-read counters, reported on the `CLIENT910_PROFILE`
/// `[perf]` line (frame_profile.rs). Relaxed increments on the slow paths only.
pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `Pack` handles created (`open_with_overlay`), i.e. fresh index caches.
    pub(super) static HANDLES: AtomicU64 = AtomicU64::new(0);
    /// Master-index containers read from a pack file or the overlay and
    /// decompressed + decoded (index-cache misses).
    pub(super) static INDEX_DECODES: AtomicU64 = AtomicU64::new(0);

    /// `(handles, index decodes)` since process start.
    #[must_use]
    pub fn snapshot() -> (u64, u64) {
        (
            HANDLES.load(Ordering::Relaxed),
            INDEX_DECODES.load(Ordering::Relaxed),
        )
    }

    pub(super) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Pack {
    /// Open a pack root (e.g. `<repo>/server/data/pack`) with the installed
    /// disk overlay, if any.
    #[must_use]
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self::open_with_overlay(root, disk_overlay())
    }

    /// Open a pack root over an explicit overlay (`None` = pack only).
    #[must_use]
    pub fn open_with_overlay(root: impl Into<PathBuf>, overlay: Option<Arc<DiskOverlay>>) -> Self {
        stats::bump(&stats::HANDLES);
        Self {
            root: root.into(),
            overlay,
            indexes: Arc::new(Mutex::new(HashMap::new())),
            keys: Arc::new(Mutex::new(None)),
        }
    }

    /// The pack file's own layout: master container length, decoded index
    /// and per-group byte ranges from the trailer.
    fn read_pack_layout(&self, archive_name: &str) -> Result<PackLayout, CacheError> {
        let path = self.archive_path(archive_name);
        let mut file = File::open(&path)?;
        let file_len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let master_len = read_container_len_prefix(&mut file, file_len, archive_name)?;
        file.seek(SeekFrom::Start(0))?;
        let mut master = vec![0_u8; master_len];
        file.read_exact(&mut master)?;
        stats::bump(&stats::INDEX_DECODES);
        let index = ArchiveIndex::decode(&decompress(&master)?)?;
        let ranges =
            pack_group_ranges(&mut file, file_len, master_len as u64, &index, archive_name)?;
        Ok(PackLayout {
            index: Arc::new(index),
            ranges,
        })
    }

    /// The decoded archive state, cached. An entry decoded from the pack is
    /// re-read once the overlay gains a replacement index.
    fn cached_entry(&self, archive_name: &str) -> Result<Arc<ArchiveEntry>, CacheError> {
        let overlay_key = self.overlay.as_ref().zip(archive_id_for_name(archive_name));
        // Read before decoding: a write racing the decode leaves the entry
        // stale for the next read instead of hiding the new index.
        let generation = self
            .overlay
            .as_ref()
            .map_or(0, |overlay| overlay.master_generation());
        if let Some(hit) = self.indexes.lock().expect("index cache").get(archive_name) {
            let stale = hit.generation != generation
                || (!hit.overlay_index
                    && overlay_key.is_some_and(|(overlay, id)| overlay.has(ARCHIVE_SET, id)));
            if !stale {
                return Ok(hit.clone());
            }
        }
        let pack = match self.read_pack_layout(archive_name) {
            Ok(layout) => Some(layout),
            Err(error) => {
                let absent = matches!(&error, CacheError::Io(io) if io.kind() == std::io::ErrorKind::NotFound);
                if !absent || overlay_key.is_none() {
                    return Err(error);
                }
                None
            }
        };
        let overlay_index = overlay_key
            .and_then(|(overlay, id)| overlay.read(ARCHIVE_SET, id))
            .and_then(|bytes| {
                stats::bump(&stats::INDEX_DECODES);
                decompress(&bytes)
                    .ok()
                    .and_then(|raw| ArchiveIndex::decode(&raw).ok())
            });
        let entry = match (overlay_index, pack) {
            (Some(index), pack) => ArchiveEntry {
                index: Arc::new(index),
                overlay_index: true,
                generation,
                pack,
            },
            (None, Some(pack)) => ArchiveEntry {
                index: pack.index.clone(),
                overlay_index: false,
                generation,
                pack: Some(pack),
            },
            (None, None) => {
                return Err(CacheError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("archive {archive_name:?}: no index in the pack or the disk store"),
                )))
            }
        };
        #[cfg(any(test, feature = "test-hooks"))]
        read_log::note(archive_name, None);
        let entry = Arc::new(entry);
        self.indexes
            .lock()
            .expect("index cache")
            .insert(archive_name.to_string(), entry.clone());
        Ok(entry)
    }

    /// Decode and retain one archive's index for every clone of this handle.
    #[allow(dead_code)]
    pub fn warm_index(&self, archive_name: &str) -> Result<(), CacheError> {
        self.cached_entry(archive_name).map(|_| ())
    }

    /// The `keys.json` map, loaded once.
    fn cached_keys(&self) -> Result<Arc<KeyMap>, CacheError> {
        if let Some(hit) = self.keys.lock().expect("key cache").as_ref() {
            return Ok(hit.clone());
        }
        let keys = Arc::new(load_keys(&self.root)?);
        *self.keys.lock().expect("key cache") = Some(keys.clone());
        Ok(keys)
    }

    /// Pack root this handle reads from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// On-disk path of one archive, `client.<name>.js5`.
    #[must_use]
    pub fn archive_path(&self, archive_name: &str) -> PathBuf {
        self.root.join(format!("client.{archive_name}.js5"))
    }

    /// Decode the archive index (master container) of one archive: the
    /// overlay's replacement when present, else the pack's (`Js5.ts:29-47`).
    pub fn read_archive_index(&self, archive_name: &str) -> Result<ArchiveIndex, CacheError> {
        Ok(self.cached_entry(archive_name)?.index.as_ref().clone())
    }

    /// The group whose index name hash equals the name hash of the
    /// lower-cased name (`h = (h << 5) - h + cp1252(c)` over its characters),
    /// or `None` when the archive carries no names or none matches.
    pub fn group_id_by_name(
        &self,
        archive_name: &str,
        name: &str,
    ) -> Result<Option<u32>, CacheError> {
        let mut hash = 0_i32;
        for c in name.to_lowercase().chars() {
            let byte = u32::from(c);
            if byte >= 0x80 {
                return Err(CacheError::Native(NativeError::Invalid(format!(
                    "non-ASCII group name {name:?}"
                ))));
            }
            hash = (hash << 5).wrapping_sub(hash).wrapping_add(byte as i32);
        }
        stats::bump(&stats::INDEX_DECODES);
        let raw = decompress(&self.read_raw_master(archive_name)?)?;
        Ok(group_name_hashes(&raw)?
            .into_iter()
            .find(|(h, _)| *h == hash)
            .map(|(_, id)| id))
    }

    /// Read one group container and split it into `file_id -> bytes`.
    ///
    /// Only the master container, the trailer, and this group's bytes are
    /// read; the rest of the (possibly multi-GB) file is never touched.
    ///
    /// Locked groups are decrypted transparently: the key is looked up from
    /// `<pack_root>/keys.json` ([`load_keys`]; missing file = open) and applied
    /// via [`tinydec`] before `unpack_group`, mirroring `Js5.ts:514-521`.
    /// A corrupt `keys.json` fails here (see [`load_keys`]); a missing key
    /// leaves a locked group undecryptable, surfacing as a decompress error.
    pub fn read_group(
        &self,
        archive_name: &str,
        group_id: u32,
    ) -> Result<BTreeMap<u32, Vec<u8>>, CacheError> {
        let keys = self.cached_keys()?;
        let key = archive_id_for_name(archive_name)
            .and_then(|archive| lookup_key(&keys, archive, group_id));
        self.read_group_with_key(archive_name, group_id, key)
    }

    /// Read one group with an explicit XTEA key (`None` = open container).
    ///
    /// Same disk reads as [`Pack::read_group`], but the key is threaded
    /// straight through instead of looked up from `keys.json` — the seam for
    /// callers (and tests) that already hold keys. Decryption runs BEFORE
    /// `unpack_group` (`Js5.ts:517-521` order); an all-zero key behaves as
    /// `None` (`Js5.ts:514`).
    pub fn read_group_with_key(
        &self,
        archive_name: &str,
        group_id: u32,
        key: Option<XteaKey>,
    ) -> Result<BTreeMap<u32, Vec<u8>>, CacheError> {
        let key = key.filter(|key| !key_is_open(key));
        let (entry, container) = self.read_raw_group_container(archive_name, group_id)?;
        let plain = decrypt_container(&container, key);
        Ok(unpack_group(&entry.index, group_id, &plain)?)
    }

    /// Lane Q-FAR1 (NXT far scene F0, `nxt-render-distance.md` §6.2): read
    /// one group like [`Pack::read_group`], but quietly. A group absent from
    /// both stores (or not in the archive's index) is `Ok(None)`: it is never
    /// queued for the client's JS5 owner ([`DiskOverlay::take_missing`] does
    /// not see it) and never noted in the test read log. Render-only readers
    /// (the NXT backend's far scene) use it, so their reads cannot change
    /// what the client requests. The archive index itself is the shared
    /// cached one (the same entry the first read decodes).
    pub fn read_group_resident(
        &self,
        archive_name: &str,
        group_id: u32,
    ) -> Result<Option<BTreeMap<u32, Vec<u8>>>, CacheError> {
        let keys = self.cached_keys()?;
        let key = archive_id_for_name(archive_name)
            .and_then(|archive| lookup_key(&keys, archive, group_id))
            .filter(|key| !key_is_open(key));
        let entry = self.cached_entry(archive_name)?;
        let Some(container) = self.resident_group_container(&entry, archive_name, group_id)? else {
            return Ok(None);
        };
        let plain = decrypt_container(&container, key);
        Ok(Some(unpack_group(&entry.index, group_id, &plain)?))
    }

    /// `read_raw_group_container`'s lookup without its side effects (no
    /// missing-group queue, no read log): the overlay's copy, else the
    /// pack's while it matches the effective index, else `None`.
    fn resident_group_container(
        &self,
        entry: &ArchiveEntry,
        archive_name: &str,
        group_id: u32,
    ) -> Result<Option<Vec<u8>>, CacheError> {
        if entry.index.group_id.binary_search(&group_id).is_err() {
            return Ok(None);
        }
        if let (Some(overlay), Some(id)) = (&self.overlay, archive_id_for_name(archive_name)) {
            if let Some(stored) = overlay.read(id, group_id) {
                if stored.len() >= 5 {
                    let header = [stored[0], stored[1], stored[2], stored[3], stored[4]];
                    let total =
                        container_total_len(&header, stored.len() as u64, archive_name, group_id)?;
                    return Ok(Some(stored[..total].to_vec()));
                }
            }
        }
        let Some(layout) = &entry.pack else {
            return Ok(None);
        };
        let g = group_id as usize;
        let current = !entry.overlay_index
            || (layout.index.group_id.binary_search(&group_id).is_ok()
                && layout.index.group_checksums.get(g) == entry.index.group_checksums.get(g));
        match layout.ranges.get(g) {
            Some(&(offset, stored)) if current && stored >= 5 => Ok(Some(
                self.read_pack_container(archive_name, group_id, offset, stored)?,
            )),
            _ => Ok(None),
        }
    }

    /// The raw master-index container bytes (compressed, exactly what a
    /// the provider's index fetch feeds the archive index): the overlay's
    /// replacement when present, else the pack's.
    pub fn read_raw_master(&self, archive_name: &str) -> Result<Vec<u8>, CacheError> {
        if let (Some(overlay), Some(id)) = (&self.overlay, archive_id_for_name(archive_name)) {
            if let Some(bytes) = overlay.read(ARCHIVE_SET, id) {
                return Ok(bytes);
            }
        }
        let path = self.archive_path(archive_name);
        let mut file = File::open(&path)?;
        let file_len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let master_len = read_container_len_prefix(&mut file, file_len, archive_name)?;
        let mut master = vec![0_u8; master_len];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut master)?;
        Ok(master)
    }

    /// The pack file's own master container (no overlay): `DiskStore.read`
    /// of the master store's pack half.
    fn pack_master(&self, archive_name: &str) -> Option<Vec<u8>> {
        let path = self.archive_path(archive_name);
        let mut file = File::open(&path).ok()?;
        let file_len = file.seek(SeekFrom::End(0)).ok()?;
        file.seek(SeekFrom::Start(0)).ok()?;
        let master_len = read_container_len_prefix(&mut file, file_len, archive_name).ok()?;
        let mut master = vec![0_u8; master_len];
        file.seek(SeekFrom::Start(0)).ok()?;
        file.read_exact(&mut master).ok()?;
        Some(master)
    }

    /// The pack half of a group `DiskStore`: with `bytes` the container plus
    /// the 2-byte version trailer the provider stores, else the recorded checksum/version ([`Stored::Recorded`]).
    fn pack_stored(&self, archive_name: &str, group_id: u32, bytes: bool) -> Option<Stored> {
        let layout_owner = self.pack_layout_entry(archive_name)?;
        let layout = layout_owner.pack.as_ref()?;
        let g = group_id as usize;
        let &(offset, stored) = layout.ranges.get(g)?;
        if stored < 5 {
            return None;
        }
        let index = &layout.index;
        let version = index.group_versions.get(g).copied().unwrap_or(0);
        if !bytes {
            return Some(Stored::Recorded {
                crc: index.group_checksums.get(g).copied().unwrap_or(0),
                version,
                digest: index
                    .group_digests
                    .as_ref()
                    .and_then(|d| d.get(g).copied().flatten()),
            });
        }
        let mut container = self
            .read_pack_container(archive_name, group_id, offset, stored)
            .ok()?;
        container.push((version >> 8) as u8);
        container.push(version as u8);
        Some(Stored::Bytes(container))
    }

    /// The archive entry when the pack file exists (pack-only reads).
    fn pack_layout_entry(&self, archive_name: &str) -> Option<Arc<ArchiveEntry>> {
        match self.cached_entry(archive_name) {
            Ok(entry) if entry.pack.is_some() => Some(entry),
            _ => None,
        }
    }

    /// One container from the pack file at its trailer range.
    fn read_pack_container(
        &self,
        archive_name: &str,
        group_id: u32,
        offset: u64,
        stored: u64,
    ) -> Result<Vec<u8>, CacheError> {
        let mut file = File::open(self.archive_path(archive_name))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 5];
        file.read_exact(&mut header)?;
        let declared_total = container_total_len(&header, stored, archive_name, group_id)?;
        let mut container = vec![0_u8; declared_total];
        container[..5].copy_from_slice(&header);
        file.read_exact(&mut container[5..])?;
        Ok(container)
    }

    /// One raw group container (compressed, undecrypted, no split): what
    /// the resource provider's group fetch returns to the archive.
    pub fn read_raw_group(&self, archive_name: &str, group_id: u32) -> Result<Vec<u8>, CacheError> {
        let (_, container) = self.read_raw_group_container(archive_name, group_id)?;
        Ok(container)
    }

    /// Read one raw group container plus its archive state (no decrypt, no
    /// split): the overlay's copy first, else the pack's while it matches the
    /// effective index; an absent group is queued for the JS5 owner.
    fn read_raw_group_container(
        &self,
        archive_name: &str,
        group_id: u32,
    ) -> Result<(Arc<ArchiveEntry>, Vec<u8>), CacheError> {
        let entry = self.cached_entry(archive_name)?;
        if entry.index.group_id.binary_search(&group_id).is_err() {
            return Err(CacheError::UnknownGroup {
                archive: archive_name.to_string(),
                group: group_id,
            });
        }
        let archive_id = archive_id_for_name(archive_name);
        if let (Some(overlay), Some(id)) = (&self.overlay, archive_id) {
            if let Some(stored) = overlay.read(id, group_id) {
                if stored.len() >= 5 {
                    let header = [stored[0], stored[1], stored[2], stored[3], stored[4]];
                    let total =
                        container_total_len(&header, stored.len() as u64, archive_name, group_id)?;
                    return Ok((entry, stored[..total].to_vec()));
                }
            }
        }
        if let Some(layout) = &entry.pack {
            let g = group_id as usize;
            let current = !entry.overlay_index
                || (layout.index.group_id.binary_search(&group_id).is_ok()
                    && layout.index.group_checksums.get(g) == entry.index.group_checksums.get(g));
            if let Some(&(offset, stored)) = layout.ranges.get(g) {
                if current && stored >= 5 {
                    let container =
                        self.read_pack_container(archive_name, group_id, offset, stored)?;
                    #[cfg(any(test, feature = "test-hooks"))]
                    read_log::note(archive_name, Some(group_id));
                    return Ok((entry, container));
                }
            }
        }
        if let (Some(overlay), Some(id)) = (&self.overlay, archive_id) {
            overlay.note_missing(id, group_id);
        }
        Err(CacheError::GroupMissing {
            archive: archive_name.to_string(),
            group: group_id,
        })
    }
}

/// Split an unpacked map group into its landscape + loc streams.
///
/// Returns `(land file 3, loc file 0)`; either side is `None` when the group
/// does not carry it (e.g. some groups omit the NPC file 2 the same way).
#[must_use]
pub fn map_group(files: &BTreeMap<u32, Vec<u8>>) -> (Option<&[u8]>, Option<&[u8]>) {
    let land = files.get(&LAND_FILE).map(Vec::as_slice);
    let loc = files.get(&LOC_FILE).map(Vec::as_slice);
    (land, loc)
}

/// Total container bytes from a 5-byte header: `5 + declared (+ 4 when
/// compressed)`, the same arithmetic as `Js5.ts:389-397` `readRaw`.
/// `stored_len` is the trailer length for this group; the header must fit it.
fn container_total_len(
    header: &[u8; 5],
    stored_len: u64,
    archive_name: &str,
    group_id: u32,
) -> Result<usize, CacheError> {
    let compression = header[0];
    let declared = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if declared < 0 {
        return Err(CacheError::Corrupt(format!(
            "archive {archive_name:?} group {group_id}: negative container length {declared}"
        )));
    }
    let mut total = 5_u64
        .checked_add(declared as u64)
        .ok_or_else(|| CacheError::Corrupt("container length overflow".to_string()))?;
    if compression > 0 {
        total = total
            .checked_add(4)
            .ok_or_else(|| CacheError::Corrupt("container length overflow".to_string()))?;
    }
    if total > stored_len {
        return Err(CacheError::Corrupt(format!(
            "archive {archive_name:?} group {group_id}: header declares {total} bytes but trailer stores {stored_len}"
        )));
    }
    usize::try_from(total)
        .map_err(|_| CacheError::Corrupt("container length does not fit memory".to_string()))
}

/// Read a 5-byte container header at the cursor and return its total byte
/// length, validated against the file size. Mirrors `Js5.ts:34-40`.
fn read_container_len_prefix(
    file: &mut File,
    file_len: u64,
    archive_name: &str,
) -> Result<usize, CacheError> {
    let mut header = [0_u8; 5];
    file.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            CacheError::Corrupt(format!(
                "archive {archive_name:?}: file shorter than one container header"
            ))
        } else {
            CacheError::Io(error)
        }
    })?;
    // The master container has no trailer entry to check against, so the
    // file length itself is the bound.
    let total = container_total_len(&header, file_len, archive_name, u32::MAX)?;
    if total as u64 > file_len {
        return Err(CacheError::Corrupt(format!(
            "archive {archive_name:?}: master container declares {total} bytes in a {file_len}-byte file"
        )));
    }
    Ok(total)
}

/// Byte ranges of every group, by group id: `(offset, stored length)`.
/// Mirrors the `Js5.ts:214-230` constructor: trailer order follows
/// archive-index order and each group's offset accumulates the stored
/// lengths before it. A range overrunning the trailer reads as absent.
fn pack_group_ranges(
    file: &mut File,
    file_len: u64,
    master_len: u64,
    index: &ArchiveIndex,
    archive_name: &str,
) -> Result<Vec<(u64, u64)>, CacheError> {
    let count = index.group_id.len();
    let trailer_len = (count as u64)
        .checked_mul(4)
        .ok_or_else(|| CacheError::Corrupt("trailer size overflow".to_string()))?;
    let trailer_start = file_len
        .checked_sub(trailer_len)
        .filter(|start| *start >= master_len)
        .ok_or_else(|| {
            CacheError::Corrupt(format!(
                "archive {archive_name:?}: file of {file_len} bytes cannot hold master ({master_len}) + trailer ({trailer_len})"
            ))
        })?;
    file.seek(SeekFrom::Start(trailer_start))?;
    let mut trailer = vec![
        0_u8;
        usize::try_from(trailer_len).map_err(|_| {
            CacheError::Corrupt("trailer does not fit memory".to_string())
        })?
    ];
    file.read_exact(&mut trailer)?;
    let mut ranges = vec![(0_u64, 0_u64); index.capacity];
    let mut offset = master_len;
    for (chunk, &group) in trailer.chunks_exact(4).zip(&index.group_id) {
        let length = i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if length < 0 {
            return Err(CacheError::Corrupt(format!(
                "archive {archive_name:?}: negative trailer length {length}"
            )));
        }
        let length = length as u64;
        let end = offset.checked_add(length).ok_or_else(|| {
            CacheError::Corrupt(format!(
                "archive {archive_name:?}: group offset overflow at group {group}"
            ))
        })?;
        if end <= trailer_start {
            ranges[group as usize] = (offset, length);
        }
        offset = end;
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn container_header_sizes_match_js5_readraw() {
        // Uncompressed: 5 + len. Compressed: 5 + len + 4.
        let total = container_total_len(&[0, 0, 0, 0, 10], 100, "test", 1).unwrap();
        assert_eq!(total, 15);
        let total = container_total_len(&[2, 0, 0, 0, 10], 100, "test", 1).unwrap();
        assert_eq!(total, 19);
        assert!(container_total_len(&[2, 0, 0, 0, 10], 18, "test", 1).is_err());
        assert!(container_total_len(&[0, 0xFF, 0xFF, 0xFF, 0xFF], 100, "test", 1).is_err());
    }

    fn unique_temp_root(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "client910-cache-{name}-{}-{stamp}",
            std::process::id()
        ))
    }

    #[test]
    fn tinydec_inverts_tinyenc_with_packet_ts_edges() {
        // Verification method (no node vectors): `tinyenc` mirrors
        // `Packet.ts:557-580`, `tinydec` mirrors `:582-612`. Round-tripping
        // both directions over multi-block input proves the mirror inverts;
        // the header/tail assertions pin the `Js5.ts:519`
        // `tinydec(key, 5, buf.length)` edges (5-byte header untouched,
        // trailing partial block untouched).
        let key: XteaKey = [0x1234_5678, 0x9ABC_DEF0, 0x0F1E_2D3C, 0x4B5A_6978];
        // 5-byte header + 27 payload bytes = 3 whole blocks + 3 tail bytes.
        let mut original = vec![2_u8, 0, 0, 0, 24];
        original.extend(1_u8..=27);
        assert_eq!(original.len(), 32);

        let mut encrypted = original.clone();
        tinyenc(&mut encrypted, key);
        // A non-degenerate key actually mixes (and the zero key does too:
        // XTEA with a zero key is NOT the identity — "open" is purely a
        // lookup-layer concept, `Js5.ts:514`).
        assert_ne!(encrypted, original);
        assert_eq!(
            &encrypted[..5],
            &original[..5],
            "container header untouched"
        );
        assert_eq!(
            &encrypted[29..],
            &original[29..],
            "partial tail block untouched"
        );

        let mut decrypted = encrypted.clone();
        tinydec(&mut decrypted, key);
        assert_eq!(decrypted, original);

        // The other direction is the identity too.
        let mut re_encrypted = decrypted.clone();
        tinyenc(&mut re_encrypted, key);
        assert_eq!(re_encrypted, encrypted);

        // A wrong key does not restore (locked groups stay opaque without keys).
        let mut wrong = encrypted.clone();
        tinydec(&mut wrong, [1, 2, 3, 4]);
        assert_ne!(wrong, original);

        // Short containers are a no-op (matches the TS negative-count loop skip).
        let mut tiny = vec![0_u8; 5];
        tinydec(&mut tiny, key);
        assert_eq!(tiny, vec![0_u8; 5]);
    }

    #[test]
    fn keys_json_fixture_parses_signed_words() {
        // Entry shape mirrors the live `.../caches/runescape/901/keys.json`
        // sample (`{archive, group, name_hash, name, mapsquare, key: [i32 x4], ...}`).
        let root = unique_temp_root("keys");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(KEYS_FILENAME),
            r#"[{"archive":5,"group":1,"name_hash":-1153472937,"name":"l40_55","mapsquare":10295,
                  "key":[88121581,-1749097894,1831922082,-1885905985]},
                 {"archive":5,"group":6450,"key":[0,0,0,0]}]"#,
        )
        .unwrap();
        let keys = load_keys(&root).unwrap();
        assert_eq!(keys.len(), 2);
        // Signed words map bit-preserving (`-1749097894_i32 as u32`).
        assert_eq!(
            keys.get(&(5, 1)),
            Some(&[
                88_121_581_u32,
                (-1_749_097_894_i32) as u32,
                1_831_922_082,
                (-1_885_905_985_i32) as u32
            ])
        );
        // All-zero entries parse but look up as open (`Js5.ts:514`).
        assert_eq!(lookup_key(&keys, 5, 6450), None);
        assert_eq!(lookup_key(&keys, 5, 1), keys.get(&(5, 1)).copied());
        assert_eq!(lookup_key(&keys, 5, 9999), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keys_json_missing_is_empty_and_corrupt_is_an_error() {
        let root = unique_temp_root("keys-missing");
        // No file written: open regions stream keyless.
        assert!(load_keys(&root).unwrap().is_empty());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(KEYS_FILENAME), b"{\"archive\":5").unwrap();
        assert!(load_keys(&root).is_err());
        std::fs::write(root.join(KEYS_FILENAME), b"[{\"archive\":5,\"group\":1}]").unwrap();
        assert!(load_keys(&root).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Minimal protocol-5 archive index naming group 7 with files {0, 1}
    /// (dense ids, no sparse table): protocol u8, flags u8, group count g2,
    /// group delta g2, checksum g4, version g4, group size g2, file deltas g2 x2.
    fn locked_test_index() -> ArchiveIndex {
        ArchiveIndex::decode(&[
            0x05, 0x00, 0x00, 0x01, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02, 0x00, 0x00, 0x00, 0x01,
        ])
        .unwrap()
    }

    /// Single-chunk striped payload for files {16 bytes, 16 bytes}: body, then
    /// the footer (`[16, 32]` cumulative BE), then the chunk-count marker `1`
    /// (mirrors `native910::js5::unpack_group`'s footer walk).
    fn locked_test_payload() -> (Vec<u8>, BTreeMap<u32, Vec<u8>>) {
        let file0: Vec<u8> = (0xA0_u8..).take(16).collect();
        let file1: Vec<u8> = (0x10_u8..).take(16).collect();
        let mut payload = Vec::new();
        payload.extend_from_slice(&file0);
        payload.extend_from_slice(&file1);
        payload.extend_from_slice(&16_i32.to_be_bytes());
        payload.extend_from_slice(&0_i32.to_be_bytes());
        payload.push(1);
        let mut files = BTreeMap::new();
        files.insert(0, file0);
        files.insert(1, file1);
        (payload, files)
    }

    #[test]
    fn locked_group_errors_before_keys_and_opens_after() {
        // Synthetic locked group: compression-0 container over the striped
        // payload, XTEA-encrypted from byte 5 with a known key (as the server
        // stores locked groups).
        const KEY: XteaKey = [0xA11C_E001, 0x5EED_1234, 0xDEAD_BEEF, 0x0BAD_F00D];
        let index = locked_test_index();
        let (payload, expected) = locked_test_payload();
        let mut container = vec![0_u8, 0, 0, 0, payload.len() as u8];
        container.extend_from_slice(&payload);
        tinyenc(&mut container, KEY);

        // BEFORE keys: no entry (or no file at all) -> passthrough -> the
        // striped footer is garbage -> unpack fails, the way locked groups
        // surface as decompress errors on a keyless client.
        let empty = BTreeMap::new();
        assert_eq!(lookup_key(&empty, 5, 7), None);
        let plain = decrypt_container(&container, None);
        assert!(unpack_group(&index, 7, &plain).is_err());

        // AFTER keys: keys.json carries (5, 7) -> decrypt -> exact files.
        let root = unique_temp_root("keys-locked");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(KEYS_FILENAME),
            format!(
                r#"[{{"archive":5,"group":7,"name":"synthetic","mapsquare":7,"key":[{},{},{},{}]}}]"#,
                KEY[0] as i32,
                KEY[1] as i32,
                KEY[2] as i32,
                KEY[3] as i32
            ),
        )
        .unwrap();
        let keys = load_keys(&root).unwrap();
        assert_eq!(lookup_key(&keys, 5, 7), Some(KEY));
        let plain = decrypt_container(&container, lookup_key(&keys, 5, 7));
        assert_eq!(unpack_group(&index, 7, &plain).unwrap(), expected);
        std::fs::remove_dir_all(&root).ok();
    }
}
