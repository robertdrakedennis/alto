//! Item-icon pixel goldens.
//!
//! The icon rasteriser has no other pixel coverage in the default suite, so
//! this pins the whole path (model decode, lighting, triangle fill) with
//! digests taken before the rasteriser was rewritten.
//!
//! `fixtures/icon-goldens/icons.txt` holds one line per sampled item:
//! `id feature-mask hash hash hash hash`, one FNV-1a 64 hash of the 36x32 ARGB
//! pixels for each entry of [`VARIANTS`] (`err` when the icon cannot be built).
//! The sample takes evenly spaced items from every combination of face
//! features found in the cache (see [`features`]), so flat, Gouraud, textured,
//! translucent and distorting faces are all covered. Two more lines hold
//! the digest of the full sweep over every item with a model (`sweep`,
//! checked by the ignored test [`every_item_icon_matches_the_sweep_digest`]).
//! Only hashes are stored, never pixels; when a
//! golden fails the test writes the differing icons as a PNG contact sheet into
//! the target directory (`icon-mismatches.png`) for looking at.
//!
//! Regenerate with `cargo test -p rs910-scene --lib regenerate_icon_goldens
//! -- --ignored` (only after an intended pixel change).

use super::*;
use crate::config::ObjStore;
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf, time::Instant};

/// Outline mode, enlarge flag and the three light factors of each rendition.
/// Outline 2 and enlarge change the zoom; the light factors change the
/// per-vertex brightness.
const VARIANTS: [(i32, bool, [f32; 3]); 4] = [
    (0, false, [1., 1., 1.]),
    (2, false, [0.97, 1.03, 0.99]),
    (1, true, [1., 1., 1.]),
    (0, false, [1.05, 0.95, 1.0]),
];
/// Items taken per feature combination.
const PER_FEATURE_SET: usize = 36;
/// Most icons on the failure contact sheet.
const SHEET_ITEMS: usize = 24;
const SHEET_COLUMNS: usize = 8;
const ITEM_WIDTH: usize = 36;
const ITEM_HEIGHT: usize = 32;

fn fixture_dir() -> PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/icon-goldens")
}

fn fnv64(pixels: &[i32]) -> u64 {
    pixels.iter().fold(0xcbf29ce484222325, |hash, &word| {
        (hash ^ u64::from(word as u32)).wrapping_mul(0x100000001b3)
    })
}

struct Fixture {
    pack: Pack,
    objs: ObjStore,
    models: Models,
}

fn load() -> Fixture {
    let pack = crate::test_support::require_pack("client.config.js5");
    let objs = ObjStore::load(&pack).expect("obj configs");
    let models = Models::new(&pack).expect("icon models");
    Fixture { pack, objs, models }
}

/// Which kinds of face an item's model has, as a bit set.
fn features(fixture: &Fixture, obj: &Obj) -> u32 {
    let Some(&model) = obj.models.first() else {
        return 0;
    };
    let Ok(files) = fixture.pack.read_group("models", model) else {
        return 0;
    };
    let Some(bytes) = files.get(&0) else {
        return 0;
    };
    let Ok(raw) = crate::model::decode(bytes) else {
        return 0;
    };
    let mut bits = 0;
    for face in 0..raw.faces.len() {
        let alpha = raw.alphas[face];
        let material = if raw.materials[face] < 0 {
            None
        } else {
            fixture
                .models
                .materials
                .get(raw.materials[face] as u32)
                .filter(|m| !m.high_detail)
        };
        bits |= match raw.face_types[face] {
            0 => 1,
            1 => 2,
            3 => 4,
            _ => 0,
        };
        bits |= match alpha {
            0 => 0,
            254 => 16,
            255 => 32,
            _ => 8,
        };
        if let Some(material) = material {
            bits |= if fixture.models.textured.get(&material.id) != Some(&true) {
                256
            } else if material.diffuse_texture.is_some() {
                64
            } else {
                128
            };
        }
    }
    if !obj.recol_s.is_empty() {
        bits |= 512;
    }
    bits
}

fn hashes(fixture: &Fixture, obj: &Obj) -> [Option<u64>; VARIANTS.len()] {
    VARIANTS.map(|(outline, enlarge, light)| {
        fixture
            .models
            .render(&fixture.pack, obj, outline, enlarge, light)
            .ok()
            .map(|pixels| fnv64(&pixels))
    })
}

