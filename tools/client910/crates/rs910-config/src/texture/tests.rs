use super::*;

// Golden vectors generated with CPython's zlib (independent encoder).
const RGBA_1X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 208, 0, 0, 4,
    129, 1, 128, 44, 85, 206, 176, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
const GRAY_2X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 0, 0,
    0, 0, 209, 73, 32, 86, 0, 0, 0, 11, 73, 68, 65, 84, 120, 156, 99, 16, 248, 0, 0, 1, 19, 1, 1,
    117, 91, 102, 252, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
const INDEXED_2X2: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 3, 0,
    0, 0, 69, 104, 253, 22, 0, 0, 0, 9, 80, 76, 84, 69, 255, 0, 0, 0, 255, 0, 0, 0, 255, 45, 74,
    205, 138, 0, 0, 0, 3, 116, 82, 78, 83, 255, 128, 0, 127, 109, 104, 120, 0, 0, 0, 14, 73, 68,
    65, 84, 120, 156, 99, 96, 96, 100, 96, 98, 0, 0, 0, 14, 0, 4, 198, 136, 124, 248, 0, 0, 0, 0,
    73, 69, 78, 68, 174, 66, 96, 130,
];
const RGB_FILTERS_3X2: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 3, 0, 0, 0, 2, 8, 2, 0,
    0, 0, 18, 22, 241, 77, 0, 0, 0, 25, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 192, 0, 198,
    76, 12, 255, 255, 51, 48, 50, 52, 52, 52, 2, 0, 62, 242, 6, 128, 29, 116, 208, 184, 0, 0, 0, 0,
    73, 69, 78, 68, 174, 66, 96, 130,
];
const GRAY_ALPHA_2X3: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 3, 8, 4, 0,
    0, 0, 19, 227, 22, 10, 0, 0, 0, 21, 73, 68, 65, 84, 120, 156, 99, 225, 58, 193, 197, 200, 44,
    153, 198, 207, 196, 36, 2, 132, 0, 16, 176, 1, 163, 206, 98, 253, 149, 0, 0, 0, 0, 73, 69, 78,
    68, 174, 66, 96, 130,
];
const RAW_PAYLOAD: &[u8] =
    b"hello 910 textures hello 910 textures hello 910 textures hello 910 textures ";
const ZLIB_STORED: &[u8] = &[
    120, 1, 1, 76, 0, 179, 255, 104, 101, 108, 108, 111, 32, 57, 49, 48, 32, 116, 101, 120, 116,
    117, 114, 101, 115, 32, 104, 101, 108, 108, 111, 32, 57, 49, 48, 32, 116, 101, 120, 116, 117,
    114, 101, 115, 32, 104, 101, 108, 108, 111, 32, 57, 49, 48, 32, 116, 101, 120, 116, 117, 114,
    101, 115, 32, 104, 101, 108, 108, 111, 32, 57, 49, 48, 32, 116, 101, 120, 116, 117, 114, 101,
    115, 32, 241, 133, 26, 73,
];
const ZLIB_FIXED: &[u8] = &[
    24, 25, 203, 72, 205, 201, 201, 87, 176, 52, 52, 80, 40, 73, 173, 40, 41, 45, 74, 45, 86, 200,
    32, 87, 8, 0, 241, 133, 26, 73,
];
const ZLIB_DYNAMIC: &[u8] = &[
    120, 156, 203, 72, 205, 201, 201, 87, 176, 52, 52, 80, 40, 73, 173, 40, 41, 45, 74, 45, 86,
    200, 32, 87, 8, 0, 241, 133, 26, 73,
];
const ZLIB_EMPTY: &[u8] = &[120, 156, 3, 0, 0, 0, 0, 1];

#[test]
fn zlib_stored_block_roundtrips() {
    assert_eq!(
        inflate_zlib(ZLIB_STORED, RAW_PAYLOAD.len()).unwrap(),
        RAW_PAYLOAD
    );
}

#[test]
fn zlib_fixed_huffman_roundtrips() {
    assert_eq!(
        inflate_zlib(ZLIB_FIXED, RAW_PAYLOAD.len()).unwrap(),
        RAW_PAYLOAD
    );
}

