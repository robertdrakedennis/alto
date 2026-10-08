//! The HSL16 colour model.
//!
//! Every floor/model colour in the 910 client is a 16-bit packed HSL short:
//! `hue6 << 10 | sat3 << 7 | lum7`. This module
//! holds the packers, the lightness adjusters and the three 65536-entry
//! lookup tables the toolkits index with those shorts:
//!
//! | table | order | built by |
//! |---|---|---|
//! | [`HslTables::rgb`] | `0x00RRGGBB` | [`build_hsl_tables`] |
//! | [`HslTables::bgr`] | `R \| G<<8 \| B<<16` | [`build_hsl_tables`] |
//! | [`build_hsv_table`] | `0xFFRRGGBB` via HSV | [`build_hsv_table`] |
//!
//! The DirectX-style toolkit indexes the `rgb` table and the GLX-style toolkit
//! the `bgr` table; the floor model always indexes `bgr`.
//!
//! All arithmetic keeps the reference `double`/`float` widths and `(int)`
//! truncation so the tables are byte-identical to the client's (checked
//! against frozen table digests in the tests).

use std::sync::OnceLock;

/// Pack 8-bit hue / saturation / lightness into an HSL16 short, crushing
/// saturation as the lightness approaches white.
#[must_use]
pub fn hsl24to16(hue: i32, mut sat: i32, lum: i32) -> i32 {
    if lum > 243 {
        sat >>= 4;
    } else if lum > 217 {
        sat >>= 3;
    } else if lum > 192 {
        sat >>= 2;
    } else if lum > 179 {
        sat >>= 1;
    }
    (lum >> 1) + (((hue & 0xFF) >> 2) << 10) + ((sat >> 5) << 7)
}

/// Convert a 24-bit RGB int to an HSL16 short (it is what the floor overlay
/// colour fudge feeds overlay `rgb`/`averagecolour` through).
#[must_use]
pub fn rgb24_to_hsl16(rgb: i32) -> i32 {
    let r = f64::from((rgb >> 16) & 0xFF) / 256.0;
    let g = f64::from((rgb >> 8) & 0xFF) / 256.0;
    let b = f64::from(rgb & 0xFF) / 256.0;
    let mut min = r;
    if g < r {
        min = g;
    }
    if b < min {
        min = b;
    }
    let mut max = r;
    if g > r {
        max = g;
    }
    if b > max {
        max = b;
    }
    let mut hue = 0.0_f64;
    let mut sat = 0.0_f64;
    let lum = (min + max) / 2.0;
    if min != max {
        if lum < 0.5 {
            sat = (max - min) / (min + max);
        }
        if lum >= 0.5 {
            sat = (max - min) / (2.0 - max - min);
        }
        if r == max {
            hue = (g - b) / (max - min);
        } else if g == max {
            hue = (b - r) / (max - min) + 2.0;
        } else if b == max {
            hue = (r - g) / (max - min) + 4.0;
        }
    }
    let hue = hue / 6.0;
    let h = (hue * 256.0) as i32;
    let mut s = (sat * 256.0) as i32;
    let mut l = (lum * 256.0) as i32;
    s = s.clamp(0, 255);
    l = l.clamp(0, 255);
    if l > 243 {
        s >>= 4;
    } else if l > 217 {
        s >>= 3;
    } else if l > 192 {
        s >>= 2;
    } else if l > 179 {
        s >>= 1;
    }
    (l >> 1) + (((h & 0xFF) >> 2) << 10) + ((s >> 5) << 7)
}

/// Scale the lightness of
/// an HSL16 short by `mul / 128`, clamped to `2..=126`; `-1` passes through
/// as the `12345678` sentinel.
#[must_use]
pub fn mul_hsl(hsl: i32, mul: i32) -> i32 {
    if hsl == -1 {
        return 12_345_678;
    }
    (hsl & 0xFF80) + clamp_lum(((hsl & 0x7F) * mul) >> 7)
}

/// [`mul_hsl`] without
/// the `-1` sentinel.
#[must_use]
pub fn scale_lightness(hsl: i32, mul: i32) -> i32 {
    (hsl & 0xFF80) + clamp_lum(((hsl & 0x7F) * mul) >> 7)
}

