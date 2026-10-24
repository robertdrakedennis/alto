use super::*;
use crate::sky_frame::SkyCache;
use crate::sky_texture::SkyAssets;
use crate::skybox::{SkyBoxType, SkyDecorType, SkyTypes, SkyboxOwner};

/// A sky box with `decors` (all lit by the first), selected for a viewport
/// `height` and updated once.
fn sky_with(decors: &[SkyDecorType], height: i32) -> SkyboxOwner {
    let mut types = SkyTypes::default();
    let ids: Vec<i32> = decors
        .iter()
        .enumerate()
        .map(|(i, d)| {
            types.decors_mut().insert(i as u32, d.clone());
            i as i32
        })
        .collect();
    types.boxes_mut().insert(
        1,
        SkyBoxType {
            decors: Some(ids),
            sun_decor: 0,
            ..SkyBoxType::default()
        },
    );
    let mut owner = SkyboxOwner::new(types);
    owner.select(Some(crate::env::SkyboxRef {
        kind: 1,
        a: 0,
        b: 0,
        c: 0,
        yaw_offset: 0,
    }));
    owner.update(height, 1);
    owner
}

fn fixed(kind: i32, texture: i32, position: [i32; 3], size: i32) -> SkyDecorType {
    SkyDecorType {
        kind,
        texture,
        position,
        size,
        fixed: true,
        ..SkyDecorType::default()
    }
}

fn stores(
    pack: &crate::cache::Pack,
) -> (
    crate::texture::MaterialStore,
    crate::billboard::BillboardStore,
    rs910_model::particle::EmitterStore,
) {
    (
        crate::texture::MaterialStore::load(pack).unwrap(),
        crate::billboard::BillboardStore::load(pack).unwrap(),
        rs910_model::particle::EmitterStore::load(pack).unwrap(),
    )
}

/// FNV-1a over the texels.
fn digest(texture: &crate::sky_texture::SkyTexture) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for &p in &texture.argb {
        for b in (p as u32).to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The three decor kinds bake to pictures of the right shape, and their
/// pixels are pinned by a digest: a texture decor is the texture's first
/// texels, a lit sphere is opaque in the middle and clear at the corners
/// and rim, a model decor covers part of the sprite; the sun's side of a
/// sphere is brighter than the far side.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn baked_decors_match_their_digest() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (materials, billboards, emitters) = stores(&pack);
    let assets = SkyAssets {
        pack: &pack,
        materials: &materials,
        billboards: &billboards,
        emitters: &emitters,
    };
    let texture = materials
        .get(2707)
        .and_then(|m| m.diffuse_texture)
        .expect("the sky material's texture") as i32;
    // The sun sits high and to the left of the decors, behind the view.
    let sun = SkyDecorType {
        colour: 0xFFEEAA,
        ..fixed(1, 2707, [-256, -256, 256], 200)
    };
    let sphere = SkyDecorType {
        colour: 0xFFCC88,
        ..fixed(1, 2707, [0, -256, -256], 200)
    };
    let sprite = SkyDecorType {
        ..fixed(0, texture, [256, 0, -256], 200)
    };
    let model = SkyDecorType {
        colour: 0xFFFFFF,
        rotation: [0, 1024, 0],
        ..fixed(2, 43756, [0, 256, -256], 200)
    };
    let owner = sky_with(&[sun, sphere, sprite, model], 300);
    let key = owner.current.expect("current");
    let placed = owner.boxes[&key].decors.as_ref().unwrap();
    let sun = &placed[0];
    let size = 64;
    let mut rows = Vec::new();
    for (i, decor) in placed.iter().enumerate() {
        let baked =
            bake(decor, size, Some(sun), &assets).unwrap_or_else(|e| panic!("decor {i}: {e:#}"));
        assert_eq!(baked.size, [size, size]);
        assert_eq!(baked.argb.len(), (size * size) as usize);
        let alpha = |x: i32, y: i32| (baked.argb[(y * size + x) as usize] as u32) >> 24;
        let covered = baked
            .argb
            .iter()
            .filter(|&&p| (p as u32) >> 24 != 0)
            .count();
        match decor.kind {
            KIND_SPHERE => {
                assert_eq!(alpha(0, 0), 0, "corner");
                assert!(alpha(size / 2, size / 2) > 0, "centre");
            }
            KIND_MODEL => {
                assert!(
                    covered > 50 && covered < (size * size) as usize,
                    "{covered}"
                );
            }
            _ => assert!(covered > (size * size) as usize / 2, "{covered}"),
        }
        // `RS910_DUMP_DECOR=<dir>` writes the sprites as PPM (over black) to look at.
        if let Some(dir) = std::env::var_os("RS910_DUMP_DECOR") {
            let mut ppm = format!("P6\n{size} {size}\n255\n").into_bytes();
            for &p in &baked.argb {
                let p = p as u32;
                let a = (p >> 24) & 0xFF;
                let over = |shift: u32| ((((p >> shift) & 0xFF) * a) / 255) as u8;
                ppm.extend([over(16), over(8), over(0)]);
            }
            let _ = std::fs::write(
                std::path::Path::new(&dir).join(format!("decor-{i}.ppm")),
                ppm,
            );
        }
        rows.push(format!(
            "decor-{}-kind-{}\t{covered}\t{:016x}",
            i,
            decor.kind,
            digest(&baked)
        ));
    }
    let path =
        rs910_core::test_support::client_dir().join("crates/rs910-scene/fixtures/sky-decor.tsv");
    let now = rows.join("\n") + "\n";
    if std::env::var_os("RS910_UPDATE_SKY_DECOR").is_some() {
        std::fs::write(&path, &now).unwrap();
    }
    let recorded = std::fs::read_to_string(&path).expect("fixtures/sky-decor.tsv");
    assert_eq!(now, recorded, "the baked decors changed");

    // The sky cache bakes what the frame's layers name, once per placement.
    let layers = owner.frame(0, 0, 0, 0, 0xABCDEF);
    let mut cache = SkyCache::default();
    cache.resolve(&assets, &owner, layers);
    let frame = cache.frame().expect("a frame");
    for i in 0..4 {
        assert!(frame.decor_sprite(key, i).is_some(), "decor {i} baked");
    }
}

