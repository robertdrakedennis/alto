//! Tests of the NXT side tables. The byte fixtures are copied from the 910
//! pack and annotated by hand; the pack-gated tests decode the whole corpus
//! strictly and check invariants and agreement with the 910 decoders. The
//! two heavy `#[ignore]` censuses pin the statistics `nxt-data-formats.md`
//! cites.

use std::collections::{BTreeMap, BTreeSet};

use super::map_environment::decode_environment;
use super::map_lights::decode_point_lights;
use super::map_terrain::{decode_terrain, TERRAIN_SIDE};
use super::map_water::decode_water;
use super::material::{decode_rt7_extra, Rt7MaterialExtraStore, Rt7TextureRef};
use super::texture_header::{
    load_texture_header, texture_header, PixelFormat, TextureArchive, TextureHeader,
};
use super::{MAP_ARCHIVE, TERRAIN_FILE};
use crate::cache::Pack;
use crate::test_support::require_pack;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

// ---------------------------------------------------------------------------
// Fixtures (no pack).
// ---------------------------------------------------------------------------

/// Material 10456 (archive 26 group 0), all 37 bytes:
/// `01` RT7 | flags `000140ec` (0x4 0x8 0x20 0x40 0x80 0x4000 0x10000) |
/// `02 00004c87` diffuse (code 2, id 19591) | `02 00004c88` normal (19592) |
/// `02 00004c89` compound (19593) | `00000000` 0x4000 float 0.0 |
/// `42000000` normal-map float 32.0 | `41200000` 0x10000 float 10.0 |
/// `09` repeat | `01` facet | `00` quality | `00` alpha | `ffff` average
/// colour | `02` size code.
const MATERIAL_10456: &str = "01 000140ec 02 00004c87 02 00004c88 02 00004c89 \
                              00000000 42000000 41200000 09 01 00 00 ffff 02";

#[test]
fn rt7_extra_decodes_material_10456_by_hand() {
    let extra = decode_rt7_extra(10456, &hex(MATERIAL_10456))
        .unwrap()
        .expect("RT7");
    assert_eq!(extra.flags, 0x140ec);
    let r = |texture| {
        Some(Rt7TextureRef {
            size_code: 2,
            texture,
        })
    };
    assert_eq!(extra.diffuse, r(19591));
    assert_eq!(extra.normal, r(19592));
    assert_eq!(extra.compound, r(19593));
    assert_eq!(extra.diffuse.unwrap().size(), 256);
    assert_eq!(extra.f_4000, Some(0.0));
    assert_eq!(extra.normal_param, Some(32.0));
    assert_eq!(extra.f_10000, Some(10.0));
    assert_eq!(
        (extra.f_1000, extra.f_2000, extra.i_8000),
        (None, None, None)
    );
    assert_eq!((extra.specular, extra.f_20000), (None, None));
    assert_eq!((extra.speed_u_raw, extra.speed_v_raw), (None, None));
    assert_eq!(
        (extra.repeat, extra.facet, extra.quality, extra.alpha),
        (9, 1, 0, 0)
    );
    assert_eq!(extra.alpha_threshold, None);
    assert_eq!((extra.average_colour, extra.size_code), (0xFFFF, 2));
}

#[test]
fn rt7_extra_rejects_trailing_and_truncated_bytes_and_skips_rt5() {
    let mut bytes = hex(MATERIAL_10456);
    bytes.push(0);
    assert!(decode_rt7_extra(1, &bytes).is_err());
    bytes.truncate(bytes.len() - 2);
    assert!(decode_rt7_extra(1, &bytes).is_err());
    assert_eq!(decode_rt7_extra(1, &[0, 1, 2]).unwrap(), None);
    assert!(decode_rt7_extra(1, &[2]).is_err());
}

/// Map group 6450 (Lumbridge, square 50,50) file 6, all 199 bytes.
const ENV_6450: &str = "00d3bf9e ffce ffc4 ffce 00f8 0138 0061 3f800000 0078b1c8 012c 01 \
    00000000 42000000 41000000 00000000 01 3827c5ac 4652f000 3f800000 3fc00000 \
    3f19999a 3f19999a 3f19999a 3e9a9a9b 3e9a9a9b 3e9a9a9b 3e9a9a9b 3e9a9a9b 3f008081 \
    00000000 3f800000 3f800000 3f800000 3f800000 3f4ccccd 3f4ccccd 42200000 \
    ffffb380 ff060606 01 03 3ca3d70a 3f99999a 3f000000 403851ec 3f000000 \
    3fc00000 3e800000 3e99999a 000000000000000000000000 0035 0cb8 00 \
    56c8 3f800000 6c2c 3f59999a";

#[test]
fn environment_decodes_lumbridge_by_hand() {
    let bytes = hex(ENV_6450);
    assert_eq!(bytes.len(), super::map_environment::ENVIRONMENT_LEN);
    let env = decode_environment(6450, &bytes).unwrap();
    assert_eq!(env.sun_colour, 0x00d3_bf9e);
    assert_eq!(env.sun_direction, [-50, -60, -50]);
    assert_eq!(env.sun_intensity_raw, [248, 312, 97]);
    assert_eq!(env.sun_intensity(), [0.968_75, 1.218_75, 0.378_906_25]);
    assert_eq!(env.f_16, 1.0);
    assert_eq!((env.fog_colour, env.fog_depth), (0x0078_b1c8, 300));
    assert!(env.flag_26 && env.flag_43 && env.flag_136 && !env.flag_186);
    assert_eq!(env.f_27, [0.0, 32.0, 8.0, 0.0]);
    assert_eq!(env.scattering_params[1], 13_500.0);
    assert_eq!(env.scattering[0], [0.6; 3]);
    assert_eq!(env.u8_137, 3);
    assert_eq!(env.f_138[0].to_bits(), 0x3ca3_d70a);
    assert_eq!(env.bloom, [1.5, 0.25, f32::from_bits(0x3e99_999a)]);
    assert_eq!(
        &env.raw_128,
        &[0xff, 0xff, 0xb3, 0x80, 0xff, 0x06, 0x06, 0x06]
    );
    assert_eq!(env.raw_170, [0; 12]);
    assert_eq!((env.skybox, env.sampler_material), (Some(53), Some(3256)));
    assert_eq!(
        env.colour_remap,
        [
            (Some(22216), 1.0),
            (Some(27692), f32::from_bits(0x3f59_999a))
        ]
    );
    assert!(decode_environment(6450, &bytes[..198]).is_err());
    let mut long = bytes.clone();
    long.push(0);
    assert!(decode_environment(6450, &long).is_err());
}

