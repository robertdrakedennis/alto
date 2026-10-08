use super::*;
use rs910_core::perlin::{interpolate, noise, smooth_noise};

#[test]
fn smart_reads_pick_one_or_two_bytes() {
    // Smart: single byte when first < 128, else g2 - 32768.
    let mut one = Reader::new(&[0x05]);
    assert_eq!(one.gsmart1or2().unwrap(), 5);
    let mut two = Reader::new(&[0x80, 0x05]);
    assert_eq!(two.gsmart1or2().unwrap(), 5);
    let mut max = Reader::new(&[0xFF, 0xFF]);
    assert_eq!(max.gsmart1or2().unwrap(), 32767);
    // Extended smart accumulates 32767 continuations.
    let mut ext = Reader::new(&[0xFF, 0xFF, 0x03]);
    assert_eq!(ext.gextended1or2().unwrap(), 32770);
    // g1b is signed.
    let mut neg = Reader::new(&[0xFF]);
    assert_eq!(neg.g1b().unwrap(), -1);
    // Truncation, not panic.
    let mut empty = Reader::new(&[]);
    assert!(empty.g1().is_err());
    assert!(empty.gsmart1or2().is_err());
    assert!(empty.gextended1or2().is_err());
}

#[test]
fn bit_reads_cross_byte_boundaries() {
    // 0b10110001 0b01100100: bits(3)=0b101, bits(5)=0b10001, bits(8)=0b01100100.
    let data = [0b1011_0001_u8, 0b0110_0100];
    let mut bits = BitReader::new(&data, 0).unwrap();
    assert_eq!(bits.gbit(3).unwrap(), 0b101);
    assert_eq!(bits.gbit(5).unwrap(), 0b1_0001);
    assert_eq!(bits.gbit(8).unwrap(), 0b0110_0100);
    assert_eq!(bits.byte_pos(), 2);
    assert!(bits.gbit(0).is_err());
    assert!(bits.gbit(33).is_err());
    assert!(BitReader::new(&data, 3).is_err());
}

#[test]
fn noise_matches_client_reference_vectors() {
    // Independent integer-math vectors (verified against a from-spec
    // Python port of the noise and smooth-noise functions).
    assert_eq!(noise(0, 0), 65);
    assert_eq!(noise(5, 9), 147);
    assert_eq!(noise(-3, 7), 214);
    assert_eq!(smooth_noise(0, 0), 128);
    assert_eq!(smooth_noise(10, 20), 78);
    // interpolate(100, 200, 0, 4) only touches cos[0] == 16384 (exact in
    // every libm), so it is bit-exact: w = 24576, 62 + 75.
    assert_eq!(interpolate(100, 200, 0, 4), 137);
}

#[test]
fn landscape_decodes_explicit_height_and_flags() {
    // Tile (0,0,0): opcode bit1|bit2|bit8, shape 0, overlay 5, flags 1,
    // height byte 2 -> -2*8<<2 = -64. Every other tile: opcode 0
    // (perlin fallback on level 0, prev - 960 above).
    let mut land = vec![0x0B, 0x00, 0x05, 0x01, 0x02];
    land.extend(std::iter::repeat_n(0x00, 4 * 64 * 64 - 1));
    let square = Landscape::decode(&land, 3200, 3200).unwrap();

    let tile = square.tile(0, 0, 0).unwrap();
    assert_eq!(tile.height, -64);
    assert_eq!(tile.flags, 1);
    assert_eq!(tile.overlay_id, Some(5));
    assert_eq!(tile.overlay_shape, 0);
    assert_eq!(tile.overlay_rot, 0);
    assert_eq!(tile.underlay_id, None);
    // Level 1 inherits without a height byte: prev - 960.
    assert_eq!(square.tile(1, 0, 0).unwrap().height, -64 - 960);
    // Untouched tiles fall back to perlin-derived ground (finite).
    assert!(square.tile(0, 63, 63).unwrap().height < 0);
    // Truncated streams error instead of panicking.
    assert!(Landscape::decode(&[0x0B], 3200, 3200).is_err());
}

