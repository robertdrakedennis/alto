//! Terrain height rules shared by the scene, the camera and the game state.

/// Fine units per tile edge, as a shift: a tile is `1 << TILE_SHIFT` units.
pub const TILE_SHIFT: i32 = 9;
/// The highest level a bridge tile can raise its heights to.
const TOP_LEVEL: i32 = 3;

/// Blends a tile's corner heights `[x0z0, x1z0, x0z1, x1z1]` at the fine
/// offset `(fx, fz)` inside a tile of `1 << shift` units.
pub fn blend_corners(corners: [i32; 4], fx: i32, fz: i32, shift: i32) -> i32 {
    let size = 1 << shift;
    let mix = |a: i32, b: i32, t: i32| {
        (size - t).wrapping_mul(a).wrapping_add(t.wrapping_mul(b)) >> shift
    };
    let [h00, h10, h01, h11] = corners;
    mix(mix(h00, h10, fx), mix(h01, h11, fx), fz)
}

/// The level whose heights a tile reads: a bridge tile takes the level above.
pub fn height_level(level: i32, bridge: bool) -> i32 {
    if level < TOP_LEVEL && bridge {
        level + 1
    } else {
        level
    }
}