#[test]
fn point_lights_decode_by_hand() {
    // Group 6450 file 7, first two of 28 lights (count byte patched to 2):
    // `01` level 1 | x 0x1f74 z 0x5198 y 0x0320 | r 1 | 3 runs of 0 |
    // colour 0x1790 | `08` flicker 8 | group -1; then `00` level 0 |
    // x 0x3c88 z 0x2088 y 0x0350 | r 1 | 3 runs | 0x1790 | `a6` (flicker 6,
    // phase 5) | group -1.
    let bytes = hex("02 01 1f74 5198 0320 01 0000 0000 0000 1790 08 ffff \
            00 3c88 2088 0350 01 0000 0000 0000 1790 a6 ffff");
    let lights = decode_point_lights(6450, &bytes).unwrap();
    assert_eq!(lights.len(), 2);
    let a = &lights[0];
    assert_eq!((a.level(), a.x, a.z, a.y), (1, 8052, 20888, 800));
    assert_eq!((a.span_radius, a.span_runs.as_slice()), (1, &[0, 0, 0][..]));
    assert_eq!(
        (a.colour_hsl, a.flicker(), a.group, a.light_type),
        (6032, 8, -1, None)
    );
    let b = &lights[1];
    assert_eq!((b.level(), b.x, b.z, b.y), (0, 15496, 8328, 848));
    assert_eq!((b.flicker(), b.flicker_phase >> 5), (6, 5));
    // A LightType light: flicker 31 carries a trailing u16 type id.
    let typed =
        decode_point_lights(1, &hex("01 00 0000 0000 0000 00 0000 1234 1f ffff 0009")).unwrap();
    assert_eq!(typed[0].light_type, Some(9));
    assert_eq!(decode_point_lights(1, &[0]).unwrap(), Vec::new());
    assert!(decode_point_lights(1, &[1]).is_err());
}

#[test]
fn water_decodes_by_hand() {
    // Group 6450 file 8, first of 11 patches (count patched to 1): tile
    // (37, 53), size (10, 33), height 25, axis (0, 1, 0), turns 0xbc39af73,
    // u16 10000, flow (10, -100), water type 0.
    let bytes = hex("01 25 35 0a 21 0019 00000000 3f800000 00000000 bc39af73 2710 0a 9c 0000");
    let patches = decode_water(6450, &bytes).unwrap();
    assert_eq!(patches.len(), 1);
    let p = &patches[0];
    assert_eq!((p.tile_x, p.tile_z, p.size_x, p.size_z), (37, 53, 10, 33));
    assert_eq!(p.height, 25);
    assert_eq!(p.rotation_axis, [0.0, 1.0, 0.0]);
    assert_eq!(p.rotation_turns.to_bits(), 0xbc39_af73);
    assert_eq!((p.u16_22, p.flow, p.water_type), (10000, [10, -100], 0));
    assert_eq!(p.flow_scaled(), [0.2, -2.0]);
    assert!(decode_water(1, &bytes[..28]).is_err());
}

#[test]
fn terrain_decodes_a_synthetic_level() {
    // One level-2 block: tile 0 is a full water tile, tile 1 an underlay
    // tile, the rest height-only (flag 0).
    let mut bytes = vec![2];
    // flags 0x13 (0x1 | 0x2 | water), height 45, water height 1, underlay
    // smart 0x8144 (id 323) + u16 0x2997, overlay smart 0x0c (id 11), water
    // overlay smart 0x70 (id 111), shape 0x19 (shape 6, rot 1), water
    // underlay smart 0x0a (id 9).
    bytes.extend(hex("13 2d 01 8144 2997 0c 70 19 0a"));
    // flags 0x01, height 41, underlay 321 + u16, overlay none (0x00).
    bytes.extend(hex("01 29 8142 2a17 00"));
    for _ in 2..TERRAIN_SIDE * TERRAIN_SIDE {
        bytes.extend([0, 7]);
    }
    let levels = decode_terrain(1, &bytes).unwrap();
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].level, 2);
    let t0 = levels[0].tile(0, 0).unwrap();
    assert!(t0.is_water());
    assert_eq!(t0.settings(), 1);
    assert_eq!((t0.height, t0.water_height), (45, Some(1)));
    assert_eq!((t0.underlay, t0.underlay_u16), (Some(323), Some(0x2997)));
    assert_eq!((t0.overlay, t0.water_overlay), (Some(11), Some(111)));
    assert_eq!((t0.overlay_shape, t0.water_underlay), (Some(0x19), Some(9)));
    let t1 = levels[0].tile(0, 1).unwrap();
    assert_eq!(
        (t1.underlay, t1.overlay, t1.overlay_shape),
        (Some(321), None, None)
    );
    let t_last = levels[0].tile(65, 65).unwrap();
    assert_eq!((t_last.flags, t_last.height), (0, 7));
    assert!(levels[0].tile(66, 0).is_none());
    assert_eq!(decode_terrain(1, &[]).unwrap(), Vec::new());
    assert!(decode_terrain(1, &bytes[..bytes.len() - 1]).is_err());
}

/// A DDS header for `w x h` DXT5 with `mips` levels (`caps2` for cubes).
fn dds(w: u32, h: u32, mips: u32, caps2: u32, faces: usize) -> Vec<u8> {
    let mut d = vec![0_u8; 128];
    d[..4].copy_from_slice(b"DDS ");
    let mut put = |at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(4, 124);
    put(8, 0xa1007);
    put(12, h);
    put(16, w);
    put(28, mips);
    put(80, 4);
    put(112, caps2);
    d[84..88].copy_from_slice(b"DXT5");
    let mut chain = 0;
    for level in 0..mips {
        chain += ((w >> level).max(1).div_ceil(4) * (h >> level).max(1).div_ceil(4) * 16) as usize;
    }
    d.resize(128 + faces * chain, 0xAA);
    d
}

fn frame(version: u8, payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![version];
    for p in payloads {
        out.extend((p.len() as u32).to_be_bytes());
        out.extend(p);
    }
    out
}

#[test]
fn texture_headers_parse_synthetic_dds_and_check_their_size() {
    let file = frame(1, &[dds(192, 192, 8, 0, 1)]);
    assert_eq!(
        texture_header(TextureArchive::Dxt, 1, &file).unwrap(),
        TextureHeader {
            version: 1,
            faces: 1,
            format: PixelFormat::Dxt5,
            width: 192,
            height: 192,
            mip_levels: 8,
        }
    );
    let cube = frame(1, &[dds(8, 8, 4, 0xfe00, 6)]);
    assert_eq!(
        texture_header(TextureArchive::Dxt, 1, &cube).unwrap().faces,
        6
    );
    let mut short = dds(192, 192, 8, 0, 1);
    short.pop();
    assert!(texture_header(TextureArchive::Dxt, 1, &frame(1, &[short])).is_err());
    let mut trailing = file.clone();
    trailing.push(0);
    assert!(texture_header(TextureArchive::Dxt, 1, &trailing).is_err());
    assert!(texture_header(TextureArchive::Dxt, 1, &[2]).is_err());
}

