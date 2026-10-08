//! Models and terrain against the faithful scene (the pack, CPU): the draw
//! list is the faithful backend's, RT7 models are their classic models, and
//! the terrain covers the classic floor at its heights.
use super::*;
use crate::models::draw_list::{DrawList, Kind};

/// Over a real scene,
/// the draw list is the faithful backend's: every opaque and
/// transparent plan entity that has a model, once, in the plan's order, as
/// the faithful mesh lists hold them (`SceneMeshes::frame`/`mesh_for`: a
/// loc's one mesh, uploaded for every scene-graph entity with a model), and
/// every level's floor with the batches and index counts the faithful floor
/// mesh selects for the plan's tiles (`FloorMesh::select_tiles`). The
/// shell's `CLIENT910_MODERN_CHECK` compares the same summary with the
/// faithful backend's uploaded meshes at run time.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn draw_list_covers_the_faithful_lists() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    for tile in [(3222, 3222), (3240, 3218)] {
        let offline = OfflineScene::new(&pack, tile, (256, 168));
        let snapshot = offline.snapshot(&pack);
        let summary = DrawList::build(&snapshot).summary();
        let scene = offline.world.scene_graph.as_ref().unwrap();
        let plan = &offline.live.draw.plan;
        let faithful = |ids: &[usize]| -> Vec<(usize, Kind)> {
            ids.iter()
                .filter(|&&id| {
                    let source = offline.live.entities[id].source;
                    crate::dynamic_scene::model(scene, source).is_some()
                })
                .map(|&id| (id, Kind::Model))
                .collect()
        };
        assert!(
            summary.opaque.len() > 100,
            "{tile:?}: {}",
            summary.opaque.len()
        );
        assert_eq!(summary.opaque, faithful(&plan.opaque), "{tile:?} opaque");
        assert_eq!(
            summary.transparent,
            faithful(&plan.transparent),
            "{tile:?} transparent"
        );
        let mut floors = Vec::new();
        for (level, selection) in plan.floors.iter().enumerate() {
            let Some(Some(g)) = offline.world.scene.normal.get(level) else {
                continue;
            };
            let tiles = selection.tiles(g.tiles_x, g.tiles_z);
            for batch in &g.batches {
                let n = batch.build_indices(g, &tiles).0.len();
                if n > 0 {
                    floors.push((level, batch.material, n));
                }
            }
        }
        assert!(!floors.is_empty());
        assert_eq!(summary.floors, floors, "{tile:?} floors");
        // Every draw's model has streams the mesh path accepts, apart
        // from empty models.
        let list = DrawList::build(&snapshot);
        let drawable = list
            .opaque
            .iter()
            .chain(&list.transparent)
            .filter(|d| {
                crate::models::mesh::model_streams(
                    d.model,
                    &offline.materials,
                    crate::models::mesh::Colour::Albedo,
                )
                .is_some()
            })
            .count();
        assert!(
            drawable * 10 >= (list.opaque.len() + list.transparent.len()) * 9,
            "{tile:?}: {drawable} of {} models have streams",
            list.opaque.len() + list.transparent.len()
        );
    }
}

/// A ground shadow the scene carries for an NPC is drawn by the modern
/// renderer as a spot shadow (no depth writes, not a shadow caster) and the
/// body beside it as an ordinary model.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn an_npc_spot_shadow_transient_draws_without_depth_writes() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let mut offline = OfflineScene::new(&pack, (3222, 3222), (256, 168));
    assert_eq!(offline.add_posed_models(2), 2);
    let scene = offline.world.scene_graph.as_mut().unwrap();
    let shadow_index = scene.temporary.len() - 1;
    scene.temporary[shadow_index].spot_shadow = true;
    let snapshot = offline.snapshot(&pack);
    let list = DrawList::build(&snapshot);
    let temporaries = &offline.live.entities[offline.static_slots()..];
    let ids: Vec<usize> =
        (offline.static_slots()..offline.static_slots() + temporaries.len()).collect();
    let draws: Vec<_> = list
        .opaque
        .iter()
        .chain(&list.transparent)
        .filter(|d| ids.contains(&d.id))
        .collect();
    let shadow = draws
        .iter()
        .find(|d| d.id == *ids.last().unwrap())
        .expect("the shadow is drawn");
    assert_eq!((shadow.kind, shadow.depth_write), (Kind::SpotShadow, false));
    assert!(draws
        .iter()
        .filter(|d| d.id != shadow.id)
        .all(|d| d.kind == Kind::Model && d.depth_write));
}

