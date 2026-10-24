use super::*;
use rs910_core::perlin::{interpolate, noise, smooth_noise};

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
