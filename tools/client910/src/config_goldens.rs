//! Tests of `rs910-config` modules that also need client910 modules
//! (`map::load_lumbridge`, `gpumodel`, `modelunlit`, `particle`), so they stay
//! in this package (tools/README.md "Tests"). Moved from each module's
//! `tests` (Phase 2.4); each submodule globs the module it tests, like the
//! `use super::*` it came from (so the bodies hash identically in fn-hash.py).

mod config {
    use crate::config::*;

    /// Real config packs.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn real_pack_loc_store_covers_lumbridge() {
        let pack = crate::test_support::require_pack("client.loc.config.js5");
        let store = LocStore::load(&pack).unwrap();
        assert!(!store.is_empty(), "loc.config decoded empty");

        // Spot-checks against castle locs from the map decoder's own loc
        // list: the "Large door" of the castle entrance and the kitchen
        // "Cooking range" (1x2 footprint) must resolve with footprints and
        // loadable models.
        let world = crate::map::load_lumbridge(&pack).unwrap();
        for id in [12349_u32, 114] {
            assert!(
                world.locs.iter().any(|spawn| spawn.id == id),
                "loc {id} must be in the Lumbridge loc list"
            );
        }
        let door = store.get(12349).expect("Large door in loc.config");
        println!(
            "spot-check loc 12349 {:?} width={} length={} models={} ({} total locs)",
            door.name,
            door.width,
            door.length,
            door.models.len(),
            store.len()
        );
        assert_eq!(door.name, "Large door");
        assert!(door.width >= 1);
        assert!(door.length >= 1);
        assert!(!door.models.is_empty());

        let range = store.get(114).expect("Cooking range in loc.config");
        assert_eq!(range.name, "Cooking range");
        assert_eq!((range.width, range.length), (1, 2));
        assert!(!range.models.is_empty());
    }
}

mod texture {
    use crate::texture::*;

    /// Moire guard (real pack): every overlay diffuse texture the
    /// Lumbridge 3x3 actually references must decode as a ver-1 Single with
    /// square `{64, 128, 256, 512}` dims and matching byte length — i.e. the
    /// atlas packs clean stone/marble/wood singles, never a cubemap face or
    /// mip-chain bytes (ground-texture moire retro-samples the atlas image at
    /// the wrong scale, so this pins the atlas-content side of that split:
    /// atlas corrupt => decode/pack bug, atlas clean => UV/sampler bug).
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn lumbridge_overlay_textures_are_clean_singles() {
        let pack = crate::test_support::require_pack("client.textures.png.js5");
        let flo = crate::flo::FloStore::load(&pack).unwrap();
        let materials = MaterialStore::load(&pack).unwrap();
        let world = crate::map::load_lumbridge(&pack).unwrap();
        let mut wires: Vec<u32> = Vec::new();
        for x in 3136..3136 + 192_i32 {
            for z in 3136..3136 + 192_i32 {
                if let Some(tile) = world.tile(0, x, z) {
                    if let Some(wire) = tile.overlay_id {
                        if wire != 0 && !wires.contains(&wire) {
                            wires.push(wire);
                        }
                    }
                }
            }
        }
        assert!(!wires.is_empty(), "Lumbridge must reference overlay wires");
        let mut tex_ids: Vec<u32> = Vec::new();
        for wire in &wires {
            let Some(config) = crate::flo::config_id(*wire) else {
                continue;
            };
            let Some(material) = flo.get_overlay(config).and_then(|o| o.texture) else {
                continue;
            };
            if let Some(tex) = materials.get(material).and_then(|m| m.diffuse_texture) {
                if !tex_ids.contains(&tex) {
                    tex_ids.push(tex);
                }
            }
        }
        assert!(
            !tex_ids.is_empty(),
            "overlay wires must resolve to diffuse textures"
        );
        assert!(
            tex_ids.len() <= 64,
            "atlas caps at 64 slots, got {}",
            tex_ids.len()
        );
        for tex_id in &tex_ids {
            match load_texture(&pack, *tex_id) {
                Ok(Texture::Single(img)) => {
                    assert!(
                        img.w == img.h && [64, 128, 256, 512].contains(&img.w),
                        "texture {tex_id}: dims {}x{} outside the squared census envelope",
                        img.w,
                        img.h
                    );
                    assert_eq!(
                        img.px.len(),
                        img.w as usize * img.h as usize * 4,
                        "texture {tex_id}: byte length must match dims"
                    );
                }
                Ok(Texture::Cube(_)) => {
                    panic!(
                        "texture {tex_id}: overlay diffuse decoded as a cubemap (skybox id reused?)"
                    );
                }
                Err(err) => {
                    panic!("texture {tex_id}: undecodable ({err:#})");
                }
            }
        }
        println!("lumbridge overlay diffuse textures: {tex_ids:?}");
    }
}

mod billboard {
    use crate::billboard::*;

    /// Cache model 2288 (loc 724 "Standing torch") carries billboards: the
    /// GpuModel keeps one table row per `ModelBillboard` with its face kept.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn cache_torch_model_keeps_billboard_tables() -> anyhow::Result<()> {
        let pack = crate::test_support::require_pack("client.models.js5");
        let unlit = crate::modelunlit::ModelUnlit::load(&pack, 2288)?;
        let raw = unlit.billboard.clone().unwrap_or_default();
        assert!(!raw.is_empty());
        let materials = crate::texture::MaterialStore::load(&pack)?;
        let store = BillboardStore::load(&pack)?;
        let emitters = crate::particle::EmitterStore::load(&pack)?;
        let model = crate::gpumodel::GpuModel::new(
            &crate::gpumodel::ModelStores {
                materials: &materials,
                billboards: &store,
                emitters: &emitters,
            },
            &unlit,
            crate::gpumodel::BuildParams {
                flags: 0x400,
                ambient: 64,
                contrast: 768,
                detail: 0x37,
            },
        )?;
        let b = model.billboards.as_ref().expect("billboards");
        assert_eq!(b.faces.len(), raw.len());
        for (face, mb) in b.faces.iter().zip(&raw) {
            assert_eq!(face.source_face, mb.face);
            assert_eq!(face.depth_offset, mb.depth_offset);
            assert_eq!(face.material, store.get(mb.billboard_type).material);
        }
        assert!(b.groups.is_some());
        assert_eq!(model.billboard_instances().len(), raw.len());
        Ok(())
    }
}