#[test]
fn zlib_dynamic_huffman_roundtrips() {
    assert_eq!(
        inflate_zlib(ZLIB_DYNAMIC, RAW_PAYLOAD.len()).unwrap(),
        RAW_PAYLOAD
    );
}

#[test]
fn zlib_empty_stream_roundtrips() {
    assert_eq!(inflate_zlib(ZLIB_EMPTY, 0).unwrap(), b"");
}

#[test]
fn zlib_rejects_bad_adler_and_size() {
    let mut bad = ZLIB_DYNAMIC.to_vec();
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;
    assert!(inflate_zlib(&bad, RAW_PAYLOAD.len()).is_err());
    assert!(inflate_zlib(ZLIB_DYNAMIC, RAW_PAYLOAD.len() + 1).is_err());
    assert!(inflate_zlib(&[0x78, 0x9C], 1).is_err());
    assert!(inflate_zlib(&[0x78, 0x9D], 1).is_err());
}

#[test]
fn png_rgba_1x1_decodes() {
    let img = decode_png(RGBA_1X1).unwrap();
    assert_eq!((img.w, img.h), (1, 1));
    assert_eq!(img.px, vec![255, 0, 0, 128]);
    assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 128]));
    assert_eq!(img.pixel(1, 0), None);
}

#[test]
fn png_gray_expands_to_rgb() {
    let img = decode_png(GRAY_2X1).unwrap();
    assert_eq!((img.w, img.h), (2, 1));
    assert_eq!(img.px, vec![0x10, 0x10, 0x10, 255, 0xF0, 0xF0, 0xF0, 255]);
}

#[test]
fn png_indexed_applies_trns_alpha() {
    let img = decode_png(INDEXED_2X2).unwrap();
    assert_eq!((img.w, img.h), (2, 2));
    assert_eq!(
        img.px,
        vec![
            255, 0, 0, 255, 0, 255, 0, 128, // row 0: red opaque, green half
            0, 0, 255, 0, 255, 0, 0, 255, // row 1: blue clear, red opaque
        ]
    );
}

#[test]
fn png_rgb_unfilters_up_row() {
    // Row 0 filter None (red/green/blue), row 1 filter Up (white/black/gray).
    let img = decode_png(RGB_FILTERS_3X2).unwrap();
    assert_eq!((img.w, img.h), (3, 2));
    assert_eq!(
        img.px,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, //
            255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 255,
        ]
    );
}

#[test]
fn png_gray_alpha_unfilters_paeth_average_up() {
    // Rows use Paeth / Average / Up over gray+alpha samples.
    let img = decode_png(GRAY_ALPHA_2X3).unwrap();
    assert_eq!((img.w, img.h), (2, 3));
    assert_eq!(
        img.px,
        vec![
            10, 10, 10, 200, 20, 20, 20, 201, //
            30, 30, 30, 202, 40, 40, 40, 203, //
            50, 50, 50, 204, 60, 60, 60, 205,
        ]
    );
}

#[test]
fn png_rejects_bad_inputs_by_name() {
    assert!(decode_png(b"not a png").is_err());
    assert!(decode_png(&RGBA_1X1[..20]).is_err());
    // 16-bit depth names the depth.
    let mut deep = RGBA_1X1.to_vec();
    deep[24] = 16;
    fix_ihdr_crc(&mut deep);
    let error = decode_png(&deep).unwrap_err().to_string();
    assert!(error.contains("bit depth 16"), "got: {error}");
    // Interlaced names the method (IHDR offset 28).
    let mut adam7 = RGBA_1X1.to_vec();
    adam7[28] = 1;
    fix_ihdr_crc(&mut adam7);
    let error = decode_png(&adam7).unwrap_err().to_string();
    assert!(error.contains("interlace"), "got: {error}");
    // Corrupt a CRC.
    let mut crc = RGBA_1X1.to_vec();
    let last = crc.len() - 5;
    crc[last] ^= 0xFF;
    assert!(decode_png(&crc).is_err());
}

