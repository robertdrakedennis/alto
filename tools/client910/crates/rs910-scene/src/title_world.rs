//! The title/lobby world: while no login is in progress the camera position is checked against
//! the build area and the area around the camera is rebuilt at the `buildArea` preference's
//! size. The title/lobby screens draw interfaces only and the scene draw fills black outside
//! the in-game scene state, so this world is never presented; its build area, region and NPCs
//! are still observable state.
//!
//! The NPCs are the spawn lists of the rebuild's map squares ([`crate::map_npcs`]). A rebuild
//! first carries the NPCs already standing across the base change (dropping those that leave
//! the area), then stands up the squares' lists. The login clears the NPC list, so none of
//! them reaches the world.

/// The map size of a `buildArea` preference id.
#[must_use]
pub fn build_area_size(id: i32) -> Option<i32> {
    Some(match id {
        0 => 104,
        1 => 120,
        2 => 136,
        3 => 168,
        4 => 72,
        5 => 256,
        _ => return None,
    })
}

use std::collections::BTreeMap;

use crate::map_npcs::{self, Placer, SquareSlots};
use crate::protocol910::npc::{NpcType, Npcs};
use crate::protocol910::rebuild_state::rebase_npcs;
use crate::protocol910::terrain::Terrain;

/// The cache group id of map square `(x, z)`.
#[must_use]
pub fn map_square_group(x: i32, z: i32) -> i32 {
    x | z << 7
}

/// The world fields the title rebuild reads and writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleWorld {
    /// The build-area preference id (`None` before the first size is set).
    pub build_area: Option<i32>,
    /// Map size in tiles (x and z are equal).
    pub map_size: i32,
    /// Scene base tile x/z.
    pub base: [i32; 2],
    /// Region x/z.
    pub region: [i32; 2],
    /// The slot of every map square that carried an NPC list (kept for the whole run).
    pub npc_slots: SquareSlots,
}

/// A title rebuild that passed the region-changed guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebuild {
    /// The client state entered: 10 (title), 16 (lobby entering game), 6 (lobby) or 8 (account
    /// creation).
    pub state: i32,
    pub region: [i32; 2],
    /// The scene base tile before this rebuild.
    pub old_base: [i32; 2],
    /// Map square group ids that have a LAND file.
    pub squares: Vec<i32>,
}

/// The files of one rebuild square the title world reads.
pub struct SquareFiles {
    /// The square's group id.
    pub group: i32,
    /// Its LAND file.
    pub land: Option<Vec<u8>>,
    /// Its NPC spawn list, when it has one.
    pub npcs: Option<Vec<u8>>,
}

/// Read the files of the rebuild's squares from the maps archive; a square that cannot be read
/// is left out (and logged).
pub fn read_squares(pack: &crate::cache::Pack, squares: &[i32]) -> Vec<SquareFiles> {
    squares
        .iter()
        .filter_map(|&group| {
            let mut files = pack
                .read_group(crate::map::MAP_ARCHIVE, u32::try_from(group).ok()?)
                .inspect_err(|error| log::warn!("title map square {group}: {error:#}"))
                .ok()?;
            Some(SquareFiles {
                group,
                land: files.remove(&crate::cache::LAND_FILE),
                npcs: files.remove(&crate::cache::NPC_FILE),
            })
        })
        .collect()
}

/// What a rebuild did to the NPC list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NpcChanges {
    /// Indices of the NPCs the base change dropped.
    pub removed: Vec<usize>,
    /// NPCs the squares' lists stood up.
    pub placed: usize,
}

/// What the NPC step of a rebuild reads besides the squares.
pub struct NpcInputs<'a> {
    /// The NPC types by id.
    pub types: &'a BTreeMap<i32, NpcType>,
    /// The logic cycle.
    pub cycle: i32,
    /// The textures preference.
    pub textures: bool,
    /// The random source of the recolour values.
    pub random: &'a mut dyn FnMut() -> f64,
}

impl TitleWorld {
    /// Reset the base and region, as on logout.
    pub fn reset_base(&mut self) {
        self.base = [0, 0];
        self.region = [0, 0];
    }

    /// Apply the size of a build-area id. An id with no known size is an error.
    fn set_build_area(&mut self, id: i32) -> anyhow::Result<()> {
        if self.build_area == Some(id) {
            return Ok(());
        }
        let size = build_area_size(id)
            .ok_or_else(|| anyhow::anyhow!("unknown build area size id {id}"))?;
        self.map_size = size;
        self.build_area = Some(id);
        Ok(())
    }

    /// Whether the camera left the inner build area.
    #[must_use]
    pub fn camera_at_edge(&self, [x, z]: [i32; 2]) -> bool {
        let size = self.map_size;
        x >> 9 < 14 || x >> 9 >= size - 14 || z >> 9 < 14 || z >> 9 >= size - 14
    }

