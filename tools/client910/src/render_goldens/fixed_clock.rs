//! Phase 0 deterministic-render gate (programme §8, `logic_clock` module
//! docs), on renderer-neutral state: the offline scene with its dynamic locs
//! animated on `LiveScene::animation_cycle` and their clock seed, driven by
//! the injected clock, digested frame by frame over what every backend draws
//! from (the `SceneSnapshot`'s draw lists and each drawn dynamic loc's entity,
//! model revision and CPU-posed model). Until lane DROP-SW this gate compared
//! the software toolkit's colour buffer, which also scrolled animated
//! textures on the toolkit clock; the GPU renderers sample the
//! same clock for material animation per frame
//! (`floor_render::begin_material_frame`), a pixel-only uniform.
use super::debug_digest;

/// The offline scene around `tile` on the injected clock: frames 0..3 run
/// 20 ms apart and the last one `elapsed_ms` after the start. Returns each
/// frame's digest and how many dynamic-loc draws were refreshed.
fn fixed_clock_scene_digests(
    pack: &crate::cache::Pack,
    tile: (i32, i32),
    elapsed_ms: i64,
    raster_hard_shadows: bool,
) -> (Vec<u64>, usize) {
    /// Restores the thread's real clock even if a frame panics.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::logic_clock::set_test_now(None);
        }
    }
    let _reset = Reset;
    const START: i64 = 1_700_000_000_000;
    crate::logic_clock::set_test_now(Some(START));
    let materials = crate::texture::MaterialStore::load(pack).unwrap();
    let locs = crate::config::LocStore::load(pack).unwrap();
    let flo = crate::flo::FloStore::load(pack).unwrap();
    let tables = crate::maploader::FloTables::from_store(&flo);
    let (tx, tz) = tile;
    let mut world = crate::rebuild::rebuild_normal(
        pack,
        &tables,
        &materials,
        Some(&locs),
        tx,
        tz,
        &Default::default(),
    )
    .unwrap();
    let base = (world.base_x, world.base_z);
    let scene = world.scene_graph.as_mut().unwrap();
    let mut live = crate::live_scene::LiveScene::new(
        scene,
        &world.scene.normal,
        world.flags.clone(),
        &world.env.lights,
    )
    .unwrap();
    live.enable_dynamic(pack, &locs, world.model_cache.clone())
        .unwrap();
    live.dynamic
        .as_mut()
        .unwrap()
        .set_hard_shadow_raster(raster_hard_shadows);
    let (w, h) = (256, 168);
    let local = [(tx - base.0) * 512 + 256, (tz - base.1) * 512 + 256];
    let ground = world.scene.normal[0]
        .as_ref()
        .unwrap()
        .heights
        .get_fine_height(local[0], local[1]);
    let mut camera =
        crate::camera::SceneCamera::new([tx * 512 + 256, ground - 200, tz * 512 + 256]);
    camera.viewport = (w, h);
    let env = world.env.target_environment(tx - base.0, tz - base.1);
    let (far, near_min) = camera.fog_reference();
    let frame = crate::env::EnvFrame::build(
        &env,
        crate::env::SunSettings {
            direction: world.env.sun_direction,
            brightness_pref: 3,
            anti_macro: 0.0,
        },
        true,
        crate::env::FogReference {
            far,
            near_min,
            view: &camera.view_entries(),
        },
    );
    let mut animated = 0;
    let mut digests = Vec::new();
    for now in [START, START + 20, START + 40, START + elapsed_ms] {
        crate::logic_clock::set_test_now(Some(now));
        live.update(scene, camera.clone(), base, 0, [-1, -1]);
        // ViewerApp::refresh_dynamic_scene's offline branch: the culled
        // updates, then the dispatched opaque/transparent entities.
        let cycle = live.animation_cycle();
        let phases = [
            live.draw.plan.culled_updates.clone(),
            live.draw.plan.dispatch_opaque.clone(),
            live.draw.plan.dispatch_transparent.clone(),
        ];
        let dynamic = live.dynamic.as_mut().unwrap();
        for (phase, ids) in phases.iter().enumerate() {
            for &id in ids {
                if !dynamic.contains(id) {
                    continue;
                }
                animated += usize::from(phase != 0);
                dynamic
                    .refresh(
                        id,
                        phase != 0,
                        cycle,
                        &mut live.entities[id],
                        crate::dynamic_scene::RefreshWorld {
                            scene,
                            floors: &mut world.scene.normal,
                            materials: &materials,
                            sun: &frame.sun,
                        },
                    )
                    .unwrap();
            }
        }
        // What the frame's `SceneSnapshot` hands a backend: the draw lists
        // and, per drawn dynamic loc, its entity, model revision
        // (`SceneSnapshot::entity_key`) and posed model.
        let plan = &live.draw.plan;
        let dynamic = live.dynamic.as_ref().unwrap();
        let drawn: Vec<_> = plan
            .opaque
            .iter()
            .chain(&plan.transparent)
            .filter(|&&id| dynamic.contains(id))
            .map(|&id| {
                let entity = &live.entities[id];
                (
                    id,
                    dynamic.revision(id),
                    entity,
                    crate::dynamic_scene::model(scene, entity.source),
                )
            })
            .collect();
        digests.push(debug_digest(&(
            cycle,
            &plan.opaque,
            &plan.transparent,
            &plan.floors,
            drawn,
        )));
    }
    (digests, animated)
}

/// Programme §8 gate: with the clock fixed, the same offline scene gives the
/// same frame state twice, and a different time a different one (the dynamic
/// loc animation really samples the injected clock).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn fixed_clock_offline_scene_is_deterministic_and_clock_driven() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    // The Lumbridge spawn and, east of the castle, animated scenery (dynamic
    // loc animation).
    for tile in [(3222, 3222), (3240, 3218)] {
        let (t1, t2) = (1_000, 2_600);
        let (first, animated) = fixed_clock_scene_digests(&pack, tile, t1, true);
        assert!(animated > 0, "{tile:?}: no dynamic loc in view");
        let (second, _) = fixed_clock_scene_digests(&pack, tile, t1, true);
        let (later, _) = fixed_clock_scene_digests(&pack, tile, t2, true);
        let (modern, modern_animated) = fixed_clock_scene_digests(&pack, tile, t1, false);
        assert_eq!(
            first, modern,
            "modern shadow preparation changed shared model or picking state"
        );
        assert_eq!(animated, modern_animated);
        assert_eq!(
            first, second,
            "{tile:?}: the same fixed time gave different frame state"
        );
        assert_eq!(
            first[..3],
            later[..3],
            "{tile:?}: the shared first frames differ"
        );
        assert_ne!(
            first[3], later[3],
            "{tile:?}: frames at {t1} ms and {t2} ms are identical: the scene animation does not follow the clock"
        );
    }
}
