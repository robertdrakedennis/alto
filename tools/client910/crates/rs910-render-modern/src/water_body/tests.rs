//! Tests of the water's CPU half against the map files (pack): the tiles'
//! surface and bed heights, and the evidence for the patches' rotation sense.
use super::*;

/// Squares 48..=51 x 49..=51: the Lumbridge river, the swamp coast, the
/// lake west of Lumbridge, as one scene.
fn lumbridge_map(pack: &crate::cache::Pack) -> WaterMap {
    WaterMap::load(pack, [48 * 64, 49 * 64], [4 * 64 - 1, 3 * 64 - 1])
}

/// The data behind the depth: a water tile's two height bytes are the classic
/// land height (the surface) and the classic underwater land height (the
/// bed; 0 where the classic map has none), on every water tile of those squares.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn water_tiles_carry_the_classic_surface_and_bed_heights() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    fn land(b: &[u8], levels: usize) -> Vec<i32> {
        let mut r = rs910_core::reader::Reader::new(b);
        let mut heights = Vec::new();
        for _ in 0..levels * 64 * 64 {
            let op = r.g1().unwrap();
            if op & 1 != 0 {
                r.g1().unwrap();
                r.gsmart1or2().unwrap();
            }
            if op & 2 != 0 {
                r.g1().unwrap();
            }
            if op & 4 != 0 {
                r.gsmart1or2().unwrap();
            }
            heights.push(if op & 8 != 0 {
                i32::from(r.g1().unwrap())
            } else {
                -1
            });
        }
        heights
    }
    let (mut water, mut beds) = (0, 0);
    for sx in 48..=51_u32 {
        for sz in 49..=51_u32 {
            let group = map_group(sx, sz);
            let files = pack.read_group(MAP_ARCHIVE, group).unwrap();
            let map = WaterMap::load(&pack, [sx as i32 * 64, sz as i32 * 64], [63, 63]);
            let surface = land(&files[&3], 4);
            let bed = files.get(&4).map(|b| land(b, 1));
            for x in 0..64 {
                for z in 0..64 {
                    let t = map.tiles[x * 64 + z];
                    if !t.water {
                        continue;
                    }
                    water += 1;
                    assert_eq!(
                        t.surface.map(i32::from),
                        Some(surface[x * 64 + z]),
                        "{sx},{sz} {x},{z}"
                    );
                    let classic_bed = bed.as_ref().map_or(-1, |b| b[x * 64 + z]);
                    assert_eq!(i32::from(t.bed), classic_bed.max(0), "{sx},{sz} {x},{z}");
                    beds += usize::from(classic_bed > 0);
                }
            }
        }
    }
    assert!(
        water > 1000 && beds > 500,
        "{water} water tiles, {beds} with a bed"
    );
}

/// The records' boxes follow the water: with [`ROTATION_SENSE`] they hold
/// more of the squares' water tiles, and fewer dry tiles, than with the
/// other sense; the Lumbridge river's flow points south (-Z); water outside
/// every record is type 0.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn patch_rotation_sense_covers_the_river() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let map = lumbridge_map(&pack);
    assert!(map.patches.len() > 20, "{}", map.patches.len());
    let score = |patches: &[Patch]| {
        let (mut wet, mut dry) = (0, 0);
        for x in 0..map.dims[0] as i32 - 1 {
            for z in 0..map.dims[1] as i32 - 1 {
                let (cx, cz) = (x as f32 * 512.0 + 256.0, z as f32 * 512.0 + 256.0);
                if patches.iter().any(|p| p.contains(cx, cz)) {
                    if map.is_water_tile(x, z) {
                        wet += 1;
                    } else {
                        dry += 1;
                    }
                }
            }
        }
        (wet, dry)
    };
    let other: Vec<Patch> = map
        .patches
        .iter()
        .map(|p| Patch {
            cos_sin: [p.cos_sin[0], -p.cos_sin[1]],
            ..*p
        })
        .collect();
    let (wet, dry) = score(&map.patches);
    let (wet_other, dry_other) = score(&other);
    eprintln!("sense {ROTATION_SENSE}: {wet} wet / {dry} dry; other: {wet_other} / {dry_other}");
    assert!(wet > wet_other && dry < dry_other);
    // The river through square 50,50 (scene tiles 128..192 x 64..128).
    let (flow, water_type) = map.flow_at((128 + 36) as f32 * 512.0, (64 + 50) as f32 * 512.0);
    assert!(flow[1] < -1.0 && water_type == 0, "{flow:?} {water_type}");
    assert_eq!(map.default_type, 0);
}
