//! Normal rebuild payload decoding and map-square selection.

/// Fixed `REBUILD_NORMAL` tail length (`2 + 1 + 1 + 1 + 2 + 1`).
pub const REBUILD_TAIL_LEN: usize = 8;

/// Parsed `REBUILD_NORMAL` (opcode 88) fixed tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuild {
    /// Zone X (`absX >> 3`), 8×8-tile units.
    pub zone_x: u16,
    /// Zone Z (`absZ >> 3`).
    pub zone_z: u16,
    /// NPC distance in bits (`p1`, server sends 5).
    pub npc_bits: u8,
    /// Map count (`p1_alt3`, server sends 9).
    pub map_count: u8,
    /// Build-area id (`p1`, default world size 104 → id 0).
    pub build_area_id: u8,
    /// Force-rebuild flag (`p1_alt3`, nonzero → true).
    pub force: bool,
    /// True when payload bytes precede the 8-byte tail (login-time
    /// `nearbyPlayers=true` bit-block).
    pub has_high_res_block: bool,
    /// Number of skipped high-res prefix bytes (0 when absent).
    pub high_res_bytes: usize,
}

/// Decode `p2_alt2`: wire `[(v >> 8) & 0xFF, (v + 128) & 0xFF]`
/// (`Packet.ts:223-226`).
pub(super) fn decode_p2_alt2(hi: u8, lo: u8) -> u16 {
    ((u16::from(hi)) << 8) | u16::from(lo.wrapping_sub(128))
}

/// Decode `p1_alt3`: wire `(128 - v) & 0xFF` (`Packet.ts:209-211`).
pub(super) fn decode_p1_alt3(raw: u8) -> u8 {
    128u8.wrapping_sub(raw)
}

/// Parse a `REBUILD_NORMAL` payload.
///
/// The fixed 8-byte tail is read from the LAST 8 bytes, in exact
/// the map-rebuild field order; any leading bytes (the optional
/// login-time high-res bit-block) are skipped but counted. Errors when fewer
/// than 8 bytes are present.
pub fn parse_rebuild_normal(payload: &[u8]) -> anyhow::Result<Rebuild> {
    if payload.len() < REBUILD_TAIL_LEN {
        anyhow::bail!(
            "REBUILD_NORMAL: need at least {REBUILD_TAIL_LEN} bytes, got {}",
            payload.len()
        );
    }
    let (prefix, tail) = payload.split_at(payload.len() - REBUILD_TAIL_LEN);
    Ok(Rebuild {
        zone_x: decode_p2_alt2(tail[0], tail[1]),
        zone_z: decode_p2_alt2(tail[5], tail[6]),
        npc_bits: tail[2],
        map_count: decode_p1_alt3(tail[3]),
        build_area_id: tail[4],
        force: decode_p1_alt3(tail[7]) != 0,
        has_high_res_block: !prefix.is_empty(),
        high_res_bytes: prefix.len(),
    })
}

/// Map a parsed rebuild to the 3×3 mapsquare group block around the player:
/// `mapsquare = zone >> 3`, clamped to the valid `0..=127` range, with
/// `group = mx | mz << 7` (same packing as `map::LUMBRIDGE_GROUPS`).
/// Duplicates from edge clamping are removed; output is sorted ascending.
pub fn rebuild_to_groups(rebuild: &Rebuild) -> Vec<u16> {
    let mx = i32::from(rebuild.zone_x >> 3);
    let mz = i32::from(rebuild.zone_z >> 3);
    let mut groups = Vec::with_capacity(9);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let gx = (mx + dx).clamp(0, 127) as u16;
            let gz = (mz + dz).clamp(0, 127) as u16;
            let group = gx | (gz << 7);
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
    groups.sort_unstable();
    groups
}

/// One live map reload: a fresh `REBUILD_NORMAL` plus the 3×3 group block
/// [`rebuild_to_groups`] derives from it. Returned by [`drain_pending_sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildEvent {
    /// Parsed `REBUILD_NORMAL` tail that triggered the reload.
    pub rebuild: Rebuild,
    /// Sorted 3×3 mapsquare groups around the player (`group = mx | mz << 7`).
    pub groups: Vec<u16>,
}

impl RebuildEvent {
    /// Attach the derived 3×3 group block to a parsed rebuild.
    #[must_use]
    pub fn new(rebuild: Rebuild) -> Self {
        let groups = rebuild_to_groups(&rebuild);
        Self { rebuild, groups }
    }
}

/// Pure dedup for the render layer: true when `next` differs from `current`
/// (order-insensitive set compare; both sides are sorted + deduped first so
/// repeats of the same `REBUILD_NORMAL` are no-ops). Pack-free.
#[must_use]
pub fn should_reload(current: &[u16], next: &[u16]) -> bool {
    let mut a: Vec<u16> = current.to_vec();
    let mut b: Vec<u16> = next.to_vec();
    a.sort_unstable();
    a.dedup();
    b.sort_unstable();
    b.dedup();
    a != b
}