    /// Resize to the build area and rebuild around the camera, guarded on the region having
    /// changed, for client state `state` (4, 15, 13 or 0). `land(group)` reports whether a map
    /// square group has a valid LAND file.
    pub fn rebuild(
        &mut self,
        [camera_x, camera_z]: [i32; 2],
        build_area: i32,
        state: i32,
        land: &mut dyn FnMut(i32) -> bool,
    ) -> anyhow::Result<Option<Rebuild>> {
        self.set_build_area(build_area)?;
        let region_x = (camera_x >> 12) + (self.base[0] >> 3);
        let region_z = (camera_z >> 12) + (self.base[1] >> 3);
        let next = match state {
            4 => 10,
            15 => 16,
            13 => 6,
            0 => 8,
            _ => anyhow::bail!("unexpected client state {state}"),
        };
        // Without a forced rebuild, an unchanged region needs no work.
        if self.region == [region_x, region_z] {
            return Ok(None);
        }
        let half = self.map_size >> 4;
        let mut squares = Vec::new();
        for x in (region_x - half) / 8..=(half + region_x) / 8 {
            for z in (region_z - half) / 8..=(half + region_z) / 8 {
                let group = map_square_group(x, z);
                if land(group) {
                    squares.push(group);
                }
            }
        }
        self.region = [region_x, region_z];
        let old_base = self.base;
        self.base = [
            (region_x - (self.map_size >> 4)) * 8,
            (region_z - (self.map_size >> 4)) * 8,
        ];
        Ok(Some(Rebuild {
            state: next,
            region: self.region,
            old_base,
            squares,
        }))
    }

    /// The NPC step of `rebuild`: carry `npcs` across the base change, then stand up the NPC
    /// lists of `files` (the rebuild's squares, in rebuild order).
    pub fn update_npcs(
        &mut self,
        rebuild: &Rebuild,
        files: &[SquareFiles],
        npcs: &mut Npcs,
        inputs: NpcInputs<'_>,
    ) -> anyhow::Result<NpcChanges> {
        let size = self.map_size;
        let removed = rebase_npcs(
            npcs,
            (
                self.base[0] - rebuild.old_base[0],
                self.base[1] - rebuild.old_base[1],
            ),
            (size, size),
            rebuild.state,
        )
        .map_err(|error| anyhow::anyhow!("NPC rebase: {error:?}"))?;
        // The bridge tiles come from the squares' landscape.
        let mut terrain = Terrain::new(size as usize, size as usize)
            .map_err(|error| anyhow::anyhow!("title terrain: {error:?}"))?;
        for square in files {
            let Some(land) = &square.land else { continue };
            let (mx, mz) = (square.group & 0x7F, square.group >> 7);
            terrain = terrain
                .read_normal(
                    land,
                    mx * 64 - self.base[0],
                    mz * 64 - self.base[1],
                    self.base[0],
                    self.base[1],
                )
                .map_err(|error| anyhow::anyhow!("title landscape: {error:?}"))?
                .state;
        }
        let bridge = |x: i32, z: i32| {
            x >= 0
                && z >= 0
                && x < size
                && z < size
                && terrain.tiles[terrain.tile(1, x as usize, z as usize)].flags & 2 != 0
        };
        let mut placer = Placer {
            size,
            base: self.base,
            cycle: inputs.cycle,
            textures: inputs.textures,
            types: inputs.types,
            bridge: &bridge,
            random: inputs.random,
        };
        let mut placed = 0;
        for square in files {
            let Some(list) = &square.npcs else { continue };
            let packed = ((square.group & 0x7F) as u32) << 8 | (square.group >> 7) as u32;
            let slot = self.npc_slots.slot(packed)?;
            let entries = map_npcs::decode(list)?;
            placed += placer.place_square(npcs, slot, packed, &entries)?;
        }
        Ok(NpcChanges { removed, placed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After a logout reset the title camera at (0, 0) is at the build-area edge every tick,
    /// the world is resized to the `buildArea` preference and the unchanged region 0,0 needs
    /// no rebuild: no rebuild state is entered.
    #[test]
    fn title_camera_at_origin_resizes_without_rebuilding() {
        let mut world = TitleWorld::default();
        assert!(world.camera_at_edge([0, 0]));
        let rebuild = world.rebuild([0, 0], 2, 4, &mut |_| true).unwrap();
        assert_eq!(rebuild, None);
        assert_eq!(world.map_size, 136);
        assert_eq!(world.build_area, Some(2));
        // An id with no known size is an error.
        assert!(world.rebuild([0, 0], 9, 13, &mut |_| true).is_err());
        assert_eq!(world.map_size, 136);
    }

    /// A title camera moved off the origin zone (e.g. by `cam_moveto`)
    /// passes the guard: `(region - half) / 8` truncates toward zero,
    /// lists square 0,0 when it has a LAND file and enters state 10 with the
    /// base moved to `(region - mapSize / 16) * 8`.
    #[test]
    fn moved_title_camera_rebuilds_its_region() {
        let mut world = TitleWorld::default();
        let camera = [10 * 512, 10 * 512];
        let rebuild = world
            .rebuild(camera, 0, 4, &mut |g| g == map_square_group(0, 0))
            .unwrap()
            .unwrap();
        assert_eq!(rebuild.state, 10);
        assert_eq!(rebuild.region, [1, 1]);
        assert_eq!(rebuild.squares, [0]);
        assert_eq!(world.base, [-40, -40]);
        // Only states 4, 15, 13 and 0 rebuild the title world.
        assert!(world.rebuild(camera, 0, 7, &mut |_| true).is_err());
    }
}