#[test]
fn landscape_opcode_vectors_cover_each_bit() {
    // Five crafted tiles back to back (decode order is level/x/z, so
    // these land at (0,0,0..4)), then opcode 0 for the rest:
    // - t0 overlay-only (0x01): shape 5, rot 2 -> byte 5*4+2 = 0x16,
    //   overlay smart 10 (single byte).
    // - t1 underlay-only (0x04): underlay smart 300 -> 300+32768 =
    //   0x812C, two bytes.
    // - t2 flags-only (0x02): flags byte 0xFE.
    // - t3 height-only (0x08): byte 3 -> -(3*8<<2) = -96 on level 0.
    // - t4 all-set (0x0F): shape 33 (> 31, proving no `& 0x1F` mask on
    //   tiles), rot 3 -> byte 33*4+3 = 135; overlay 7; flags 5;
    //   underlay 11; height byte 2 -> -64.
    let mut land = vec![
        0x01, 0x16, 0x0A, //
        0x04, 0x81, 0x2C, //
        0x02, 0xFE, //
        0x08, 0x03, //
        0x0F, 135, 0x07, 0x05, 0x0B, 0x02,
    ];
    land.extend(std::iter::repeat_n(0x00, 4 * 64 * 64 - 5));
    let square = Landscape::decode(&land, 3200, 3200).unwrap();

    let t0 = square.tile(0, 0, 0).unwrap();
    assert_eq!(t0.overlay_id, Some(10));
    assert_eq!(t0.overlay_shape, 5);
    assert_eq!(t0.overlay_rot, 2);
    assert_eq!(t0.underlay_id, None);
    assert_eq!(t0.flags, 0);
    assert!(t0.height < 0, "overlay-only tile keeps perlin ground");

    let t1 = square.tile(0, 0, 1).unwrap();
    assert_eq!(t1.underlay_id, Some(300));
    assert_eq!(t1.overlay_id, None);
    assert_eq!(t1.flags, 0);

    let t2 = square.tile(0, 0, 2).unwrap();
    assert_eq!(t2.flags, 0xFE);
    assert_eq!(t2.overlay_id, None);
    assert_eq!(t2.underlay_id, None);

    let t3 = square.tile(0, 0, 3).unwrap();
    assert_eq!(t3.height, -((3 * 8) << 2));
    assert_eq!(t3.overlay_id, None);
    assert_eq!(t3.underlay_id, None);

    let t4 = square.tile(0, 0, 4).unwrap();
    assert_eq!(t4.overlay_id, Some(7));
    assert_eq!(t4.overlay_shape, 33);
    assert_eq!(t4.overlay_rot, 3);
    assert_eq!(t4.flags, 5);
    assert_eq!(t4.underlay_id, Some(11));
    assert_eq!(t4.height, -64);
}

#[test]
fn rotation_matches_mapcoordutil() {
    // Vectors for (x, z) = (2, 5): rot0 identity, rot1 (z, 7-x), rot2 (7-x, 7-z), rot3
    // (7-z, x).
    assert_eq!((rotate_x(2, 5, 0), rotate_z(2, 5, 0)), (2, 5));
    assert_eq!((rotate_x(2, 5, 1), rotate_z(2, 5, 1)), (5, 5));
    assert_eq!((rotate_x(2, 5, 2), rotate_z(2, 5, 2)), (5, 2));
    assert_eq!((rotate_x(2, 5, 3), rotate_z(2, 5, 3)), (2, 2));
    // Rotation masks to two bits (`rotation & 0x3`).
    assert_eq!((rotate_x(2, 5, 5), rotate_z(2, 5, 5)), (5, 5));
    // Loc angle: `angle + rotation & 0x3`.
    assert_eq!(rotate_loc_angle(3, 1), 0);
    assert_eq!(rotate_loc_angle(2, 2), 0);
    assert_eq!(rotate_loc_angle(1, 2), 3);
    assert_eq!(rotate_loc_angle(0, 0), 0);
    // Tile overlay rotation: `(region_rot + shape_byte) & 0x3`; normal loads pass
    // region_rot 0.
    assert_eq!(overlay_rotation(0, 135), 3);
    assert_eq!(overlay_rotation(2, 135), 1);
}

