//! Map file 5 (the modern terrain) of one map square, decoded with its heights
//! (`nxt-data-formats.md` §2; renderer plan §4(j)). Moved from
//! `rs910_render_modern::terrain`, which builds the near
//! terrain from the same decode: the far squares and the classic window's
//! terrain read one height function, so they meet at equal heights.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rs910_config::nxt::map_terrain::{decode_terrain, NxtTerrainTile, TERRAIN_SIDE};
use rs910_config::nxt::{map_group, MAP_ARCHIVE, TERRAIN_FILE};

use crate::far_ring::{in_world, SquareId};

/// One map square's decoded terrain: per level (0..4) its 66 x 66 tiles
/// (the square's 64 x 64 and a one-tile border of its neighbours', `x *
/// 66 + z` from -1) and their heights (y up, fine units, cumulative over the
/// levels below).
#[derive(Clone, Debug, Default)]
pub struct SquareTerrain {
    pub levels: [Option<(Vec<NxtTerrainTile>, Vec<f32>)>; 4],
}

/// The offset of height code `c` over the level below, y up: `32 c` fine
/// units, `c == 1` none, `c == 0` 960 (a level's default step; also at
/// level 0, where file 5 always stores an explicit byte but for 71 water
/// surfaces). The same units as the classic landscape (`h * 8 << 2` per
/// step, 960 per implicit level; classic y = `-y_up`), proven over the
/// pack (`nxt-data-formats.md` §2).
#[must_use]
pub fn height_offset(code: u8) -> f32 {
    match code {
        0 => 960.0,
        1 => 0.0,
        c => 32.0 * f32::from(c),
    }
}

/// A tile's terrain height, y up: its height code over the level below; a
/// water tile's is its bed, the surface (the water height code over the
/// level below) minus `32` per step of its height byte (the classic
/// underwater land byte, `land + 32 depth`).
#[must_use]
pub fn tile_height_up(t: &NxtTerrainTile, below: f32) -> f32 {
    match t.water_height {
        Some(surface) => below + height_offset(surface) - 32.0 * f32::from(t.height),
        None => below + height_offset(t.height),
    }
}

/// Decode map file 5 of group `group` (`None`: undecodable or empty).
#[must_use]
pub fn decode_square(group: u32, file5: &[u8]) -> Option<SquareTerrain> {
    let blocks = decode_terrain(group, file5).ok()?;
    if blocks.is_empty() {
        return None;
    }
    let mut square = SquareTerrain {
        levels: [None, None, None, None],
    };
    let mut below = vec![0.0_f32; TERRAIN_SIDE * TERRAIN_SIDE];
    for level in 0..4 {
        let Some(block) = blocks.iter().find(|b| usize::from(b.level) == level) else {
            // DecodeHeightCode: a missing level counts 960 more.
            if level > 0 {
                for b in &mut below {
                    *b += 960.0;
                }
            }
            continue;
        };
        let heights: Vec<f32> = block
            .tiles
            .iter()
            .zip(&below)
            .map(|(t, &b)| tile_height_up(t, b))
            .collect();
        // The next level stands on this one's surface: a water tile's
        // surface, not its bed.
        below = block
            .tiles
            .iter()
            .zip(&below)
            .map(|(t, &b)| b + height_offset(t.water_height.unwrap_or(t.height)))
            .collect();
        square.levels[level] = Some((block.tiles.clone(), heights));
    }
    Some(square)
}

/// Square `sq`'s terrain read quietly (`Pack::read_group_resident`: a group
/// absent from the pack and the disk store is `None`, never a JS5 request).
#[must_use]
pub fn read_square(pack: &rs910_js5::cache::Pack, sq: SquareId) -> Option<SquareTerrain> {
    if !in_world(sq) {
        return None;
    }
    let group = map_group(sq.0 as u32, sq.1 as u32);
    let files = pack.read_group_resident(MAP_ARCHIVE, group).ok()??;
    decode_square(group, files.get(&TERRAIN_FILE)?)
}

/// The decoded squares the far scene's workers share (milestone F5): each
/// square read quietly and decoded once while it stays near the ring, then
/// handed out by reference.
#[derive(Clone, Default)]
pub struct TerrainCache(Arc<Mutex<HashMap<SquareId, Option<Arc<SquareTerrain>>>>>);

impl TerrainCache {
    /// Square `sq`'s terrain ([`read_square`] on first use; two workers
    /// may decode the same square at once, both results are equal).
    #[must_use]
    pub fn get(&self, pack: &rs910_js5::cache::Pack, sq: SquareId) -> Option<Arc<SquareTerrain>> {
        if let Some(t) = self.0.lock().expect("far terrain cache").get(&sq) {
            return t.clone();
        }
        let t = read_square(pack, sq).map(Arc::new);
        self.0
            .lock()
            .expect("far terrain cache")
            .insert(sq, t.clone());
        t
    }

    /// `sq` and its eight neighbours, as the terrain builder takes them
    /// (squares without terrain left out).
    #[must_use]
    pub fn neighbourhood(
        &self,
        pack: &rs910_js5::cache::Pack,
        sq: SquareId,
    ) -> HashMap<SquareId, SquareTerrain> {
        let mut out = HashMap::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let n = (sq.0 + dx, sq.1 + dz);
                if let Some(t) = self.get(pack, n) {
                    out.insert(n, (*t).clone());
                }
            }
        }
        out
    }

    /// Keep only the squares `keep` names.
    pub fn retain(&self, keep: &dyn Fn(SquareId) -> bool) {
        self.0
            .lock()
            .expect("far terrain cache")
            .retain(|&sq, _| keep(sq));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The height codes' fine heights ([`height_offset`]: the classic units,
    /// `nxt-data-formats.md` §2).
    #[test]
    fn height_codes_decode_to_fine_heights() {
        assert_eq!(height_offset(0), 960.0);
        assert_eq!(height_offset(1), 0.0);
        assert_eq!(height_offset(10), 320.0);
    }

    /// Lumbridge's square decodes quietly with four levels' worth of
    /// heights; an ocean square without a group is simply absent.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn squares_read_quietly() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let lumbridge = read_square(&pack, (50, 50)).expect("Lumbridge's file 5");
        let (tiles, heights) = lumbridge.levels[0].as_ref().unwrap();
        assert_eq!(tiles.len(), TERRAIN_SIDE * TERRAIN_SIDE);
        assert_eq!(heights.len(), tiles.len());
        assert!(read_square(&pack, (-1, 3)).is_none());
        // The one-tile border is the neighbour's interior: square (50, 50)'s
        // east border column is square (51, 50)'s first column.
        let east = read_square(&pack, (51, 50)).unwrap();
        let (etiles, eheights) = east.levels[0].as_ref().unwrap();
        for z in 1..=64 {
            assert_eq!(tiles[65 * TERRAIN_SIDE + z], etiles[TERRAIN_SIDE + z]);
            assert_eq!(heights[65 * TERRAIN_SIDE + z], eheights[TERRAIN_SIDE + z]);
        }
    }
}
