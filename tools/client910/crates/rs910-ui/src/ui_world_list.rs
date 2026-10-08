//! The lobby world list: chunked list updates from the game server, host
//! resolution and the script queries over the list.
use crate::ui_text_compare::{self, Language};
use anyhow::{bail, ensure, Result};
use rs910_core::reader::{Eof, Reader as CoreReader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    pub id: i32,
    pub flags: i32,
    pub country: i32,
    pub country_name: String,
    pub activity: String,
    pub hostname: String,
    pub players: i32,
    /// The packed host address: -1 until the host lookup resolves it.
    pub hostpacked: i32,
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    /// One `rs910_core::reader` read of `n` bytes at `pos`, all or nothing
    /// (the byte arithmetic lives there).
    fn read<T>(
        &mut self,
        n: usize,
        read: impl FnOnce(&mut CoreReader<'a>) -> std::result::Result<T, Eof>,
    ) -> Result<T> {
        let mut r = CoreReader::at(self.bytes, self.pos);
        match r.atomic(read) {
            Ok(value) => {
                self.pos = r.pos();
                Ok(value)
            }
            Err(_) if self.pos.checked_add(n).is_none() => bail!("world list size overflow"),
            Err(_) => bail!("truncated world list"),
        }
    }
    fn u8(&mut self) -> Result<u8> {
        self.read(1, CoreReader::g1)
    }
    fn u16(&mut self) -> Result<u16> {
        self.read(2, CoreReader::g2)
    }
    fn i32(&mut self) -> Result<i32> {
        self.read(4, CoreReader::g4s)
    }
    /// `gSmart1or2`: an empty stream fails on the peek.
    fn smart(&mut self) -> Result<usize> {
        ensure!(self.pos < self.bytes.len(), "truncated world list smart");
        self.read(2, CoreReader::gsmart1or2).map(|v| v as usize)
    }
    /// `gjstr2`: a zero version byte, then a `gjstr`.
    fn string(&mut self) -> Result<String> {
        ensure!(self.u8()? == 0, "world list gjstr2 prefix");
        let mut r = CoreReader::at(self.bytes, self.pos);
        let bytes = r.gjstr_bytes();
        self.pos = r.pos();
        let bytes = bytes.map_err(|_| anyhow::anyhow!("truncated world list"))?;
        Ok(bytes
            .iter()
            .map(|&b| rs910_core::cp1252::cp1252_decode_byte(b))
            .collect())
    }
}
#[derive(Clone, Debug)]
pub struct State {
    pub token: i32,
    pub fetching: bool,
    pub last_fetch_ms: i64,
    buffer: Vec<u8>,
    worlds: Vec<Option<World>>,
    min_id: i32,
    count: usize,
    order: Vec<usize>,
    cursor: usize,
    /// Whether hosts are resolved, and the scan position of that resolution.
    pub resolve_hosts_enabled: bool,
    resolve_cursor: usize,
    resolve_pending: Option<i32>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            token: 0,
            fetching: false,
            last_fetch_ms: 0,
            buffer: vec![],
            worlds: vec![],
            min_id: 0,
            count: 0,
            order: vec![],
            cursor: 1001127,
            resolve_hosts_enabled: false,
            resolve_cursor: 0,
            resolve_pending: None,
        }
    }
}
impl State {
    /// `worldlist_fetch`: the active game connection is state 18. Returns
    /// its CS2 result and queues the request only when the rate limit and
    /// pending-request rules permit it.
    pub fn request(&mut self, now: i64, eligible: bool, outgoing: &mut Vec<u8>) -> i32 {
        if !eligible {
            return 1;
        }
        if self.fetching {
            return 0;
        }
        if self.last_fetch_ms > now - 1000 {
            return 1;
        }
        self.fetching = true;
        outgoing.extend(crate::framing::encode_worldlist_fetch(self.token));
        0
    }
    /// Client WORLDLIST_FETCH_REPLY strips the final-chunk byte before GWC.
    /// Parse transactionally so a malformed update cannot damage the last list.
    pub fn apply_reply(&mut self, payload: &[u8], now: i64) -> Result<()> {
        let (&last, chunk) = payload
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty world list chunk"))?;
        ensure!(
            self.buffer.len() + chunk.len() <= 20000,
            "world list exceeds the 20000-byte buffer"
        );
        self.buffer.extend_from_slice(chunk);
        if last != 1 {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.buffer);
        let mut next = self.clone();
        next.decode(&bytes)?;
        next.order = next
            .worlds
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.as_ref().map(|_| i))
            .collect();
        // The list is replaced but the iteration cursor is not reset.
        next.fetching = false;
        next.last_fetch_ms = now;
        next.resolve_pending = None;
        *self = next;
        Ok(())
    }
    fn decode(&mut self, bytes: &[u8]) -> Result<()> {
        let mut r = Reader { bytes, pos: 0 };
        if r.u8()? != 2 {
            return Ok(());
        } // Any other list version is rejected without changing the old list.
        if r.u8()? == 1 {
            let count = r.smart()?;
            let mut locations = Vec::with_capacity(count);
            for _ in 0..count {
                locations.push((r.smart()? as i32, r.string()?));
            }
            let min = r.smart()? as i32;
            let max = r.smart()? as i32;
            let count = r.smart()?;
            ensure!(max >= min, "world list id range");
            let mut worlds = vec![None; (max - min + 1) as usize];
            for _ in 0..count {
                let index = r.smart()?;
                let location = r.u8()? as usize;
                let flags = r.i32()?;
                let country_override = r.smart()? as i32;
                let (country, country_name) = if country_override != 0 {
                    (country_override, r.string()?)
                } else {
                    locations
                        .get(location)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("world list location {location}"))?
                };
                let activity = r.string()?;
                let hostname = r.string()?;
                let slot = worlds
                    .get_mut(index)
                    .ok_or_else(|| anyhow::anyhow!("world list index {index}"))?;
                ensure!(slot.is_none(), "duplicate world list index {index}");
                *slot = Some(World {
                    id: min + index as i32,
                    flags,
                    country,
                    country_name,
                    activity,
                    hostname,
                    players: 0,
                    hostpacked: -1,
                });
            }
            self.token = r.i32()?;
            self.worlds = worlds;
            self.min_id = min;
            self.count = count;
            self.resolve_cursor = self.resolve_cursor.min(self.worlds.len());
            self.resolve_pending = None;
        }
        // Counts use sparse GWC indexes, not positions in the sorted list.
        for _ in 0..self.count {
            let index = r.smart()?;
            let players = r.u16()?;
            let slot = self
                .worlds
                .get_mut(index)
                .ok_or_else(|| anyhow::anyhow!("world count index {index}"))?;
            if let Some(world) = slot {
                world.players = if players == 65535 { -1 } else { players as i32 };
            }
        }
        if r.pos != bytes.len() {
            bail!("trailing world list bytes");
        }
        Ok(())
    }
    pub fn specific(&self, id: i32) -> Option<&World> {
        let index = id.checked_sub(self.min_id)?;
        self.worlds.get(usize::try_from(index).ok()?)?.as_ref()
    }
    pub fn start(&mut self) -> Option<World> {
        self.cursor = 0;
        self.next()
    }
    #[allow(
        clippy::should_implement_trait,
        reason = "a cursor over the list the scripts drive, not an Iterator"
    )]
    pub fn next(&mut self) -> Option<World> {
        let index = *self.order.get(self.cursor)?;
        self.cursor += 1;
        self.worlds[index].clone()
    }
    pub fn sort(
        &mut self,
        primary: i32,
        reverse: bool,
        secondary: i32,
        secondary_reverse: bool,
        language: Option<Language>,
    ) {
        // Same pivot and <= partition as the original client, including
        // ties (unstable).
        fn quick(
            order: &mut [usize],
            worlds: &[Option<World>],
            p: i32,
            r: bool,
            s: i32,
            sr: bool,
            l: Option<Language>,
        ) {
            if order.len() < 2 {
                return;
            }
            let end = order.len() - 1;
            let mid = end / 2;
            order.swap(mid, end);
            let pivot = order[end];
            let mut at = 0;
            for i in 0..end {
                let a = worlds[order[i]].as_ref().unwrap();
                let b = worlds[pivot].as_ref().unwrap();
                let mut c = compare(a, b, p, r, l);
                if r {
                    c = -c;
                }
                if c == 0 && s != -1 {
                    c = compare(a, b, s, sr, l);
                    if sr {
                        c = -c;
                    }
                }
                if c <= 0 {
                    order.swap(i, at);
                    at += 1;
                }
            }
            order.swap(at, end);
            let (left, right) = order.split_at_mut(at);
            quick(left, worlds, p, r, s, sr, l);
            quick(&mut right[1..], worlds, p, r, s, sr, l);
        }
        quick(
            &mut self.order,
            &self.worlds,
            primary,
            reverse,
            secondary,
            secondary_reverse,
            language,
        );
        // Sorting resets host resolution, not iteration.
        self.resolve_cursor = 0;
        self.resolve_pending = None;
    }

    /// Applies the `worldlist_pingworlds` state toggle. The script only
    /// reaches this owner while the lobby world list is active.
    pub fn set_resolve_hosts_enabled(&mut self, enabled: bool) {
        self.resolve_hosts_enabled = enabled;
        if !enabled {
            self.resolve_pending = None;
        }
    }

    /// Return the next unresolved hostname for the asynchronous resolver.
    /// The pending world remains pinned until its result is installed, which
    /// keeps a single outstanding lookup.
    pub fn next_host_resolution(&mut self) -> Option<(i32, String)> {
        if !self.resolve_hosts_enabled || self.resolve_pending.is_some() {
            return None;
        }
        while self.resolve_cursor < self.worlds.len() {
            let index = self.resolve_cursor;
            let id = self.min_id + i32::try_from(index).ok()?;
            self.resolve_cursor += 1;
            let Some(world) = self.worlds.get(index).and_then(Option::as_ref) else {
                continue;
            };
            if world.hostpacked != -1 {
                continue;
            }
            self.resolve_pending = Some(id);
            return Some((id, world.hostname.clone()));
        }
        None
    }

    /// Install one asynchronous host lookup result. A failed lookup leaves
    /// the `-1` scripts see and permits a later retry.
    pub fn set_hostpacked(&mut self, id: i32, hostpacked: i32) {
        let Some(index) = id
            .checked_sub(self.min_id)
            .and_then(|v| usize::try_from(v).ok())
        else {
            return;
        };
        if let Some(Some(world)) = self.worlds.get_mut(index) {
            world.hostpacked = hostpacked;
        }
        if self.resolve_pending == Some(id) {
            self.resolve_pending = None;
            if hostpacked != -1 {
                self.resolve_cursor = self.resolve_cursor.max(index.saturating_add(1));
            } else {
                self.resolve_cursor = self.resolve_cursor.min(index);
            }
        }
    }
}
fn compare(a: &World, b: &World, key: i32, reverse: bool, language: Option<Language>) -> i32 {
    let strings = |a: &str, b: &str| {
        ui_text_compare::compare(
            &a.encode_utf16().collect::<Vec<_>>(),
            &b.encode_utf16().collect::<Vec<_>>(),
            language,
        )
    };
    match key {
        1 => {
            let count = |v| if v == -1 && !reverse { 2001 } else { v };
            count(a.players) - count(b.players)
        }
        2 => strings(&a.country_name, &b.country_name),
        3 => {
            if a.activity == "-" {
                if b.activity == "-" {
                    0
                } else if reverse {
                    -1
                } else {
                    1
                }
            } else if b.activity == "-" {
                if reverse {
                    1
                } else {
                    -1
                }
            } else {
                strings(&a.activity, &b.activity)
            }
        }
        4..=7 => {
            let mask = match key {
                4 => 8,
                5 => 2,
                6 => 4,
                _ => 1,
            };
            i32::from(a.flags & mask != 0) - i32::from(b.flags & mask != 0)
        }
        8 => {
            let ping = |v| {
                if reverse && v == 1000 {
                    -1
                } else if !reverse && v == -1 {
                    1000
                } else {
                    v
                }
            };
            ping(a.hostpacked) - ping(b.hostpacked)
        }
        _ => a.id - b.id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn full() -> Vec<u8> {
        let mut b = vec![1, 2, 1, 1, 10, 0, b'U', b'S', 0, 1, 4, 2];
        for (index, override_id, activity) in [(0, 0, b'-'), (3, 9, 0x80)] {
            b.extend([index, 0, 0, 0, 1, 9, override_id]);
            if override_id != 0 {
                b.extend([0, 0x8c, 0]);
            }
            b.extend([0, activity, 0, 0, b'h', 0]);
        }
        b.extend(42i32.to_be_bytes());
        b.extend([0, 0xff, 0xff, 3, 0, 32]);
        b
    }
    #[test]
    fn sparse_chunked_and_partial_updates() {
        let bytes = full();
        let mut s = State::default();
        let mut wire = vec![];
        assert_eq!(s.request(1000, true, &mut wire), 0);
        assert_eq!(wire, vec![77, 0, 0, 0, 0]);
        let mut first = vec![0];
        first.extend(&bytes[1..17]);
        s.apply_reply(&first, 1100).unwrap();
        assert!(s.fetching);
        assert!(s.specific(1).is_none());
        let mut last = vec![1];
        last.extend(&bytes[17..]);
        s.apply_reply(&last, 1200).unwrap();
        assert!(!s.fetching);
        assert_eq!(s.token, 42);
        assert!(s.next().is_none());
        assert_eq!(s.specific(1).unwrap().players, -1);
        assert_eq!(s.specific(1).unwrap().hostpacked, -1);
        s.set_resolve_hosts_enabled(true);
        assert_eq!(s.next_host_resolution(), Some((1, "h".to_string())));
        s.set_hostpacked(1, i32::from_be_bytes([127, 0, 0, 1]));
        assert_eq!(s.specific(1).unwrap().hostpacked, 0x7f00_0001);
        assert!(s.specific(2).is_none());
        let w = s.specific(4).unwrap();
        assert_eq!(
            (
                w.country,
                w.country_name.as_str(),
                w.activity.as_str(),
                w.flags
            ),
            (9, "Œ", "€", 265)
        );
        s.sort(1, true, -1, false, Some(Language::En));
        assert_eq!(s.start().unwrap().id, 4);
        s.apply_reply(&[1, 2, 0, 3, 0, 77, 0, 0, 66], 2200).unwrap();
        assert_eq!(s.specific(4).unwrap().players, 77);
        assert_eq!(s.specific(1).unwrap().players, 66);
        assert_eq!(s.request(3199, true, &mut wire), 1);
        assert_eq!(s.request(3200, true, &mut wire), 0);
        assert_eq!(&wire[5..], &[77, 0, 0, 0, 42]);
    }
    #[test]
    fn malformed_update_preserves_list() {
        let mut s = State::default();
        s.apply_reply(&full(), 2000).unwrap();
        let previous = s.specific(1).cloned();
        assert!(s.apply_reply(&[1, 2, 1], 3000).is_err());
        assert_eq!(s.specific(1).cloned(), previous);
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    fn reader(bytes: &[u8]) -> Reader<'_> {
        Reader { bytes, pos: 0 }
    }

    /// A smart value: one byte below 128, else
    /// `g2 - 32768`; the peek fails on an empty stream.
    #[test]
    fn smart_reads_one_or_two_bytes_and_peeks_first() {
        let mut r = reader(&[5, 0x80, 0x80, 0xFF, 0xFF]);
        assert_eq!(r.smart().unwrap(), 5);
        assert_eq!(r.smart().unwrap(), 128);
        assert_eq!(r.smart().unwrap(), 32767);
        assert_eq!(r.pos, 5);
        assert!(r.smart().is_err());
        // A two-byte form cut after its first byte leaves `pos` unchanged.
        let mut cut = reader(&[0x80]);
        assert!(cut.smart().is_err());
        assert_eq!(cut.pos, 0);
    }

    /// A string: a zero version byte, then a
    /// NUL-terminated Cp1252 string; a non-zero prefix throws.
    #[test]
    fn gjstr2_checks_the_prefix_and_decodes_cp1252() {
        let mut r = reader(&[0, b'h', 0x8C, 0x80, 0, 0, 0]);
        assert_eq!(r.string().unwrap(), "h\u{152}\u{20ac}");
        assert_eq!(r.string().unwrap(), "");
        assert_eq!(r.pos, 7);
        assert!(reader(&[1, b'h', 0]).string().is_err());
        assert!(reader(&[0, b'h']).string().is_err(), "missing NUL");
    }

    /// Fixed-width reads are big-endian and all-or-nothing.
    #[test]
    fn fixed_reads_are_big_endian_and_atomic() {
        let mut r = reader(&[0x12, 0x34, 0xFF, 0xFF, 0xFF, 0xFE, 0x01]);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert_eq!(r.i32().unwrap(), -2);
        assert!(r.u16().is_err());
        assert_eq!(r.pos, 6);
        assert_eq!(r.u8().unwrap(), 1);
    }
    /// The dev server's list of worlds 1 and 2 (its own encoder's output,
    /// `fixtures/world_list_1_2.hex`, which its tests pin) lists both, on
    /// this host, in world order.
    #[test]
    fn the_dev_servers_two_worlds_are_both_listed() {
        let hex = include_str!("../../../fixtures/world_list_1_2.hex").trim();
        let reply: Vec<u8> = (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
            .collect();
        let mut s = State::default();
        s.apply_reply(&reply, 5000).unwrap();
        let listed: Vec<(i32, &str, i32)> = [1, 2]
            .iter()
            .map(|&id| {
                let world = s.specific(id).expect("listed");
                (world.id, world.hostname.as_str(), world.players)
            })
            .collect();
        assert_eq!(listed, [(1, "localhost", 0), (2, "localhost", 0)]);
        assert!(s.specific(3).is_none());
        assert_eq!(s.start().unwrap().id, 1);
        assert_eq!(s.next().unwrap().id, 2);
        assert!(s.next().is_none());
    }
}
