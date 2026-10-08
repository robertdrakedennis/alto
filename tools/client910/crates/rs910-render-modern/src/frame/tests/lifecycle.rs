//! The renderer's lifecycle (performance plan P5): creating it off the
//! render thread (`frame::startup`) and building a scene's meshes, maps and
//! materials ahead of their draws on the threads (`frame::prebuild`). Both
//! only move work: the frames must be the ones of doing it in place.

use std::sync::Arc;

use super::*;

/// The builds ahead of the draws give the frame of building each loc mesh
/// and material at its draw, with the same RT7 counters and loc pages:
/// Lumbridge with posed models and ultra shadows (off-screen casters), 4x,
/// settled (the probes' capture builds its locs ahead too).
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn builds_ahead_of_the_draws_leave_the_frame_unchanged() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let mut offline = OfflineScene::new(&pack, (3222, 3222), (480, 300));
    let posed = offline.add_posed_models(6);
    const ANIMATION_CYCLE: i32 = 50;
    assert!(
        offline.animate(ANIMATION_CYCLE) > 0,
        "animated locs are posed too"
    );
    let snapshot = offline.snapshot(&pack);
    let frame = |inline: bool| {
        let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
        r.set_shadow_settings(crate::shadows::Settings::from_options(2, 3, 1));
        r.test_inline_builds = inline;
        let frame = settled(&device, &queue, &mut r, &snapshot, size);
        (frame.pixels, r)
    };
    // Warm-up (see the module docs).
    frame(true);
    let (inline, at_draws) = frame(true);
    let (ahead, threads) = frame(false);
    let differ = Noise::of(&inline, &ahead);
    eprintln!(
        "{posed} posed models; {} loc meshes built ahead, {} at their draws; {:?}; {differ:?}",
        threads.prebuilds.taken, at_draws.prebuilds.taken, threads.rt7.stats
    );
    assert!(threads.prebuilds.taken > 1000 && at_draws.prebuilds.taken == 0);
    assert!(
        threads.rt7.anim.stats.maps > 0,
        "the posed models have RT7 maps"
    );
    assert_eq!(differ, Noise::default());
    assert_eq!(threads.rt7.stats, at_draws.rt7.stats);
    assert_eq!(threads.rt7.anim.stats, at_draws.rt7.anim.stats);
    assert_eq!(threads.loc_arena.used(), at_draws.loc_arena.used());
    assert_eq!(threads.textures().len(), at_draws.textures().len());
}

/// A renderer made on its startup thread is the one made here: its settled
/// frame is a fresh renderer's at the count it was started with, and at
/// another count (the anti-aliasing level changed before the renderer was
/// taken: its sets built ahead on the thread when the shader cache is warm,
/// else when it is taken) a fresh renderer's at that count.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn a_renderer_made_on_its_startup_thread_draws_the_frames_of_one_made_here() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    let size = [480, 300];
    let offline = OfflineScene::new(&pack, (3222, 3222), (480, 300));
    let snapshot = offline.snapshot(&pack);
    let fresh = |samples: u32| {
        let mut r = renderer(&device, &queue, samples, ModernSettings::DEFAULT);
        settled(&device, &queue, &mut r, &snapshot, size).pixels
    };
    let started = |samples: u32| {
        let startup = crate::frame::startup::Startup::spawn(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            4,
            &[1],
            ModernSettings::DEFAULT,
        );
        let mut r = startup.finish(&device, &queue, samples);
        r.far.sync = true;
        assert_eq!(r.samples(), samples);
        settled(&device, &queue, &mut r, &snapshot, size).pixels
    };
    // Warm-up (see the module docs).
    fresh(4);
    let (d4, d1) = (
        Noise::of(&fresh(4), &started(4)),
        Noise::of(&fresh(1), &started(1)),
    );
    eprintln!("4x {d4:?}, 1x {d1:?}");
    assert_eq!(d4, Noise::default());
    assert!(d1.is_repeat_noise(size), "{d1:?}");
}

