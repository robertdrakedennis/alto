use super::*;

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
