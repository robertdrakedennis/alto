//! Models of the cache that used to panic the build or the posing. Each fault
//! class has one real model; the original client throws on the same input,
//! and the client must drop or skip instead of crashing.

use crate::billboard::BillboardStore;
use crate::cache::Pack;
use crate::floor::FloorHeights;
use crate::gpumodel::{BuildParams, GpuModel, ModelStores, TerrainHeights, MODEL_DETAIL_FLAGS};
use crate::modelunlit::ModelUnlit;
use crate::particle::EmitterStore;
use crate::texture::MaterialStore;

/// A face of this model reads texture vertex 117 of a table of 114.
const MODEL_TEXTURE_VERTEX_OVERRUN: u32 = 24445;
/// A model with no vertices.
const MODEL_EMPTY: u32 = 1077;
/// A model with vertices that is exactly flat (no height).
const MODEL_FLAT: u32 = 596;
/// A model with a height.
const MODEL_TALL: u32 = 6321;
/// A model whose successor has more than 32767 unique vertices.
const MODEL_BEFORE_HUGE: u32 = 67632;
const MODEL_HUGE: u32 = 67633;

/// The loc lighting the build uses: flags, ambient and contrast.
const LOC_LIGHTING_FLAGS: i32 = 0x1_F01F;
const LOC_AMBIENT: i32 = 64;
const LOC_CONTRAST: i32 = 768;

/// `hill_change` modes: follow the ground, follow it below a fraction of the
/// height, and stretch between the floor and the level above.
const HILL_FOLLOW_BELOW: i32 = 2;
const HILL_STRETCH: i32 = 5;
const HILL_MODES: std::ops::RangeInclusive<i32> = 1..=5;
/// A hill-change value with something to scale by, and one without.
const HILL_VALUE: i32 = 30;
const HILL_VALUE_ZERO: i32 = 0;
/// Where the model stands on the synthetic ground: the middle of a tile.
const HILL_ORIGIN: [i32; 3] = [512, 40, 512];

/// The synthetic ground: 5x5 tiles of 512 units over a 6x6 corner grid.
const GROUND_TILES: usize = 5;
const GROUND_TILE_SIZE: i32 = 512;
const GROUND_CORNERS_PER_ROW: i32 = 6;
const GROUND_CORNERS: i32 = GROUND_CORNERS_PER_ROW * GROUND_CORNERS_PER_ROW;
/// Height pattern: a corner's depth below zero, varied per row, column and
/// level so that every hill-change mode has slope to follow.
const GROUND_COLUMN_STEP: i32 = 37;
const GROUND_ROW_STEP: i32 = 53;
const GROUND_LEVEL_STEP: i32 = 400;
const GROUND_DEPTH: i32 = 300;

/// A normal-merge offset between two neighbouring models.
const MERGE_OFFSET: [i32; 3] = [8, 0, -8];
/// The largest unique-vertex count a slot can address before it reads negative.
const MAX_READABLE_UNIQUE: i32 = 32767;

struct Stores {
    materials: MaterialStore,
    billboards: BillboardStore,
    emitters: EmitterStore,
}

fn load_stores(pack: &Pack) -> Stores {
    Stores {
        materials: MaterialStore::load(pack).expect("materials"),
        billboards: BillboardStore::load(pack).expect("billboards"),
        emitters: EmitterStore::load(pack).expect("particle emitters"),
    }
}

/// The lit model `id` at the loc lighting settings.
fn lit_model(pack: &Pack, stores: &Stores, id: u32) -> anyhow::Result<GpuModel> {
    let unlit = ModelUnlit::load(pack, id)?;
    GpuModel::new(
        &ModelStores {
            materials: &stores.materials,
            billboards: &stores.billboards,
            emitters: &stores.emitters,
        },
        &unlit,
        BuildParams {
            flags: LOC_LIGHTING_FLAGS,
            ambient: LOC_AMBIENT,
            contrast: LOC_CONTRAST,
            detail: MODEL_DETAIL_FLAGS,
        },
    )
}

