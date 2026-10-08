//! The frame as a whole: a real scene draws finite, covered and repeatable
//! frames at every quality setting, each setting reaching the frame, and
//! a long session keeps its resources bounded.
use super::*;
use crate::settings::{AoMode, EnvReflections, LookMode, ShadowQuality, Shadows};

/// A Lumbridge frame rendered offscreen from a real snapshot has no
/// non-finite HDR value, covers most of the viewport with geometry, and
/// repeats: frame after frame of one renderer and from a second renderer,
/// with and without MSAA, within the repeat noise.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn lumbridge_frame_is_finite_covered_and_repeatable() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [320, 200];
    let offline = OfflineScene::new(&pack, (3222, 3222), (320, 200));
    let snapshot = offline.snapshot(&pack);
    // Warm-up (see the module docs).
    settled(
        &device,
        &queue,
        &mut renderer(&device, &queue, 4, ModernSettings::DEFAULT),
        &snapshot,
        size,
    );
    for samples in [4, 1] {
        let mut first = renderer(&device, &queue, samples, ModernSettings::DEFAULT);
        let frame = settled(&device, &queue, &mut first, &snapshot, size);
        assert!(first.stats.draws > 100, "{:?}", first.stats);
        let bad = frame.hdr.iter().filter(|v| !v.is_finite()).count();
        assert_eq!(bad, 0, "{samples}x: {bad} non-finite HDR values");
        let clear = offline.env.clear.map(|c| (c * 255.0).round() as i32);
        let covered = frame
            .pixels
            .chunks_exact(4)
            .filter(|p| (0..3).any(|c| (i32::from(p[c]) - clear[c]).abs() > 2))
            .count();
        let total = (size[0] * size[1]) as usize;
        assert!(
            covered * 2 > total,
            "{samples}x: only {covered} of {total} pixels differ from the clear colour"
        );
        let again = render(&device, &queue, &mut first, &snapshot, size);
        let mut second = renderer(&device, &queue, samples, ModernSettings::DEFAULT);
        let other = settled(&device, &queue, &mut second, &snapshot, size);
        let (n_again, n_other) = (
            Noise::of(&frame.pixels, &again.pixels),
            Noise::of(&frame.pixels, &other.pixels),
        );
        eprintln!("{samples}x: frame to frame {n_again:?}, renderer to renderer {n_other:?}");
        assert!(n_again.is_repeat_noise(size), "{samples}x: {n_again:?}");
        assert!(n_other.is_repeat_noise(size), "{samples}x: {n_other:?}");
    }
}