fn hash_text(hash: Option<u64>) -> String {
    hash.map_or_else(|| "err".to_string(), |h| format!("{h:016x}"))
}

fn items_with_models(fixture: &Fixture) -> Vec<&Obj> {
    fixture
        .objs
        .iter()
        .map(|(_, obj)| obj)
        .filter(|obj| !obj.models.is_empty())
        .collect()
}

/// Items whose icons are always in the sample: a coin stack, a whip, a cape, a
/// torch and other items of the earlier oracle set.
const HAND_PICKED: [u32; 10] = [995, 1004, 1205, 4151, 385, 554, 4152, 11694, 1050, 14484];

/// The sampled items: evenly spaced picks from every feature combination
/// (see [`PER_FEATURE_SET`]) plus the hand-picked ones, thinned to every third
/// item of each combination to keep the default test quick.
fn sample(fixture: &Fixture) -> Vec<(u32, u32)> {
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for obj in items_with_models(fixture) {
        groups
            .entry(features(fixture, obj))
            .or_default()
            .push(obj.id);
    }
    let mut picked: BTreeMap<u32, u32> = BTreeMap::new();
    for (&mask, ids) in &groups {
        let take = ids.len().min(PER_FEATURE_SET);
        for n in 0..take {
            let index = if take == 1 {
                0
            } else {
                n * (ids.len() - 1) / (take - 1)
            };
            picked.insert(ids[index], mask);
        }
        picked.insert(ids[0], mask);
        picked.insert(ids[ids.len() - 1], mask);
    }
    for id in HAND_PICKED {
        if let Some(obj) = fixture.objs.get(id) {
            picked.insert(id, features(fixture, obj));
        }
    }
    let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
    picked
        .into_iter()
        .filter(|(id, mask)| {
            let count = seen.entry(*mask).or_default();
            *count += 1;
            HAND_PICKED.contains(id) || (*count - 1).is_multiple_of(3)
        })
        .collect()
}

fn sweep_digest(fixture: &Fixture) -> (usize, u64) {
    let mut digest = 0xcbf29ce484222325u64;
    let items = items_with_models(fixture);
    for obj in &items {
        digest = (digest ^ u64::from(obj.id)).wrapping_mul(0x100000001b3);
        for hash in hashes(fixture, obj) {
            digest = (digest ^ hash.unwrap_or(0)).wrapping_mul(0x100000001b3);
        }
    }
    (items.len(), digest)
}

fn icons_text(fixture: &Fixture) -> String {
    let mut text = String::from(
        "# id feature-mask hashes-per-variant (see ui_icon_model/tests.rs); sweep line: item count, digest\n",
    );
    for (id, mask) in sample(fixture) {
        let obj = fixture.objs.get(id).expect("sampled item");
        let [a, b, c, d] = hashes(fixture, obj);
        let _ = writeln!(
            text,
            "{id} {mask:x} {} {} {} {}",
            hash_text(a),
            hash_text(b),
            hash_text(c),
            hash_text(d)
        );
    }
    let (count, digest) = sweep_digest(fixture);
    let _ = writeln!(text, "sweep {count} {digest:016x}");
    text
}

/// A contact sheet of up to [`SHEET_ITEMS`] items' icons, for debugging a
/// failed golden.
fn sheet_png(fixture: &Fixture, items: &[u32]) -> Vec<u8> {
    let rows = items.len().div_ceil(SHEET_COLUMNS);
    let (width, height) = (SHEET_COLUMNS * ITEM_WIDTH, rows * ITEM_HEIGHT);
    let mut argb = vec![0xff202020u32 as i32; width * height];
    for (n, id) in items.iter().enumerate() {
        let obj = fixture.objs.get(*id).expect("sheet item");
        let pixels = fixture
            .models
            .render(&fixture.pack, obj, 0, false, [1.; 3])
            .expect("sheet icon");
        let (left, top) = (
            n % SHEET_COLUMNS * ITEM_WIDTH,
            n / SHEET_COLUMNS * ITEM_HEIGHT,
        );
        for (i, &pixel) in pixels.iter().enumerate() {
            if pixel >> 24 != 0 || pixel & 0xffffff != 0 {
                argb[(top + i / ITEM_WIDTH) * width + left + i % ITEM_WIDTH] =
                    pixel | 0xff000000u32 as i32;
            }
        }
    }
    png_rgba(width, height, &argb)
}

