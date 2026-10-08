use super::*;

fn mask(rows: &[&str]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| r.bytes().map(|b| if b == b'1' { 0xFF } else { 0 }))
        .collect()
}

#[test]
fn tile_shapes_cover_the_expected_texels() {
    let s = tile_shapes(4);
    assert_eq!(s.len(), 8);
    // [0][0]: c <= r, rows ascending.
    assert_eq!(s[0][0], mask(&["1000", "1100", "1110", "1111"]));
    // [1][1]: c >= r << 1.
    assert_eq!(s[1][1], mask(&["1111", "0011", "0000", "0000"]));
    // [2][0]: rows and columns descending, c <= r >> 1.
    assert_eq!(s[2][0], mask(&["0011", "0011", "0001", "0001"]));
    // [5][0]: c <= size / 2.
    assert_eq!(s[5][0], mask(&["1110", "1110", "1110", "1110"]));
    // [6][1]: rows descending, c <= r - size / 2.
    assert_eq!(s[6][1], mask(&["1100", "1000", "0000", "0000"]));
    // [7][3]: columns descending, c >= r - size / 2.
    assert_eq!(s[7][3], mask(&["1111", "1111", "1111", "1110"]));
    // The mask size is the target zoom halved: zoom 2 and 3 use one texel.
    assert_eq!(tile_shapes(1)[3][2], vec![0xFF]);
}

#[test]
fn shape_rotation_and_table_values() {
    assert_eq!(
        (
            shape_rotation(3, 9),
            shape_rotation(0, 10),
            shape_rotation(1, 11),
            shape_rotation(2, 4)
        ),
        (0, 3, 0, 2)
    );
    assert_eq!(
        (
            shape_table(9),
            shape_table(10),
            shape_table(11),
            shape_table(5)
        ),
        (1, 1, 8, 5)
    );
}

#[test]
fn shaped_overlay_scales_the_mask() {
    // A 2x2 texel mask over a 4x4 tile at the sprite's bottom-left.
    let mut px = vec![0; 8 * 8];
    let m = mask(&["10", "01"]);
    shaped(
        &mut px,
        8,
        &ShapedOverlay {
            rect: [0, 3, 0, 3],
            under: 1,
            over: 2,
            mask: &m,
            shape_size: 2,
            mode: 0,
        },
    );
    // Row 4 of the sprite is the tile's top (z = 3): texel row 0.
    let row = |y: usize| px[y * 8..y * 8 + 4].to_vec();
    assert_eq!(row(4), [2, 2, 1, 1]);
    assert_eq!(row(5), [2, 2, 1, 1]);
    assert_eq!(row(6), [1, 1, 2, 2]);
    assert_eq!(row(7), [1, 1, 2, 2]);
    assert!(px[..32].iter().all(|&p| p == 0));
    // Mode 1 blends by each colour's alpha over the existing pixel.
    let mut px = vec![0xFF00_0000u32 as i32; 4];
    shaped(
        &mut px,
        2,
        &ShapedOverlay {
            rect: [0, 1, 0, 1],
            under: 0,
            over: 0x80FF_FFFFu32 as i32,
            mask: &mask(&["1"]),
            shape_size: 1,
            mode: 1,
        },
    );
    assert_eq!(px[0], 0xFF7F_7F7Fu32 as i32);
}

#[test]
fn walls_and_fills_use_sprite_rows() {
    let mut px = vec![0; 16];
    fill(&mut px, 1, 2, 0, 0, 4, 7);
    assert_eq!(&px[12..16], &[0, 7, 7, 0]);
    let mut px = vec![0; 16];
    // `row` at y = 3 is sprite row 0; `column` runs downwards.
    row(&mut px, 0, 3, 4, 4, 5);
    column(&mut px, 3, 3, 4, 4, 6);
    assert_eq!(&px[0..4], &[5, 5, 5, 6]);
    assert_eq!([px[7], px[11], px[15]], [6, 6, 6]);
}

#[test]
fn blend_averages_the_11x11_window() {
    let mut flo = crate::flo::FloStore::default();
    let mut u = crate::flo::decode_underlay(0, &[1, 0x40, 0x80, 0x20, 0]).unwrap();
    u.id = 0;
    flo.underlays.insert(0, u.clone());
    let palette = crate::colour::build_hsv_table();
    let size = [16, 16];
    let mut ids = vec![1i16; 256];
    ids[0] = 0;
    let mut out = vec![0x123456; 256];
    blend(&ids, &mut out, size, &flo, 0, 0, &palette);
    // A tile without an underlay is blanked; the others share the one
    // type's colour through the HSL conversion and the palette table.
    assert_eq!(out[0], 0);
    let hsl = crate::colour::hsl24to16(
        u.hue * 256 / u.chroma as i32,
        u.saturation as i32,
        u.lightness as i32,
    );
    let expected = palette[(crate::colour::renormalise_saturation(crate::colour::mul_hsl(hsl, 96))
        & 0xFFFF) as usize]
        & 0xFF_FFFF;
    assert_eq!(out[17], expected);
    assert_eq!(out[255], expected);
    // An all-empty window leaves the previous output (the arrays are shared).
    let mut out = vec![0x123456; 256];
    blend(&vec![0i16; 256], &mut out, size, &flo, 0, 0, &palette);
    assert!(out.iter().all(|&v| v == 0x123456));
}

