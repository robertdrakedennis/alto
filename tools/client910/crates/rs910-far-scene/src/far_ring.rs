//! Map squares, the ring around the camera focus and the classic window's
//! exclusion (design §1.3, §6.2 "`FarWorld`").
//!
//! The modern client builds the focus square, then rings `1..=r` outwards;
//! squares outside the ring's box are destroyed. Squares are addressed by
//! map-square coordinates; the
//! `mapsv2` group of square `(x, z)` is `x | z << 7`
//! (`rs910_config::nxt::map_group`).

/// Tiles per map square side.
pub const SQUARE_TILES: i32 = 64;

/// Fine units per map square side.
pub const SQUARE_UNITS: i32 = SQUARE_TILES * 512;

/// The `mapsv2` group space: `x < 128`, `z < 256`.
pub const WORLD_SQUARES: [i32; 2] = [128, 256];

/// The first underground square row: the surface world's squares are below
/// it (dungeons sit 6,400 tiles north of the surface; design §6.2
/// "Instances": the far scene is surface-only until a world-area bound
/// exists).
pub const UNDERGROUND_SQUARE_Z: i32 = 100;

/// A map square `(x, z)`.
pub type SquareId = (i32, i32);

/// A rectangle of absolute tiles, `[x0, x1) x [z0, z1)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TileRect {
    pub x0: i32,
    pub z0: i32,
    pub x1: i32,
    pub z1: i32,
}

impl TileRect {
    /// The window of `tiles` tiles at `base` (the classic scene's
    /// `floor_base` and floor size).
    #[must_use]
    pub fn at(base: [i32; 2], tiles: [usize; 2]) -> Self {
        Self {
            x0: base[0],
            z0: base[1],
            x1: base[0] + tiles[0] as i32,
            z1: base[1] + tiles[1] as i32,
        }
    }

    /// Square `sq`'s tiles.
    #[must_use]
    pub fn of_square(sq: SquareId) -> Self {
        Self {
            x0: sq.0 * SQUARE_TILES,
            z0: sq.1 * SQUARE_TILES,
            x1: (sq.0 + 1) * SQUARE_TILES,
            z1: (sq.1 + 1) * SQUARE_TILES,
        }
    }

    #[must_use]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.x0 && x < self.x1 && z >= self.z0 && z < self.z1
    }

    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.z0 < other.z1 && other.z0 < self.z1
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.z1 <= self.z0
    }
}

/// Whether `sq` is a square of the `mapsv2` group space.
#[must_use]
pub fn in_world(sq: SquareId) -> bool {
    sq.0 >= 0 && sq.1 >= 0 && sq.0 < WORLD_SQUARES[0] && sq.1 < WORLD_SQUARES[1]
}

/// Whether `sq` is on the surface (below [`UNDERGROUND_SQUARE_Z`]).
#[must_use]
pub fn on_surface(sq: SquareId) -> bool {
    in_world(sq) && sq.1 < UNDERGROUND_SQUARE_Z
}

/// The square holding absolute fine position `focus` (x, z).
#[must_use]
pub fn square_of(focus: [i32; 2]) -> SquareId {
    (
        focus[0].div_euclid(SQUARE_UNITS),
        focus[1].div_euclid(SQUARE_UNITS),
    )
}

/// The 2D distance, fine units, from `focus` (absolute x, z) to square
/// `sq`'s box (0 inside it): what the modern client maps to a square's
/// distance class.
#[must_use]
pub fn distance_to_square(focus: [i32; 2], sq: SquareId) -> i32 {
    let axis = |p: i32, lo: i32| {
        let hi = lo + SQUARE_UNITS;
        if p < lo {
            lo - p
        } else if p > hi {
            p - hi
        } else {
            0
        }
    };
    let dx = f64::from(axis(focus[0], sq.0 * SQUARE_UNITS));
    let dz = f64::from(axis(focus[1], sq.1 * SQUARE_UNITS));
    dx.hypot(dz).round() as i32
}

/// The ring of radius `radius` around the square of `focus`, nearest first:
/// the focus square, then each ring `1..=radius` outwards (the modern client's order), the
/// squares of one ring by their distance to the focus (ties by z, then x).
/// Squares outside the `mapsv2` group space are left out.
#[must_use]
pub fn ring(focus: [i32; 2], radius: i32) -> Vec<SquareId> {
    let centre = square_of(focus);
    let mut out = Vec::new();
    for r in 0..=radius.max(0) {
        let mut ring: Vec<SquareId> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) == r {
                    let sq = (centre.0 + dx, centre.1 + dz);
                    if in_world(sq) {
                        ring.push(sq);
                    }
                }
            }
        }
        ring.sort_by_key(|&sq| (distance_to_square(focus, sq), sq.1, sq.0));
        out.extend(ring);
    }
    out
}

/// Whether `sq` is within `radius` squares (Chebyshev) of `centre`.
#[must_use]
pub fn within(centre: SquareId, sq: SquareId, radius: i32) -> bool {
    (sq.0 - centre.0).abs().max((sq.1 - centre.1).abs()) <= radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_nearest_first_and_complete() {
        // Lumbridge, in the east half of square (50, 50).
        let focus = [3250 * 512 + 256, 3218 * 512 + 256];
        assert_eq!(square_of(focus), (50, 50));
        let one = ring(focus, 1);
        assert_eq!(one.len(), 9);
        assert_eq!(one[0], (50, 50));
        // The nearer east neighbour comes before the west one.
        let east = one.iter().position(|&s| s == (51, 50)).unwrap();
        let west = one.iter().position(|&s| s == (49, 50)).unwrap();
        assert!(east < west);
        let two = ring(focus, 2);
        assert_eq!(two.len(), 25);
        assert_eq!(
            &two[..9].iter().collect::<std::collections::BTreeSet<_>>(),
            &one.iter().collect::<std::collections::BTreeSet<_>>()
        );
        // The group space's edge.
        assert_eq!(ring([0, 0], 1).len(), 4);
    }

    #[test]
    fn distances_and_rects() {
        let focus = [50 * SQUARE_UNITS + 100, 50 * SQUARE_UNITS + 100];
        assert_eq!(distance_to_square(focus, (50, 50)), 0);
        assert_eq!(distance_to_square(focus, (49, 50)), 100);
        let window = TileRect::at([3160, 3160], [104, 104]);
        assert!(window.contains(3160, 3263) && !window.contains(3264, 3200));
        assert!(window.intersects(&TileRect::of_square((50, 50))));
        assert!(!window.intersects(&TileRect::of_square((51, 51))));
        assert!(on_surface((50, 50)) && !on_surface((50, 150)));
    }
}
