//! The NPC spawn list of a map square (file 2 of the square's group) and the title world's
//! use of it.
//!
//! The client stands these NPCs up only while it builds the title/lobby world (the build that
//! follows the title camera leaving its build area). A rebuild the server asks for never reads
//! the file, so in the world the list never shows: the title world is not presented and the
//! login clears the NPC list before the world's first rebuild.
//!
//! The file is a run of 4-byte entries: a packed position (`level << 14 | x << 7 | z`, local
//! to the square, x and z in 0..64) and the NPC type id. An entry stands its NPC up only when
//! the type allows it (bit 0 of the walk flags), the NPC's footprint lies inside the build
//! area, and the NPC list holds no NPC at the entry's index yet. The index is the square's
//! slot (assigned in order of first use and kept for the whole run, so a square keeps its
//! indices when the camera comes back) with the entry number above the low six bits. Each
//! entry takes its index whether or not it stands anything up.

use std::collections::BTreeMap;

use crate::entities910::Npc;
use crate::map::MapError;
use crate::protocol910::npc::{head_icons, NpcType, Npcs};

/// Entries the client reads from one square's list.
pub const MAX_ENTRIES: usize = 511;
/// NPCs the client holds at once; the entry loop stops at this many in the slot list.
pub const MAX_NPCS: usize = 1023;
/// Squares that can hold a slot: the index keeps the slot in its low six bits.
pub const MAX_SLOTS: usize = 64;

/// One entry of a square's NPC spawn list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapNpc {
    /// Level of the tile.
    pub level: u8,
    /// Tile x inside the square (0..64).
    pub x: u8,
    /// Tile z inside the square (0..64).
    pub z: u8,
    /// NPC type id.
    pub type_id: i32,
}

/// Decode a square's NPC spawn list. Entries past the 511th are not read.
pub fn decode(bytes: &[u8]) -> Result<Vec<MapNpc>, MapError> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() && out.len() < MAX_ENTRIES {
        let Some((entry, tail)) = rest.split_first_chunk::<4>() else {
            return Err(MapError::Truncated {
                what: "map NPC entry",
            });
        };
        let position = u16::from_be_bytes([entry[0], entry[1]]);
        out.push(MapNpc {
            level: (position >> 14) as u8,
            x: (position >> 7 & 0x3F) as u8,
            z: (position & 0x3F) as u8,
            type_id: i32::from(u16::from_be_bytes([entry[2], entry[3]])),
        });
        rest = tail;
    }
    Ok(out)
}

/// The slot each square that carried a spawn list was given. It is never reset, so a square
/// keeps its slot (and its NPC indices) for the whole run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SquareSlots {
    squares: Vec<u32>,
}

impl SquareSlots {
    /// The slot of `square` (`mx << 8 | mz`), assigned on first use.
    pub fn slot(&mut self, square: u32) -> Result<usize, MapError> {
        if let Some(slot) = self.squares.iter().position(|&s| s == square) {
            return Ok(slot);
        }
        if self.squares.len() >= MAX_SLOTS {
            return Err(MapError::Invalid(format!(
                "more than {MAX_SLOTS} map squares carry NPC spawn lists"
            )));
        }
        self.squares.push(square);
        Ok(self.squares.len() - 1)
    }
}

/// What a square's entries are placed into.
pub struct Placer<'a> {
    /// Side of the build area, in tiles.
    pub size: i32,
    /// Tile of the build area's origin.
    pub base: [i32; 2],
    /// The logic cycle stamped on the new NPCs.
    pub cycle: i32,
    /// The textures preference (it picks the range of the fourth recolour value).
    pub textures: bool,
    /// The NPC types by id.
    pub types: &'a BTreeMap<i32, NpcType>,
    /// Whether the tile `(x, z)` of the build area is a bridge tile (the NPC stands a level up).
    pub bridge: &'a dyn Fn(i32, i32) -> bool,
    /// The random source of the recolour values.
    pub random: &'a mut dyn FnMut() -> f64,
}

impl Placer<'_> {
    /// Stand up the NPCs of the square `square` (`mx << 8 | mz`) whose list is `entries`, under
    /// `slot`. Returns how many were stood up.
    pub fn place_square(
        &mut self,
        npcs: &mut Npcs,
        slot: usize,
        square: u32,
        entries: &[MapNpc],
    ) -> Result<usize, MapError> {
        let mut placed = 0;
        for (number, entry) in entries.iter().take(MAX_ENTRIES).enumerate() {
            if npcs.slots.len() >= MAX_NPCS {
                break;
            }
            let index = slot | number << 6;
            let Some(kind) = self.types.get(&entry.type_id) else {
                continue;
            };
            let x = (square >> 8) as i32 * 64 - self.base[0] + i32::from(entry.x);
            let z = (square & 0xFF) as i32 * 64 - self.base[1] + i32::from(entry.z);
            if npcs.entities.contains_key(&index)
                || kind.walkflags & 1 <= 0
                || !(x >= 0 && kind.size + x < self.size && z >= 0 && kind.size + z < self.size)
            {
                continue;
            }
            let facing = kind.respawndir.ok_or_else(|| {
                MapError::Invalid(format!(
                    "NPC type {} has no respawn direction",
                    entry.type_id
                ))
            })?;
            let draw: [f64; 4] = std::array::from_fn(|_| (self.random)());
            let mut npc = Npc::new([
                (draw[0] * 4.) as i32 + 32,
                (draw[1] * 2.) as i32 + 3,
                (draw[2] * 3.) as i32 + 16,
                (draw[3] * if self.textures { 6. } else { 12. }) as i32,
            ]);
            npc.update_serial = self.cycle;
            npc.type_id = entry.type_id;
            npc.name = kind.name.clone();
            npc.vislevel = kind.vislevel;
            npc.head_icons = head_icons(kind.head_icons.as_deref());
            npc.covermarker = kind.covermarker;
            npc.path.size = kind.size;
            npc.turn_speed = kind.turnspeed.wrapping_shl(3);
            // The facing is the opposite compass point of the stored one, set at once.
            let angle = (facing + 4) << 11 & 0x3FFF;
            npc.path.angle = angle;
            npc.path.desired_angle = angle;
            let level = i32::from(entry.level);
            npc.path.level = level;
            npc.path.occlude_level = level + i32::from((self.bridge)(x, z));
            npc.path.tele(x, z);
            npcs.entities.insert(index, npc);
            npcs.snapshot.push(index);
            npcs.slots.push(index);
            placed += 1;
        }
        Ok(placed)
    }
}

#[cfg(test)]
mod tests;
