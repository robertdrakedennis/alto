use super::*;

fn pack() -> crate::cache::Pack {
    crate::test_support::require_pack("client.config.js5")
}

#[test]
fn skybox_type_decodes_every_opcode() {
    // 1 material, 2 decors, 3 sun, 4 fill, 5 model (4-byte smart), 6 skip.
    let bytes = [
        1, 0x0A, 0x93, 2, 2, 0, 5, 0, 6, 3, 1, 4, 1, 5, 0x80, 0x01, 0x0A, 0xBC, 6, 0, 26, 0,
    ];
    let t = SkyBoxType::decode(&bytes).unwrap();
    assert_eq!(t.material, 2707);
    assert_eq!(t.decors, Some(vec![5, 6]));
    assert_eq!(t.sun_decor, 1);
    assert_eq!(t.fill, Some(SkyBoxFillMode::Horizon));
    assert_eq!(t.model, 0x10ABC);
    // 2-byte smart 32767 is -1; unknown fill id decodes to None.
    let t = SkyBoxType::decode(&[5, 0x7F, 0xFF, 4, 9, 0]).unwrap();
    assert_eq!(t.model, -1);
    assert_eq!(t.fill, None);
    assert_eq!(SkyBoxType::decode(&[0]).unwrap(), SkyBoxType::default());
}

