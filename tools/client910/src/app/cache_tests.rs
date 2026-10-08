//! The caches' redraw schedule on a replayed client with a fixed clock:
//! models nothing draws go idle, a read keeps one, and memory pressure (or
//! the client cheat) drops the idle ones and only those.
use super::console_tests::app_in_game;
use super::*;
use crate::gpumodel::{BuildParams, GpuModel, ModelStores, MODEL_DETAIL_FLAGS};
use rs910_core::cache_schedule::Schedule;

/// A real model, built the way the scene's caches build theirs.
fn a_model(app: &mut ViewerApp) -> GpuModel {
    let mut models = crate::ui_models::Models::new(app.pack.clone());
    let resources = models.resources().unwrap();
    let raw = crate::modelunlit::ModelUnlit::load(&app.pack, 197).unwrap();
    GpuModel::new(
        &ModelStores {
            materials: &resources.materials,
            billboards: &resources.billboards,
            emitters: &resources.emitters,
        },
        &raw,
        BuildParams {
            flags: 0,
            ambient: 64,
            contrast: 850,
            detail: MODEL_DETAIL_FLAGS,
        },
    )
    .unwrap()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn idle_models_go_soft_and_memory_pressure_drops_only_them() -> anyhow::Result<()> {
    let (mut app, _keep) = app_in_game()?;
    let model = a_model(&mut app);
    let tree = (1, 0, 0, 0, None, None, 0);
    app.entities.loc_models.insert(tree, model.clone());
    app.entities.effect_models.insert(7, model.clone());
    app.entities.hint_models.insert(3, model);
    // Plenty of room: the schedule only ages the caches.
    app.cache_schedule = Some(Schedule::new(1 << 20));
    let at = |millis: i64| crate::logic_clock::set_test_now(Some(millis));
    for frame in 0..5 {
        at(1_000 + frame);
        app.clean_caches();
    }
    assert_eq!(
        app.entities.loc_models.soft_len(),
        0,
        "not idle for long enough"
    );
    at(1_005);
    app.clean_caches();
    assert_eq!(app.entities.loc_models.soft_len(), 1);
    assert_eq!(app.entities.effect_models.soft_len(), 1);

    // A model drawn again is an ordinary entry again; the others stay idle.
    assert!(app.entities.loc_models.get(&tree).is_some());
    assert_eq!(app.entities.loc_models.soft_len(), 0);

    // Over the limit, at the next check: the idle models go, the one in use
    // stays.
    app.cache_schedule = Some(Schedule::with_limit(1));
    at(10_000);
    app.clean_caches();
    assert_eq!(
        app.entities.effect_models.len(),
        1,
        "the timer is only armed"
    );
    at(15_000);
    app.clean_caches();
    assert_eq!(app.entities.effect_models.len(), 0);
    assert_eq!(app.entities.hint_models.len(), 0);
    assert!(app.entities.loc_models.get(&tree).is_some());

    // The client cheat gives back what is idle at once.
    app.entities
        .effect_models
        .insert(9, app.entities.loc_models.get(&tree).unwrap().clone());
    for _ in 0..6 {
        app.entities.effect_models.clean(5);
    }
    assert_eq!(app.remove_soft_references(), 1);
    assert_eq!(app.entities.effect_models.len(), 0);
    crate::logic_clock::set_test_now(None);
    Ok(())
}