/// Recompute the IHDR CRC after mutating the golden header bytes.
fn fix_ihdr_crc(png: &mut [u8]) {
    let mut check = Vec::with_capacity(17);
    check.extend_from_slice(&png[12..12 + 4 + 13]);
    let crc = crc32(&check);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
}

fn framed_single(png: &[u8]) -> Vec<u8> {
    let mut file = vec![1_u8];
    file.extend_from_slice(&(png.len() as u32).to_be_bytes());
    file.extend_from_slice(png);
    file
}

#[test]
fn framing_splits_single_and_cube() {
    let single = framed_single(RGBA_1X1);
    let faces = split_png_file(&single).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(decode_png(faces[0]).unwrap().px, vec![255, 0, 0, 128]);

    let mut cube = vec![6_u8];
    for _ in 0..6 {
        cube.extend_from_slice(&(GRAY_2X1.len() as u32).to_be_bytes());
        cube.extend_from_slice(GRAY_2X1);
    }
    let faces = split_png_file(&cube).unwrap();
    assert_eq!(faces.len(), 6);
    for face in faces {
        assert_eq!(decode_png(face).unwrap().w, 2);
    }

    assert!(split_png_file(&[]).is_err());
    assert!(split_png_file(&[7_u8]).is_err());
    let mut trailing = single.clone();
    trailing.push(0);
    assert!(split_png_file(&trailing).is_err());
}

/// Real material 0 bytes from the pack (RT5, 21 bytes).
const MAT0_RT5: &[u8] = &[
    0x00, 0x03, 0x02, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x15, 0x67, 0x00, 0x00, 0x00, 0x00, 0x0c,
    0x00, 0x01, 0x00, 0x00, 0x00,
];

#[test]
fn material_rt5_golden_vector() {
    let mat = decode_material(0, MAT0_RT5).unwrap();
    assert_eq!(mat.id, 0);
    assert!(mat.environment_cube);
    assert_eq!(mat.mip_mode, 0);
    assert_eq!(mat.size, Some(256));
    assert_eq!(mat.diffuse_texture, Some(5479));
    assert_eq!(mat.aux_texture, None);
    assert_eq!(mat.texture_ids, vec![5479]);
    assert_eq!(mat.repeat_s, 0);
    assert_eq!(mat.repeat_t, 0);
    assert_eq!(mat.alpha, AlphaMode::None);
    assert_eq!(mat.alpha_threshold, 0xFF);
    assert_eq!((mat.speed_u, mat.speed_v), (0.0, 0.0));
    assert!(!mat.high_detail && !mat.low_detail);
    assert_eq!(mat.average_colour, 0);
}

#[test]
fn material_rt7_minimal_vector() {
    // ver 1, flags 0x20 (diffuse), skip, diffuse 5479, repeat 0,
    // facet 1, quality 0, alpha 0 (NONE), average 0x1234, size code 2.
    let bytes = [
        0x01, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x15, 0x67, 0x00, 0x01, 0x00, 0x00, 0x12,
        0x34, 0x02,
    ];
    let mat = decode_material(7, &bytes).unwrap();
    assert_eq!(mat.id, 7);
    assert!(!mat.environment_cube);
    assert_eq!(mat.mip_mode, 0);
    assert_eq!(mat.diffuse_texture, Some(5479));
    assert_eq!(mat.texture_ids, vec![5479]);
    assert_eq!(mat.size, Some(256));
    assert_eq!(mat.average_colour, 0x1234);
    assert_eq!(mat.alpha, AlphaMode::None);
}

#[test]
fn material_rejects_unknown_version_and_truncation() {
    assert!(decode_material(0, &[9_u8]).is_err());
    assert!(decode_material(0, &MAT0_RT5[..10]).is_err());
    assert!(decode_material(0, &[]).is_err());
}

fn synthetic_dds(fourcc: &[u8; 4], w: u32, h: u32) -> Vec<u8> {
    let mut dds = vec![0_u8; 128];
    dds[..4].copy_from_slice(b"DDS ");
    dds[4..8].copy_from_slice(&124_u32.to_le_bytes());
    dds[12..16].copy_from_slice(&h.to_le_bytes());
    dds[16..20].copy_from_slice(&w.to_le_bytes());
    dds[76..80].copy_from_slice(&32_u32.to_le_bytes());
    dds[80..84].copy_from_slice(&4_u32.to_le_bytes());
    dds[84..88].copy_from_slice(fourcc);
    dds.extend_from_slice(&[0xA5_u8; 64]);
    dds
}

