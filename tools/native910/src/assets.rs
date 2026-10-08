//! Interface asset linker: what assets each component uses, and which
//! components use each asset.
//!
//! Components name four asset kinds by id, each resolved against its pack
//! roster (group presence = existence; only sprites are decoded anywhere,
//! and the linker never decodes — it links):
//!
//! | Field | Body | Pack roster |
//! |-------|------|-------------|
//! | `graphic` | graphic (type 5) | `client.sprites.js5` (group id IS the sprite id) |
//! | `model` | model (type 6) | `client.models.js5` |
//! | `anim` | model (type 6) | `client.anims.js5` |
//! | `font` | text (type 4) | `client.fontmetrics.js5` |
//!
//! Negative ids are the none-sentinel (the cache convention: `-1` =
//! absent) and are skipped silently — but counted, so a corpus where
//! negatives stop meaning "none" shows up as a use-count collapse, not a
//! silent pass. A non-negative id absent from its roster is recorded as
//! [`MissingAsset`], never dropped: a missing asset is either a dead
//! reference or a wrong roster mapping, and both deserve eyes.
//!
//! Deliberate non-coverage (stated, not overlooked):
//!
//! * `int_params`/`str_params` keys can carry asset ids in specific games,
//!   but no per-key mapping is proven for 910 — tagging them would be
//!   guessing.
//! * Script-side asset references (`if_setgraphic`'s graphic id, hook-arg
//!   sprite ids) ride the xref literal-window machinery, not this module —
//!   a follow-up on top of [`crate::xref`].
//! * Model transforms (`ox/ax/zoom/…`) and colours are data, not assets.

use crate::error::Result;
use crate::interface::ComponentBody;
use crate::pack::PackArchive;
use crate::xref::ComponentMap;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// An asset kind a component can reference.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AssetKind {
    /// Sprite sheet (`graphic` field).
    Sprite,
    /// 3D model (`model` field).
    Model,
    /// Animation (`anim` field).
    Anim,
    /// Font metrics (`font` field).
    Font,
}

impl AssetKind {
    /// Source word for the kind.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Sprite => "sprite",
            Self::Model => "model",
            Self::Anim => "anim",
            Self::Font => "font",
        }
    }

    /// Parse a source word; anything else is `None`.
    #[must_use]
    pub fn parse_word(word: &str) -> Option<Self> {
        match word {
            "sprite" => Some(Self::Sprite),
            "model" => Some(Self::Model),
            "anim" => Some(Self::Anim),
            "font" => Some(Self::Font),
            _ => None,
        }
    }

    /// Pack file holding this kind's roster.
    #[must_use]
    pub fn pack_file(self) -> &'static str {
        match self {
            Self::Sprite => "client.sprites.js5",
            Self::Model => "client.models.js5",
            Self::Anim => "client.anims.js5",
            Self::Font => "client.fontmetrics.js5",
        }
    }

    /// All kinds, in display order.
    pub const ALL: [Self; 4] = [Self::Sprite, Self::Model, Self::Anim, Self::Font];
}

/// One asset reference: its kind plus the referenced id.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssetRef {
    /// Referenced kind.
    pub kind: AssetKind,
    /// Referenced id (pack group).
    pub id: i32,
}

/// One component→asset use: component (`iface`, `child`) references `asset`
/// through body field `field` (`graphic`, `model`, `anim`, or `font`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetUse {
    /// Owning interface id.
    pub iface: i32,
    /// Owning child (pack file) index.
    pub child: u32,
    /// Referenced asset.
    pub asset: AssetRef,
    /// Body field carrying the reference.
    pub field: &'static str,
}

/// A non-negative asset id absent from its pack roster: a dead reference or
/// a wrong roster mapping — recorded, never dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingAsset {
    /// Referencing interface id.
    pub iface: i32,
    /// Referencing child index.
    pub child: u32,
    /// Missing asset.
    pub asset: AssetRef,
    /// Body field carrying the reference.
    pub field: &'static str,
}

/// Build counters, for gates and reports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssetStats {
    /// Components scanned.
    pub components: usize,
    /// Uses recorded.
    pub uses: usize,
    /// Negative (none-sentinel) fields skipped.
    pub nones: usize,
    /// Missing-asset references recorded.
    pub missing: usize,
}

/// The whole index: uses per component, reverse lookup per asset, missing
/// references, and build counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetIndex {
    /// Per `(interface, child)`: every asset use, in field order.
    pub uses: BTreeMap<(i32, u32), Vec<AssetUse>>,
    /// Per asset: every using `(interface, child)`, in scan order.
    pub by_asset: BTreeMap<AssetRef, Vec<(i32, u32)>>,
    /// Non-negative ids absent from their roster, in scan order.
    pub missing: Vec<MissingAsset>,
    /// Build counters.
    pub stats: AssetStats,
}

