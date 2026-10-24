use super::*;

fn pack() -> crate::cache::Pack {
    crate::test_support::require_pack("client.config.js5")
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