#[test]
fn view_projects_element_and_polygon_points() {
    // The main draw at zoom 8 over an 800x600 component:
    // 200 x 150 tiles, `zoom / 2` pixels per tile.
    let mut map = WorldMap::default();
    map.allocate([3136, 3136], [128, 128]);
    map.set_view(View {
        left: 3100,
        top: 3275,
        right: 3300,
        bottom: 3125,
        x0: 0,
        y0: 0,
        x1: 800,
        y1: 601,
    });
    let v = map.view;
    assert_eq!([v.left, v.top, v.right, v.bottom], [-36, 139, 164, -11]);
    assert_eq!(v.project(3200 - 3136, 3200 - 3136), [400, 301]);
    assert_eq!(v.project(3210 - 3136, 3200 - 3136), [440, 301]);
    assert_eq!(v.project(3200 - 3136, 3210 - 3136), [400, 261]);
    assert_eq!(v.project_exact(3210 - 3136, 3210 - 3136), [440, 261]);
}

#[test]
fn decode_reads_regions_blocks_and_upper_levels() {
    // One 8x8 block record of region (50, 50): a ground tile
    // with palette underlay 0, an overlay record, an upper-level tile
    // with a loc, then 61 absent tiles.
    let mut file = vec![1, 7, 1, 9];
    file.push(1);
    file.extend([1, 50, 50, 1, 2, 1]);
    file.push(0); // underlay palette 0
    file.extend([2 | 4, 3]); // overlay palette 1, underlay 3
    file.extend([1 | 2 | 0x10, 5, 0, 6, 1, 0x01, 0x23, 0x42]); // two levels, one loc on level 1
    file.extend(std::iter::repeat_n(62 << 2, 61));
    let mut area = Area::new([3136, 3136], [128, 128]);
    let mut c = Cursor::new(&file[5..]);
    assert_eq!(c.g1().unwrap(), 1);
    let (rx, rz, bx, bz) = (
        c.g1().unwrap() as i32,
        c.g1().unwrap() as i32,
        c.g1().unwrap() as i32,
        c.g1().unwrap() as i32,
    );
    area.set_member_block(
        rx * 8 + bx - 3136 / 8,
        rz * 8 + bz - 3136 / 8,
        c.g1().unwrap() != 0,
    );
    for x in 0..8 {
        for z in 0..8 {
            area.decode_tile(
                &mut c,
                [rx, rz],
                rx * 64 + bx * 8 + x - 3136,
                rz * 64 + bz * 8 + z - 3136,
                &[7],
                &[9, 4],
            )
            .unwrap();
        }
    }
    assert_eq!(c.remaining(), 0);
    let at = |x: i32, z: i32| (128 * (z - 3136) + x - 3136) as usize;
    let (x0, z0) = (50 * 64 + 8, 50 * 64 + 16);
    assert!(area.member_block(x0 - 3136, z0 - 3136));
    assert_eq!(area.underlay[at(x0, z0)], 7);
    assert_eq!(
        (area.overlay[at(x0, z0 + 1)], area.underlay[at(x0, z0 + 1)]),
        (4, 3)
    );
    assert_eq!(
        (area.underlay[at(x0, z0 + 2)], area.overlay[at(x0, z0 + 2)]),
        (5, 0)
    );
    let chunk = area.upper_chunk(0, 50 - 49, 50 - 49).unwrap();
    let t = chunk.get(&(((8 << 8) + 18) as u16)).unwrap();
    assert_eq!(
        (t.colour, t.locs.clone()),
        (6, Some((vec![0x123], vec![0x42])))
    );
}

#[test]
fn defaults_decode_font_table() {
    let d = Defaults::decode(&[
        1,
        0,
        0,
        0,
        28,
        5,
        6,
        100,
        0x4F,
        0x38,
        108 + 2,
        0x52,
        0xD7,
        0,
    ])
    .unwrap();
    assert_eq!((d.members_map, d.corner), (28, 6));
    assert_eq!(d.fonts[0][0], 20280);
    // opcode 110: textSize 10 & 7 = 2, zoom column 10 >> 3 = 1.
    assert_eq!(d.fonts[2][1], 21207);
}

#[test]
fn chunk_cache_turns_unused_sprites_soft() {
    let sprite = || {
        Rc::new(Sprite {
            paletted: None,
            size: [2, 2],
            padding: [0; 4],
            argb: vec![0; 4],
        })
    };
    // Room for two 16-byte sprites among the soft entries.
    let mut cache = ChunkCache::with_soft_limit(32);
    cache.put(sprite(), 1);
    for _ in 0..CHUNK_IDLE_CLEANS + 3 {
        cache.clean(CHUNK_IDLE_CLEANS);
    }
    // Idle for long: soft, but still found (and hard again after the hit).
    assert_eq!(cache.width(1), 2);
    cache.put(sprite(), 2);
    cache.put(sprite(), 3);
    cache.put(sprite(), 4);
    for _ in 0..CHUNK_IDLE_CLEANS + 1 {
        cache.clean(CHUNK_IDLE_CLEANS);
    }
    // Four soft entries do not fit; the least recently used two go.
    let kept: Vec<i64> = (1..=4).filter(|k| cache.width(*k) == 2).collect();
    assert_eq!(kept, [3, 4]);
    // The cache holds 4096 sprites; the oldest is dropped for a new one.
    let mut cache = ChunkCache::default();
    for key in 0..4097 {
        cache.put(sprite(), key);
    }
    assert_eq!(
        (cache.width(0), cache.width(1), cache.width(4096)),
        (-1, 2, 2)
    );
}
