//! Whole-pipeline item-icon goldens: model, rasteriser, outlines, drop
//! shadows, noted-item overlays and stack-count labels.
//!
//! `fixtures/icon-goldens/pipeline.txt` holds an FNV-1a 64 hash of each icon's
//! final 36x32 ARGB sprite: one line per sampled item (`id` and one hash per
//! entry of [`VARIANTS`]) and `stack` lines for a coin pile at several counts
//! (which switches the stack's model and count label). The sample is every
//! n-th item with a model plus the noted items. The lighting jitter icons
//! normally get from the clock is seeded per icon so the hashes are stable.
//!
//! Regenerate with `cargo test -p rs910-ui --lib regenerate_pipeline_goldens
//! -- --ignored` (only after an intended pixel change).

use super::*;
use crate::{config::ObjStore, ui_fonts::Fonts};
use std::{fmt::Write as _, path::PathBuf};

/// Count, outline mode, shadow colour and count-label mode of each rendition.
/// Mode 1 always labels, mode 2 labels stackable items and counts above 1.
const VARIANTS: [(i32, i32, i32, i32); 6] = [
    (1, 0, 0, 0),
    (1, 1, 0xff000000u32 as i32, 0),
    (5, 2, 0xff202020u32 as i32, 2),
    (100_000, 0, 0, 1),
    (12_345_678, 1, 0xff000000u32 as i32, 1),
    (2, 0, 0xff000000u32 as i32, 2),
];
const STACK_COUNTS: [i32; 12] = [
    1, 2, 3, 4, 5, 25, 100, 250, 1000, 10_000, 100_000, 10_000_000,
];
const COINS: i32 = rs910_symbols::obj::COINS.id();

fn fixture_path() -> PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/icon-goldens/pipeline.txt")
}

struct Fixture {
    icons: Icons,
    ids: Vec<i32>,
}

fn load() -> Fixture {
    let pack = crate::test_support::require_pack("client.config.js5");
    let objs = Rc::new(ObjStore::load(&pack).expect("obj configs"));
    let fonts = Fonts::from_pack(pack.clone(), None).expect("fonts");
    let icons = Icons::new(pack, objs.clone(), &fonts).expect("icons");
    let noted = |obj: &crate::config::Obj| obj.inventory.derived.iter().any(|p| p[1] != -1);
    let with_models: Vec<i32> = objs
        .iter()
        .filter(|(_, obj)| !obj.models.is_empty())
        .map(|(&id, _)| id as i32)
        .collect();
    let notes: Vec<i32> = objs
        .iter()
        .filter(|(_, obj)| noted(obj))
        .map(|(&id, _)| id as i32)
        .collect();
    let mut ids: Vec<i32> = with_models
        .iter()
        .copied()
        .step_by(with_models.len() / 150)
        .collect();
    ids.extend(notes.iter().copied().step_by(notes.len() / 40));
    ids.sort_unstable();
    ids.dedup();
    Fixture { icons, ids }
}

fn hash(icons: &mut Icons, id: i32, count: i32, outline: i32, shadow: i32, mode: i32) -> String {
    icons.reset();
    icons
        .random
        .set_seed(i64::from(id) * 64 + i64::from(mode) * 8 + i64::from(outline));
    let key = Key {
        id,
        count,
        outline,
        shadow,
        mode,
        enlarge: false,
        wear: false,
    };
    match icons.build(key, 0) {
        Ok(sprite) => {
            let hash = sprite.argb.iter().fold(0xcbf29ce484222325u64, |h, &word| {
                (h ^ u64::from(word as u32)).wrapping_mul(0x100000001b3)
            });
            format!("{hash:016x}")
        }
        Err(_) => "err".to_string(),
    }
}

fn pipeline_text(fixture: &mut Fixture) -> String {
    let mut text = String::from(
        "# id + one hash per variant; stack lines: item count hash (ui_icons/tests.rs)\n",
    );
    for &id in &fixture.ids {
        let hashes: Vec<String> = VARIANTS
            .iter()
            .map(|&(count, outline, shadow, mode)| {
                hash(&mut fixture.icons, id, count, outline, shadow, mode)
            })
            .collect();
        let _ = writeln!(text, "{id} {}", hashes.join(" "));
    }
    for count in STACK_COUNTS {
        let _ = writeln!(
            text,
            "stack {COINS} {count} {}",
            hash(&mut fixture.icons, COINS, count, 0, 0, 2)
        );
    }
    text
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn item_icons_match_the_pipeline_goldens() {
    let mut fixture = load();
    let recorded = std::fs::read_to_string(fixture_path()).expect("pipeline.txt");
    let mut differing = Vec::new();
    let mut checked = 0;
    for line in recorded.lines().filter(|l| !l.starts_with('#')) {
        let fields: Vec<&str> = line.split(' ').collect();
        let (id, hashes) = if fields[0] == "stack" {
            let count: i32 = fields[2].parse().expect("count");
            (COINS, vec![hash(&mut fixture.icons, COINS, count, 0, 0, 2)])
        } else {
            let id: i32 = fields[0].parse().expect("item id");
            let hashes = VARIANTS
                .iter()
                .map(|&(count, outline, shadow, mode)| {
                    hash(&mut fixture.icons, id, count, outline, shadow, mode)
                })
                .collect();
            (id, hashes)
        };
        let golden = if fields[0] == "stack" {
            &fields[3..]
        } else {
            &fields[1..]
        };
        if hashes != golden {
            differing.push(format!("item {id}: golden {golden:?}, got {hashes:?}"));
        }
        checked += 1;
    }
    assert!(checked > 150, "only {checked} golden lines");
    assert!(
        differing.is_empty(),
        "{} icons differ:\n{}",
        differing.len(),
        differing.join("\n")
    );
}

#[test]
#[ignore = "rewrites the goldens; only after an intended pixel change"]
fn regenerate_pipeline_goldens() {
    let mut fixture = load();
    let text = pipeline_text(&mut fixture);
    std::fs::write(fixture_path(), text).expect("write pipeline.txt");
}