/// Acquiring after preparation and submitting the units with the post chain
/// produces the same captured scene as the existing eager submission API.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn deferred_scene_commands_match_eager_submissions() {
    const SIZE: [u32; 2] = [480, 300];
    const SAMPLE_COUNT: u32 = 4;
    const SETTLE_FRAMES: usize = 40;
    const RGBA_BYTES: u32 = 4;
    const COLOR_CHANNELS: usize = 3;
    const SKIPPED_FRAMES: [usize; 3] = [0, 10, 20];
    const START_ANIMATION_CYCLE: i32 = 0;
    const ANIMATION_CYCLES_PER_RETRY: i32 = 5;
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let gpu = pollster::block_on(rs910_gpu_device::gpu_device::Device::headless(
        (SIZE[0], SIZE[1]),
        rs910_gpu_device::gpu_device::DeviceOptions::default(),
        wgpu::Features::empty(),
    ))
    .expect("headless GPU device");
    let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
    let location = rs910_symbols::location::DEV_PLAYER_SPAWN;
    let mut offline = OfflineScene::new(
        &pack,
        (location.x(), location.z()),
        (SIZE[0] as i32, SIZE[1] as i32),
    );
    let mut eager = renderer(&device, &queue, SAMPLE_COUNT, ModernSettings::DEFAULT);
    let mut deferred = renderer(&device, &queue, SAMPLE_COUNT, ModernSettings::DEFAULT);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("deferred scene output"),
        size: wgpu::Extent3d {
            width: SIZE[0],
            height: SIZE[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let [width, height] = SIZE.map(|v| v as i32);
    let viewport = [0, 0, width, height];
    let target = |clip| crate::frame::PrepareTarget {
        device: &device,
        queue: &gpu,
        size: SIZE,
        rect: viewport,
        clip,
    };
    const OUTSIDE_CLIP_SCALE: i32 = 2;
    for clip in [
        [0, 0, 0, height],
        [
            width,
            height,
            width * OUTSIDE_CLIP_SCALE,
            height * OUTSIDE_CLIP_SCALE,
        ],
    ] {
        let frames = deferred.frames();
        let exposure = (deferred.post.adapted_slot, deferred.post.last_ms);
        assert!(deferred
            .prepare_frame(target(clip), &offline.snapshot(&pack))
            .is_none());
        assert_eq!(deferred.frames(), frames);
        assert_eq!(
            (deferred.post.adapted_slot, deferred.post.last_ms),
            exposure
        );
        assert_eq!(deferred.probes.captured, None);
    }
    let mut expected = Vec::new();
    let mut animation_cycle = START_ANIMATION_CYCLE;
    for cycle in 0..SETTLE_FRAMES {
        if SKIPPED_FRAMES.contains(&cycle) {
            let snapshot = offline.snapshot(&pack);
            let frames = deferred.frames();
            let exposure = (deferred.post.adapted_slot, deferred.post.last_ms);
            let probes = (deferred.probes.captured, deferred.probes.last_env);
            let order = deferred.probes.job.as_ref().map(|job| job.order.clone());
            let unsent = deferred.prepare_frame(target(viewport), &snapshot).unwrap();
            deferred.frame_skipped(unsent);
            gpu.submit(std::iter::empty());
            assert_eq!(deferred.frames(), frames);
            assert_eq!(
                (deferred.post.adapted_slot, deferred.post.last_ms),
                exposure
            );
            assert_eq!((deferred.probes.captured, deferred.probes.last_env), probes);
            assert_eq!(
                deferred.probes.job.as_ref().map(|job| &job.order),
                order.as_ref()
            );
            // The retry reuses the renderer frame number after the game advances.
            animation_cycle += ANIMATION_CYCLES_PER_RETRY;
            assert!(offline.animate(animation_cycle) > 0);
        }
        let snapshot = offline.snapshot(&pack);
        expected = render(&device, &queue, &mut eager, &snapshot, SIZE).pixels;
        let prepared = deferred.prepare_frame(target(viewport), &snapshot).unwrap();
        // The surface view and final encoder need not exist during preparation.
        let view = output.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = deferred.record_frame(&device, &mut encoder, &view, prepared);
        gpu.submit(commands.into_iter().chain([encoder.finish()]));
        deferred.frame_submitted();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
    let actual = read_back(&device, &queue, &output, RGBA_BYTES);
    assert!(eager.stats.opaque > 0 && eager.stats.draws > 0);
    assert!(
        expected.chunks_exact(RGBA_BYTES as usize).any(|pixel| {
            pixel[..COLOR_CHANNELS] != expected[..COLOR_CHANNELS]
                && pixel[..COLOR_CHANNELS].iter().any(|&channel| channel != 0)
        }),
        "the real scene must contain varied nonblack colour"
    );
    assert_eq!(Noise::of(&expected, &actual), Noise::default());
    assert_eq!(eager.frames(), deferred.frames());
}

/// Borrowed and owned frames produce the same pixels; moving preparation onto
/// another thread and changing pool width retains packet/counter order.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn owned_scene_worker_keeps_pixels_and_job_result_order() {
    const SIZE: [u32; 2] = [320, 200];
    const SAMPLE_COUNT: u32 = 4;
    const INLINE_THREADS: usize = 1;
    const WORKER_THREADS: usize = 4;
    const ANIMATION_CYCLE: i32 = 50;
    const POSED_MODELS: usize = 4;
    const ALTERNATING_FRAMES: usize = 6;
    const FRAME_SLOTS: usize = 2;
    const FIRST_LIGHT: usize = 0;
    const FIRST_SLOT: usize = 0;
    const SECOND_SLOT: usize = 1;
    const LIGHT_INTENSITY_DELTA: f32 = 0.25;
    const FADE_GROUP: i32 = 0;
    const FADE_DURATION_MS: i32 = 0;
    const FADE_COLOUR: i32 = 0xff40a0;
    const NEXT_ANIMATION_CYCLE: i32 = ANIMATION_CYCLE + 1;
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let location = rs910_symbols::location::DEV_PLAYER_SPAWN;
    let mut offline = OfflineScene::new(
        &pack,
        (location.x(), location.z()),
        (SIZE[0] as i32, SIZE[1] as i32),
    );
    // Group membership is non-spatial; use one real cache light to exercise
    // the ordinary colour-fade consumer without replacing its tile grid.
    offline.live.model_lights.lights[FIRST_LIGHT].group = FADE_GROUP;
    offline.add_posed_models(POSED_MODELS);
    offline.animate(ANIMATION_CYCLE);
    let snapshot = offline.snapshot(&pack);
    let mut builder = rs910_scene::scene_snapshot::owned::SnapshotBuilder::default();
    let owned = builder.capture(&snapshot, None);
    let mut inline = renderer(&device, &queue, SAMPLE_COUNT, ModernSettings::DEFAULT);
    inline.set_threads(INLINE_THREADS);
    let mut worker = renderer(&device, &queue, SAMPLE_COUNT, ModernSettings::DEFAULT);
    worker.set_threads(WORKER_THREADS);
    let reference = settled(&device, &queue, &mut inline, &snapshot, SIZE).pixels;
    let (pixels, mut worker, owned) = std::thread::scope(|scope| {
        let device = &device;
        let queue = &queue;
        scope
            .spawn(move || {
                let pixels = settled(device, queue, &mut worker, &owned.snapshot(), SIZE).pixels;
                (pixels, worker, owned)
            })
            .join()
            .unwrap()
    });
    assert_eq!(Noise::of(&reference, &pixels), Noise::default());
    assert_eq!(inline.stats.draws, worker.stats.draws);
    assert_eq!(
        inline.stats.shadow_cascade_casters,
        worker.stats.shadow_cascade_casters
    );
    assert_eq!(inline.rt7.stats, worker.rt7.stats);
    assert_eq!(inline.rt7.anim.stats, worker.rt7.anim.stats);
    assert_eq!(inline.loc_arena.used(), worker.loc_arena.used());
    assert!(
        !worker.shadow.point.active.is_empty(),
        "real cache lights must exercise point slots"
    );
    let installed_grid = worker.lights.grid.clone();
    let installed_key = worker.lights.key();
    let point_keys = worker.shadow.point.keys;
    let point_redraws = worker
        .shadow
        .point
        .plan
        .slots
        .iter()
        .map(|slot| slot.redraw.count_ones())
        .sum::<u32>();
    let mut slots = [Some(owned), None];
    for index in 0..ALTERNATING_FRAMES {
        let slot = index % FRAME_SLOTS;
        let next = builder.capture(&snapshot, slots[slot].take());
        let output = render(&device, &queue, &mut worker, &next.snapshot(), SIZE);
        assert_eq!(Noise::of(&pixels, &output.pixels), Noise::default());
        assert_eq!(
            worker.lights.grid, installed_grid,
            "an owned slot must reuse the installed GPU light grid"
        );
        assert_eq!(worker.lights.key(), installed_key);
        assert_eq!(
            worker.shadow.point.keys, point_keys,
            "slot alternation must not reset point faces"
        );
        assert!(worker
            .shadow
            .point
            .plan
            .slots
            .iter()
            .all(|slot| !slot.clear_block));
        assert_eq!(
            worker
                .shadow
                .point
                .plan
                .slots
                .iter()
                .map(|slot| slot.redraw.count_ones())
                .sum::<u32>(),
            point_redraws
        );
        slots[slot] = Some(next);
    }
    drop(snapshot);
    let original_intensity = offline.live.model_lights.intensities[FIRST_LIGHT];
    offline.live.model_lights.intensities[FIRST_LIGHT] += LIGHT_INTENSITY_DELTA;
    let changed = builder.capture(&offline.snapshot(&pack), slots[FIRST_SLOT].take());
    render(&device, &queue, &mut worker, &changed.snapshot(), SIZE);
    assert_eq!(worker.lights.grid, installed_grid);
    assert_eq!(worker.lights.key(), installed_key);
    assert_eq!(
        worker.lights.frame[FIRST_LIGHT].intensity,
        original_intensity + LIGHT_INTENSITY_DELTA
    );
    assert_eq!(
        slots[SECOND_SLOT]
            .as_ref()
            .unwrap()
            .snapshot()
            .live_frame()
            .unwrap()
            .model_lights
            .intensities[FIRST_LIGHT],
        original_intensity
    );
    let frozen_colour = slots[SECOND_SLOT]
        .as_ref()
        .unwrap()
        .snapshot()
        .live_frame()
        .unwrap()
        .model_lights
        .colour(FIRST_LIGHT)
        .unwrap();
    offline.live.model_lights.point_light_colour(
        FADE_GROUP,
        FADE_DURATION_MS,
        FADE_COLOUR,
        ANIMATION_CYCLE,
    );
    offline
        .live
        .model_lights
        .animate(NEXT_ANIMATION_CYCLE, false);
    let faded = builder.capture(&offline.snapshot(&pack), Some(changed));
    let expected = render(&device, &queue, &mut inline, &offline.snapshot(&pack), SIZE).pixels;
    let actual = render(&device, &queue, &mut worker, &faded.snapshot(), SIZE).pixels;
    assert_eq!(Noise::of(&expected, &actual), Noise::default());
    assert_eq!(worker.lights.grid, installed_grid);
    assert_eq!(
        worker.lights.frame[FIRST_LIGHT].colour,
        crate::models::shading::colour_term(
            FADE_COLOUR,
            offline.live.model_lights.intensities[FIRST_LIGHT]
        )
    );
    assert_ne!(frozen_colour, FADE_COLOUR);
    assert_eq!(
        slots[SECOND_SLOT]
            .as_ref()
            .unwrap()
            .snapshot()
            .live_frame()
            .unwrap()
            .model_lights
            .colour(FIRST_LIGHT),
        Some(frozen_colour)
    );
    let scene = offline.world.scene_graph.as_ref().unwrap();
    offline.live.model_lights = rs910_scene::model_lights::ModelLights::new(
        scene,
        &offline.live.model_lights.lights,
        offline.live.entities.len(),
    );
    let replacement = builder.capture(&offline.snapshot(&pack), slots[SECOND_SLOT].take());
    render(&device, &queue, &mut worker, &replacement.snapshot(), SIZE);
    assert_ne!(
        worker.lights.grid, installed_grid,
        "fresh table installation must rebuild the grid"
    );
    assert_ne!(worker.lights.key(), installed_key);
    assert_eq!(worker.shadow.point.scene, Some(worker.lights.key()));
    assert!(
        worker
            .shadow
            .point
            .plan
            .slots
            .iter()
            .any(|slot| slot.clear_block),
        "fresh installation must invalidate point slots"
    );

    assert!(pixels
        .chunks_exact(4)
        .any(|pixel| pixel[..3].iter().any(|&channel| channel > 0)));
}