#[test]
fn region_groups_match_lumbridge_block() {
    // Center mapsquare (50, 50) + radius 1 -> the 3x3 LUMBRIDGE_GROUPS set,
    // ascending (`group = mx | mz << 7`, mz outer / mx inner).
    let lumbridge: Vec<u16> = LUMBRIDGE_GROUPS.iter().map(|group| *group as u16).collect();
    assert_eq!(region_groups(50, 50, 1).unwrap(), lumbridge);
    // Radius 0 is the center square alone; corners bound the 14-bit range.
    assert_eq!(region_groups(50, 50, 0).unwrap(), vec![6450]);
    assert_eq!(region_groups(0, 0, 0).unwrap(), vec![0]);
    assert_eq!(region_groups(127, 127, 0).unwrap(), vec![16383]);
    // Round-trips group_base: every emitted group maps back into the block.
    for group in region_groups(50, 50, 1).unwrap() {
        let (gx, gz) = group_base(u32::from(group));
        let (mx, mz) = (gx / 64, gz / 64);
        assert!((49..=51).contains(&mx), "mx {mx} for group {group}");
        assert!((49..=51).contains(&mz), "mz {mz} for group {group}");
    }
    // Out-of-range blocks are rejected, never wrapped.
    assert!(region_groups(200, 50, 1).is_err());
    assert!(region_groups(50, 50, 100).is_err());
    assert!(region_groups(127, 127, 1).is_err());
    assert!(region_groups(1, 1, 2).is_err());
}

#[test]
fn locs_decode_id_coord_shape_angle() {
    // loc delta 10 -> id 9; coord delta 67 -> packed 66 = level 0, x 1,
    // z 2; info 0 -> shape 0 angle 0; terminators close both loops.
    let loc = [10, 67, 0x00, 0x00, 0x00];
    let spawns = decode_locs(&loc, 3200, 3200).unwrap();
    assert_eq!(
        spawns,
        vec![LocSpawn {
            id: 9,
            level: 0,
            x: 3201,
            z: 3202,
            shape: 0,
            angle: 0,
            srt: None,
        }]
    );
}

#[test]
fn locs_skip_scale_rot_trans_payload() {
    // info 0x80 | shape 10 << 2 | angle 2, srt flags 0x01 (8-byte quat).
    let loc = [10, 67, 0xAA, 0x01, 1, 2, 3, 4, 5, 6, 7, 8, 0x00, 0x00];
    let spawns = decode_locs(&loc, 3200, 3200).unwrap();
    assert_eq!(spawns.len(), 1);
    assert_eq!(spawns[0].shape, 10);
    assert_eq!(spawns[0].angle, 2);
    let srt = spawns[0].srt.expect("srt decoded");
    assert_eq!(
        srt.rot,
        [
            258.0 / 32768.0,
            772.0 / 32768.0,
            1286.0 / 32768.0,
            1800.0 / 32768.0
        ]
    );
    assert_eq!(srt.trans, [0.0; 3]);
    assert_eq!(srt.scale, [1.0; 3]);
    assert!(decode_locs(&[10, 67, 0x80], 3200, 3200).is_err());
}

fn bridge_world(bridged: &[(i32, i32)]) -> World {
    // 4x4 block at (3200, 3200); all tiles EMPTY except the listed
    // (x, z) columns whose LEVEL-1 tile carries flags 0x2 (the bridge
    // bit). LocSpawn.level stays the wire level; resolved_level reads
    // the level-1 column flag.
    let extent = 4_usize;
    let mut tiles = vec![Tile::EMPTY; 4 * extent * extent];
    for (x, z) in bridged {
        let lx = (*x - 3200) as usize;
        let lz = (*z - 3200) as usize;
        let mut tile = Tile::EMPTY;
        tile.flags = 0x2;
        // Level 1: (level * extent + lx) * extent + lz.
        tiles[(extent + lx) * extent + lz] = tile;
    }
    World {
        base_x: 3200,
        base_z: 3200,
        extent,
        tiles,
        locs: Vec::new(),
        groups: Vec::new(),
    }
}