#[test]
fn sky_decor_type_decodes_every_opcode() {
    let bytes = [
        1, 0, 64, 2, 3, 0xFF, 0xFF, 0, 10, 0x80, 0, 4, 1, 5, 0, 7, 6, 0x12, 0x34, 0x56, 7, 0, 1, 0,
        2, 0xFF, 0xFD, 0,
    ];
    let t = SkyDecorType::decode(&bytes).unwrap();
    assert_eq!(t.size, 64);
    assert!(t.fixed);
    assert_eq!(t.position, [-1, 10, -32768]);
    assert_eq!(t.kind, 1);
    assert_eq!(t.texture, 7);
    assert_eq!(t.colour, 0x123456);
    assert_eq!(t.rotation, [1, 2, -3]);
    assert_eq!(SkyDecorType::decode(&[0]).unwrap().colour, 16_777_216);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_cache_skybox_types_decode() {
    let pack = pack();
    let types = SkyTypes::load(&pack).unwrap();
    assert_eq!(types.skybox_count(), 133);
    // Survey of config group 29 (scratchpad js5 dump): type 0 is the
    // material-2707 horizon sky with model 43756.
    let t0 = types.skybox(0);
    assert_eq!(t0.material, 2707);
    assert_eq!(t0.fill, Some(SkyBoxFillMode::Horizon));
    assert_eq!(t0.model, 43756);
    assert_eq!(t0.decors, None);
    // Type 5 has no model opcode: the default model 0 is loaded.
    let t5 = types.skybox(5);
    assert_eq!((t5.material, t5.model), (1151, 0));
    // Type 65 keeps the default tiled fill.
    assert_eq!(types.skybox(65).fill, Some(SkyBoxFillMode::Tiled));
    // No type references a decor, and the cache has no decor group.
    assert!((0..133).all(|id| types.skybox(id).decors.is_none()));
    assert_eq!(types.decor(0), SkyDecorType::default());
}

#[test]
fn bit_helpers_round_to_powers_of_two() {
    assert_eq!(highest_bit(600), 512);
    assert_eq!(highest_bit(512), 512);
    assert_eq!(highest_bit(1), 1);
    assert_eq!(highest_bit(0), 0);
    assert_eq!(bitceil(600), 1024);
    assert_eq!(bitceil(8), 8);
    assert_eq!(bitceil(9), 16);
}

#[test]
fn update_selects_model_by_preference_and_sprite_size_by_height() {
    let mut types = SkyTypes::default();
    types.boxes_mut().insert(
        3,
        SkyBoxType {
            material: 2707,
            model: 3845,
            fill: Some(SkyBoxFillMode::Horizon),
            ..SkyBoxType::default()
        },
    );
    let mut owner = SkyboxOwner::new(types);
    owner.select(Some(crate::env::SkyboxRef {
        kind: 3,
        a: 0,
        b: 0,
        c: 0,
        yaw_offset: 0,
    }));
    owner.update(600, 1);
    let sky = &owner.boxes[&(3, 0, 0, 0)];
    assert!(sky.model_wanted);
    assert_eq!(sky.sprite_size, 512);
    let layers = owner.frame(0, 1983, 8139, 0, 0x123456).unwrap();
    assert_eq!(
        layers,
        vec![
            SkyLayer::Clear { rgb: 0x123456 },
            SkyLayer::Model {
                key: (3, 0, 0, 0),
                pitch: 1983,
                yaw: 8139,
                roll: 0,
                fade: 0
            }
        ]
    );
    // Skyboxes off: the 2D material path, yaw offset by 16 (2 << 3).
    owner.update(300, 0);
    let sky = &owner.boxes[&(3, 0, 0, 0)];
    assert!(!sky.model_wanted);
    assert_eq!(sky.sprite_size, 256);
    match &owner.frame(2, 1983, 16380, 0, 0x123456).unwrap()[0] {
        SkyLayer::Material {
            yaw, alpha, first, ..
        } => {
            assert_eq!(*yaw, (16 + 16380) & 0x3FFF);
            assert_eq!(*alpha, 255);
            assert!(*first);
        }
        other => panic!("{other:?}"),
    }
    // No environment skybox: the frame falls back to clear(3, fog).
    owner.select(None);
    assert!(owner.frame(0, 0, 0, 0, 0).is_none());
}

#[test]
fn material_minus_one_fills_the_fog() {
    let mut types = SkyTypes::default();
    types.boxes_mut().insert(
        1,
        SkyBoxType {
            material: -1,
            model: -1,
            ..SkyBoxType::default()
        },
    );
    let mut owner = SkyboxOwner::new(types);
    owner.current = Some(owner.create((1, 0, 0, 0)));
    owner.update(400, 1);
    let layers = owner.frame(0, 0, 0, 0, 0xABCDEF).unwrap();
    assert_eq!(
        layers,
        vec![
            SkyLayer::Fill {
                argb: 0xFFAB_CDEF,
                blend: false
            },
            SkyLayer::Fill {
                argb: 0xFFAB_CDEF,
                blend: false
            }
        ]
    );
}

#[test]
fn horizon_quads_scroll_and_wrap() {
    // 800x600 viewport, pitch 1983, yaw 0: one row offset down by
    // h * pitch / -4096 (negative => above), wrapped horizontally.
    let q = flat_quads(
        [800, 600],
        FlatSky {
            pitch: 1983,
            yaw: 0,
            alpha: 255,
            fill: Some(SkyBoxFillMode::Horizon),
            edges: [0xFF00_0001, 0],
        },
    );
    let scroll_y = 600 * 1983 / -4096;
    assert_eq!(scroll_y, -290);
    // scroll_x = (800 - 600) / 2 = 100: tiles at -500, 100, 700.
    assert_eq!(
        &q[..3],
        &[
            FlatQuad::Sprite {
                rect: [-500, scroll_y, 600, 600],
                alpha: 255
            },
            FlatQuad::Sprite {
                rect: [100, scroll_y, 600, 600],
                alpha: 255
            },
            FlatQuad::Sprite {
                rect: [700, scroll_y, 600, 600],
                alpha: 255
            },
        ]
    );
    // First pixel opaque: fill above the row; last pixel transparent: no fill.
    assert_eq!(
        q[3],
        FlatQuad::Fill {
            rect: [0, 0, 800, scroll_y + 1],
            argb: 0xFF00_0001
        }
    );
    assert_eq!(q.len(), 4);
    // Tiled: rows wrap into [0, h).
    let q = flat_quads(
        [600, 600],
        FlatSky {
            pitch: 1983,
            yaw: 0,
            alpha: 128,
            fill: Some(SkyBoxFillMode::Tiled),
            edges: [0, 0],
        },
    );
    assert!(q
        .iter()
        .all(|quad| matches!(quad, FlatQuad::Sprite { alpha: 128, .. })));
    assert_eq!(q.len(), 4);
}

#[test]
fn decor_selection_by_distance() {
    let mut d = SkyboxDecor::from_type(&SkyDecorType {
        size: 64,
        position: [0, -1000, 1000],
        ..SkyDecorType::default()
    });
    assert!(d.select(0, 0, 0, 600));
    // distance = sqrt(2) * 1000.
    assert_eq!(d.distance, 1414);
    assert_eq!(d.screen_size, 64 * 600 / 1414);
    assert_eq!(d.sprite_size, 32);
    // Too small on screen: not selected.
    assert!(!d.select(0, 0, 0, 100));
    // Fixed direction: distance is the sentinel and size scales by 1024.
    let mut f = SkyboxDecor::from_type(&SkyDecorType {
        size: 512,
        fixed: true,
        position: [0, 0, 256],
        ..SkyDecorType::default()
    });
    assert!(f.select(9, 9, 9, 512));
    assert_eq!(f.distance, 1_073_741_823);
    assert_eq!(f.screen_size, 256);
}

#[test]
fn fade_state_matches_skybox_helpers() {
    let mut types = SkyTypes::default();
    types.boxes_mut().insert(1, SkyBoxType::default());
    types.boxes_mut().insert(2, SkyBoxType::default());
    let mut owner = SkyboxOwner::new(types);
    let a = owner.create((1, 0, 0, 0));
    let b = owner.create((2, 0, 0, 0));
    owner.current = Some(a);
    owner.begin_fade(Some(b));
    assert!(owner.boxes[&a].fading);
    assert_eq!(owner.boxes[&a].fade_partner, Some(b));
    owner.fade_sample(Some(a), Some(b), 0.5);
    assert_eq!(owner.boxes[&a].fade, 127);
    owner.end_fade(Some(b));
    assert_eq!(owner.current, Some(b));
    assert_eq!(faded_alpha(None, 40), 40);
    assert_eq!(faded_alpha(Some(0), 255), 255);
    assert_eq!(faded_alpha(Some(100), 0), 100);
}