/// Every quality setting reaches the frame and keeps it finite and
/// repeatable: over an aerial Lumbridge view (far terrain, water, the
/// castle's interiors in the distance), each setting's frame differs from
/// the default frame, has no non-finite value and repeats frame to frame.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn every_setting_reaches_a_finite_repeatable_frame() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let offline = OfflineScene::with_camera(&pack, (3222, 3218), (480, 300), |c| {
        c.pitch = 1700.0;
        c.yaw = 12_000.0;
        c.distance_scale = 3.0;
    });
    let sky = offline.sky(&pack);
    let mut snapshot = offline.snapshot(&pack);
    snapshot.sky = sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
    let d = ModernSettings::DEFAULT;
    let variants: [(&str, ModernSettings); 13] = [
        (
            "shadows off",
            ModernSettings {
                shadows: Shadows::Fixed(None),
                ..d
            },
        ),
        (
            "shadows ultra+",
            ModernSettings {
                shadows: Shadows::Fixed(Some(ShadowQuality::UltraPlus)),
                ..d
            },
        ),
        (
            "ao off",
            ModernSettings {
                ao: AoMode::Off,
                ..d
            },
        ),
        (
            "ao ssao",
            ModernSettings {
                ao: AoMode::Ssao,
                ..d
            },
        ),
        (
            "ao hbao ultra",
            ModernSettings {
                ao: AoMode::HbaoUltra,
                ..d
            },
        ),
        (
            "env reflections all",
            ModernSettings {
                env_reflections: EnvReflections::All,
                ..d
            },
        ),
        ("far off", ModernSettings { far: None, ..d }),
        (
            "far 4",
            ModernSettings {
                far: rs910_far_scene::far_level::FarLevel::new(4),
                ..d
            },
        ),
        (
            "volumetrics off",
            ModernSettings {
                volumetrics: false,
                ..d
            },
        ),
        (
            "fxaa on",
            ModernSettings {
                fxaa: Some(true),
                ..d
            },
        ),
        ("dof on", ModernSettings { dof: true, ..d }),
        (
            "classic-calibrated look",
            ModernSettings {
                look: LookMode::ClassicCalibrated,
                ..d
            },
        ),
        (
            "render scale 150%",
            ModernSettings {
                render_scale: Some(crate::settings::RenderScale::percent(150).unwrap()),
                ..d
            },
        ),
    ];
    let mut base = renderer(&device, &queue, 4, d);
    let reference = settled(&device, &queue, &mut base, &snapshot, size);
    for (name, settings) in variants {
        let mut r = renderer(&device, &queue, 4, settings);
        let frame = settled(&device, &queue, &mut r, &snapshot, size);
        let again = render(&device, &queue, &mut r, &snapshot, size);
        let bad = frame.hdr.iter().filter(|v| !v.is_finite()).count();
        let (change, repeat) = (
            Noise::of(&reference.pixels, &frame.pixels),
            Noise::of(&frame.pixels, &again.pixels),
        );
        eprintln!("{name}: against the default {change:?}; frame to frame {repeat:?}");
        assert_eq!(bad, 0, "{name}: {bad} non-finite HDR values");
        assert!(repeat.is_repeat_noise(size), "{name}: {repeat:?}");
        assert!(
            !change.is_repeat_noise(size),
            "{name}: the setting does not reach the frame ({change:?})"
        );
    }
    // Bloom glows only what the HDR target holds over its threshold, which
    // the verified look's lights do not reach in this view; the earlier look's
    // do.
    let classic = ModernSettings {
        look: LookMode::ClassicCalibrated,
        ..d
    };
    let mut plain = renderer(&device, &queue, 4, classic);
    let plain = settled(&device, &queue, &mut plain, &snapshot, size);
    let mut glow = renderer(
        &device,
        &queue,
        4,
        ModernSettings {
            bloom: Some(true),
            ..classic
        },
    );
    let glow = settled(&device, &queue, &mut glow, &snapshot, size);
    let change = Noise::of(&plain.pixels, &glow.pixels);
    eprintln!("bloom on (classic-calibrated look): against its default {change:?}");
    assert!(
        !change.is_repeat_noise(size),
        "bloom does not reach the frame ({change:?})"
    );
}

/// Long-session resource lifetime: Lumbridge with its animated locs posed
/// every frame for many frames: after the first frames no loc mesh buffer
/// is created and the cache does not grow (one mesh per scene slot, a pose
/// rewritten in place); a temporary joining the scene keeps every cached
/// mesh and starts no probe capture.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn animated_locs_and_temporaries_keep_the_loc_cache_bounded() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [256, 160];
    let mut offline = OfflineScene::new(&pack, (3222, 3222), (256, 160));
    let mut r = renderer(&device, &queue, 1, ModernSettings::DEFAULT);
    let mut cycle = 0;
    offline.animate(cycle);
    // The verified look captures its ambient one face a frame over a few
    // hundred frames of the clock; the cache is bounded once it has settled.
    let t0 = 1_700_000_000_000_i64;
    for k in 0..900_i64 {
        let now = t0 + 40 * k;
        crate::logic_clock::set_test_now(Some(now));
        render(&device, &queue, &mut r, &offline.snapshot(&pack), size);
        if k > 10 && r.ambient_settled(now) && !r.probe_capture_pending() {
            break;
        }
    }
    let mut frame = |offline: &mut OfflineScene, r: &mut ModernRenderer| {
        cycle += 5;
        let posed = offline.animate(cycle);
        render(&device, &queue, r, &offline.snapshot(&pack), size);
        posed
    };
    // A pose may outgrow its first buffers.
    for _ in 0..10 {
        frame(&mut offline, &mut r);
    }
    let (meshes, buffers) = r.loc_mesh_cache();
    let posed: usize = (0..60).map(|_| frame(&mut offline, &mut r)).sum();
    let (meshes_after, buffers_after) = r.loc_mesh_cache();
    eprintln!(
        "{posed} poses over 60 frames: {meshes} -> {meshes_after} loc meshes, {buffers} -> {buffers_after} buffers created"
    );
    assert!(posed >= 60, "the view's locs animate: {posed} poses");
    assert_eq!(
        buffers_after, buffers,
        "a posed loc's mesh is rewritten in place"
    );
    assert_eq!(meshes_after, meshes, "one mesh per slot");
    assert!(meshes_after <= offline.static_slots());
    assert!(!r.probe_capture_pending());
    offline.add_temporary();
    render(&device, &queue, &mut r, &offline.snapshot(&pack), size);
    assert!(
        !r.probe_capture_pending(),
        "a temporary is not a scene change for the probes"
    );
    assert_eq!(
        r.loc_mesh_cache(),
        (meshes, buffers),
        "a temporary keeps the cached loc meshes"
    );
}

