//! The frame as a whole: the sorted depth passes and the threaded encode
//! leave the frame of the plain path unchanged.
use super::*;

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
    let size = [480, 300];
    let gpu = pollster::block_on(rs910_gpu_device::gpu_device::Device::headless(
        (size[0], size[1]),
        Default::default(),
        crate::device_features::optional_device_features(),
    ))
    .expect("headless modern device");
    let (device, queue) = (&*gpu.device, &*gpu.queue);
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
        let mut r = renderer(device, queue, 4, ModernSettings::DEFAULT);
        r.set_shadow_settings(crate::shadows::Settings::from_options(2, 3, 1));
        // Every cascade redraws its casters each frame (no shadow cache).
        r.history.shadow.sun.test_uncached = true;
        let sorted = settled(device, queue, &mut r, &snapshot, size).pixels;
        let lists: Vec<Vec<crate::frame::Draw>> =
            std::iter::once(r.frame_resources.packets.prepass.clone())
                .chain(r.history.shadow.sun.static_lists.iter().cloned())
                .collect();
        r.preparation.test_unsorted_packets = true;
        let ordered = render(device, queue, &mut r, &snapshot, size).pixels;
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
            if device
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
            {
                assert!(
                    list.windows(2).all(|pair| {
                        pair[0]
                            .indirect
                            .zip(pair[1].indirect)
                            .is_some_and(|(first, next)| next == first + 1)
                    }),
                    "{name} list {k}: sorted packets retain contiguous arguments"
                );
            }
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
