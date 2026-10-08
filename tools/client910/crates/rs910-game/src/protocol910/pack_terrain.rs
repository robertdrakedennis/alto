//! Only group_count slots are loaded, then the world processes
//! the full allocated LAND array, including missing/trailing slots. Environment
//! trailers are explicit pending work; this result never authorizes map completion.
use crate::{
    cache::Pack,
    protocol910::{
        rebuild_state::{Kind, World},
        terrain::{RegionCopy, Terrain},
    },
};
use anyhow::Result;
use std::collections::BTreeSet;
#[derive(Clone)]
pub struct Environment {
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub consumed: usize,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub bytes: Vec<u8>,
}
#[derive(Clone)]
pub struct Inputs {
    pub terrain: Terrain,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub environments: Vec<Option<Environment>>,
    #[allow(
        dead_code,
        reason = "land bytes retained with the terrain; no reader yet"
    )]
    pub lands: Vec<Option<Vec<u8>>>,
}
/// Group validity tests the group's file capacity, not membership of file 3 in a sparse list.
pub fn land_groups(pack: &Pack) -> Result<BTreeSet<i32>> {
    let index = pack.read_archive_index("mapsv2")?;
    let mut out = BTreeSet::new();
    for &g in &index.group_id {
        let count = index.file_count_for_group(g)?;
        if count > 0 && index.file_id_for_group_index(g, count - 1)? >= 3 {
            out.insert(g as i32);
        }
    }
    Ok(out)
}
pub fn load_normal(pack: &Pack, w: &World) -> Result<Inputs> {
    // A normal rebuild after a cutscene retains lastRebuildType == CUTSCENE
    // (the rebuild-type check); cutscene worlds themselves always carry a
    // region layout and load through `load_region`.
    anyhow::ensure!(
        matches!(w.last_kind, Kind::Normal | Kind::Cutscene),
        "TODO(#gap-G-terrain-region): normal surface loading requires normal rebuild kind"
    );
    anyhow::ensure!(
        w.map_squares.len() == w.groups.len() && w.group_count <= w.groups.len(),
        "rebuild map array dimensions"
    );
    let mut terrain = Terrain::new(usize::try_from(w.width)?, usize::try_from(w.height)?)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut lands = vec![None; w.groups.len()];
    for (land, &group) in lands[..w.group_count].iter_mut().zip(&w.groups) {
        *land = pack.read_group("mapsv2", u32::try_from(group)?)?.remove(&3);
    }
    let mut environments = vec![];
    for (i, land) in lands.iter().enumerate() {
        let square = w.map_squares[i];
        let x = (square >> 8).wrapping_mul(64).wrapping_sub(w.base_x);
        let z = (square & 255).wrapping_mul(64).wrapping_sub(w.base_z);
        if let Some(b) = land {
            let decoded = terrain
                .read_normal(b, x, z, w.base_x, w.base_z)
                .map_err(|e| anyhow::anyhow!("LAND slot {i}: {e:?}"))?;
            terrain = decoded.state;
            environments.push(Some(Environment {
                consumed: decoded.consumed,
                bytes: decoded.environment,
            }));
        } else {
            environments.push(None);
        }
    }
    if w.region_z < 800 {
        for (i, land) in lands.iter().enumerate() {
            if land.is_none() {
                let square = w.map_squares[i];
                terrain
                    .clear_landscape(
                        (square >> 8).wrapping_mul(64).wrapping_sub(w.base_x),
                        (square & 255).wrapping_mul(64).wrapping_sub(w.base_z),
                        64,
                        64,
                    )
                    .map_err(|e| anyhow::anyhow!("clear LAND: {e:?}"))?;
            }
        }
    }
    Ok(Inputs {
        terrain,
        environments,
        lands,
    })
}

/// Load the source LAND squares and assemble the destination terrain for an
/// instanced region.  Templates are already validated and retained by the
/// rebuild transaction; this owner performs the real rotated tile copy before
/// the renderer can acknowledge the map.
pub fn load_region(
    pack: &Pack,
    w: &World,
    layout: &crate::protocol910::rebuild_state::RegionLayout,
) -> Result<Inputs> {
    anyhow::ensure!(
        w.map_squares.len() == w.groups.len() && w.group_count <= w.groups.len(),
        "rebuild map array dimensions"
    );
    anyhow::ensure!(
        layout.templates.len() == 4 * layout.chunks_x * layout.chunks_z,
        "region template count"
    );
    let mut terrain = Terrain::new(usize::try_from(w.width)?, usize::try_from(w.height)?)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut lands = vec![None; w.groups.len()];
    let mut by_square = std::collections::BTreeMap::new();
    for (i, land) in lands[..w.group_count].iter_mut().enumerate() {
        let mut files = pack.read_group("mapsv2", u32::try_from(w.groups[i])?)?;
        *land = files.remove(&3);
        by_square.insert(w.map_squares[i] as u32, land.as_ref().cloned());
    }
    let mut environments = vec![];
    for level in 0..4 {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                let template =
                    layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz];
                if template < 0 {
                    environments.push(None);
                    continue;
                }
                let src_x = (template >> 14) & 0x3ff;
                let src_z = (template >> 3) & 0x7ff;
                let packed = (((src_x >> 3) << 8) | (src_z >> 3)) as u32;
                let Some(Some(land)) = by_square.get(&packed) else {
                    environments.push(None);
                    continue;
                };
                let decoded = terrain
                    .read_region(
                        land,
                        RegionCopy {
                            level,
                            tile_x: (cx * 8) as i32,
                            tile_z: (cz * 8) as i32,
                            src_level: ((template >> 24) & 3) as usize,
                            src_chunk_x: src_x,
                            src_chunk_z: src_z,
                            rotation: (template >> 1) & 3,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("region template {template:#x}: {e:?}"))?;
                terrain = decoded.state;
                environments.push(Some(Environment {
                    consumed: decoded.consumed,
                    bytes: decoded.environment,
                }));
            }
        }
    }
    // Once every chunk is read, each level of a chunk nothing is copied
    // into is cleared (that level only; a chunk whose square is absent keeps
    // what it has).
    for level in 0..4 {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                if layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz] == -1 {
                    terrain
                        .clear_landscape_level(level, (cx * 8) as i32, (cz * 8) as i32, 8, 8)
                        .map_err(|e| anyhow::anyhow!("clear region template: {e:?}"))?;
                }
            }
        }
    }
    Ok(Inputs {
        terrain,
        environments,
        lands,
    })
}
