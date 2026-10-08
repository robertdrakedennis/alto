//! The 16384-entry Q14 sine/cosine tables.
//!
//! `sin[i] = (int)(sin(i * STEP) * 16384)` and likewise for cosine, with
//! `STEP = 2π / 16384` as a double literal. Angles are 14-bit units
//! (`16384 == 2π`). The tables are built once (`OnceLock`) with exactly that
//! expression so every consumer (map noise, camera, model rotation) reads the
//! same ints the client does.

use std::sync::OnceLock;

/// Table step: `2π / 16384`, as the exact double literal.
pub const STEP: f64 = 3.834_951_969_714_103E-4;

struct Tables {
    sin: Vec<i32>,
    cos: Vec<i32>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut sin = Vec::with_capacity(16384);
        let mut cos = Vec::with_capacity(16384);
        for i in 0..16384_i32 {
            // `(int)` cast truncates toward zero, like `as i32`.
            sin.push(((f64::from(i) * STEP).sin() * 16384.0) as i32);
            cos.push(((f64::from(i) * STEP).cos() * 16384.0) as i32);
        }
        Tables { sin, cos }
    })
}

/// `sin[angle & 0x3FFF]`.
#[must_use]
pub fn sin(angle: i32) -> i32 {
    tables().sin[(angle & 0x3FFF) as usize]
}

/// `cos[angle & 0x3FFF]`.
#[must_use]
pub fn cos(angle: i32) -> i32 {
    tables().cos[(angle & 0x3FFF) as usize]
}

/// Raw table access without the mask — `cos[index]` as used by the map
/// noise interpolation, where the index is
/// `t * 8192 / freq` and always in range.
#[must_use]
pub fn cos_raw(index: usize) -> i32 {
    tables().cos[index]
}

/// 14-bit angle to float radians with
/// the float/double mix preserved (`(float) angle / 16384.0F` then widened
/// and multiplied by `2π` as a double, narrowed back to float).
#[must_use]
pub fn radians(angle: i32) -> f32 {
    let masked = angle & 0x3FFF;
    (f64::from(masked as f32 / 16384.0_f32) * std::f64::consts::TAU) as f32
}

/// `round(atan2(y, x) * 16384/2π) & 0x3FFF`.
/// The angle rounds half up (toward +∞); Rust `f64::round` rounds
/// half away from zero, so implement the rule explicitly.
#[must_use]
pub fn atan2(y: i32, x: i32) -> i32 {
    let value = fdlibm_atan2(f64::from(y), f64::from(x)) * 2_607.594_587_617_613_3;
    (round_half_up(value) as i32) & 0x3FFF
}

/// The original client's `atan2` is fdlibm's `e_atan2.c`.
/// The platform libm (`f64::atan2`) is not bit-identical: macOS gives
/// `atan2(-11118881, 1656748493) * K = -17.500000000000004` where fdlibm gives
/// exactly `-17.5`, which flips the table angle by one unit.
#[allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    reason = "fdlibm constants kept digit-for-digit"
)]
fn fdlibm_atan2(y: f64, x: f64) -> f64 {
    const TINY: f64 = 1.0e-300;
    const PI_O_4: f64 = 7.853_981_633_974_482_790_0E-01;
    const PI_O_2: f64 = 1.570_796_326_794_896_558_0E+00;
    const PI: f64 = 3.141_592_653_589_793_116_0E+00;
    const PI_LO: f64 = 1.224_646_799_147_353_177_2E-16;
    let (hx, lx) = words(x);
    let (hy, ly) = words(y);
    let ix = hx & 0x7fff_ffff;
    let iy = hy & 0x7fff_ffff;
    if (ix as u32 | u32::from(lx != 0)) > 0x7ff0_0000
        || (iy as u32 | u32::from(ly != 0)) > 0x7ff0_0000
    {
        return x + y; // NaN
    }
    if hx.wrapping_sub(0x3ff0_0000) as u32 | lx == 0 {
        return fdlibm_atan(y); // x == 1.0
    }
    let m = ((hy >> 31) & 1) | ((hx >> 30) & 2); // 2 * sign(x) + sign(y)
    if iy as u32 | ly == 0 {
        return match m {
            0 | 1 => y,
            2 => PI + TINY,
            _ => -PI - TINY,
        };
    }
    if ix as u32 | lx == 0 {
        return if hy < 0 {
            -PI_O_2 - TINY
        } else {
            PI_O_2 + TINY
        };
    }
    if ix == 0x7ff0_0000 {
        return if iy == 0x7ff0_0000 {
            match m {
                0 => PI_O_4 + TINY,
                1 => -PI_O_4 - TINY,
                2 => 3.0 * PI_O_4 + TINY,
                _ => -3.0 * PI_O_4 - TINY,
            }
        } else {
            match m {
                0 => 0.0,
                1 => -0.0,
                2 => PI + TINY,
                _ => -PI - TINY,
            }
        };
    }
    if iy == 0x7ff0_0000 {
        return if hy < 0 {
            -PI_O_2 - TINY
        } else {
            PI_O_2 + TINY
        };
    }
    let k = (iy - ix) >> 20;
    let z = if k > 60 {
        PI_O_2 + 0.5 * PI_LO
    } else if hx < 0 && k < -60 {
        0.0
    } else {
        fdlibm_atan((y / x).abs())
    };
    match m {
        0 => z,
        1 => -z,
        2 => PI - (z - PI_LO),
        _ => (z - PI_LO) - PI,
    }
}

