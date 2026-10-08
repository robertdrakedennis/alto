//! Milestone-3 sprite gate: every sheet in the real 910 sprites pack must
//! decode and re-encode byte-identical.
//!
//! Byte-identity alone cannot catch semantic misreads (a shifted walk over
//! zero-filled regions still round-trips), so the meaning gate below pins
//! decoded CONTENT on representative sheets alongside the byte gate.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::pack::PackArchive;
use native910::sprite::{
    FullSheet, FullSprite, PalettedSheet, PalettedSprite, SpriteSheet, decode_sprite, encode_sprite,
};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_sprites_are_byte_exact() {
    let path = common::pack_root().join("client.sprites.js5");
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open sprites pack");

    let mut groups = 0_usize;
    let mut sheets = 0_usize;
    let mut sprites = 0_usize;
    let mut paletted = 0_usize;
    let mut full = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        groups += 1;
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        if files.len() != 1 || !files.contains_key(&0) {
            failures.push(format!(
                "{group}: expected single file 0, got {:?}",
                files.keys().collect::<Vec<_>>()
            ));
            continue;
        }
        let bytes = &files[&0];
        let sheet = match decode_sprite(bytes) {
            Ok(sheet) => sheet,
            Err(error) => {
                failures.push(format!("{group}: decode: {error}"));
                continue;
            }
        };
        sheets += 1;
        sprites += sheet.sprite_count();
        if sheet.is_paletted() {
            paletted += 1;
        } else {
            full += 1;
        }
        match encode_sprite(&sheet) {
            Ok(out) if out == *bytes => {}
            Ok(out) => failures.push(format!(
                "{group}: re-encoded {} bytes, original {}",
                out.len(),
                bytes.len()
            )),
            Err(error) => {
                failures.push(format!("{group}: re-encode: {error}"));
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} sprite failure(s) over {sprites} sprites in {sheets} sheets ({groups} groups)",
        failures.len()
    );
    eprintln!(
        "sprite corpus: {sprites} sprites in {sheets} sheets over {groups} groups \
         ({paletted} paletted, {full} full-colour), byte-exact"
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_meaning_spot_checks() {
    let path = common::pack_root().join("client.sprites.js5");
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open sprites pack");
    let sheet_bytes = |group: u32| {
        archive
            .group_files(group)
            .expect("unpack group")
            .expect("group present")[&0]
            .clone()
    };

    // Group 0: paletted 48x48 icon on a 48x48 canvas, 133-entry palette,
    // flags 3 (column-major + alpha plane).
    let sheet = decode_sprite(&sheet_bytes(0)).expect("decode group 0");
    let SpriteSheet::Paletted(paletted) = sheet else {
        panic!("group 0 must be paletted");
    };
    assert_eq!(paletted.canvas_width, 48);
    assert_eq!(paletted.canvas_height, 48);
    assert_eq!(paletted.palette.len(), 133);
    assert_eq!(paletted.sprites.len(), 1);
    let sprite = &paletted.sprites[0];
    assert_eq!((sprite.width, sprite.height), (48, 48));
    assert_eq!((sprite.padding_left, sprite.padding_top), (0, 0));
    assert_eq!(sprite.padding_right(paletted.canvas_width), 0);
    assert_eq!(sprite.padding_bottom(paletted.canvas_height), 0);
    assert_eq!(sprite.flags, 3);
    assert!(sprite.column_major());
    assert!(sprite.has_alpha());
    assert_eq!(sprite.colour.len(), 48 * 48);
    assert_eq!(sprite.alpha.as_ref().map(Vec::len), Some(48 * 48));

    // Group 4: full-colour 233x104 sheet without an alpha plane.
    let sheet = decode_sprite(&sheet_bytes(4)).expect("decode group 4");
    let SpriteSheet::Full(full) = sheet else {
        panic!("group 4 must be full-colour");
    };
    assert!(!full.has_alpha);
    assert_eq!((full.width, full.height), (233, 104));
    assert_eq!(full.sprites.len(), 1);
    assert_eq!(full.sprites[0].rgb.len(), 233_usize * 104 * 3);
    assert!(full.sprites[0].alpha.is_none());

    // Group 1509: multi-sprite paletted sheet (count 2), 90x7 strips.
    let sheet = decode_sprite(&sheet_bytes(1509)).expect("decode group 1509");
    let SpriteSheet::Paletted(paletted) = sheet else {
        panic!("group 1509 must be paletted");
    };
    assert_eq!(paletted.sprites.len(), 2);
    assert_eq!((paletted.canvas_width, paletted.canvas_height), (90, 7));
    assert_eq!(paletted.palette.len(), 95);
    for sprite in &paletted.sprites {
        assert_eq!((sprite.width, sprite.height), (90, 7));
        assert_eq!(sprite.flags, 0);
    }

    // Group 23771: the count-30 atlas, 35x35 cells on a 35x35 canvas with a
    // full 256-entry palette.
    let sheet = decode_sprite(&sheet_bytes(23771)).expect("decode group 23771");
    let SpriteSheet::Paletted(paletted) = sheet else {
        panic!("group 23771 must be paletted");
    };
    assert_eq!(paletted.sprites.len(), 30);
    assert_eq!((paletted.canvas_width, paletted.canvas_height), (35, 35));
    assert_eq!(paletted.palette.len(), 256);
    for sprite in &paletted.sprites {
        assert_eq!((sprite.width, sprite.height), (35, 35));
    }
}

fn handbuilt_paletted() -> PalettedSheet {
    PalettedSheet {
        canvas_width: 4,
        canvas_height: 5,
        // Index 0 is the implicit transparent slot; entry 1 is a raw zero,
        // which must survive the round trip (the client forces it to 1 only
        // when resolving pixels for display).
        palette: vec![0, 0, 0xFF_0000, 0x00_FF00, 0x00_00FF, 0x12_3456],
        sprites: vec![
            PalettedSprite {
                padding_left: 1,
                padding_top: 1,
                width: 2,
                height: 3,
                flags: 0x01,
                colour: vec![0, 1, 2, 3, 4, 5],
                alpha: None,
            },
            PalettedSprite {
                padding_left: 0,
                padding_top: 2,
                width: 1,
                height: 2,
                // Alpha bit plus a raw high bit the client masks away.
                flags: 0x22,
                colour: vec![2, 3],
                // All-opaque planes are kept, not dropped to None.
                alpha: Some(vec![0xFF, 0xFF]),
            },
        ],
    }
}

#[test]
fn handbuilt_paletted_roundtrip_is_byte_identical() {
    let sheet = SpriteSheet::Paletted(handbuilt_paletted());
    assert_eq!(sheet.sprite_count(), 2);
    assert!(sheet.is_paletted());
    let bytes = encode_sprite(&sheet).expect("encode hand-built paletted");
    // Sprite 0 is column-major on the wire: columns [0,2,4] then [1,3,5].
    assert_eq!(&bytes[0..8], &[0x01, 0, 2, 4, 1, 3, 5, 0x22]);
    let decoded = decode_sprite(&bytes).expect("decode hand-built paletted");
    assert_eq!(decoded, sheet);
    assert_eq!(
        encode_sprite(&decoded).expect("re-encode hand-built paletted"),
        bytes
    );
    // Transpose check: the decoded plane is row-major storage again.
    let SpriteSheet::Paletted(paletted) = decoded else {
        panic!("must stay paletted");
    };
    assert_eq!(paletted.sprites[0].colour, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(paletted.palette[1], 0);
    assert_eq!(paletted.sprites[1].flags, 0x22);
    assert_eq!(paletted.sprites[1].alpha, Some(vec![0xFF, 0xFF]));
}

#[test]
fn handbuilt_full_roundtrip_is_byte_identical() {
    let sheet = SpriteSheet::Full(FullSheet {
        width: 2,
        height: 1,
        has_alpha: true,
        sprites: vec![FullSprite {
            // Opaque magenta stays magenta bytes: the transparent keying is
            // a render mapping, never storage.
            rgb: vec![0xFF, 0x00, 0xFF, 0x01, 0x02, 0x03],
            alpha: Some(vec![0x00, 0x80]),
        }],
    });
    assert_eq!(sheet.sprite_count(), 1);
    assert!(!sheet.is_paletted());
    let bytes = encode_sprite(&sheet).expect("encode hand-built full");
    let decoded = decode_sprite(&bytes).expect("decode hand-built full");
    assert_eq!(decoded, sheet);
    assert_eq!(
        encode_sprite(&decoded).expect("re-encode hand-built full"),
        bytes
    );

    let plain = SpriteSheet::Full(FullSheet {
        width: 1,
        height: 1,
        has_alpha: false,
        sprites: vec![FullSprite {
            rgb: vec![0x10, 0x20, 0x30],
            alpha: None,
        }],
    });
    let bytes = encode_sprite(&plain).expect("encode opaque full");
    assert_eq!(decode_sprite(&bytes).expect("decode opaque full"), plain);
    assert_eq!(encode_sprite(&plain).expect("re-encode opaque full"), bytes);
}

#[test]
fn malformed_inputs_are_rejected() {
    // Decode side: every input must fail, never panic or guess.
    let bad: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x00],
        // Zero-sprite trailer.
        vec![0x00, 0x00],
        // Trailer claims 5 sprites; the dims block cannot exist.
        vec![0xAA, 0xBB, 0xCC, 0x00, 0x05],
        // Paletted: dims fit (dimpos 5) but palette size 256 needs 765
        // bytes that are not there.
        vec![
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ],
        // Paletted: palette holds only index 0, colour references index 5.
        vec![
            0x00, 0x05, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x01,
        ],
        // Full-colour: sub-format 1 is unsupported (the client throws).
        vec![
            0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x10, 0x20, 0x30, 0x80, 0x01,
        ],
        // Full-colour: unknown sub-format.
        vec![
            0x07, 0x00, 0x00, 0x01, 0x00, 0x01, 0x10, 0x20, 0x30, 0x80, 0x01,
        ],
        // Full-colour: alpha flag must be 0 or 1.
        vec![
            0x00, 0x02, 0x00, 0x01, 0x00, 0x01, 0x10, 0x20, 0x30, 0x80, 0x01,
        ],
        // Full-colour: truncated RGB plane (the trailer gets eaten, then the
        // layout check fires).
        vec![0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x10, 0x80, 0x01],
        // Full-colour: a byte smuggled into the RGB run breaks the layout.
        vec![
            0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x10, 0xFF, 0x20, 0x30, 0x80, 0x01,
        ],
    ];
    for (index, bytes) in bad.iter().enumerate() {
        assert!(
            decode_sprite(bytes).is_err(),
            "malformed input {index} decoded without error"
        );
    }

    // Paletted: flags promise an alpha plane the payload cannot carry.
    // flags(1) + 64 colour bytes for an 8x8 image + 8x8 dims + trailer:
    // the 64-byte alpha read runs 49 bytes past the end.
    let mut trunc_alpha = vec![0x02];
    trunc_alpha.extend(vec![0; 64]);
    trunc_alpha.extend_from_slice(&[
        0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08, 0x00, 0x01,
    ]);
    assert_eq!(trunc_alpha.len(), 80);
    assert!(decode_sprite(&trunc_alpha).is_err());

    // Encode side: inconsistent hand-built models must fail.
    let empty_paletted = SpriteSheet::Paletted(PalettedSheet {
        canvas_width: 1,
        canvas_height: 1,
        palette: vec![0, 1],
        sprites: Vec::new(),
    });
    assert!(encode_sprite(&empty_paletted).is_err());
    let empty_full = SpriteSheet::Full(FullSheet {
        width: 1,
        height: 1,
        has_alpha: false,
        sprites: Vec::new(),
    });
    assert!(encode_sprite(&empty_full).is_err());

    let mut sheet = handbuilt_paletted();
    sheet.palette.clear();
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    let mut sheet = handbuilt_paletted();
    sheet.palette[0] = 0xAB_CDEF;
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    let mut sheet = handbuilt_paletted();
    sheet.sprites[0].colour.pop();
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    // Alpha plane present on the model but absent from the flags byte.
    let mut sheet = handbuilt_paletted();
    sheet.sprites[0].alpha = Some(vec![0xFF; 6]);
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    // Flags promise alpha but the model carries none.
    let mut sheet = handbuilt_paletted();
    sheet.sprites[1].alpha = None;
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    // Palette index outside the carried palette.
    let mut sheet = handbuilt_paletted();
    sheet.sprites[0].colour[0] = 6;
    assert!(encode_sprite(&SpriteSheet::Paletted(sheet)).is_err());

    // Full-colour plane-size and presence mismatches.
    let bad_full = SpriteSheet::Full(FullSheet {
        width: 2,
        height: 2,
        has_alpha: true,
        sprites: vec![FullSprite {
            rgb: vec![0; 11],
            alpha: Some(vec![0; 4]),
        }],
    });
    assert!(encode_sprite(&bad_full).is_err());
    let bad_full = SpriteSheet::Full(FullSheet {
        width: 1,
        height: 1,
        has_alpha: true,
        sprites: vec![FullSprite {
            rgb: vec![0; 3],
            alpha: None,
        }],
    });
    assert!(encode_sprite(&bad_full).is_err());
    let bad_full = SpriteSheet::Full(FullSheet {
        width: 1,
        height: 1,
        has_alpha: false,
        sprites: vec![FullSprite {
            rgb: vec![0; 3],
            alpha: Some(vec![0; 1]),
        }],
    });
    assert!(encode_sprite(&bad_full).is_err());
}