// ---------------------------------------------------------------------------
// Whole-corpus tests (pack).
// ---------------------------------------------------------------------------

fn texture_ids(pack: &Pack, archive: TextureArchive) -> BTreeSet<u32> {
    pack.read_archive_index(archive.name())
        .unwrap()
        .group_id
        .into_iter()
        .collect()
}

/// Flag bits the RT7 decoder knows: the ones
/// with a payload (`0x20`-`0x200`, `0x800`-`0x20000`) and the payload-free
/// `0x1`-`0x10` and `0x400`.
const RT7_KNOWN_FLAGS: u32 = 0x3_FFFF;

/// Every RT7 material file decodes strictly (each record consumes exactly
/// its bytes), sets only known flag bits, names textures present in all
/// four texture archives (52-55), and agrees with the 910
/// RT7 material decoder port (`crate::texture::MaterialStore`) on every
/// field 910 keeps.
#[test]
#[cfg_attr(feature = "no-pack", ignore)]
fn rt7_extras_decode_resolve_and_agree_with_the_910_material_store() {
    let pack = require_pack("client.materials.js5");
    let store = Rt7MaterialExtraStore::load(&pack).unwrap();
    assert!(!store.is_empty());
    let archives: Vec<BTreeSet<u32>> = TextureArchive::ALL
        .iter()
        .map(|&a| texture_ids(&pack, a))
        .collect();
    let store910 = crate::texture::MaterialStore::load(&pack).unwrap();
    for extra in store.iter() {
        let id = extra.id;
        assert_eq!(extra.flags & !RT7_KNOWN_FLAGS, 0, "material {id}: flags");
        for r in [extra.diffuse, extra.normal, extra.compound]
            .into_iter()
            .flatten()
        {
            assert!(r.size_code <= 4, "material {id}: size code {}", r.size_code);
            let texture = u32::try_from(r.texture).unwrap();
            for ids in &archives {
                assert!(
                    ids.contains(&texture),
                    "material {id}: texture {texture} missing"
                );
            }
        }
        let m = store910.get(id).unwrap();
        assert_eq!(
            m.diffuse_texture,
            extra.diffuse.and_then(|r| u32::try_from(r.texture).ok()),
            "material {id}"
        );
        assert_eq!(m.average_colour, extra.average_colour, "material {id}");
        assert_eq!(
            (m.repeat_s, m.repeat_t),
            (extra.repeat & 7, (extra.repeat >> 3) & 7),
            "material {id}"
        );
        let speed = |raw: Option<i16>| raw.map_or(0.0, |v| f32::from(v) * 127.0 / 32767.0 / 64.0);
        assert_eq!(
            (m.speed_u, m.speed_v),
            (speed(extra.speed_u_raw), speed(extra.speed_v_raw)),
            "material {id}"
        );
        assert_eq!(
            m.alpha_threshold,
            extra.alpha_threshold.unwrap_or(0xFF),
            "material {id}"
        );
    }
    assert_eq!(
        decode_rt7_extra(10456, &hex(MATERIAL_10456))
            .unwrap()
            .as_ref(),
        store.get(10456)
    );
}

/// Every mapsv2 group id.
fn map_groups(pack: &Pack) -> Vec<u32> {
    pack.read_archive_index(MAP_ARCHIVE)
        .unwrap()
        .group_id
        .into_iter()
        .collect()
}

/// The 910 LAND file (3): per-level 64x64 tiles `(settings, overlay raw,
/// shape, underlay raw, height)` and the trailer that follows (test
/// scaffolding over the 910 landscape reader and the LAND
/// environment framing, as `rs910_scene::map`/`env` decode it).
struct Land910<'a> {
    tiles: Vec<[i32; 5]>,
    trailer: &'a [u8],
}

fn land910(b: &[u8]) -> Land910<'_> {
    let mut r = rs910_core::reader::Reader::new(b);
    let mut tiles = Vec::with_capacity(4 * 64 * 64);
    for _ in 0..4 * 64 * 64 {
        let op = r.g1().unwrap();
        let mut t = [0, -1, -1, -1, -1];
        if op & 1 != 0 {
            t[2] = i32::from(r.g1().unwrap());
            t[1] = r.gsmart1or2().unwrap() - 1;
        }
        if op & 2 != 0 {
            t[0] = i32::from(r.g1().unwrap());
        }
        if op & 4 != 0 {
            t[3] = r.gsmart1or2().unwrap() - 1;
        }
        if op & 8 != 0 {
            t[4] = i32::from(r.g1().unwrap());
        }
        tiles.push(t);
    }
    Land910 {
        tiles,
        trailer: &b[r.pos()..],
    }
}

/// The 910 trailer fields NXT file 6/7 carry.
#[derive(Default)]
struct Trailer910 {
    env: Option<[i64; 9]>,
    lights: Vec<Vec<u8>>,
    bloom: Option<[u32; 3]>,
    remap: Option<(u16, u32)>,
    skybox: Option<u16>,
}