/// `nxt-data-formats.md` §9: an RT7 model is its classic model re-exported:
/// over a sample of model ids every RT7 vertex is a classic vertex (y negated,
/// times 4 below version 13), LOD 0 holds one triangle per non-degenerate
/// classic face, and the face colours are the classic ones.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn rt7_models_are_the_classic_models() {
    use rs910_config::nxt::model_rt7::{decode_model_rt7, MODEL_RT7_ARCHIVE};
    let pack = crate::test_support::require_pack("client.modelsrt7.js5");
    let ids: Vec<u32> = pack
        .read_archive_index(MODEL_RT7_ARCHIVE)
        .unwrap()
        .group_id
        .into_iter()
        .step_by(199)
        .collect();
    let (mut models, mut vertices, mut faces) = (0, 0, 0);
    for id in ids {
        let Ok(mut classic) = crate::modelunlit::ModelUnlit::load(&pack, id) else {
            continue;
        };
        if classic.version < 13 {
            classic.scale_by_power_of_two(2);
        }
        let files = pack.read_group(MODEL_RT7_ARCHIVE, id).unwrap();
        let rt7 = decode_model_rt7(id, &files[&0]).unwrap();
        let pos = |i: usize| {
            [
                classic.vertex_x[i],
                classic.vertex_y[i],
                classic.vertex_z[i],
            ]
        };
        let set: std::collections::HashSet<[i32; 3]> =
            (0..classic.vertex_count as usize).map(pos).collect();
        let mut classic_colours: Vec<(u16, [[i32; 3]; 3])> = Vec::new();
        for f in 0..classic.face_count as usize {
            let i = [
                classic.face_vertex1[f],
                classic.face_vertex2[f],
                classic.face_vertex3[f],
            ];
            let v = i.map(|i| pos(i as usize));
            // RT7 drops only the faces whose corners are one point.
            let _ = i;
            if !(v[0] == v[1] && v[1] == v[2]) {
                let mut k = v;
                k.sort_unstable();
                classic_colours.push((classic.face_colour[f] as u16, k));
            }
        }
        let mut rt7_colours = Vec::new();
        for mesh in &rt7.meshes {
            for (f, tri) in mesh.lods[0].chunks_exact(3).enumerate() {
                let v = [tri[0], tri[1], tri[2]].map(|i| {
                    let p = mesh.positions[usize::from(i)];
                    [i32::from(p[0]), -i32::from(p[1]), i32::from(p[2])]
                });
                for p in v {
                    assert!(set.contains(&p), "model {id}: RT7 vertex {p:?}");
                    vertices += 1;
                }
                let mut k = v;
                k.sort_unstable();
                rt7_colours.push((mesh.face_colours[f], k));
            }
        }
        classic_colours.sort_unstable();
        rt7_colours.sort_unstable();
        if rt7_colours != classic_colours {
            panic!(
                "model {id}: {} RT7 faces, {} classic faces; first RT7-only {:?}, first classic-only {:?}",
                rt7_colours.len(),
                classic_colours.len(),
                rt7_colours.iter().find(|f| !classic_colours.contains(f)),
                classic_colours.iter().find(|f| !rt7_colours.contains(f))
            );
        }
        models += 1;
        faces += rt7_colours.len();
    }
    eprintln!("{models} models, {faces} faces, {vertices} corners");
    assert!(models > 500);
}

/// Every loc of the scene around the Lumbridge Swamp mine builds its RT7
/// streams, and the ones that are built draw the faces their classic models
/// draw. The scene holds a loc model past 32,767 unique vertices, whose
/// classic face indices are stored as negative shorts and read unsigned.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn rt7_streams_build_for_every_loc_around_the_swamp_mine() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let offline = OfflineScene::new(&pack, (3229, 3150), (256, 168));
    let snapshot = offline.snapshot(&pack);
    let locs = DrawList::scene_locs(&snapshot);
    let mut cache = crate::models::rt7::Rt7Cache::default();
    let (mut built, mut wide) = (0, 0);
    for entity in &locs {
        let wide_model = entity.model.unique_count > i32::from(i16::MAX);
        if cache
            .streams(&snapshot, &offline.materials, entity)
            .is_some()
        {
            built += 1;
            wide += usize::from(wide_model);
        }
    }
    eprintln!(
        "{} locs, {built} from RT7 ({wide} wide): {:?}",
        locs.len(),
        cache.stats
    );
    assert!(
        wide > 0,
        "no built loc with more than 32,767 unique vertices"
    );
    assert!(built > 50, "{built} of {} locs from RT7", locs.len());
    let (checked, mismatched) = cache.check(&locs);
    assert_eq!((checked > 0, mismatched), (true, 0));
}

