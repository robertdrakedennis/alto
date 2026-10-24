//! Floor tile shapes, triangle connectivity and perimeter blending tables.

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

/// Overlay triangle count per shape, simple path.
pub static OVERLAY_TRIS_SIMPLE: [i32; 15] = [2, 1, 1, 1, 2, 2, 2, 1, 3, 3, 3, 2, 0, 4, 0];

/// Underlay triangle count per shape, simple path.
pub static UNDERLAY_TRIS_SIMPLE: [i32; 15] = [0, 1, 2, 2, 1, 1, 2, 3, 1, 3, 3, 4, 2, 0, 4];

/// Overlay count, split (non-blend-overlay) path.
pub static OVERLAY_TRIS_SPLIT: [i32; 13] = [4, 2, 1, 1, 2, 2, 3, 1, 3, 3, 3, 2, 0];

/// Underlay count, split path.
pub static UNDERLAY_TRIS_SPLIT: [i32; 13] = [0, 2, 2, 2, 1, 1, 3, 3, 1, 3, 3, 4, 4];

/// Overlay count, blend-overlay path.
pub static OVERLAY_TRIS_BLEND: [i32; 13] = [4, 2, 1, 1, 2, 2, 3, 1, 3, 3, 3, 2, 0];

/// Underlay count, blend-overlay path.
pub static UNDERLAY_TRIS_BLEND: [i32; 13] = [0, 4, 3, 3, 1, 1, 3, 5, 1, 5, 3, 6, 4];

/// `TILE_POINT_X`: the 13 tile points, fine units.
pub static TILE_POINT_X: [i32; 13] = [0, 256, 512, 512, 512, 256, 0, 0, 128, 256, 128, 384, 256];

/// `TILE_POINT_Z`.
pub static TILE_POINT_Z: [i32; 13] = [0, 0, 0, 256, 512, 512, 512, 256, 256, 384, 128, 128, 256];

/// Point belongs to the overlay part.
pub static OVERLAY_POINT: [[bool; 13]; 15] = [
    [
        true, true, true, true, true, true, true, true, true, true, true, true, true,
    ],
    [
        true, true, true, false, false, false, true, true, false, false, false, false, true,
    ],
    [
        true, false, false, false, false, true, true, true, false, false, false, false, false,
    ],
    [
        false, false, true, true, true, true, false, false, false, false, false, false, false,
    ],
    [
        true, true, true, true, true, true, false, false, false, false, false, false, false,
    ],
    [
        true, true, true, false, false, true, true, true, false, false, false, false, false,
    ],
    [
        true, true, false, false, false, true, true, true, false, false, false, false, true,
    ],
    [
        true, true, false, false, false, false, false, true, false, false, false, false, false,
    ],
    [
        false, true, true, true, true, true, true, true, false, false, false, false, false,
    ],
    [
        true, false, false, false, true, true, true, true, true, true, false, false, false,
    ],
    [
        true, true, true, true, true, false, false, false, true, true, false, false, false,
    ],
    [
        true, true, true, false, false, false, false, false, false, false, true, true, false,
    ],
    [
        false, false, false, false, false, false, false, false, false, false, false, false, false,
    ],
    [
        true, true, true, true, true, true, true, true, true, true, true, true, true,
    ],
    [
        false, false, false, false, false, false, false, false, false, false, false, false, false,
    ],
];

/// Point belongs to the underlay part.
pub static UNDERLAY_POINT: [[bool; 13]; 15] = [
    [
        false, false, false, false, false, false, false, false, false, false, false, false, false,
    ],
    [
        false, false, true, true, true, true, true, false, false, false, false, false, true,
    ],
    [
        true, true, true, true, true, true, false, false, false, false, false, false, false,
    ],
    [
        true, true, true, false, false, true, true, true, false, false, false, false, false,
    ],
    [
        true, false, false, false, false, true, true, true, false, false, false, false, false,
    ],
    [
        false, false, true, true, true, true, false, false, false, false, false, false, false,
    ],
    [
        false, true, true, true, true, true, false, false, false, false, false, false, true,
    ],
    [
        false, true, true, true, true, true, true, true, false, false, false, false, true,
    ],
    [
        true, true, false, false, false, false, false, true, false, false, false, false, false,
    ],
    [
        true, true, true, true, true, false, false, false, true, true, false, false, false,
    ],
    [
        true, false, false, false, true, true, true, true, true, true, false, false, false,
    ],
    [
        true, false, true, true, true, true, true, true, false, false, true, true, false,
    ],
    [
        true, true, true, true, true, true, true, true, true, true, true, true, true,
    ],
    [
        false, false, false, false, false, false, false, false, false, false, false, false, false,
    ],
    [
        true, true, true, true, true, true, true, true, true, true, true, true, true,
    ],
];