/// Load every asset roster: pack group ids per kind. Existence is group
/// presence — no pack is decoded.
pub fn load_rosters(pack_root: &Path) -> Result<BTreeMap<AssetKind, BTreeSet<i32>>> {
    let mut rosters = BTreeMap::new();
    for kind in AssetKind::ALL {
        let archive = PackArchive::open(&pack_root.join(kind.pack_file()))?;
        let mut ids = BTreeSet::new();
        for group in archive.group_ids() {
            ids.insert(i32::try_from(group).unwrap_or(i32::MAX));
        }
        rosters.insert(kind, ids);
    }
    Ok(rosters)
}

/// Build the index over decoded components. Total: negatives are counted
/// nones, roster misses are [`MissingAsset`], everything else is an edge.
#[must_use]
pub fn build_asset_index(
    components: &ComponentMap,
    rosters: &BTreeMap<AssetKind, BTreeSet<i32>>,
) -> AssetIndex {
    let mut index = AssetIndex::default();
    index.stats.components = components.len();
    for ((iface, child), component) in components {
        let fields: Vec<(&'static str, AssetKind, i32)> = match &component.body {
            ComponentBody::Graphic { graphic, .. } => {
                vec![("graphic", AssetKind::Sprite, *graphic)]
            }
            ComponentBody::Model { id, anim, .. } => {
                vec![
                    ("model", AssetKind::Model, *id),
                    ("anim", AssetKind::Anim, *anim),
                ]
            }
            ComponentBody::Text { font, .. } => vec![("font", AssetKind::Font, *font)],
            ComponentBody::Layer { .. }
            | ComponentBody::Rectangle { .. }
            | ComponentBody::Line { .. } => Vec::new(),
        };
        for (field, kind, id) in fields {
            if id < 0 {
                index.stats.nones += 1;
                continue;
            }
            let asset = AssetRef { kind, id };
            let present = rosters
                .get(&kind)
                .is_some_and(|roster| roster.contains(&id));
            if present {
                index.stats.uses += 1;
                index
                    .uses
                    .entry((*iface, *child))
                    .or_default()
                    .push(AssetUse {
                        iface: *iface,
                        child: *child,
                        asset,
                        field,
                    });
                index
                    .by_asset
                    .entry(asset)
                    .or_default()
                    .push((*iface, *child));
            } else {
                index.stats.missing += 1;
                index.missing.push(MissingAsset {
                    iface: *iface,
                    child: *child,
                    asset,
                    field,
                });
            }
        }
    }
    index
}

/// One-line asset summary: uses per kind, nones skipped, missing count with
/// the first samples.
#[must_use]
pub fn format_asset_summary(index: &AssetIndex) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mut per_kind: BTreeMap<AssetKind, usize> = BTreeMap::new();
    for uses in index.uses.values() {
        for use_ in uses {
            *per_kind.entry(use_.asset.kind).or_insert(0) += 1;
        }
    }
    let _ = write!(out, "assets: ");
    let mut first = true;
    for kind in AssetKind::ALL {
        if !first {
            out.push_str(", ");
        }
        first = false;
        let _ = write!(
            out,
            "{} {}",
            per_kind.get(&kind).copied().unwrap_or(0),
            kind.word()
        );
    }
    let _ = writeln!(
        out,
        " uses ({} nones skipped, {} missing)",
        index.stats.nones, index.stats.missing
    );
    for sample in index.missing.iter().take(10) {
        let _ = writeln!(
            out,
            "  missing {}/{}: {} {} via {}",
            sample.iface,
            sample.child,
            sample.asset.kind.word(),
            sample.asset.id,
            sample.field
        );
    }
    out
}

/// Render one component's asset lines (`uses` plus any missing), for the
/// interface view.
#[must_use]
pub fn format_component_assets(iface: i32, child: u32, index: &AssetIndex) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(uses) = index.uses.get(&(iface, child)) {
        for use_ in uses {
            lines.push(format!(
                "    asset {}: {} {}",
                use_.field,
                use_.asset.kind.word(),
                use_.asset.id
            ));
        }
    }
    for missing in index
        .missing
        .iter()
        .filter(|miss| miss.iface == iface && miss.child == child)
    {
        lines.push(format!(
            "    asset {}: {} {} (MISSING)",
            missing.field,
            missing.asset.kind.word(),
            missing.asset.id
        ));
    }
    lines
}