/// The near RT7 build over a grid of scenes across the mainland (every loc
/// of each, as the renderer builds them) does not panic and draws RT7 for
/// most locs. A sweep: minutes, so on demand
/// (`cargo test -p rs910-render-modern rt7_streams_build_across_the_map -- --ignored`).
#[test]
#[ignore = "sweep over many scenes; needs server/data/pack"]
fn rt7_streams_build_across_the_map() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (mut scenes, mut locs, mut drawn) = (0, 0, 0);
    for tx in (2432..=3712).step_by(160) {
        for tz in (2432..=3712).step_by(160) {
            // Squares the pack does not hold have no scene to build; the
            // scene is made inside the guard, the RT7 builds outside it.
            let Ok(offline) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                OfflineScene::new(&pack, (tx, tz), (256, 168))
            })) else {
                continue;
            };
            let snapshot = offline.snapshot(&pack);
            let draws = DrawList::scene_locs(&snapshot);
            // A cache per scene: entity keys are scene slots.
            let mut cache = crate::models::rt7::Rt7Cache::default();
            for entity in &draws {
                let _ = cache.streams(&snapshot, &offline.materials, entity);
            }
            scenes += 1;
            locs += draws.len();
            drawn += cache.stats.drawn;
        }
    }
    eprintln!("{scenes} scenes, {locs} locs, {drawn} from RT7");
    assert!(scenes > 20 && drawn > 10_000);
}

/// Over the Lumbridge scene the terrain is usable,
/// covers every classic ground tile of every level's full selection, and its
/// grid vertices sit at the classic floor's heights (land; a water tile's
/// vertex is its bed, below the classic surface).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn terrain_covers_the_classic_floor_at_classic_heights() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let offline = OfflineScene::new(&pack, (3222, 3222), (320, 200));
    let snapshot = offline.snapshot(&pack);
    let scene = crate::terrain::TerrainScene::build(&pack, &offline.materials, &snapshot);
    assert!(scene.usable, "{}", scene.summary());
    let (mut land, mut equal, mut water, mut below) = (0, 0, 0, 0);
    for (level, floor) in snapshot.floors.iter().enumerate() {
        let (Some(floor), Some(Some(mesh))) = (floor, scene.levels.get(level)) else {
            continue;
        };
        let nz = floor.tiles_z + 1;
        for x in 0..=floor.tiles_x {
            for z in 0..=floor.tiles_z {
                // The classic grid ends at the scene edge (0 past it).
                if x == floor.tiles_x || z == floor.tiles_z {
                    continue;
                }
                let (y, is_water) = mesh.grid[x * nz + z];
                let classic = floor.heights.get_tile_height(x, z) as f32;
                if is_water {
                    water += 1;
                    below += usize::from(y >= classic);
                } else {
                    land += 1;
                    equal += usize::from(y == classic);
                    if y != classic && land % 37 == 0 {
                        eprintln!("l{level} ({x},{z}): terrain {y} classic {classic}");
                    }
                }
            }
        }
    }
    eprintln!("{land} land vertices, {equal} at the classic height; {water} water vertices, {below} at or below the classic surface");
    assert!(land > 40_000 && equal == land, "{equal} of {land}");
    assert!(water > 0 && below == water, "{below} of {water}");
    // Every classic ground tile of every level has terrain (the check over the
    // whole scene).
    let list = DrawList::build(&snapshot);
    let mut all = Vec::new();
    for f in &list.floors {
        let n = 2 * 104 + 2;
        all.push(rs910_scene::draw::FloorSelection {
            whole: true,
            origin: [0, 0],
            distance: 104,
            mask: vec![vec![true; n]; n],
        });
        let _ = f;
    }
    let full = DrawList {
        opaque: Vec::new(),
        transparent: Vec::new(),
        floors: list
            .floors
            .iter()
            .zip(&all)
            .map(|(f, s)| crate::models::draw_list::FloorDraw {
                level: f.level,
                geometry: f.geometry,
                selection: s,
            })
            .collect(),
    };
    let (ok, report) = scene.check(&full, Some(&offline.materials));
    eprintln!("{report}");
    assert!(ok, "{report}");
}