/// `TILE_SHAPE_TRIANGLES_A1` (simple path, vertex A per triangle).
pub static SIMPLE_A: [&[i32]; 15] = [
    &[0, 2],
    &[0, 2],
    &[0, 0, 2],
    &[2, 0, 0],
    &[0, 2, 0],
    &[0, 0, 2],
    &[0, 5, 1, 4],
    &[0, 4, 4, 4],
    &[4, 4, 4, 0],
    &[6, 6, 6, 2, 2, 2],
    &[2, 2, 2, 6, 6, 6],
    &[0, 11, 6, 6, 6, 4],
    &[0, 2],
    &[0, 4, 4, 4],
    &[0, 4, 4, 4],
];

/// `TILE_SHAPE_TRIANGLES_A2`.
pub static SIMPLE_B: [&[i32]; 15] = [
    &[2, 4],
    &[2, 4],
    &[5, 2, 4],
    &[4, 5, 2],
    &[2, 4, 5],
    &[5, 2, 4],
    &[1, 6, 2, 5],
    &[1, 6, 7, 1],
    &[6, 7, 1, 1],
    &[0, 8, 9, 8, 9, 4],
    &[8, 9, 4, 0, 8, 9],
    &[2, 10, 0, 10, 11, 11],
    &[2, 4],
    &[1, 6, 7, 1],
    &[1, 6, 7, 1],
];

/// `TILE_SHAPE_TRIANGLES_A3`.
pub static SIMPLE_C: [&[i32]; 15] = [
    &[6, 6],
    &[6, 6],
    &[6, 5, 5],
    &[5, 6, 5],
    &[5, 5, 6],
    &[6, 5, 5],
    &[5, 0, 4, 1],
    &[7, 7, 1, 2],
    &[7, 1, 2, 7],
    &[8, 9, 4, 0, 8, 9],
    &[0, 8, 9, 8, 9, 4],
    &[11, 0, 10, 11, 4, 2],
    &[6, 6],
    &[7, 7, 1, 2],
    &[7, 7, 1, 2],
];

/// Edge blends when the tile's overlay does NOT blend.
pub static EDGE_MASK_NOBLEND: [[bool; 4]; 13] = [
    [false, false, false, false],
    [false, false, false, false],
    [false, false, true, false],
    [false, false, true, false],
    [false, false, true, false],
    [false, false, true, false],
    [true, false, true, false],
    [true, false, false, true],
    [true, false, false, true],
    [false, false, false, false],
    [false, false, false, false],
    [false, false, false, false],
    [false, false, false, false],
];

/// Edge blends when the tile's overlay blends.
pub static EDGE_MASK_BLEND: [[bool; 4]; 13] = [
    [false, false, false, false],
    [false, true, true, false],
    [true, false, true, false],
    [true, false, true, false],
    [false, false, true, false],
    [false, false, true, false],
    [true, false, true, false],
    [true, false, false, true],
    [true, false, false, true],
    [true, true, false, false],
    [false, false, false, false],
    [false, true, false, true],
    [false, false, false, false],
];

/// Split-path per-edge triangle index (`-1` = none).
pub static SPLIT_EDGE_TRI: [[i32; 4]; 13] = [
    [0, 1, 2, 3],
    [1, 2, 3, 0],
    [1, 2, -1, 0],
    [2, 0, -1, 1],
    [0, 1, -1, 2],
    [1, 2, -1, 0],
    [-1, 4, -1, 1],
    [-1, 1, 3, -1],
    [-1, 0, 2, -1],
    [3, 5, 2, 0],
    [0, 2, 5, 3],
    [0, 2, 3, 5],
    [0, 1, 2, 3],
];

/// First corner point of each triangle, per shape, split path.
pub static SPLIT_A: [&[i32]; 13] = [
    &[0, 2, 4, 6],
    &[6, 0, 2, 4],
    &[6, 0, 2],
    &[2, 6, 0],
    &[0, 2, 6],
    &[6, 0, 2],
    &[5, 6, 0, 1, 2, 4],
    &[7, 2, 4, 4],
    &[2, 4, 4, 7],
    &[6, 6, 4, 0, 2, 2],
    &[0, 2, 2, 6, 6, 4],
    &[0, 2, 2, 4, 6, 6],
    &[0, 2, 4, 6],
];