/// A decor's selection and placement match the original's: whether it is
/// drawn, its distance, screen and sprite sizes, and its pitch and yaw, over
/// fixed and unfixed positions, sizes, sky origins and viewport heights.
#[test]
fn decor_placement_matches_the_recording() {
    let positions = [
        [0, -1000, 1000],
        [500, -300, -2000],
        [-3000, 800, 100],
        [0, 0, 256],
        [256, 0, 0],
        [-100, -256, -256],
        [0, -20000, 5],
        [1, 1, 1],
        [40000, -40000, 40000],
        [0, 0, 0],
    ];
    let sizes = [1, 64, 300, 4000, 90000];
    let origins = [[0, 0, 0], [100, -50, 700], [-2000, 1000, 3000]];
    let heights = [8, 100, 334, 600, 1080];
    let mut lines = String::new();
    for fixed in [false, true] {
        for p in positions {
            for size in sizes {
                for o in origins {
                    for h in heights {
                        let mut decor = SkyboxDecor::from_type(&SkyDecorType {
                            kind: 1,
                            texture: 5,
                            position: p,
                            size,
                            colour: 0xFFFFFF,
                            fixed,
                            ..SkyDecorType::default()
                        });
                        let visible = decor.select(o[0], o[1], o[2], h);
                        lines.push_str(&format!(
                            "{} {} {} {} {size} {} {} {} {h} -> {}",
                            i32::from(fixed),
                            p[0],
                            p[1],
                            p[2],
                            o[0],
                            o[1],
                            o[2],
                            i32::from(visible)
                        ));
                        if visible {
                            lines.push_str(&format!(
                                " {} {} {} {} {}",
                                decor.distance,
                                decor.screen_size,
                                decor.sprite_size,
                                decor.pitch,
                                decor.yaw
                            ));
                        }
                        lines.push('\n');
                    }
                }
            }
        }
    }
    rs910_core::test_support::frozen::assert_stream(
        "sky-decor-placement/recording",
        lines.as_bytes(),
    );
}