/// A hill of 5x5 tiles and the level above it.
fn synthetic_ground() -> (FloorHeights, FloorHeights) {
    let heights = |level: i32| -> Vec<i32> {
        (0..GROUND_CORNERS)
            .map(|i| {
                let column = i % GROUND_CORNERS_PER_ROW;
                let row = i / GROUND_CORNERS_PER_ROW;
                -((column * GROUND_COLUMN_STEP + row * GROUND_ROW_STEP + level * GROUND_LEVEL_STEP)
                    % GROUND_DEPTH)
            })
            .collect()
    };
    (
        FloorHeights::new(GROUND_TILES, GROUND_TILES, GROUND_TILE_SIZE, heights(0)),
        FloorHeights::new(GROUND_TILES, GROUND_TILES, GROUND_TILE_SIZE, heights(1)),
    )
}

fn vertices(m: &GpuModel) -> Vec<[i32; 3]> {
    (0..m.vertex_count as usize)
        .map(|i| [m.vx[i], m.vy[i], m.vz[i]])
        .collect()
}

/// A face whose stored texture coordinates run past the model's table: the
/// build fails (the caller drops the model) instead of indexing out of range.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn stored_texture_coordinates_outside_the_table_fail_the_build() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let Err(error) = lit_model(&pack, &stores, MODEL_TEXTURE_VERTEX_OVERRUN) else {
        panic!("model {MODEL_TEXTURE_VERTEX_OVERRUN} must not build");
    };
    assert!(format!("{error:#}").contains("texture vertex 117 of 114"));
}

/// A model with no vertices has nothing to drape: every hill change leaves it
/// alone (its bounds are the empty sentinels, which walk off the height grid).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn empty_model_ignores_every_hill_change() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let base = lit_model(&pack, &stores, MODEL_EMPTY).expect("builds");
    assert_eq!(base.vertex_count, 0);
    let (ground, above) = synthetic_ground();
    for mode in HILL_MODES {
        let mut m = base.clone();
        m.hill_change(
            mode,
            HILL_VALUE,
            TerrainHeights {
                floor: &ground,
                above: Some(&above),
            },
            HILL_ORIGIN,
        );
        assert!(vertices(&m).is_empty());
    }
}

/// The stretch hill change divides by the model's height and the softened
/// one by its value; where the original divides by zero the model stays as
/// it was.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn hill_changes_that_divide_by_zero_leave_the_model_alone() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let (ground, above) = synthetic_ground();
    let terrain = TerrainHeights {
        floor: &ground,
        above: Some(&above),
    };
    let mut flat = lit_model(&pack, &stores, MODEL_FLAT).expect("builds");
    assert!(flat.vertex_count > 0 && flat.max_y() == flat.min_y());
    let before = vertices(&flat);
    flat.hill_change(HILL_STRETCH, HILL_VALUE, terrain, HILL_ORIGIN);
    assert_eq!(vertices(&flat), before);
    let mut tall = lit_model(&pack, &stores, MODEL_TALL).expect("builds");
    assert!(tall.max_y() != tall.min_y());
    let before = vertices(&tall);
    tall.hill_change(HILL_FOLLOW_BELOW, HILL_VALUE_ZERO, terrain, HILL_ORIGIN);
    assert_eq!(vertices(&tall), before);
}

/// The softened hill change multiplies in 32 bits and wraps, as the original
/// does; the test profile checks overflow, so this fails if it ever traps.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn softened_hill_change_wraps_instead_of_trapping() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let (ground, above) = synthetic_ground();
    let base = lit_model(&pack, &stores, MODEL_TALL).expect("builds");
    let mut m = base.clone();
    m.hill_change(
        HILL_FOLLOW_BELOW,
        HILL_VALUE,
        TerrainHeights {
            floor: &ground,
            above: Some(&above),
        },
        HILL_ORIGIN,
    );
    assert_ne!(vertices(&m), vertices(&base));
}

/// A neighbour with more than 32767 unique vertices has slots that read
/// negative in one walk of the normal merge; the merge stops there instead of
/// indexing with them.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn normal_merge_with_a_huge_neighbour_stops_at_unreadable_slots() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let mut model = lit_model(&pack, &stores, MODEL_BEFORE_HUGE).expect("builds");
    let mut neighbour = lit_model(&pack, &stores, MODEL_HUGE).expect("builds");
    assert!(neighbour.unique_count > MAX_READABLE_UNIQUE);
    let [dx, dy, dz] = MERGE_OFFSET;
    model.merge_normals(&mut neighbour, dx, dy, dz);
}