/// Second corner point of each triangle, per shape, split path.
pub static SPLIT_B: [&[i32]; 13] = [
    &[2, 4, 6, 0],
    &[0, 2, 4, 6],
    &[0, 2, 4],
    &[4, 0, 2],
    &[2, 4, 0],
    &[0, 2, 4],
    &[6, 0, 1, 2, 4, 5],
    &[0, 4, 7, 6],
    &[4, 7, 6, 0],
    &[0, 8, 6, 2, 9, 4],
    &[2, 9, 4, 0, 8, 6],
    &[2, 11, 4, 6, 10, 0],
    &[2, 4, 6, 0],
];

/// Third (apex) point of each triangle, per shape, split path.
pub static SPLIT_C: [&[i32]; 13] = [
    &[12, 12, 12, 12],
    &[12, 12, 12, 12],
    &[5, 5, 5],
    &[5, 5, 5],
    &[5, 5, 5],
    &[5, 5, 5],
    &[12, 12, 12, 12, 12, 12],
    &[1, 1, 1, 7],
    &[1, 1, 7, 1],
    &[8, 9, 9, 8, 8, 9],
    &[8, 8, 9, 8, 9, 9],
    &[10, 10, 11, 11, 11, 10],
    &[12, 12, 12, 12],
];

/// Blend-overlay-path per-edge triangle index.
pub static BLEND_EDGE_TRI: [[i32; 4]; 13] = [
    [0, 1, 2, 3],
    [1, -1, -1, 0],
    [-1, 2, -1, 0],
    [-1, 0, -1, 2],
    [0, 1, -1, 2],
    [1, 2, -1, 0],
    [-1, 4, -1, 1],
    [-1, 3, 4, -1],
    [-1, 0, 2, -1],
    [-1, -1, 2, 0],
    [0, 2, 5, 3],
    [0, -1, 6, -1],
    [0, 1, 2, 3],
];

/// First corner point of each triangle, per shape, blend-overlay path.
pub static BLEND_A: [&[i32]; 13] = [
    &[0, 2, 4, 6],
    &[6, 0, 2, 3, 5, 3],
    &[6, 0, 2, 4],
    &[2, 5, 6, 1],
    &[0, 2, 6],
    &[6, 0, 2],
    &[5, 6, 0, 1, 2, 4],
    &[7, 7, 1, 2, 4, 6],
    &[2, 4, 4, 7],
    &[6, 6, 4, 0, 1, 1, 3, 3],
    &[0, 2, 2, 6, 6, 4],
    &[0, 2, 2, 3, 7, 0, 4, 3],
    &[0, 2, 4, 6],
];

/// Second corner point of each triangle, per shape, blend-overlay path.
pub static BLEND_B: [&[i32]; 13] = [
    &[2, 4, 6, 0],
    &[0, 2, 3, 5, 6, 4],
    &[0, 1, 4, 5],
    &[4, 6, 0, 2],
    &[2, 4, 0],
    &[0, 2, 4],
    &[6, 0, 1, 2, 4, 5],
    &[0, 1, 2, 4, 6, 7],
    &[4, 7, 6, 0],
    &[0, 8, 6, 1, 9, 2, 9, 4],
    &[2, 9, 4, 0, 8, 6],
    &[2, 11, 3, 7, 10, 10, 6, 6],
    &[2, 4, 6, 0],
];

/// Third (apex) point of each triangle, per shape, blend-overlay path.
pub static BLEND_C: [&[i32]; 13] = [
    &[12, 12, 12, 12],
    &[12, 12, 12, 12, 12, 5],
    &[5, 5, 1, 1],
    &[5, 1, 1, 5],
    &[5, 5, 5],
    &[5, 5, 5],
    &[12, 12, 12, 12, 12, 12],
    &[1, 12, 12, 12, 12, 12],
    &[1, 1, 7, 1],
    &[8, 9, 9, 8, 8, 3, 1, 9],
    &[8, 8, 9, 8, 9, 9],
    &[10, 10, 11, 11, 11, 7, 3, 7],
    &[12, 12, 12, 12],
];
