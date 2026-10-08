//! Corpus gate for the interface asset linker: build it over the real
//! packs and pin the shape. Every use is exact by construction (body-field
//! ids checked against pack rosters), so this gate pins engagement — plus
//! the missing count, which is the drift detector for roster mappings and
//! the none-sentinel convention.
//!
//! Measured over this corpus: 21,284 sprite, 5,314 model, 612 anim and
//! 16,200 font uses, 7,676 none-sentinels skipped, zero missing.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::assets::{AssetKind, build_asset_index, format_asset_summary, load_rosters};
use native910::xref::load_corpus;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn pack_root() -> PathBuf {
    common::pack_root()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_assets_link_components_to_packs() {
    let root = pack_root();
    let needed = [
        "client.scripts.js5",
        "client.interfaces.js5",
        "client.sprites.js5",
        "client.models.js5",
        "client.anims.js5",
        "client.fontmetrics.js5",
    ];
    for file in needed {
        common::require_present(&root.join(file));
    }
    let (_scripts, components) = load_corpus(&root).expect("load corpus packs");
    let rosters = load_rosters(&root).expect("load asset rosters");
    for kind in AssetKind::ALL {
        eprintln!(
            "roster {}: {} ids",
            kind.word(),
            rosters.get(&kind).map_or(0, BTreeSet::len)
        );
    }
    let index = build_asset_index(&components, &rosters);
    let stats = &index.stats;
    eprintln!(
        "asset corpus: {} components, {} uses, {} nones, {} missing",
        stats.components, stats.uses, stats.nones, stats.missing
    );
    let mut per_kind = std::collections::BTreeMap::new();
    for uses in index.uses.values() {
        for use_ in uses {
            *per_kind.entry(use_.asset.kind).or_insert(0_usize) += 1;
        }
    }
    // Engagement pins with margin below the measured shape.
    for (kind, minimum) in [
        (AssetKind::Sprite, 18_000),
        (AssetKind::Model, 4_500),
        (AssetKind::Anim, 500),
        (AssetKind::Font, 14_000),
    ] {
        let count = per_kind.get(&kind).copied().unwrap_or(0);
        assert!(
            count >= minimum,
            "{} uses far below expectation: {count}",
            kind.word()
        );
    }
    // Zero missing over this corpus: every non-negative id resolves. Any
    // count above zero is a dead reference or a mapping bug — fail loudly
    // with the sample, like the xref dangling pin.
    assert!(
        index.missing.is_empty(),
        "{} missing asset(s), first: {:?}",
        index.missing.len(),
        index.missing.first()
    );
    // Every recorded edge resolves on both ends (true by construction;
    // asserted over the full index as the invariant).
    for uses in index.uses.values() {
        for use_ in uses {
            assert!(
                components.contains_key(&(use_.iface, use_.child)),
                "use on absent component: {use_:?}"
            );
            assert!(
                rosters
                    .get(&use_.asset.kind)
                    .is_some_and(|roster| roster.contains(&use_.asset.id)),
                "use of absent asset: {use_:?}"
            );
        }
    }
    // The summary renders (smoke).
    let summary = format_asset_summary(&index);
    assert!(summary.contains("sprite"));
    assert!(summary.contains("missing)"));
}