/// Render every user of one asset, for reverse lookup.
#[must_use]
pub fn format_asset_users(asset: AssetRef, index: &AssetIndex) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "{} {} used by:", asset.kind.word(), asset.id);
    match index.by_asset.get(&asset) {
        None => out.push_str("  (nobody)\n"),
        Some(users) => {
            for (iface, child) in users {
                let fields: Vec<&str> = index
                    .uses
                    .get(&(*iface, *child))
                    .map(|uses| {
                        uses.iter()
                            .filter(|use_| use_.asset == asset)
                            .map(|use_| use_.field)
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = writeln!(out, "  {iface}/{child} via {}", fields.join(", "));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::{ComponentBody, InterfaceComponent, Transmits};

    fn component(body: ComponentBody) -> InterfaceComponent {
        InterfaceComponent {
            version: -1,
            type_id: 5,
            name: None,
            clientcode: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            width_mode: 0,
            height_mode: 0,
            x_mode: 0,
            y_mode: 0,
            aspect: None,
            layer: -1,
            hide: false,
            noclickthrough: false,
            body,
            keymask: 0,
            keybinds: Vec::new(),
            opbase: String::new(),
            ops: Vec::new(),
            opname_nibble: 0,
            opname_first: None,
            opname_second: None,
            pausetext: None,
            dragdeadzone: 0,
            dragdeadtime: 0,
            dragrenderbehaviour: 0,
            targetverb: String::new(),
            target: None,
            mouseovercursor: -1,
            int_params: Vec::new(),
            str_params: Vec::new(),
            hooks: crate::interface::Hooks::default(),
            transmits: Transmits::default(),
        }
    }

    fn graphic_body(graphic: i32) -> ComponentBody {
        ComponentBody::Graphic {
            graphic,
            angle: 0,
            tiling: false,
            alpha: false,
            trans: 0,
            outline: 0,
            shadow: 0,
            vflip: false,
            hflip: false,
            colour: 0,
            clickmask: false,
        }
    }

    fn rosters() -> BTreeMap<AssetKind, BTreeSet<i32>> {
        BTreeMap::from([
            (AssetKind::Sprite, BTreeSet::from([7])),
            (AssetKind::Model, BTreeSet::from([9])),
            (AssetKind::Anim, BTreeSet::from([11])),
            (AssetKind::Font, BTreeSet::from([12])),
        ])
    }

    #[test]
    fn graphic_uses_resolve_and_reverse_lookup() {
        let components = BTreeMap::from([((3, 1), component(graphic_body(7)))]);
        let index = build_asset_index(&components, &rosters());
        assert_eq!(index.stats.uses, 1);
        assert_eq!(index.stats.missing, 0);
        let uses = &index.uses[&(3, 1)];
        assert_eq!(uses.len(), 1);
        assert_eq!(
            uses[0].asset,
            AssetRef {
                kind: AssetKind::Sprite,
                id: 7
            }
        );
        assert_eq!(uses[0].field, "graphic");
        assert_eq!(index.by_asset[&uses[0].asset], vec![(3, 1)]);
        let text = format_asset_users(uses[0].asset, &index);
        assert!(text.contains("3/1 via graphic"), "unexpected:\n{text}");
    }

    #[test]
    fn negatives_are_nones_and_absent_ids_are_missing() {
        let components = BTreeMap::from([
            ((3, 1), component(graphic_body(-1))),
            ((3, 2), component(graphic_body(7777))),
        ]);
        let index = build_asset_index(&components, &rosters());
        assert_eq!(index.stats.nones, 1);
        assert_eq!(index.stats.missing, 1);
        assert_eq!(index.stats.uses, 0);
        assert!(!index.uses.contains_key(&(3, 1)));
        let miss = &index.missing[0];
        assert_eq!((miss.iface, miss.child), (3, 2));
        assert_eq!(
            miss.asset,
            AssetRef {
                kind: AssetKind::Sprite,
                id: 7777
            }
        );
    }

    #[test]
    fn model_bodies_link_model_and_anim() {
        let components = BTreeMap::from([(
            (3, 1),
            component(ComponentBody::Model {
                id: 9,
                origin: false,
                extended: false,
                orthog: false,
                nodepth: false,
                ox: 0,
                oy: 0,
                oz: 0,
                ax: 0,
                ay: 0,
                az: 0,
                zoom: 0,
                anim: 11,
                objwidth: None,
                objheight: None,
            }),
        )]);
        let index = build_asset_index(&components, &rosters());
        assert_eq!(index.stats.uses, 2);
        let fields: Vec<&str> = index.uses[&(3, 1)].iter().map(|use_| use_.field).collect();
        assert_eq!(fields, vec!["model", "anim"]);
    }
}