fn trailer910(bytes: &[u8]) -> Trailer910 {
    let mut r = rs910_core::reader::Reader::at(bytes, 8);
    let mut out = Trailer910::default();
    while r.remaining() > 0 {
        match r.g1().unwrap() {
            0 => {
                // Environment decode with the environment defaults.
                let bits = r.g1().unwrap();
                let g = |bit: u8, read: &mut dyn FnMut() -> i64, default: i64| {
                    if bits & bit != 0 {
                        read()
                    } else {
                        default
                    }
                };
                let mut rr = r;
                let sun = g(
                    1,
                    &mut || i64::from(rr.g4s().unwrap()) & 0xFF_FFFF,
                    0xFF_FFFF,
                );
                let amb = g(2, &mut || i64::from(rr.g2().unwrap()), 295);
                let dif = g(4, &mut || i64::from(rr.g2().unwrap()), 179);
                let sha = g(8, &mut || i64::from(rr.g2().unwrap()), 307);
                let (dx, dy, dz) = if bits & 16 != 0 {
                    (
                        i64::from(rr.g2s().unwrap()),
                        i64::from(rr.g2s().unwrap()),
                        i64::from(rr.g2s().unwrap()),
                    )
                } else {
                    (-50, -60, -50)
                };
                let fog = g(
                    32,
                    &mut || i64::from(rr.g4s().unwrap()) & 0xFF_FFFF,
                    13_156_520,
                );
                let fog_depth = g(64, &mut || i64::from(rr.g2().unwrap()), 0);
                let sampler = g(128, &mut || i64::from(rr.g2().unwrap()), 0xFFFF);
                r = rr;
                out.env = Some([
                    sun,
                    amb,
                    dif,
                    sha,
                    dx,
                    dy,
                    dz,
                    fog,
                    fog_depth * 0x1_0000 + sampler,
                ]);
            }
            1 => {
                let count = r.g1().unwrap();
                for _ in 0..count {
                    let start = r.pos();
                    r.skip(7).unwrap();
                    let radius = usize::from(r.g1().unwrap());
                    r.skip(2 * (2 * radius + 1) + 2).unwrap();
                    let flicker = r.g1().unwrap();
                    r.skip(2).unwrap();
                    if flicker & 0x1F == 31 {
                        r.skip(2).unwrap();
                    }
                    out.lights.push(bytes[start..r.pos()].to_vec());
                }
            }
            2 => {
                out.bloom = Some([0; 3].map(|_| r.g4s().unwrap() as u32));
            }
            3 => out.remap = Some((r.g2().unwrap(), r.g4s().unwrap() as u32)),
            128 => {
                out.skybox = Some(r.g2().unwrap());
                r.skip(8).unwrap();
            }
            129 => {
                for _ in 0..4 {
                    if r.g1().unwrap() == 1 {
                        r.skip(256).unwrap();
                    }
                }
            }
            130 => {}
            op => panic!("trailer opcode {op}"),
        }
    }
    out
}

/// Every light field but the coordinates.
#[derive(Debug, PartialEq, Eq)]
struct NxtLightRest(u8, u8, Vec<u16>, u16, u8, i16, Option<u16>);

impl NxtLightRest {
    fn of(l: &super::map_lights::NxtPointLight) -> Self {
        Self(
            l.flags,
            l.span_radius,
            l.span_runs.clone(),
            l.colour_hsl,
            l.flicker_phase,
            l.group,
            l.light_type,
        )
    }
}

/// Every PNG texture header (archive 53) parses: square, one mip, a single
/// image or a 6-face cube. Every 97th id of the block archives (DXT 52, ETC
/// 55) matches it: same faces, the DXT and ETC copies the same width, a
/// full DXT mip chain, ETC2 RGBA8.
#[test]
#[cfg_attr(feature = "no-pack", ignore)]
fn texture_headers_png_corpus_and_a_block_format_sample() {
    let pack = require_pack("client.textures.png.js5");
    let png = texture_ids(&pack, TextureArchive::Png);
    assert!(!png.is_empty());
    for &id in &png {
        let h = load_texture_header(&pack, TextureArchive::Png, id).unwrap();
        assert_eq!((h.width, h.mip_levels), (h.height, 1), "png {id}");
        assert!(matches!(h.faces, 1 | 6), "png {id}: {} faces", h.faces);
    }
    for &id in png.iter().step_by(97) {
        let png = load_texture_header(&pack, TextureArchive::Png, id).unwrap();
        let dxt = load_texture_header(&pack, TextureArchive::Dxt, id).unwrap();
        let etc = load_texture_header(&pack, TextureArchive::Etc, id).unwrap();
        assert_eq!(
            (dxt.width, dxt.faces),
            (etc.width, etc.faces),
            "texture {id}"
        );
        assert_eq!(dxt.faces, png.faces, "texture {id}");
        assert_eq!(dxt.mip_levels, dxt.width.ilog2() + 1, "texture {id}");
        assert_eq!(etc.format, PixelFormat::Etc2Rgba8);
    }
}

/// The whole-corpus census of archives 52, 54 and 55 (3.7 GB compressed:
/// minutes at the test profile's opt-level, so it runs on demand with
/// `--ignored`). Pins every (format, size, mips) class.
#[test]
#[ignore = "heavy: decompresses archives 52, 54 and 55 (3.7 GB); run with --ignored"]
fn texture_headers_block_archive_census() {
    let pack = require_pack("client.textures.dxt.js5");
    let png: BTreeSet<u32> = texture_ids(&pack, TextureArchive::Png);
    let mut classes = BTreeMap::<(TextureArchive, u8, u8, u32), usize>::new();
    let mut headers = BTreeMap::<(TextureArchive, u32), TextureHeader>::new();
    for archive in [
        TextureArchive::Dxt,
        TextureArchive::PngMipped,
        TextureArchive::Etc,
    ] {
        let ids = texture_ids(&pack, archive);
        for id in ids {
            let h = load_texture_header(&pack, archive, id).unwrap();
            assert!(png.contains(&id));
            assert_eq!(h.width, h.height, "{archive:?} {id}");
            assert_eq!(h.mip_levels, h.width.ilog2() + 1, "{archive:?} {id}");
            let expected_format = match (archive, h.faces) {
                (TextureArchive::Dxt, 1) => PixelFormat::Dxt5,
                (TextureArchive::Dxt, _) => PixelFormat::Dxt1,
                (TextureArchive::Etc, _) => PixelFormat::Etc2Rgba8,
                _ => h.format,
            };
            assert_eq!(h.format, expected_format, "{archive:?} {id}");
            *classes
                .entry((archive, h.version, h.faces, h.width))
                .or_default() += 1;
            headers.insert((archive, id), h);
        }
    }
    let count = |a| headers.keys().filter(|(x, _)| *x == a).count();
    assert_eq!(
        (
            count(TextureArchive::Dxt),
            count(TextureArchive::PngMipped),
            count(TextureArchive::Etc)
        ),
        (14_305, 14_253, 14_305)
    );
    for ((archive, id), h) in &headers {
        if *archive != TextureArchive::Dxt {
            let dxt = headers[&(TextureArchive::Dxt, *id)];
            assert_eq!(
                (h.width, h.faces),
                (dxt.width, dxt.faces),
                "{archive:?} {id}"
            );
        }
    }
    // RT7 size codes name the guttered size: width == (64 << k) + 64.
    let store = Rt7MaterialExtraStore::load(&pack).unwrap();
    for extra in store.iter() {
        for r in [extra.diffuse, extra.normal, extra.compound]
            .into_iter()
            .flatten()
        {
            let dxt = headers[&(TextureArchive::Dxt, r.texture as u32)];
            assert_eq!(dxt.width, r.size() + 64, "material {}", extra.id);
        }
    }
    // (archive, version, faces, width) -> textures. DXT cubemaps are one
    // 6-face DDS in a version-1 frame; ETC and PNG cubemaps are 6 frames.
    let sizes = [64, 128, 192, 256, 320, 576, 1088];
    let single = [2, 1625, 6415, 12, 3656, 2146, 339];
    let mipped = [2, 1615, 6410, 12, 3644, 2125, 335];
    let mut expected = BTreeMap::new();
    for (archive, version, counts) in [
        (TextureArchive::Dxt, 1, single),
        (TextureArchive::Etc, 1, single),
        (TextureArchive::PngMipped, 1, mipped),
    ] {
        for (width, count) in sizes.into_iter().zip(counts) {
            expected.insert((archive, version, 1, width), count);
        }
        let cube_version = if archive == TextureArchive::Dxt { 1 } else { 6 };
        expected.insert((archive, cube_version, 6, 128), 68);
        expected.insert((archive, cube_version, 6, 512), 42);
    }
    assert_eq!(classes, expected);
}