/// fdlibm `s_atan.c` (`StrictMath.atan`).
#[allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    reason = "fdlibm constants kept digit-for-digit"
)]
fn fdlibm_atan(x: f64) -> f64 {
    const ATANHI: [f64; 4] = [
        4.636_476_090_008_060_935_15E-01,
        7.853_981_633_974_482_789_99E-01,
        9.827_937_232_473_290_540_82E-01,
        1.570_796_326_794_896_558_00E+00,
    ];
    const ATANLO: [f64; 4] = [
        2.269_877_745_296_168_709_24E-17,
        3.061_616_997_868_383_017_93E-17,
        1.390_331_103_123_099_845_16E-17,
        6.123_233_995_736_766_035_87E-17,
    ];
    const AT: [f64; 11] = [
        3.333_333_333_333_293_180_27E-01,
        -1.999_999_999_987_648_324_76E-01,
        1.428_571_427_250_346_637_11E-01,
        -1.111_111_040_546_235_578_80E-01,
        9.090_887_133_436_506_561_96E-02,
        -7.691_876_205_044_829_994_95E-02,
        6.661_073_137_387_531_206_69E-02,
        -5.833_570_133_790_573_486_45E-02,
        4.976_877_994_615_932_360_17E-02,
        -3.653_157_274_421_691_552_70E-02,
        1.628_582_011_536_578_236_23E-02,
    ];
    let (hx, lx) = words(x);
    let ix = hx & 0x7fff_ffff;
    if ix >= 0x4410_0000 {
        // |x| >= 2^66
        if ix > 0x7ff0_0000 || (ix == 0x7ff0_0000 && lx != 0) {
            return x + x; // NaN
        }
        return if hx > 0 {
            ATANHI[3] + ATANLO[3]
        } else {
            -ATANHI[3] - ATANLO[3]
        };
    }
    let (id, x) = if ix < 0x3fdc_0000 {
        // |x| < 0.4375
        if ix < 0x3e20_0000 {
            return x; // |x| < 2^-29 (raises inexact in C)
        }
        (None, x)
    } else {
        let x = x.abs();
        if ix < 0x3ff3_0000 {
            if ix < 0x3fe6_0000 {
                (Some(0), (2.0 * x - 1.0) / (2.0 + x))
            } else {
                (Some(1), (x - 1.0) / (x + 1.0))
            }
        } else if ix < 0x4003_8000 {
            (Some(2), (x - 1.5) / (1.0 + 1.5 * x))
        } else {
            (Some(3), -1.0 / x)
        }
    };
    let z = x * x;
    let w = z * z;
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    match id {
        None => x - x * (s1 + s2),
        Some(id) => {
            let z = ATANHI[id] - ((x * (s1 + s2) - ATANLO[id]) - x);
            if hx < 0 {
                -z
            } else {
                z
            }
        }
    }
}

/// fdlibm `__HI`/`__LO`: the signed high and unsigned low 32-bit words.
fn words(value: f64) -> (i32, u32) {
    let bits = value.to_bits();
    ((bits >> 32) as u32 as i32, bits as u32)
}

/// `floor(x + 0.5)` as a long: round half up.
fn round_half_up(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_anchors_hold() {
        assert_eq!(sin(0), 0);
        assert_eq!(cos(0), 16384);
        assert_eq!(sin(4096), 16384);
        // cos(π/2) = 6.1e-17 * 16384 truncates to 0.
        assert_eq!(cos(4096), 0);
        assert_eq!(sin(8192), 0);
        assert_eq!(cos(8192), -16384);
        // Mask wraps.
        assert_eq!(cos(16384), cos(0));
        assert_eq!(cos(-1), cos(16383));
    }

    /// fdlibm gives `atan2(-11118881, 1656748493) * K == -17.5`
    /// exactly, so rounding half up yields -17 and the table angle 16367. macOS
    /// libm is one ulp low (-17.500000000000004 → 16366); the golden above
    /// leaves such inputs out, so this pins the fdlibm port.
    #[test]
    fn atan2_uses_fdlibm_not_platform_libm() {
        assert_eq!(atan2(-11_118_881, 1_656_748_493), 16367);
        assert_eq!(
            fdlibm_atan2(-11_118_881.0, 1_656_748_493.0) * 2_607.594_587_617_613_3,
            -17.5
        );
    }
}