/// An owned render frame outlives pose/loc/tile changes and destruction of the
/// game scene, while the next slot observes those changes. No GPU is needed.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn owned_scene_inputs_survive_source_mutation_and_destruction() {
    const SIZE: (i32, i32) = (320, 200);
    const POSED_MODELS: usize = 2;
    const MODEL_DELTA: i32 = 37;
    const TILE_DELTA: i8 = 1;
    const ANIMATION_CYCLE: i32 = 50;
    const CAPTURED_TIME: i64 = 1_700_000_000_000;
    const LIGHT_INTENSITY_DELTA: f32 = 0.25;
    const FIRST_LIGHT: usize = 0;
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let location = rs910_symbols::location::DEV_PLAYER_SPAWN;
    let mut offline = OfflineScene::new(&pack, (location.x(), location.z()), SIZE);
    offline.add_posed_models(POSED_MODELS);
    offline.animate(ANIMATION_CYCLE);
    let mut builder = rs910_scene::scene_snapshot::owned::SnapshotBuilder::default();
    let mut borrowed = offline.snapshot(&pack);
    borrowed.time_ms = Some(CAPTURED_TIME);
    let before = builder.capture(&borrowed, None);
    let expected = DrawList::build(&borrowed).summary();
    let original_lights = borrowed.live_frame().unwrap().model_lights;
    assert!(!original_lights.intensities.is_empty());
    let light_values = original_lights.intensities.clone();
    let light_identity = original_lights.grid_identity().clone();
    let first = borrowed
        .live_frame()
        .unwrap()
        .entities
        .iter()
        .position(|entity| {
            borrowed
                .model(entity.source)
                .is_some_and(|model| !model.vx.is_empty())
        })
        .unwrap();
    let source = borrowed.live_frame().unwrap().entities[first].source;
    let positions = borrowed.model(source).unwrap().vx.clone();
    let scene = borrowed.scene.unwrap();
    let tile = (0..scene.max_level)
        .flat_map(|level| {
            (0..scene.max_x).flat_map(move |x| (0..scene.max_z).map(move |z| (level, x, z)))
        })
        .find(|&(level, x, z)| scene.tile(level, x, z).is_some())
        .unwrap();
    let old_tile = scene.tile(tile.0, tile.1, tile.2).unwrap().clone();
    drop(borrowed);
    let scene = offline.world.scene_graph.as_mut().unwrap();
    let model = match source {
        crate::scene::EntityRef::Scenery(index) => scene.scenery[index].model.as_mut().unwrap(),
        crate::scene::EntityRef::Wall(index) => scene.walls[index].model.as_mut().unwrap(),
        crate::scene::EntityRef::WallDecor(index) => {
            scene.wall_decors[index].model.as_mut().unwrap()
        }
        crate::scene::EntityRef::GroundDecor(index) => {
            scene.ground_decors[index].model.as_mut().unwrap()
        }
        crate::scene::EntityRef::Temporary(index) => scene.temporary[index].model.as_mut().unwrap(),
    };
    model.vx[0] += MODEL_DELTA;
    offline.live.mark_slot_changed(first);
    let changed_tile = scene.get_tile(tile.0, tile.1, tile.2).unwrap();
    changed_tile.level += TILE_DELTA;
    changed_tile.bridge = Some(Box::new(old_tile.clone()));
    scene.temporary.clear();
    offline.live.install_temporary(scene);
    offline.live.model_lights.intensities[FIRST_LIGHT] += LIGHT_INTENSITY_DELTA;
    let after = builder.capture(&offline.snapshot(&pack), None);
    let next_snapshot = after.snapshot();
    let next_lights = next_snapshot.live_frame().unwrap().model_lights;
    assert!(std::sync::Arc::ptr_eq(
        next_lights.grid_identity(),
        &light_identity
    ));
    assert_eq!(
        next_lights.intensities[FIRST_LIGHT],
        light_values[FIRST_LIGHT] + LIGHT_INTENSITY_DELTA
    );
    assert_eq!(
        before
            .snapshot()
            .live_frame()
            .unwrap()
            .model_lights
            .intensities,
        light_values
    );
    let scene = offline.world.scene_graph.as_ref().unwrap();
    offline.live.model_lights = rs910_scene::model_lights::ModelLights::new(
        scene,
        &offline.live.model_lights.lights,
        offline.live.entities.len(),
    );
    let replaced = builder.capture(&offline.snapshot(&pack), None);
    assert!(!std::sync::Arc::ptr_eq(
        replaced
            .snapshot()
            .live_frame()
            .unwrap()
            .model_lights
            .grid_identity(),
        &light_identity
    ));
    assert_ne!(after.snapshot().model(source).unwrap().vx, positions);
    assert_eq!(before.snapshot().model(source).unwrap().vx, positions);
    assert_eq!(
        before
            .snapshot()
            .scene
            .unwrap()
            .tile(tile.0, tile.1, tile.2)
            .unwrap()
            .level,
        old_tile.level
    );
    assert_eq!(
        after
            .snapshot()
            .scene
            .unwrap()
            .tile(tile.0, tile.1, tile.2)
            .unwrap()
            .bridge
            .as_ref()
            .unwrap()
            .level,
        old_tile.level
    );
    drop(offline);
    assert_eq!(DrawList::build(&before.snapshot()).summary(), expected);
    assert_eq!(before.snapshot().time_ms, Some(CAPTURED_TIME));
    assert_eq!(before.snapshot().model(source).unwrap().vx, positions);
}