/// An uncompressed (stored-block) RGBA PNG: enough to look at, byte-stable.
fn png_rgba(width: usize, height: usize, argb: &[i32]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb88320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width * 4 + 1) * height);
    for row in argb.chunks(width) {
        raw.push(0);
        for &p in row {
            raw.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(block) = chunks.next() {
        zlib.push(u8::from(chunks.peek().is_none()));
        zlib.extend((block.len() as u16).to_le_bytes());
        zlib.extend((!(block.len() as u16)).to_le_bytes());
        zlib.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend((width as u32).to_be_bytes());
    header.extend((height as u32).to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sampled_item_icons_match_the_goldens() {
    let fixture = load();
    let expected = std::fs::read_to_string(fixture_dir().join("icons.txt")).expect("icons.txt");
    let mut mismatches = Vec::new();
    let mut differing = Vec::new();
    let mut checked = 0;
    for line in expected
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("sweep"))
    {
        let fields: Vec<&str> = line.split(' ').collect();
        let id: u32 = fields[0].parse().expect("item id");
        let obj = fixture.objs.get(id).expect("golden item exists");
        let got = hashes(&fixture, obj).map(hash_text);
        if got[..] != fields[2..] {
            mismatches.push(format!("item {id}: golden {:?}, got {got:?}", &fields[2..]));
            differing.push(id);
        }
        checked += 1;
    }
    assert!(checked > 100, "only {checked} golden items");
    if !differing.is_empty() {
        differing.truncate(SHEET_ITEMS);
        let dir =
            std::env::var_os("CARGO_TARGET_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let out = dir.join("icon-mismatches.png");
        let _ = std::fs::write(&out, sheet_png(&fixture, &differing));
        panic!(
            "{} icons differ (contact sheet of the first few: {}):\n{}",
            mismatches.len(),
            out.display(),
            mismatches.join("\n")
        );
    }
}

/// Every item with a model, four renditions each. Ignored because it renders
/// tens of thousands of icons; run it after any rasteriser change.
#[test]
#[ignore = "renders every item icon; cargo test -p rs910-scene --lib every_item_icon -- --ignored --nocapture"]
fn every_item_icon_matches_the_sweep_digest() {
    let fixture = load();
    let expected = std::fs::read_to_string(fixture_dir().join("icons.txt")).expect("icons.txt");
    let line = expected
        .lines()
        .find(|l| l.starts_with("sweep"))
        .expect("sweep line");
    let start = Instant::now();
    let (count, digest) = sweep_digest(&fixture);
    println!(
        "swept {count} items x {} renditions in {:?}",
        VARIANTS.len(),
        start.elapsed()
    );
    assert_eq!(line, format!("sweep {count} {digest:016x}"));
}

#[test]
#[ignore = "rewrites the goldens; only after an intended pixel change"]
fn regenerate_icon_goldens() {
    let fixture = load();
    std::fs::create_dir_all(fixture_dir()).expect("fixture dir");
    std::fs::write(fixture_dir().join("icons.txt"), icons_text(&fixture)).expect("write icons.txt");
}

/// Renders the sampled items repeatedly and prints the mean time per icon.
/// Not a pass/fail check (timing depends on the host).
#[test]
#[ignore = "timing only; cargo test -p rs910-scene --lib time_item_icons --release -- --ignored --nocapture"]
fn time_item_icons() {
    let fixture = load();
    let recorded = std::fs::read_to_string(fixture_dir().join("icons.txt")).expect("icons.txt");
    let items: Vec<&Obj> = recorded
        .lines()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
        .map(|l| {
            fixture
                .objs
                .get(l.split(' ').next().unwrap().parse().unwrap())
                .unwrap()
        })
        .collect();
    let render_all = || {
        for obj in &items {
            hashes(&fixture, obj);
        }
    };
    render_all();
    let passes = 5;
    let start = Instant::now();
    for _ in 0..passes {
        render_all();
    }
    let icons = passes * items.len() * VARIANTS.len();
    println!(
        "ICON-TIMING {icons} icons, {:.1} us per icon",
        start.elapsed().as_secs_f64() * 1e6 / icons as f64
    );
}