fn spawn(id: u32, level: u8, x: i32, z: i32) -> LocSpawn {
    LocSpawn {
        id,
        level,
        x,
        z,
        shape: 10,
        angle: 0,
        srt: None,
    }
}

#[test]
fn resolved_level_drops_on_bridged_columns() {
    let world = bridge_world(&[(3201, 3201)]);
    // Wire level 1 on a bridged column drops to 0 (TS actualLevel).
    assert_eq!(resolved_level(&world, &spawn(1, 1, 3201, 3201)), 0);
    // Wire level 2 on the same column drops to 1 (flag read at level 1
    // per CollisionManager.ts:185-189, not at the wire level).
    assert_eq!(resolved_level(&world, &spawn(1, 2, 3201, 3201)), 1);
    // Wire level 3 drops to 2 the same way.
    assert_eq!(resolved_level(&world, &spawn(1, 3, 3201, 3201)), 2);
    // Level 0 never underflows: bridged or not it stays 0 (TS would skip
    // actualLevel -1; the renderer clamps instead of dropping).
    assert_eq!(resolved_level(&world, &spawn(1, 0, 3201, 3201)), 0);
}

#[test]
fn resolved_level_keeps_wire_without_bridge_flag() {
    let world = bridge_world(&[(3201, 3201)]);
    // Same wire levels off the bridged column keep their level.
    assert_eq!(resolved_level(&world, &spawn(1, 1, 3202, 3202)), 1);
    assert_eq!(resolved_level(&world, &spawn(1, 2, 3202, 3202)), 2);
    assert_eq!(resolved_level(&world, &spawn(1, 0, 3202, 3202)), 0);
    // Outside the merged block there is no flag to read: wire kept.
    assert_eq!(resolved_level(&world, &spawn(1, 1, 9999, 9999)), 1);
    assert_eq!(resolved_level(&world, &spawn(1, 2, 9999, 9999)), 2);
    // Wire level itself is kept intact by decode: resolved_level is the
    // only place the drop happens (no decode change).
    let loc = spawn(7, 1, 3201, 3201);
    assert_eq!(loc.level, 1);
    assert_eq!(resolved_level(&world, &loc), 0);
}

/// Merges the full 3x3 Lumbridge block from disk.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn lumbridge_world_merges_3x3() {
    let world = load_lumbridge(&crate::test_support::require_pack("client.mapsv2.js5")).unwrap();
    assert_eq!((world.base_x, world.base_z), LUMBRIDGE_BASE);
    assert_eq!(world.extent, LUMBRIDGE_EXTENT);
    assert_eq!(world.tiles.len(), 4 * 192 * 192);
    assert!(!world.locs.is_empty());
    assert_eq!(world.groups.len(), LUMBRIDGE_GROUPS.len());
    assert!(world.groups.contains(&6450));

    let (level, x, z) = LUMBRIDGE_SPAWN;
    let spawn = world.tile(level, x, z).expect("spawn tile in world");
    assert!(spawn.height.abs() < 240_000);
    assert!(world.tile(0, 0, 0).is_none());
    assert!(world.tile(4, x, z).is_none());
}

/// Merges one mapsquare from disk; the world is exactly that square's
/// 64x64 box.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn load_groups_single_group_world_extent() {
    let world = load_groups(
        &crate::test_support::require_pack("client.mapsv2.js5"),
        &[6450],
    )
    .unwrap();
    assert_eq!((world.base_x, world.base_z), group_base(6450));
    assert_eq!((world.base_x, world.base_z), (3200, 3200));
    assert_eq!(world.extent, 64);
    assert_eq!(world.tiles.len(), 4 * 64 * 64);
    assert_eq!(world.groups, vec![6450]);
    // Spawn tile (3222, 3222) is local (22, 22) of mapsquare (50, 50).
    let spawn = world.tile(0, 3222, 3222).expect("spawn tile in world");
    assert!(spawn.height.abs() < 240_000);
    assert!(world.tile(0, 3199, 3200).is_none());
}