/// Underwater render metadata and model streams are owned independently of
/// the source and observe replacement on the next capture.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn owned_underwater_inputs_keep_their_frame_version() {
    const SIZE: (i32, i32) = (320, 200);
    const MODEL_DELTA: i32 = 37;
    const HEIGHT_DELTA: f32 = 19.0;
    const FOG_COLOUR: [f32; 4] = [0.2, 0.4, 0.6, 1.0];
    const NEXT_FOG_COLOUR: [f32; 4] = [0.6, 0.2, 0.4, 1.0];
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let location = rs910_symbols::location::DEV_PLAYER_SPAWN;
    let offline = OfflineScene::new(&pack, (location.x(), location.z()), SIZE);
    let mut source = offline.snapshot(&pack);
    let mut floor = source.floors.iter().flatten().next().unwrap().clone();
    let entity = source
        .live_frame()
        .unwrap()
        .entities
        .iter()
        .find(|entity| {
            source
                .model(entity.source)
                .is_some_and(|model| !model.vx.is_empty())
        })
        .unwrap()
        .clone();
    let mut models = vec![rs910_scene::rebuild::UnderwaterModel {
        model: source.model(entity.source).unwrap().clone(),
        position: [entity.x, entity.y, entity.z],
        transparent: entity.transparent,
        fog_plane: [0.0, 1.0, 0.0, 0.0],
        fog_colour: FOG_COLOUR,
        entity,
        water_height: 0,
        water_colour: 0,
        water_scale: 1,
    }];
    let mut builder = rs910_scene::scene_snapshot::owned::SnapshotBuilder::default();
    source.underwater = Some(rs910_scene::scene_snapshot::Underwater {
        floor: &floor,
        models: &models,
    });
    let before = builder.capture(&source, None);
    let positions = models[0].model.vx.clone();
    let old_min = floor.min_y;
    drop(source);
    models[0].model.vx[0] += MODEL_DELTA;
    models[0].fog_colour = NEXT_FOG_COLOUR;
    floor.min_y += HEIGHT_DELTA;
    let mut source = offline.snapshot(&pack);
    source.underwater = Some(rs910_scene::scene_snapshot::Underwater {
        floor: &floor,
        models: &models,
    });
    let after = builder.capture(&source, None);
    drop(source);
    drop(offline);
    drop(models);
    drop(floor);
    let previous = before.snapshot();
    let next = after.snapshot();
    let previous = previous.underwater.unwrap();
    let next = next.underwater.unwrap();
    assert_eq!(previous.models[0].model.vx, positions);
    assert_ne!(next.models[0].model.vx, positions);
    assert_eq!(previous.models[0].fog_colour, FOG_COLOUR);
    assert_eq!(next.models[0].fog_colour, NEXT_FOG_COLOUR);
    assert_eq!(previous.floor.min_y, old_min);
    assert_eq!(next.floor.min_y, old_min + HEIGHT_DELTA);
}