/// Scale the lightness of a colour that may be one of the two sentinels
/// (`-2` and `-1`).
#[must_use]
pub fn adjust_lightness(hsl: i32, mul: i32) -> i32 {
    if hsl == -2 {
        12_345_678
    } else if hsl == -1 {
        clamp_lum(mul)
    } else {
        (hsl & 0xFF80) + clamp_lum(((hsl & 0x7F) * mul) >> 7)
    }
}

fn clamp_lum(lum: i32) -> i32 {
    lum.clamp(2, 126)
}

/// Re-normalise the
/// saturation of an HSL16 short against its lightness (the software
/// rasteriser's model-colour transform). Returns the `short` widened.
#[must_use]
pub fn renormalise_saturation(hsl: i32) -> i32 {
    let hue = (hsl >> 10) & 0x3F;
    let sat = (hsl >> 3) & 0x70;
    let lum = hsl & 0x7F;
    let v4 = if lum <= 64 {
        (sat * lum) >> 7
    } else {
        ((127 - lum) * sat) >> 7
    };
    let v5 = lum + v4;
    let v6 = if v5 == 0 { v4 << 1 } else { (v4 << 8) / v5 };
    i32::from((hue << 10 | (v6 >> 4) << 7 | v5) as i16)
}

/// Alpha-blend `over`
/// (with its alpha in the top byte) onto `under`.
#[must_use]
pub fn blend_argb(under: i32, over: i32) -> i32 {
    let a = ((over as u32) >> 24) as i32;
    let inv = 255 - a;
    let top = (((over & 0xFF00FF).wrapping_mul(a) & 0xFF00FF00u32 as i32)
        | ((over & 0xFF00).wrapping_mul(a) & 0xFF0000)) as u32
        >> 8;
    let bottom = (((under & 0xFF00FF).wrapping_mul(inv) & 0xFF00FF00u32 as i32)
        | ((under & 0xFF00).wrapping_mul(inv) & 0xFF0000)) as u32
        >> 8;
    (bottom as i32).wrapping_add(top as i32)
}

/// Lerp two RGB ints
/// by `t / 255`.
#[must_use]
pub fn lerp_rgb(from: i32, to: i32, t: i32) -> i32 {
    let inv = 255 - t;
    let top = (((to & 0xFF00FF).wrapping_mul(t) & 0xFF00FF00u32 as i32)
        | ((to & 0xFF00).wrapping_mul(t) & 0xFF0000)) as u32
        >> 8;
    let bottom = (((from & 0xFF00FF).wrapping_mul(inv) & 0xFF00FF00u32 as i32)
        | ((from & 0xFF00).wrapping_mul(inv) & 0xFF0000)) as u32
        >> 8;
    (bottom as i32).wrapping_add(top as i32)
}

/// The two HSL16 lookup tables.
pub struct HslTables {
    /// HSL16 -> `0x00RRGGBB`, gamma 0.7.
    pub rgb: Vec<i32>,
    /// HSL16 -> `R | G << 8 | B << 16` (BGR packing), gamma 0.7.
    pub bgr: Vec<i32>,
}

/// The HSL16 tables, evaluated once.
#[must_use]
pub fn hsl_tables() -> &'static HslTables {
    static TABLES: OnceLock<HslTables> = OnceLock::new();
    TABLES.get_or_init(build_hsl_tables)
}

/// The HSL16 table build. Public so the oracle test can rebuild it;
/// prefer [`hsl_tables`].
#[must_use]
pub fn build_hsl_tables() -> HslTables {
    let gamma = 0.7_f64;
    let mut rgb = Vec::with_capacity(65536);
    let mut bgr = Vec::with_capacity(65536);
    for c in 0..65536_i32 {
        let h = f64::from((c >> 10) & 0x3F) / 64.0 + 0.0078125;
        let s = f64::from((c >> 7) & 0x7) / 8.0 + 0.0625;
        let l = f64::from(c & 0x7F) / 128.0;
        let mut r = l;
        let mut g = l;
        let mut b = l;
        if s != 0.0 {
            let q = if l < 0.5 {
                (s + 1.0) * l
            } else {
                s + l - s * l
            };
            let p = l * 2.0 - q;
            let mut hr = h + 0.333_333_333_333_333_3;
            if hr > 1.0 {
                hr -= 1.0;
            }
            let mut hb = h - 0.333_333_333_333_333_3;
            if hb < 0.0 {
                hb += 1.0;
            }
            r = hue_to_channel(hr, p, q);
            g = hue_to_channel(h, p, q);
            b = hue_to_channel(hb, p, q);
        }
        let ri = (r.powf(gamma) * 256.0) as i32;
        let gi = (g.powf(gamma) * 256.0) as i32;
        let bi = (b.powf(gamma) * 256.0) as i32;
        rgb.push(((ri << 16) + (gi << 8) + bi) & 0xFF_FFFF);
        bgr.push((gi << 8) + (bi << 16) + ri);
    }
    HslTables { rgb, bgr }
}

