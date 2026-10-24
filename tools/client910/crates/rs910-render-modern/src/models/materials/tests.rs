//! Material cache tests over the pack: the atlas gutters hold the wrapped
//! texture, and the BC texels equal the lossless PNG copy's within the block
//! compression's error.
use super::*;

/// Decode one BC3 (`DXT5`) block to 16 RGBA texels, row by row (the
/// published S3TC rules: interpolated alpha with 8 or 6 steps, four-colour
/// RGB565 endpoints).
fn decode_bc3_block(block: &[u8]) -> [[u8; 4]; 16] {
    let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
    let alphas: [u32; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            i => ((8 - i as u32) * a0 + (i as u32 - 1) * a1) / 7,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            6 => 0,
            7 => 255,
            i => ((6 - i as u32) * a0 + (i as u32 - 1) * a1) / 5,
        })
    };
    let abits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    let rgb = |v: u16| {
        let r = u32::from((v >> 11) & 31);
        let g = u32::from((v >> 5) & 63);
        let b = u32::from(v & 31);
        [
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ]
    };
    let c0 = rgb(u16::from_le_bytes([block[8], block[9]]));
    let c1 = rgb(u16::from_le_bytes([block[10], block[11]]));
    let colours: [[u32; 3]; 4] = [
        c0,
        c1,
        std::array::from_fn(|k| (2 * c0[k] + c1[k]) / 3),
        std::array::from_fn(|k| (c0[k] + 2 * c1[k]) / 3),
    ];
    let cbits = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
    std::array::from_fn(|i| {
        let c = colours[((cbits >> (2 * i)) & 3) as usize];
        let a = alphas[((abits >> (3 * i)) & 7) as usize];
        [c[0] as u8, c[1] as u8, c[2] as u8, a as u8]
    })
}

/// Decode a BC3 level to RGBA8 rows.
fn decode_bc3(w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0_u8; (w * h * 4) as usize];
    let bx = w.div_ceil(4);
    for (i, block) in data.chunks_exact(16).enumerate() {
        let (x0, y0) = ((i as u32 % bx) * 4, (i as u32 / bx) * 4);
        for (j, texel) in decode_bc3_block(block).iter().enumerate() {
            let (x, y) = (x0 + j as u32 % 4, y0 + j as u32 / 4);
            if x < w && y < h {
                let at = ((y * w + x) * 4) as usize;
                out[at..at + 4].copy_from_slice(texel);
            }
        }
    }
    out
}

/// Every RT7 texture reference of the pack.
fn rt7_refs(pack: &Pack) -> Vec<(u32, MapKind, Rt7TextureRef)> {
    let store = Rt7MaterialExtraStore::load(pack).unwrap();
    let mut out = Vec::new();
    for extra in store.iter() {
        for (kind, r) in [
            (MapKind::Diffuse, extra.diffuse),
            (MapKind::Normal, extra.normal),
            (MapKind::Compound, extra.compound),
        ] {
            if let Some(r) = r {
                out.push((extra.id, kind, r));
            }
        }
    }
    out
}

/// A sample of the references: all three kinds, every size code.
fn sample(
    refs: &[(u32, MapKind, Rt7TextureRef)],
    every: usize,
) -> Vec<(u32, MapKind, Rt7TextureRef)> {
    refs.iter().copied().step_by(every).collect()
}