/// The submission core (`frame::submit`, `frame::arenas`): the depth-only
/// passes' sorted lists give the frame of drawing them in the draw order,
/// and they are sorted so each material's draws on a loc page form one run
/// (one bind-group and one buffer bind per run), with the locs' meshes on
/// a few shared pages. Lumbridge (ultra shadows: four cascades) and the
/// river (the water's reflection pass), at 4x.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn sorted_depth_passes_on_shared_loc_pages_leave_the_frame_unchanged() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let views = [
        (
            "lumbridge",
            OfflineScene::new(&pack, (3222, 3222), (480, 300)),
        ),
        ("river", river_scene(&pack, size, 1400.0, 1.2)),
    ];
    // Runs of equal (material, buffer set) in a list.
    let runs = |list: &[crate::frame::Draw]| {
        let key = |d: &crate::frame::Draw| (d.material, d.geometry.buffers());
        let runs = list.windows(2).filter(|w| key(&w[0]) != key(&w[1])).count()
            + usize::from(!list.is_empty());
        let mut distinct: Vec<_> = list.iter().map(key).collect();
        distinct.sort_unstable();
        distinct.dedup();
        (runs, distinct.len())
    };
    for (name, offline) in &views {
        let snapshot = offline.snapshot(&pack);
        let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
        r.set_shadow_settings(crate::shadows::Settings::from_options(2, 3, 1));
        // Every cascade redraws its casters each frame (no shadow cache).
        r.history.shadow.sun.test_uncached = true;
        let sorted = settled(&device, &queue, &mut r, &snapshot, size).pixels;
        let lists: Vec<Vec<crate::frame::Draw>> =
            std::iter::once(r.frame_resources.packets.prepass.clone())
                .chain(r.history.shadow.sun.static_lists.iter().cloned())
                .collect();
        r.preparation.test_unsorted_packets = true;
        let ordered = render(&device, &queue, &mut r, &snapshot, size).pixels;
        let differ = Noise::of(&sorted, &ordered);
        let (pages, meshes) = (
            r.scene_resources.loc_arena.pages.len(),
            r.loc_mesh_cache().0,
        );
        eprintln!("{name}: {differ:?}; {meshes} loc meshes on {pages} pages");
        assert_eq!(differ, Noise::default(), "{name}");
        assert!(
            lists.len() == 5 && !lists[0].is_empty(),
            "{name}: {}",
            lists.len()
        );
        for (k, list) in lists.iter().enumerate() {
            let (runs, distinct) = runs(list);
            assert_eq!(
                runs, distinct,
                "{name} list {k}: one run per material and buffer set"
            );
        }
        assert!(
            pages * 20 < meshes,
            "{name}: {meshes} loc meshes on {pages} pages"
        );
    }
}

/// The parallel frame (`frame::jobs`, `frame::units`, `frame::posing`):
/// the encode units recorded on several threads and submitted in frame
/// order, the posed models posed on them, give exactly the frame of one
/// thread. The river (the water's reflection and the forward pass split
/// around the water surface), ultra shadows (four cascades), point-light
/// shadows, and temporaries whose models are posed each frame, at 4x.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn threaded_and_synchronous_encodes_draw_the_identical_frame() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let mut offline = river_scene(&pack, size, 1400.0, 1.2);
    let posed = offline.add_posed_models(6);
    let snapshot = offline.snapshot(&pack);
    let frame = |threads: usize| {
        let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
        r.set_shadow_settings(crate::shadows::Settings::from_options(2, 3, 1));
        r.set_threads(threads);
        let frame = settled(&device, &queue, &mut r, &snapshot, size);
        (frame.pixels, r)
    };
    // Warm-up (see the module docs).
    frame(1);
    let (synchronous, one) = frame(1);
    let (threaded, four) = frame(4);
    let differ = Noise::of(&synchronous, &threaded);
    eprintln!(
        "{posed} posed models ({} from RT7); {} and {} threads: {differ:?}",
        four.rt7.anim.stats.drawn,
        one.threads(),
        four.threads()
    );
    assert_eq!((one.threads(), four.threads()), (1, 4));
    assert!(
        posed > 0 && four.rt7.anim.stats.drawn > 0,
        "the view poses RT7 models"
    );
    assert!(
        !four.frame_resources.water.draws.is_empty(),
        "the view has water"
    );
    assert_eq!(differ, Noise::default());
}
