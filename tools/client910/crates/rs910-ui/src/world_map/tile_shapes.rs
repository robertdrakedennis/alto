//! World-map overlay shape masks and rotations.

/// The rotation used for shapes 9-11.
pub fn shape_rotation(rotation: i32, shape: i32) -> i32 {
    match shape {
        9 => (rotation + 1) & 3,
        10 | 11 => (rotation + 3) & 3,
        _ => rotation,
    }
}

/// The shape table row (1-based).
pub fn shape_table(shape: i32) -> i32 {
    match shape {
        9 | 10 => 1,
        11 => 8,
        _ => shape,
    }
}

/// Eight shapes in four rotations as `size * size` masks. The overlay painter
/// only tests a texel for zero, so covered texels are `0xFF`.
pub fn tile_shapes(size: i32) -> Vec<[Vec<u8>; 4]> {
    let n = size.max(0) as usize;
    let s = size;
    let mut out: Vec<[Vec<u8>; 4]> = (0..8)
        .map(|_| std::array::from_fn(|_| vec![0; n * n]))
        .collect();
    // Each mask walks rows `r` (ascending or descending) and columns `c`
    // (ascending or descending), writing sequentially.
    let fill = |rows_desc: bool, cols_desc: bool, covered: &dyn Fn(i32, i32) -> bool| -> Vec<u8> {
        let mut v = vec![0u8; n * n];
        let mut at = 0;
        for ri in 0..s {
            let r = if rows_desc { s - 1 - ri } else { ri };
            for ci in 0..s {
                let c = if cols_desc { s - 1 - ci } else { ci };
                if covered(r, c) {
                    v[at] = 0xFF;
                }
                at += 1;
            }
        }
        v
    };
    let half = s / 2;
    // Shape 0 in its four rotations.
    out[0][0] = fill(false, false, &|r, c| c <= r);
    out[0][1] = fill(true, false, &|r, c| c <= r);
    out[0][2] = fill(false, false, &|r, c| c >= r);
    out[0][3] = fill(true, false, &|r, c| c >= r);
    // Shape 1 in its four rotations.
    out[1][0] = fill(true, false, &|r, c| c <= r >> 1);
    out[1][1] = fill(false, false, &|r, c| c >= r << 1);
    out[1][2] = fill(false, true, &|r, c| c <= r >> 1);
    out[1][3] = fill(true, true, &|r, c| c >= r << 1);
    // Shape 2 in its four rotations.
    out[2][0] = fill(true, true, &|r, c| c <= r >> 1);
    out[2][1] = fill(true, false, &|r, c| c >= r << 1);
    out[2][2] = fill(false, false, &|r, c| c <= r >> 1);
    out[2][3] = fill(false, true, &|r, c| c >= r << 1);
    // Shape 3 in its four rotations.
    out[3][0] = fill(true, false, &|r, c| c >= r >> 1);
    out[3][1] = fill(false, false, &|r, c| c <= r << 1);
    out[3][2] = fill(false, true, &|r, c| c >= r >> 1);
    out[3][3] = fill(true, true, &|r, c| c <= r << 1);
    // Shape 4 in its four rotations.
    out[4][0] = fill(true, true, &|r, c| c >= r >> 1);
    out[4][1] = fill(true, false, &|r, c| c <= r << 1);
    out[4][2] = fill(false, false, &|r, c| c >= r >> 1);
    out[4][3] = fill(false, true, &|r, c| c <= r << 1);
    // Shape 5 in its four rotations.
    out[5][0] = fill(false, false, &|_, c| c <= half);
    out[5][1] = fill(false, false, &|r, _| r <= half);
    out[5][2] = fill(false, false, &|_, c| c >= half);
    out[5][3] = fill(false, false, &|r, _| r >= half);
    // Shape 6 in its four rotations.
    out[6][0] = fill(false, false, &|r, c| c <= r - half);
    out[6][1] = fill(true, false, &|r, c| c <= r - half);
    out[6][2] = fill(true, true, &|r, c| c <= r - half);
    out[6][3] = fill(false, true, &|r, c| c <= r - half);
    // Shape 7 in its four rotations.
    out[7][0] = fill(false, false, &|r, c| c >= r - half);
    out[7][1] = fill(true, false, &|r, c| c >= r - half);
    out[7][2] = fill(true, true, &|r, c| c >= r - half);
    out[7][3] = fill(false, true, &|r, c| c >= r - half);
    out
}
