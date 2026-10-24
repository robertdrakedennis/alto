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
use native910::sprite::{SpriteSheet, decode_sprite, encode_sprite};

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