/// One channel of the HSL16 table build: the standard
/// HSL sextant ladder with the reference literals.
fn hue_to_channel(t: f64, p: f64, q: f64) -> f64 {
    if t * 6.0 < 1.0 {
        (q - p) * 6.0 * t + p
    } else if t * 2.0 < 1.0 {
        q
    } else if t * 3.0 < 2.0 {
        (q - p) * (0.666_666_666_666_666_6 - t) * 6.0 + p
    } else {
        p
    }
}

/// The software rasteriser's HSL16 -> `0xFFRRGGBB` table built through an HSV
/// ladder in `float`. Index order is `hue6 << 10 | sat3 << 7 | lum7`
/// (`v3 = hue6 << 3 | sat3`, `v6 = lum`).
#[must_use]
pub fn build_hsv_table() -> Vec<i32> {
    let gamma = 0.7_f64;
    let mut out = Vec::with_capacity(65536);
    for v3 in 0..512_i32 {
        let hue_deg = ((v3 >> 3) as f32 / 64.0_f32 + 0.0078125_f32) * 360.0_f32;
        let sat = (v3 & 0x7) as f32 / 8.0_f32 + 0.0625_f32;
        for v6 in 0..128_i32 {
            let val = v6 as f32 / 128.0_f32;
            let sector = hue_deg / 60.0_f32;
            let si = sector as i32;
            let sm = si % 6;
            let frac = sector - si as f32;
            let p = (1.0_f32 - sat) * val;
            let q = (1.0_f32 - sat * frac) * val;
            let t = (1.0_f32 - (1.0_f32 - frac) * sat) * val;
            let (r, g, b) = match sm {
                0 => (val, t, p),
                1 => (q, val, p),
                2 => (p, val, t),
                3 => (p, q, val),
                4 => (t, p, val),
                5 => (val, p, q),
                _ => (0.0, 0.0, 0.0),
            };
            let rr = (f64::from(r).powf(gamma)) as f32;
            let gg = (f64::from(g).powf(gamma)) as f32;
            let bb = (f64::from(b).powf(gamma)) as f32;
            let ri = (rr * 256.0_f32) as i32;
            let gi = (gg * 256.0_f32) as i32;
            let bi = (bb * 256.0_f32) as i32;
            out.push(
                (gi << 8)
                    .wrapping_add(ri << 16)
                    .wrapping_add(-16_777_216)
                    .wrapping_add(bi),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hsl24to16`: `(lum >> 1) +
    /// ((hue & 0xFF) >> 2 << 10) + (sat >> 5 << 7)`, saturation shifted right
    /// by 1/2/3/4 above lightness 179/192/217/243. (The map loader's
    /// `flo::hsl24to16` copy was merged into this one in Phase 2.1.)
    #[test]
    fn hsl24to16_packs_fields() {
        for ((hue, sat, lum), packed) in [
            ((0, 0, 0), 0),
            // hue 255 -> 63 << 10; sat 255 -> 7 << 7; lum 100 -> 50.
            ((255, 255, 100), (63 << 10) + (7 << 7) + 50),
            // lum 180: sat 255 >> 1 = 127 -> 3 << 7; 90.
            ((0, 255, 180), 90 + (3 << 7)),
            // lum 244: sat 255 >> 4 = 15 -> 0; 122.
            ((0, 255, 244), 122),
            // Hue is masked `& 0xFF`: -1 packs like 255.
            ((-1, 0, 0), 63 << 10),
            ((255, 0, 0), 63 << 10),
            ((0, 0, 255), 127),
        ] {
            assert_eq!(hsl24to16(hue, sat, lum), packed, "{hue},{sat},{lum}");
        }
    }

    #[test]
    fn rgb24_to_hsl16_pure_colours() {
        // Pure black: everything 0.
        assert_eq!(rgb24_to_hsl16(0), 0);
        // Pure white: lum 255 -> 127, saturation crushed to 0, hue 0.
        assert_eq!(rgb24_to_hsl16(0xFFFFFF), 127);
    }
}