// ---------------------------------------------------------------------------
// RT7 models (archive 47, renderer plan M10).
// ---------------------------------------------------------------------------

/// A synthetic RT7 model with every conditional field: one mesh (colours,
/// alphas, face and vertex labels), two LODs, one billboard item, one
/// emitter, one effector.
#[test]
fn rt7_model_decodes_a_synthetic_model() {
    use super::model_rt7::{decode_model_rt7, MESH_HIDDEN};
    let mut b = vec![2, 3, 15];
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&[1, 1, 1]);
    b.extend_from_slice(&0x0fu32.to_le_bytes());
    b.push(4); // priority
    b.extend_from_slice(&1494u16.to_le_bytes()); // material 1493
    b.extend_from_slice(&1u16.to_le_bytes()); // one face
    b.extend_from_slice(&0x1234u16.to_le_bytes()); // colour
    b.push(200); // alpha
    b.extend_from_slice(&(-1i16).to_le_bytes()); // face label
    b.push(2); // LODs
    for _ in 0..2 {
        b.extend_from_slice(&3u16.to_le_bytes());
        for i in [0u16, 1, 2] {
            b.extend_from_slice(&i.to_le_bytes());
        }
    }
    b.extend_from_slice(&3u16.to_le_bytes()); // vertices
    for p in [[0i16, 11, 0], [512, 11, 0], [0, 11, 512]] {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    b.extend_from_slice(&[0, 127, 0, 0, 127, 0, 0, 127, 0]); // normals
    b.extend_from_slice(&[127, 0, 0, 127, 127, 0, 0, 0x80, 127, 0, 0, 127]);
    // UVs, big-endian f16: 0.0, 1.0 (0x3c00), 0.5 (0x3800).
    b.extend_from_slice(&[0, 0, 0, 0, 0x3c, 0, 0, 0, 0, 0, 0x38, 0]);
    for l in [3i16, 3, -1] {
        b.extend_from_slice(&l.to_le_bytes());
    }
    // Billboard: priority 7, material 3017, no second, one item.
    b.push(7);
    b.extend_from_slice(&3018u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    for f in [1.0f32, -2.0, 3.0, 64.0, 32.0] {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b.extend_from_slice(&0x2222u16.to_le_bytes());
    b.push(255);
    for l in [1i16, 2, 3] {
        b.extend_from_slice(&l.to_le_bytes());
    }
    b.push(9);
    // Emitter 12 over three points; effector 5.
    b.extend_from_slice(&12u16.to_le_bytes());
    for k in 0..3u16 {
        for f in [f32::from(k), 0.0, 1.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(&k.to_le_bytes());
    }
    b.extend_from_slice(&5u16.to_le_bytes());
    for f in [4.0f32, 5.0, 6.0] {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b.extend_from_slice(&7u16.to_le_bytes());
    let m = decode_model_rt7(0, &b).unwrap();
    assert_eq!((m.magic, m.model_version, m.meshes.len()), ([2, 3], 15, 1));
    let mesh = &m.meshes[0];
    assert_eq!(
        (mesh.priority, mesh.material, mesh.face_count()),
        (4, Some(1493), 1)
    );
    assert!(!mesh.hidden() && mesh.flags & MESH_HIDDEN == 0);
    assert_eq!(
        (&mesh.face_colours[..], &mesh.face_alphas[..]),
        (&[0x1234][..], &[200][..])
    );
    assert_eq!(mesh.lods, vec![vec![0, 1, 2], vec![0, 1, 2]]);
    assert_eq!(mesh.positions[1], [512, 11, 0]);
    assert_eq!(mesh.normals[2], [0, 127, 0]);
    assert_eq!(mesh.tangents[1], [127, 0, 0, -128]);
    assert_eq!(mesh.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 0.5]]);
    assert_eq!(mesh.vertex_labels, vec![3, 3, -1]);
    assert_eq!(m.billboards[0].material, Some(3017));
    assert_eq!(m.billboards[0].material2, None);
    assert_eq!(
        (m.billboards[0].size, m.billboards[0].depth_offset),
        ([64.0, 32.0], 9)
    );
    assert_eq!(m.emitters[0].points[2], ([2.0, 0.0, 1.0], 2));
    assert_eq!((m.effectors[0].kind, m.effectors[0].label), (5, 7));
    // A trailing byte and a truncation are errors.
    let mut long = b.clone();
    long.push(0);
    assert!(decode_model_rt7(0, &long).is_err());
    assert!(decode_model_rt7(0, &b[..b.len() - 1]).is_err());
}

/// The per-model statistics the census pins.
#[derive(Default, Debug, PartialEq, Eq)]
struct Rt7Census {
    groups: usize,
    meshes: usize,
    hidden: usize,
    faces: usize,
    vertices: usize,
    billboards: usize,
    emitters: usize,
    effectors: usize,
    untextured: usize,
    over_16_bit: usize,
}

fn rt7_census(pack: &Pack, step: usize) -> (Rt7Census, BTreeMap<u8, usize>) {
    use super::model_rt7::{decode_model_rt7, MODEL_RT7_ARCHIVE};
    let groups: Vec<u32> = pack
        .read_archive_index(MODEL_RT7_ARCHIVE)
        .unwrap()
        .group_id
        .into_iter()
        .collect();
    assert_eq!(groups.len(), 117_217);
    let mut c = Rt7Census::default();
    let mut versions = BTreeMap::new();
    for &group in groups.iter().step_by(step) {
        let files = pack.read_group(MODEL_RT7_ARCHIVE, group).unwrap();
        assert_eq!(
            files.keys().copied().collect::<Vec<_>>(),
            vec![0],
            "group {group}"
        );
        let m = decode_model_rt7(group, &files[&0]).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(m.magic, [2, 3], "group {group}");
        *versions.entry(m.model_version).or_default() += 1;
        c.groups += 1;
        for mesh in &m.meshes {
            c.meshes += 1;
            c.hidden += usize::from(mesh.hidden());
            c.faces += mesh.face_count();
            c.vertices += mesh.positions.len();
            c.untextured += usize::from(mesh.material.is_none());
            c.over_16_bit += usize::from(mesh.face_count() * 3 > 0xFFFF);
            assert_eq!(mesh.face_colours.len(), mesh.face_count(), "group {group}");
            assert!(mesh.tangents.iter().all(|t| t[3] == 127 || t[3] == -128));
            for w in mesh.lods.windows(2) {
                assert!(w[1].len() <= w[0].len(), "group {group}: a longer LOD");
            }
        }
        c.billboards += m.billboards.len();
        c.emitters += m.emitters.len();
        c.effectors += m.effectors.len();
    }
    (c, versions)
}

/// A fast sample of archive 47 (every 97th group) decodes exactly.
#[test]
#[cfg_attr(feature = "no-pack", ignore)]
fn rt7_models_decode_a_sample() {
    let pack = require_pack("client.modelsrt7.js5");
    let (c, _) = rt7_census(&pack, 97);
    assert_eq!(c.groups, 1209);
    assert!(c.meshes > 5000 && c.faces > 700_000, "{c:?}");
}

/// The whole of archive 47: every group decodes and is consumed exactly
/// (`nxt-data-formats.md` §9 pins these counts). Heavy (3.7 GB decoded),
/// so ignored by default like the texture census.
#[test]
#[ignore = "heavy: decodes all of archive 47 (needs server/data/pack)"]
fn rt7_models_consume_every_group() {
    let pack = require_pack("client.modelsrt7.js5");
    let (c, versions) = rt7_census(&pack, 1);
    eprintln!("{c:?} {versions:?}");
    assert_eq!(
        c,
        Rt7Census {
            groups: 117_217,
            meshes: 626_797,
            hidden: 7_318,
            faces: 74_340_981,
            vertices: 131_937_819,
            billboards: 44_346,
            emitters: 18_731,
            effectors: 123,
            untextured: 101_767,
            over_16_bit: 1,
        }
    );
}

// ---------------------------------------------------------------------------
// Map file 5 against the 910 LAND files: heights, water fields and the
// tile colour (renderer plan M10, `nxt-data-formats.md` §2).
// ---------------------------------------------------------------------------

/// [`land910`] of an `UNDERWATER_LAND` file (4), which may hold fewer
/// levels: the tiles up to the end of the file.
fn land910_levels(b: &[u8]) -> Land910<'_> {
    let mut r = rs910_core::reader::Reader::new(b);
    let mut tiles = Vec::with_capacity(4 * 64 * 64);
    while tiles.len() < 4 * 4096 && r.remaining() > 0 {
        let Ok(op) = r.g1() else { break };
        let mut t = [0, -1, -1, -1, -1];
        if op & 1 != 0 {
            t[2] = i32::from(r.g1().unwrap());
            t[1] = r.gsmart1or2().unwrap() - 1;
        }
        if op & 2 != 0 {
            t[0] = i32::from(r.g1().unwrap());
        }
        if op & 4 != 0 {
            t[3] = r.gsmart1or2().unwrap() - 1;
        }
        if op & 8 != 0 {
            t[4] = i32::from(r.g1().unwrap());
        }
        tiles.push(t);
    }
    Land910 {
        tiles,
        trailer: &[],
    }
}

/// the 910 client's LAND heights of one square (the landscape reader, rotation
/// 0; 910 y, down positive): per level 64 x 64 vertices, the explicit byte
/// (`1` as 0) or the level-0 noise / 960 above the level below.
fn heights910(land: &Land910<'_>, square: (i32, i32)) -> Vec<i32> {
    let mut h = vec![0; 4 * 4096];
    for level in 0..4 {
        for x in 0..64 {
            for z in 0..64 {
                let i = level * 4096 + x * 64 + z;
                let raw = land.tiles[i][4];
                h[i] = if raw >= 0 {
                    let b = if raw == 1 { 0 } else { raw };
                    if level == 0 {
                        -b * 32
                    } else {
                        h[i - 4096] - b * 32
                    }
                } else if level == 0 {
                    let (ax, az) = (square.0 * 64 + x as i32, square.1 * 64 + z as i32);
                    -rs910_core::perlin::perlin(ax + 932_731, az + 556_238) * 32
                } else {
                    h[i - 4096] - 960
                };
            }
        }
    }
    h
}

/// 865's `DecodeHeightCode` offset (L186137): `32 c`, `c == 1` none,
/// `c == 0` 960.
fn nxt_offset(code: u8) -> i32 {
    match code {
        0 => 960,
        1 => 0,
        c => 32 * i32::from(c),
    }
}

// ---------------------------------------------------------------------------
// The mapsv2 corpus: one pass, every square against the 910 LAND files.
// ---------------------------------------------------------------------------

/// Squares with level-0 water columns whose surface height code is 0 (NXT:
/// 960 below the level, 865 `DecodeHeightCode`) where the 910 LAND file
/// has no height byte, so 910 takes the Perlin noise
/// (the landscape reader). Only those columns may differ from the 910 client,
/// and each level above carries the same difference.
const NOISE_WATER_SQUARES: [u32; 17] = [
    6188, 9383, 9398, 9653, 10024, 13350, 13351, 13352, 13479, 13607, 14479, 18482, 18855, 19124,
    20263, 20396, 20520,
];

/// Squares whose precomputed floor colour (the `u16` after an underlay)
/// differs from the 910 client's ground-builder blend over file 5's own
/// underlays on some inner tile: hue wrap and squares missing beside their
/// neighbours (lane Q-M10's research).
const COLOUR_BLEND_SQUARES: [u32; 8] = [5447, 5448, 5575, 12320, 12322, 12323, 12450, 13000];

/// Map files 5-8 of every square decode strictly with in-range fields, and
/// agree with the 910 files of the same square: file 6/7 with the LAND
/// trailer (`Environment.decode` values, skybox, bloom, colour remap, the
/// point lights with coordinates shifted by 2); file 5 with LAND (3) and
/// `UNDERWATER_LAND` (4) tile by tile (settings, underlay, overlay, shape,
/// height byte; a water tile's bed fields from file 4 and its surface
/// fields from file 3), with the 910 client's heights (`heights910`) and with
/// the 910 client's floor-colour blend. Every failure names the square; the only
/// tolerated differences are the named squares above, and each of them
/// must still show its difference.
#[test]
#[cfg_attr(feature = "no-pack", ignore)]
fn map_squares_decode_and_match_the_910_land_files() {
    let pack = require_pack("client.mapsv2.js5");
    let flo = crate::flo::FloStore::load(&pack).unwrap();
    let water_types = super::water_type::NxtWaterTypes::load(&pack).unwrap();
    let config_ids = |group: u32| -> BTreeSet<u32> {
        pack.read_group(crate::flo::FLO_ARCHIVE, group)
            .unwrap()
            .into_keys()
            .collect()
    };
    // Skybox types (config group 29) and light types (group 31).
    let (skyboxes, light_types) = (config_ids(29), config_ids(31));
    let mut noise_seen = BTreeSet::new();
    let mut colour_seen = BTreeSet::new();
    for group in map_groups(&pack) {
        let files = pack.read_group(MAP_ARCHIVE, group).unwrap();
        let terrain_bytes = files.get(&TERRAIN_FILE).expect("file 5 in every group");
        let terrain = decode_terrain(group, terrain_bytes).unwrap();
        for level in &terrain {
            assert!(level.level < 4, "group {group}: level {}", level.level);
            for tile in &level.tiles {
                if let Some(shape) = tile.overlay_shape {
                    assert!(shape >> 2 <= 12, "group {group}: overlay shape {shape}");
                }
            }
        }
        let Some(land_bytes) = files.get(&3) else {
            assert!(terrain.is_empty(), "group {group}: terrain without LAND");
            assert!(
                !(6..=8).any(|f| files.contains_key(&f)),
                "group {group}: files 6-8 without LAND"
            );
            continue;
        };
        assert!(!terrain.is_empty(), "group {group}: LAND without terrain");
        let land = land910(land_bytes);
        let trailer = trailer910(land.trailer);
        check_environment(group, &files[&6], &trailer, &skyboxes);
        check_point_lights(group, &files[&7], &trailer, &light_types);
        for p in decode_water(group, &files[&8]).unwrap() {
            let axis = p.rotation_axis;
            assert!(
                axis.iter()
                    .chain([&p.rotation_turns])
                    .all(|v| v.abs() <= 1.0),
                "group {group}: water rotation"
            );
            assert!(
                water_types.get(u32::from(p.water_type)).is_some(),
                "group {group}: water type {}",
                p.water_type
            );
        }
        check_terrain_tiles(group, &terrain, &land, files.get(&4).map(Vec::as_slice));
        if check_terrain_heights(group, &terrain, &land) {
            assert!(
                NOISE_WATER_SQUARES.contains(&group),
                "group {group}: a noise-water height difference outside the known squares"
            );
            noise_seen.insert(group);
        }
        if check_floor_colours(&terrain, &flo) {
            assert!(
                COLOUR_BLEND_SQUARES.contains(&group),
                "group {group}: floor colour differs from the 910 client's blend"
            );
            colour_seen.insert(group);
        }
    }
    // The exception lists stay exact: a square that now agrees must leave.
    assert_eq!(noise_seen, BTreeSet::from(NOISE_WATER_SQUARES));
    assert_eq!(colour_seen, BTreeSet::from(COLOUR_BLEND_SQUARES));
}

/// File 6 decodes with finite, bounded values and equals the LAND
/// trailer's environment (environment defaults when absent), skybox,
/// bloom and colour remap.
fn check_environment(group: u32, bytes: &[u8], t: &Trailer910, skyboxes: &BTreeSet<u32>) {
    let e = decode_environment(group, bytes).unwrap();
    assert!(e.sun_colour <= 0xFF_FFFF && e.fog_colour <= 0xFF_FFFF);
    let floats = e
        .f_27
        .iter()
        .chain(&e.scattering_params)
        .chain(e.scattering.iter().flatten())
        .chain(&e.f_138)
        .chain(&e.bloom)
        .chain(std::iter::once(&e.f_16));
    for v in floats {
        assert!(v.is_finite() && v.abs() <= 50_000.0, "group {group}: {v}");
    }
    for (_, weight) in e.colour_remap {
        assert!(
            (0.0..=1.0).contains(&weight),
            "group {group}: remap weight {weight}"
        );
    }
    if let Some(skybox) = e.skybox {
        assert!(
            skyboxes.contains(&u32::from(skybox)),
            "group {group}: skybox {skybox}"
        );
    }
    let env910 = t
        .env
        .unwrap_or([0xFF_FFFF, 295, 179, 307, -50, -60, -50, 13_156_520, 0xFFFF]);
    let nxt_env = [
        i64::from(e.sun_colour),
        i64::from(e.sun_intensity_raw[0]),
        i64::from(e.sun_intensity_raw[1]),
        i64::from(e.sun_intensity_raw[2]),
        i64::from(e.sun_direction[0]),
        i64::from(e.sun_direction[1]),
        i64::from(e.sun_direction[2]),
        i64::from(e.fog_colour),
        i64::from(e.fog_depth) * 0x1_0000 + i64::from(e.sampler_material.unwrap_or(0xFFFF)),
    ];
    assert_eq!(nxt_env, env910, "group {group}: environment");
    assert_eq!(e.skybox, t.skybox, "group {group}: skybox");
    if let Some(bloom) = t.bloom {
        assert_eq!(e.bloom.map(f32::to_bits), bloom, "group {group}: bloom");
    }
    match t.remap {
        Some((id, weight)) => assert_eq!(
            e.colour_remap[0],
            (Some(id), f32::from_bits(weight)),
            "group {group}: remap"
        ),
        // No 910 remap: NXT has none, or an id with weight 0.
        None => assert_eq!(e.colour_remap[0].1, 0.0, "group {group}: remap"),
    }
}

/// File 7 holds the LAND trailer's lights, record for record, with the
/// coordinates pre-shifted by 2; levels and light types are in range.
fn check_point_lights(group: u32, bytes: &[u8], t: &Trailer910, light_types: &BTreeSet<u32>) {
    let nxt = decode_point_lights(group, bytes).unwrap();
    assert_eq!(nxt.len(), t.lights.len(), "group {group}: light count");
    for (n, j) in nxt.iter().zip(&t.lights) {
        assert!(n.level() < 4, "group {group}: level {}", n.level());
        if let Some(kind) = n.light_type {
            assert!(
                light_types.contains(&u32::from(kind)),
                "group {group}: light type {kind}"
            );
        }
        let mut lights910 = decode_point_lights(group, &[&[1_u8][..], j].concat()).unwrap();
        let light910 = lights910.remove(0);
        let shift = |v: u16| v.wrapping_shl(2);
        assert_eq!(
            (n.x, n.z, n.y),
            (shift(light910.x), shift(light910.z), shift(light910.y)),
            "group {group}"
        );
        assert_eq!(
            NxtLightRest::of(n),
            NxtLightRest::of(&light910),
            "group {group}"
        );
    }
}

/// File 5 against LAND (3) on the inner 64 x 64 tiles: settings equal
/// everywhere; a land tile's underlay, overlay, shape and height byte are
/// LAND's (`1` stored as 0); a water tile's underlay, overlay, shape and
/// depth byte are `UNDERWATER_LAND`'s (4), its water overlay LAND's
/// overlay and its water underlay (with an overlay) LAND's underlay.
fn check_terrain_tiles(
    group: u32,
    terrain: &[super::map_terrain::NxtTerrainLevel],
    land: &Land910<'_>,
    underwater: Option<&[u8]>,
) {
    let under = underwater.map(|b| land910_levels(b));
    let id = |v: Option<u16>| v.map_or(-1, i32::from);
    for block in terrain {
        let level = block.level;
        for x in 0..64 {
            for z in 0..64 {
                let t = block.tile(x + 1, z + 1).unwrap();
                let i = usize::from(level) * 4096 + x * 64 + z;
                let j = land.tiles[i];
                let at = format!("group {group} level {level} tile {x},{z}");
                assert_eq!(i32::from(t.settings()), j[0], "{at}: settings");
                if t.is_water() {
                    let w = under
                        .as_ref()
                        .and_then(|u| u.tiles.get(i))
                        .unwrap_or_else(|| panic!("{at}: water without file 4"));
                    assert_eq!(i32::from(t.height), w[4].max(0), "{at}: depth byte");
                    assert_eq!(
                        (id(t.underlay), id(t.overlay)),
                        (w[3], w[1]),
                        "{at}: bed underlay/overlay"
                    );
                    if let Some(shape) = t.overlay_shape {
                        assert_eq!(i32::from(shape), w[2], "{at}: bed shape");
                    }
                    let underlay910 = if t.overlay.is_some() { j[3] } else { -1 };
                    assert_eq!(
                        (id(t.water_overlay), id(t.water_underlay)),
                        (j[1], underlay910),
                        "{at}: surface overlay/underlay"
                    );
                } else {
                    assert_eq!(
                        (id(t.underlay), id(t.overlay)),
                        (j[3], j[1]),
                        "{at}: underlay/overlay"
                    );
                    if let Some(shape) = t.overlay_shape {
                        assert_eq!(i32::from(shape), j[2], "{at}: shape");
                    }
                    if j[4] >= 0 {
                        let h = i32::from(t.height);
                        assert!(h == j[4] || (j[4] == 1 && h == 0), "{at}: height byte");
                    }
                }
            }
        }
    }
}

/// File 5's heights (`y_910 = -y_up`, a code step 32 fine units, 960 per
/// implicit level, a missing level 960 more; a water tile's height is its
/// surface) equal [`heights910`] on every inner tile, except the
/// [`NOISE_WATER_SQUARES`] case: a level-0 water surface with code 0 where
/// LAND has no height byte, whose difference every level above carries.
/// Returns whether that exception occurred.
fn check_terrain_heights(
    group: u32,
    terrain: &[super::map_terrain::NxtTerrainLevel],
    land: &Land910<'_>,
) -> bool {
    let square = ((group & 0x7F) as i32, (group >> 7) as i32);
    let heights = heights910(land, square);
    let mut carried = vec![0_i32; 64 * 64];
    let mut noise = false;
    let mut below = vec![0_i32; TERRAIN_SIDE * TERRAIN_SIDE];
    for level in 0..4u8 {
        let Some(block) = terrain.iter().find(|b| b.level == level) else {
            if level > 0 {
                below.iter_mut().for_each(|b| *b += 960);
            }
            continue;
        };
        let up: Vec<i32> = block
            .tiles
            .iter()
            .zip(&below)
            .map(|(t, &b)| b + nxt_offset(t.water_height.unwrap_or(t.height)))
            .collect();
        for x in 0..64 {
            for z in 0..64 {
                let t = block.tile(x + 1, z + 1).unwrap();
                let i = usize::from(level) * 4096 + x * 64 + z;
                let diff = -up[(x + 1) * TERRAIN_SIDE + z + 1] - heights[i];
                if diff == carried[x * 64 + z] {
                    continue;
                }
                let noise_water = level == 0 && t.water_height == Some(0) && land.tiles[i][4] < 0;
                assert!(
                    noise_water,
                    "group {group} level {level} tile {x},{z}: height {diff} off the 910 land file"
                );
                carried[x * 64 + z] = diff;
                noise = true;
            }
        }
        below = up;
    }
    noise
}

/// The `u16` after an underlay is the 910 client's floor colour
/// (the ground builder: the 10 x 10 window `x-4..x+5`, hue weighted by
/// chroma, `hsl24to16`) over file 5's own underlay ids, where the window
/// lies inside the 66 x 66 block (local 3..=59). Returns whether some tile
/// differs.
fn check_floor_colours(
    terrain: &[super::map_terrain::NxtTerrainLevel],
    flo: &crate::flo::FloStore,
) -> bool {
    let hsl = |id: Option<u16>| id.and_then(|id| flo.get_underlay(u32::from(id)));
    let mut differs = false;
    for block in terrain {
        for x in 3..=59usize {
            for z in 3..=59usize {
                let t = block.tile(x + 1, z + 1).unwrap();
                let Some(colour) = t.underlay_u16 else {
                    continue;
                };
                let (mut hue, mut sat, mut lum, mut chroma, mut count) =
                    (0i64, 0i64, 0i64, 0i64, 0i64);
                // Local `x-4..=x+5` is block index `x-3..=x+6` (border 1).
                for bx in x - 3..=x + 6 {
                    for bz in z - 3..=z + 6 {
                        let bt = block.tile(bx, bz).unwrap();
                        if let Some(u) = hsl(bt.underlay) {
                            hue += i64::from(u.hue);
                            sat += i64::from(u.saturation);
                            lum += i64::from(u.lightness);
                            chroma += i64::from(u.chroma);
                            count += 1;
                        }
                    }
                }
                if chroma > 0 && count > 0 {
                    let want = rs910_core::colour::hsl24to16(
                        (hue * 256 / chroma) as i32,
                        (sat / count) as i32,
                        (lum / count) as i32,
                    );
                    differs |= i32::from(colour) != want & 0xFFFF;
                }
            }
        }
    }
    differs
}