#[test]
fn dds_parse_roundtrips_stably() {
    let dds = synthetic_dds(b"DXT1", 64, 64);
    let tex = parse_dxt(&dds).unwrap();
    assert_eq!((tex.w, tex.h), (64, 64));
    assert_eq!(tex.format, TextureFormat::Dxt1);
    assert_eq!(tex.format.index(), Some(1));
    assert_eq!(tex.bytes, dds);
    // Round-trip stability: re-parsing the stored bytes is identical.
    assert_eq!(parse_dxt(&tex.bytes).unwrap(), tex);

    let dds5 = synthetic_dds(b"DXT5", 128, 128);
    assert_eq!(parse_dxt(&dds5).unwrap().format, TextureFormat::Dxt5);
    assert!(parse_dxt(&synthetic_dds(b"DXT3", 64, 64)).is_err());
    assert!(parse_dxt(b"nope").is_err());
}

fn synthetic_ktx(internal: u32, w: u32, h: u32) -> Vec<u8> {
    let mut ktx = Vec::new();
    ktx.extend_from_slice(&KTX_MAGIC);
    for word in [
        0x0403_0201_u32,
        0,
        1,
        0,
        internal,
        0x1908,
        w,
        h,
        0,
        0,
        1,
        1,
        0,
    ] {
        ktx.extend_from_slice(&word.to_le_bytes());
    }
    ktx.extend_from_slice(&16_u32.to_le_bytes());
    ktx.extend_from_slice(&[0x5A_u8; 16]);
    ktx
}

#[test]
fn ktx_parse_roundtrips_stably() {
    let ktx = synthetic_ktx(0x9278, 128, 128);
    let tex = parse_etc(&ktx).unwrap();
    assert_eq!((tex.w, tex.h), (128, 128));
    assert_eq!(tex.format, TextureFormat::Etc2Rgba8);
    assert_eq!(tex.bytes, ktx);
    assert_eq!(parse_etc(&tex.bytes).unwrap(), tex);

    assert_eq!(
        parse_etc(&synthetic_ktx(0x8D64, 64, 64)).unwrap().format,
        TextureFormat::Etc1Rgb8
    );
    assert!(parse_etc(&synthetic_ktx(0x1234, 64, 64)).is_err());
    assert!(parse_etc(b"nope").is_err());
}

