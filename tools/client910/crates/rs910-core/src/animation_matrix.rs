//! 4x4 animation matrices. Cofactor expressions keep the reference client's
//! float evaluation order.
pub const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

/// Euler extraction uses double transcendental functions.
pub fn euler(a: &[f32; 16]) -> [f32; 3] {
    let x = -(a[6] as f64).asin() as f32;
    if (x as f64).cos().abs() > 0.005 {
        [
            x,
            (a[2] as f64).atan2(a[10] as f64) as f32,
            (a[4] as f64).atan2(a[5] as f64) as f32,
        ]
    } else {
        [x, (a[1] as f64).atan2(a[0] as f64) as f32, 0.]
    }
}
/// Rotation from (yaw, pitch, roll), including quaternion operation order.
pub fn rotation(a: f32, b: f32, c: f32) -> [f32; 16] {
    let s0 = (a as f64 / 2.).sin();
    let c0 = (a as f64 / 2.).cos();
    let s1 = (b as f64 / 2.).sin();
    let c1 = (b as f64 / 2.).cos();
    let s2 = (c as f64 / 2.).sin();
    let c2 = (c as f64 / 2.).cos();
    let x = (s0 * c1 * s2 + c0 * s1 * c2) as f32;
    let y = (s0 * c1 * c2 - c0 * s1 * s2) as f32;
    let z = (c0 * c1 * s2 - s0 * s1 * c2) as f32;
    let w = (s0 * s1 * s2 + c0 * c1 * c2) as f32;
    let ww = w * w;
    let xw = x * w;
    let yw = y * w;
    let zw = z * w;
    let xx = x * x;
    let xy = x * y;
    let xz = x * z;
    let yy = y * y;
    let yz = y * z;
    let zz = z * z;
    let mut out = IDENTITY;
    out[0] = ww + xx - zz - yy;
    out[1] = zw + xy + xy + zw;
    out[2] = xz - yw - yw + xz;
    out[4] = xy - zw - zw + xy;
    out[5] = ww + yy - xx - zz;
    out[6] = xw + yz + yz + xw;
    out[8] = yw + xz + xz + yw;
    out[9] = yz - xw - xw + yz;
    out[10] = ww + zz - yy - xx;
    out
}
pub fn vector(a: &[f32; 16], p: [f32; 3], translate: bool) -> [f32; 3] {
    let mut out = std::array::from_fn(|i| a[8 + i] * p[2] + a[i] * p[0] + a[4 + i] * p[1]);
    if translate {
        for i in 0..3 {
            out[i] += a[12 + i];
        }
    }
    out
}
pub fn determinant(a: &[f32; 16]) -> f32 {
    a[3] * a[6] * a[9] * a[12]
        + (a[3] * a[5] * a[8] * a[14]
            + a[3] * a[4] * a[10] * a[13]
            + (a[2] * a[7] * a[8] * a[13]
                + a[2] * a[5] * a[11] * a[12]
                + (a[2] * a[4] * a[9] * a[15]
                    + a[1] * a[7] * a[10] * a[12]
                    + (a[1] * a[6] * a[8] * a[15]
                        + a[1] * a[4] * a[11] * a[14]
                        + (a[0] * a[7] * a[9] * a[14]
                            + a[0] * a[6] * a[11] * a[13]
                            + (a[0] * a[5] * a[10] * a[15]
                                - a[0] * a[5] * a[11] * a[14]
                                - a[0] * a[6] * a[9] * a[15])
                            - a[0] * a[7] * a[10] * a[13]
                            - a[1] * a[4] * a[10] * a[15])
                        - a[1] * a[6] * a[11] * a[12]
                        - a[1] * a[7] * a[8] * a[14])
                    - a[2] * a[4] * a[11] * a[13]
                    - a[2] * a[5] * a[8] * a[15])
                - a[2] * a[7] * a[9] * a[12]
                - a[3] * a[4] * a[9] * a[14])
            - a[3] * a[5] * a[10] * a[12]
            - a[3] * a[6] * a[8] * a[13])
}
pub fn inverse(a: &[f32; 16]) -> [f32; 16] {
    let inv_det: f32 = 1.0 / determinant(a);
    let cofactor_0: f32 = (a[7] * a[9] * a[14]
        + a[6] * a[11] * a[13]
        + (a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[6] * a[9] * a[15])
        - a[7] * a[10] * a[13])
        * inv_det;
    let cofactor_1: f32 = (a[3] * a[10] * a[13]
        + (a[2] * a[9] * a[15] + a[10] * -a[1] * a[15] + a[1] * a[11] * a[14]
            - a[2] * a[11] * a[13]
            - a[3] * a[9] * a[14]))
        * inv_det;
    let cofactor_2: f32 = (a[3] * a[5] * a[14]
        + a[2] * a[7] * a[13]
        + (a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[2] * a[5] * a[15])
        - a[3] * a[6] * a[13])
        * inv_det;
    let cofactor_3: f32 = (a[3] * a[6] * a[9]
        + (a[2] * a[5] * a[11] + a[6] * -a[1] * a[11] + a[1] * a[7] * a[10]
            - a[2] * a[7] * a[9]
            - a[3] * a[5] * a[10]))
        * inv_det;
    let cofactor_4: f32 = (a[7] * a[10] * a[12]
        + (a[6] * a[8] * a[15] + a[10] * -a[4] * a[15] + a[4] * a[11] * a[14]
            - a[6] * a[11] * a[12]
            - a[7] * a[8] * a[14]))
        * inv_det;
    let cofactor_5: f32 = (a[3] * a[8] * a[14]
        + a[2] * a[11] * a[12]
        + (a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[2] * a[8] * a[15])
        - a[3] * a[10] * a[12])
        * inv_det;
    let cofactor_6: f32 = (a[3] * a[6] * a[12]
        + (a[2] * a[4] * a[15] + a[6] * -a[0] * a[15] + a[0] * a[7] * a[14]
            - a[2] * a[7] * a[12]
            - a[3] * a[4] * a[14]))
        * inv_det;
    let cofactor_7: f32 = (a[3] * a[4] * a[10]
        + a[2] * a[7] * a[8]
        + (a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[2] * a[4] * a[11])
        - a[3] * a[6] * a[8])
        * inv_det;
    let cofactor_8: f32 = (a[7] * a[8] * a[13]
        + a[5] * a[11] * a[12]
        + (a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[5] * a[8] * a[15])
        - a[7] * a[9] * a[12])
        * inv_det;
    let cofactor_9: f32 = (a[3] * a[9] * a[12]
        + (a[1] * a[8] * a[15] + a[9] * -a[0] * a[15] + a[0] * a[11] * a[13]
            - a[1] * a[11] * a[12]
            - a[3] * a[8] * a[13]))
        * inv_det;
    let cofactor_10: f32 = (a[3] * a[4] * a[13]
        + a[1] * a[7] * a[12]
        + (a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[1] * a[4] * a[15])
        - a[3] * a[5] * a[12])
        * inv_det;
    let cofactor_11: f32 = (a[3] * a[5] * a[8]
        + (a[1] * a[4] * a[11] + a[5] * -a[0] * a[11] + a[0] * a[7] * a[9]
            - a[1] * a[7] * a[8]
            - a[3] * a[4] * a[9]))
        * inv_det;
    let cofactor_12: f32 = (a[6] * a[9] * a[12]
        + (a[5] * a[8] * a[14] + a[9] * -a[4] * a[14] + a[4] * a[10] * a[13]
            - a[5] * a[10] * a[12]
            - a[6] * a[8] * a[13]))
        * inv_det;
    let cofactor_13: f32 = (a[2] * a[8] * a[13]
        + a[1] * a[10] * a[12]
        + (a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[1] * a[8] * a[14])
        - a[2] * a[9] * a[12])
        * inv_det;
    let cofactor_14: f32 = (a[2] * a[5] * a[12]
        + (a[1] * a[4] * a[14] + a[5] * -a[0] * a[14] + a[0] * a[6] * a[13]
            - a[1] * a[6] * a[12]
            - a[2] * a[4] * a[13]))
        * inv_det;
    let cofactor_15: f32 = (a[2] * a[4] * a[9]
        + a[1] * a[6] * a[8]
        + (a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[1] * a[4] * a[10])
        - a[2] * a[5] * a[8])
        * inv_det;
    [
        cofactor_0,
        cofactor_1,
        cofactor_2,
        cofactor_3,
        cofactor_4,
        cofactor_5,
        cofactor_6,
        cofactor_7,
        cofactor_8,
        cofactor_9,
        cofactor_10,
        cofactor_11,
        cofactor_12,
        cofactor_13,
        cofactor_14,
        cofactor_15,
    ]
}

/// `a * b`, row-major.
#[must_use]
pub fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0_f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            // Summed as `a3*b[12+c] + a2*b[8+c] + a0*b[c] + a1*b[4+c]` (float order matters).
            out[r * 4 + c] = a[r * 4 + 3] * b[12 + c]
                + a[r * 4 + 2] * b[8 + c]
                + a[r * 4] * b[c]
                + a[r * 4 + 1] * b[4 + c];
        }
    }
    out
}

/// Row vector `(x, y, z, 1)` times matrix.
pub fn transform(e: &[f32; 16], x: f32, y: f32, z: f32) -> [f32; 4] {
    [
        e[8] * z + e[0] * x + e[4] * y + e[12],
        e[9] * z + e[1] * x + e[5] * y + e[13],
        e[10] * z + e[2] * x + e[6] * y + e[14],
        e[11] * z + e[3] * x + e[7] * y + e[15],
    ]
}
