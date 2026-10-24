//! The camera scene snapshot, entity references and terrain height queries.

pub use crate::cam2_scene::*;

use crate::protocol910::terrain::Terrain;

use rs910_core::fault::Fault;

/// The `(type, index)` pair the camera re-resolves against the scene snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackableRef {
    pub kind: i32,
    pub index: i32,
}

/// Terrain heights (`[4][w+1][h+1]`) and the level-1 link-below bits, copied
/// from the installed CPU terrain.
#[derive(Clone, Debug, PartialEq)]
pub struct Heightmap {
    pub width: usize,
    pub height: usize,
    heights: Vec<i32>,
    link_below: Vec<bool>,
    pub generation: u64,
}

impl Heightmap {
    pub fn from_terrain(t: &Terrain, generation: u64) -> Self {
        let mut link_below = Vec::with_capacity(t.width * t.height);
        for x in 0..t.width {
            for z in 0..t.height {
                link_below.push(t.tiles[t.tile(1, x, z)].flags & 2 != 0);
            }
        }
        Self {
            width: t.width,
            height: t.height,
            heights: t.heights.clone(),
            link_below,
            generation,
        }
    }
    /// Height at `[level][x][z]`; an index past the grid is an out-of-range fault.
    pub(crate) fn get(&self, level: i32, x: i32, z: i32) -> Result<i64, String> {
        if !(0..4).contains(&level)
            || x < 0
            || z < 0
            || x as usize > self.width
            || z as usize > self.height
        {
            return Err(Fault::IndexOutOfRange.message(format!("height grid [{level}][{x}][{z}]")));
        }
        Ok(i64::from(
            self.heights
                [(level as usize * (self.width + 1) + x as usize) * (self.height + 1) + z as usize],
        ))
    }
    /// Interpolated terrain height at a fine coordinate (0 outside the grid).
    pub fn heightmap_y(&self, x: i32, z: i32, level: i32) -> i32 {
        let (tx, tz) = (x >> 9, z >> 9);
        if tx < 0 || tz < 0 || tx as usize >= self.width || tz as usize >= self.height {
            return 0;
        }
        let level = if level < 3 && self.is_link_below(tx, tz) {
            level + 1
        } else {
            level
        };
        let h = |dx: i32, dz: i32| self.get(level, tx + dx, tz + dz).unwrap_or(0) as i32;
        let (fx, fz) = (x & 511, z & 511);
        let mix =
            |a: i32, b: i32, t: i32| (512 - t).wrapping_mul(a).wrapping_add(t.wrapping_mul(b)) >> 9;
        mix(mix(h(0, 0), h(1, 0), fx), mix(h(0, 1), h(1, 1), fx), fz)
    }
    /// Whether the tile links to the level below.
    pub fn is_link_below(&self, x: i32, z: i32) -> bool {
        x >= 0
            && z >= 0
            && (x as usize) < self.width
            && (z as usize) < self.height
            && self.link_below[x as usize * self.height + z as usize]
    }
    /// The bilinear height sample shared by the point and entity eye collision:
    /// `x`, `z` in fine units relative to the base.
    pub(super) fn bilinear(
        &self,
        level: i32,
        tx: i32,
        tz: i32,
        x: f32,
        z: f32,
    ) -> Result<i64, String> {
        let frac_x = (x as i64) % 512;
        let frac_z = (z as i64) % 512;
        let acc00 = (512 - frac_x) * self.get(level, tx, tz)? * (512 - frac_z);
        let acc10 = self.get(level, tx + 1, tz)? * frac_x * (512 - frac_z) + acc00;
        let acc01 = (512 - frac_x) * self.get(level, tx, tz + 1)? * frac_z + acc10;
        let acc11 = self.get(level, tx + 1, tz + 1)? * frac_x * frac_z + acc01;
        Ok(acc11 / 262144)
    }
}

/// What the client hands the camera each cycle: the scene base in fine units,
/// the local player trackable and the terrain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub base: [i32; 2],
    pub local_player: Option<Trackable>,
    /// Retained NPC entries indexed by their server slot. The
    /// camera copies this snapshot each cycle so entity owners can re-resolve
    /// a packet's `(type,index)` reference after movement updates.
    pub npcs: std::collections::BTreeMap<i32, Trackable>,
    /// Size of the local player entity (the scene pick offsets it).
    pub local_size: i32,
    pub heightmap: Option<Heightmap>,
}

impl Scene {
    /// Resolves a reference against the local player and the retained NPC slots.
    pub fn trackable(&self, r: TrackableRef) -> Option<Trackable> {
        if r.kind == TRACKABLE_PLAYER {
            return self.local_player.filter(|p| p.index == r.index);
        }
        (r.kind == TRACKABLE_NPC)
            .then(|| self.npcs.get(&r.index).copied())
            .flatten()
    }
}