/// Real pack: archive survey plus a strict decode sweep.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_pack_survey_and_spot_decode() {
    let pack = crate::test_support::require_pack("client.textures.png.js5");
    // Archive survey: file bytes + group counts.
    for archive in [
        MATERIALS_ARCHIVE,
        "shaders",
        TEXTURES_DXT_ARCHIVE,
        TEXTURES_PNG_ARCHIVE,
        TEXTURES_PNG_MIPPED_ARCHIVE,
        TEXTURES_ETC_ARCHIVE,
    ] {
        let bytes = std::fs::metadata(pack.archive_path(archive)).unwrap().len();
        let index = pack.read_archive_index(archive).unwrap();
        println!(
            "archive {archive}: {bytes} bytes, {} groups, first {:?}",
            index.group_count,
            &index.group_id[..index.group_id.len().min(4)]
        );
    }

    // Every real material must decode (strict load proves the RT5/RT7 port).
    let store = MaterialStore::load(&pack).unwrap();
    println!(
        "materials: {} slots, {} present",
        store.len(),
        store.iter().count()
    );
    assert!(store.len() > 10_000);
    let mat0 = store.get(0).expect("material 0 exists");
    assert_eq!(mat0.diffuse_texture, Some(5479));
    assert_eq!(mat0.size, Some(256));

    // Spot-decode the first single + first cubemap in archive 53. Low ids
    // are cubemarks (skyboxes); diffuse singles start higher up. Every
    // texture in range must fully decode: a 600-texture sweep over the
    // real pack's color types, filters, and PLTE/tRNS combinations.
    let index = pack.read_archive_index(TEXTURES_PNG_ARCHIVE).unwrap();
    let (mut single, mut cube) = (None, None);
    let mut decoded = 0_usize;
    let mut dims = std::collections::BTreeSet::new();
    for &group in index.group_id.iter().take(600) {
        let files = pack.read_group(TEXTURES_PNG_ARCHIVE, group).unwrap();
        let Some(bytes) = files.get(&0) else { continue };
        match bytes.first() {
            Some(1) if single.is_none() => single = Some(group),
            Some(6) if cube.is_none() => cube = Some(group),
            _ => {}
        }
        if !matches!(bytes.first(), Some(1) | Some(6)) {
            continue;
        }
        for png in split_png_file(bytes).unwrap() {
            let img = decode_png(png).unwrap();
            dims.insert((img.w, img.h));
            assert_eq!(img.px.len(), img.w as usize * img.h as usize * 4);
            decoded += 1;
        }
    }
    println!("sweep: {decoded} real PNGs decoded, dims {dims:?}");
    let single = single.expect("a ver-1 texture in the first 600 groups");
    let img = load_png(&pack, single, 0).unwrap();
    println!("single {single}: {}x{}", img.w, img.h);
    assert!(img.w == img.h && img.w <= 1024);
    assert_eq!(img.px.len(), img.w as usize * img.h as usize * 4);

    let cube = cube.expect("a ver-6 texture in the first 600 groups");
    let faces = load_cube(&pack, cube, 0).unwrap();
    for (i, face) in faces.iter().enumerate() {
        println!("cube {cube} face {i}: {}x{}", face.w, face.h);
        assert_eq!(face.px.len(), face.w as usize * face.h as usize * 4);
    }

    // Mipped + raw spot checks ride the same framing.
    let mipped = load_mipped(&pack, single, 0).unwrap();
    assert_eq!(mipped.faces.len(), 1);
    assert!(mipped.faces[0].len() >= 2);
    assert_eq!(mipped.faces[0][0].w, img.w);
    let dxt = load_dxt(&pack, single, 0).unwrap();
    assert!(!dxt.is_empty());
    println!(
        "dxt {single}: {}x{} {:?}",
        dxt[0].w, dxt[0].h, dxt[0].format
    );
    let etc = load_etc(&pack, single, 0).unwrap();
    assert!(!etc.is_empty());
    println!(
        "etc {single}: {}x{} {:?}",
        etc[0].w, etc[0].h, etc[0].format
    );
}

#[test]
fn colour_table_size_and_sentinels() {
    // 64K entries, procedurally generated.
    assert_eq!(COLOUR_TABLE_SIZE, 65536);
    let table = build_colour_table();
    assert_eq!(table.len(), 65536);
    // Black: packed 0 (hue/sat/light 0 -> all channels zero) -> opaque black:
    // `0xFF000000 | 0 = 0xFF000000`.
    assert_eq!(colour_table_entry(0), 0xFF00_0000);
    assert_eq!(table[0], 0xFF00_0000);
    // Any zero lightness is black regardless of hue/sat: indices `128`
    // (hue 1, light 0) and `256` (hue 2) pin two more.
    assert_eq!(colour_table_entry(128), 0xFF00_0000);
    assert_eq!(colour_table_entry(256), 0xFF00_0000);
    // White pack (`flo` white `0xFFFFFF` packs to HSL short 127:
    // `hue6=0,sat3=0,light7=127`): near-white via the `0.7` gamma.
    // Hand-computed with the exact f32 + f64 pow arithmetic: hue 2.8125deg,
    // saturation 0.0625, lightness 0.9921875, sector 0, `pow*256` ->
    // `(254,243,243)` = `0xFFFEF3F3`.
    let white = colour_table_entry(127);
    assert_eq!(white, 0xFFFE_F3F3, "white 127 {white:#010x}");
    let (wr, wg, wb) = ((white >> 16) & 0xFF, (white >> 8) & 0xFF, white & 0xFF);
    assert!(
        wr > 200 && wg > 200 && wb > 200,
        "white {white:#010x} -> ({wr},{wg},{wb})"
    );
    assert_eq!(white & 0xFF00_0000, 0xFF00_0000, "opaque alpha");
    // Magenta pack (`0xFF00FF` packs to 55231 via `colourFudge`, though the
    // sentinel `-1` itself never indexes the table): hand-computed
    // `0xFF9B169A -> (155,22,154)` (red+blue dominate green).
    let magenta_packed = 55231_u16;
    let magenta = colour_table_entry(magenta_packed);
    assert_eq!(magenta, 0xFF9B_169A, "magenta 55231 {magenta:#010x}");
    let (mr, mg, mb) = (
        (magenta >> 16) & 0xFF,
        (magenta >> 8) & 0xFF,
        magenta & 0xFF,
    );
    assert!(
        mr > 100 && mb > 100 && mg < 50,
        "magenta {magenta:#010x} -> ({mr},{mg},{mb})"
    );
    // Linear decode stays in unit range.
    for index in [0_u16, 127, 55231, 65535] {
        let linear = colour_table_linear(index);
        assert!(linear.iter().all(|c| (0.0..=1.0).contains(c)));
    }
    assert_eq!(colour_table_linear(0), [0.0, 0.0, 0.0]);
}