/// The 32-pixel gutter of every sampled atlas is the wrapped texture: each
/// gutter strip of the lossless PNG copy (archive 54) equals the inner
/// square's opposite edge, so a repeating lookup filters across the seam
/// as if the texture tiled.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn atlas_gutters_hold_the_wrapped_texture() {
    let pack = crate::test_support::require_pack("client.textures.png.mipped.js5");
    let refs = rt7_refs(&pack);
    let mut checked = 0;
    for (material, kind, r) in sample(&refs, 29) {
        let map = load_map(&pack, TextureSource::PngMipped, MapKind::Compound, r).unwrap();
        let (w, _, px) = &map.levels[0];
        let (w, s) = (*w as usize, r.size() as usize);
        let at = |x: usize, y: usize| &px[(y * w + x) * 4..(y * w + x) * 4 + 4];
        for i in 0..32 {
            for y in (32..32 + s).step_by(5) {
                // Left gutter column i shows inner column s - 32 + i, etc.
                assert_eq!(at(i, y), at(s + i, y), "material {material} {kind:?} left");
                assert_eq!(
                    at(32 + s + i, y),
                    at(32 + i, y),
                    "material {material} {kind:?} right"
                );
                assert_eq!(at(y, i), at(y, s + i), "material {material} {kind:?} top");
                assert_eq!(
                    at(y, 32 + s + i),
                    at(y, 32 + i),
                    "material {material} {kind:?} bottom"
                );
            }
        }
        checked += 1;
    }
    assert!(checked > 40, "{checked}");
}

/// Decoded BC texels equal the lossless PNG copy (archive 54) within BC3's
/// error (bounds per map kind below) at the two largest levels (the smaller levels of the two archives
/// were filtered separately and drift further apart), on the channels the
/// map carries (a normal map's red and blue differ by design: 0 in the BC
/// copy, Z-like in the PNG). Raw texels first (no gamma), then the diffuse
/// with the endpoint gamma against the PNG with the texel gamma.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn bc_texels_match_the_mipped_png() {
    let pack = crate::test_support::require_pack("client.textures.dxt.js5");
    let refs = rt7_refs(&pack);
    let mut worst = [0.0_f64; 2];
    for (material, kind, r) in sample(&refs, 53) {
        let channels: &[usize] = if kind == MapKind::Normal {
            &[1, 3]
        } else {
            &[0, 1, 2, 3]
        };
        // (mean difference, share of differences over 24). The compound
        // map's red and green vary independently, which BC3's shared RGB
        // endpoints encode worst (sample: mean up to 7.6, 12.5% far); the
        // diffuse reaches 3.5 and 1%, the normal map's X/Y 1.8 and 0.1%.
        let bound = match kind {
            MapKind::Diffuse => (5.0, 0.02),
            MapKind::Normal => (3.0, 0.01),
            MapKind::Compound => (10.0, 0.15),
        };
        // Loaded as a compound map: linear, no gamma on either copy.
        let mut cases = vec![MapKind::Compound];
        if kind == MapKind::Diffuse {
            cases.push(MapKind::Diffuse);
        }
        for as_kind in cases {
            let bc = load_map(&pack, TextureSource::Bc, as_kind, r).unwrap();
            let png = load_map(&pack, TextureSource::PngMipped, as_kind, r).unwrap();
            for (level, ((w, h, blocks), (pw, ph, px))) in
                bc.levels.iter().zip(&png.levels).enumerate().take(2)
            {
                assert_eq!((w, h), (pw, ph));
                let texels = decode_bc3(*w, *h, blocks);
                let (mut sum, mut far, mut n) = (0_u64, 0_usize, 0_usize);
                for (a, b) in texels.chunks_exact(4).zip(px.chunks_exact(4)) {
                    for &c in channels {
                        let d = a[c].abs_diff(b[c]);
                        sum += u64::from(d);
                        far += usize::from(d > 24);
                        n += 1;
                    }
                }
                let mean = sum as f64 / n as f64;
                worst[usize::from(as_kind == MapKind::Diffuse)] =
                    worst[usize::from(as_kind == MapKind::Diffuse)].max(mean);
                assert!(
                    mean < bound.0 && (far as f64) <= bound.1 * n as f64,
                    "material {material} {kind:?} as {as_kind:?} level {level}: mean {mean:.2}, {far} of {n} far"
                );
            }
        }
    }
    eprintln!(
        "worst mean texel difference: raw {:.2}, gamma'd diffuse {:.2}",
        worst[0], worst[1]
    );
}