#[test]
fn diffuse_aux_combine_boosts_and_clamps() {
    // Factor `(aux/255*31+1)`.
    assert!((diffuse_aux_factor(0) - 1.0).abs() < 1e-6);
    assert!((diffuse_aux_factor(255) - 32.0).abs() < 1e-5);
    // Mid: aux 128 -> `128*31/255+1 = 16.560...`.
    let mid = diffuse_aux_factor(128);
    assert!((mid - (128.0 * 31.0 / 255.0 + 1.0)).abs() < 1e-5);
    // Float combine: `rgb* factor/255`, alpha passthrough.
    let (rgb, alpha) = combine_diffuse_aux_float([100, 150, 200, 255], 0);
    assert!((rgb[0] - 100.0 / 255.0).abs() < 1e-6);
    assert!((rgb[1] - 150.0 / 255.0).abs() < 1e-6);
    assert!((rgb[2] - 200.0 / 255.0).abs() < 1e-6);
    assert!((alpha - 1.0).abs() < 1e-6);
    // Aux 255 boosts 32x (HDR, unclamped like a float array).
    let (bright, _) = combine_diffuse_aux_float([100, 0, 0, 128], 255);
    assert!((bright[0] - 100.0 * 32.0 / 255.0).abs() < 1e-4);
    // Byte (atlas) form: factor 1.0 is identity, 32x saturates to 255.
    assert_eq!(
        combine_diffuse_aux_byte([100, 150, 200, 128], 0),
        [100, 150, 200, 128]
    );
    assert_eq!(
        combine_diffuse_aux_byte([100, 150, 200, 128], 255),
        [255, 255, 255, 128]
    );
    assert_eq!(combine_diffuse_aux_byte([10, 20, 30, 40], 128)[3], 40);
    // Whole-image: `None` aux clones diffuse; `Some` combines per-texel
    // via the aux red channel, tiling when dims differ.
    let diffuse = RgbaImage {
        w: 2,
        h: 1,
        px: vec![100, 0, 0, 255, 0, 100, 0, 255],
    };
    let cloned = combine_diffuse_aux_image(&diffuse, None);
    assert_eq!(cloned, diffuse);
    let aux_black = RgbaImage {
        w: 2,
        h: 1,
        px: vec![0, 0, 0, 255, 0, 0, 0, 255],
    };
    let kept = combine_diffuse_aux_image(&diffuse, Some(&aux_black));
    assert_eq!(kept, diffuse, "aux 0 factor 1.0 keeps bytes");
    let aux_white = RgbaImage {
        w: 1,
        h: 1,
        px: vec![255, 255, 255, 255],
    };
    let boosted = combine_diffuse_aux_image(&diffuse, Some(&aux_white));
    assert_eq!(boosted.px, vec![255, 0, 0, 255, 0, 255, 0, 255]);
}
