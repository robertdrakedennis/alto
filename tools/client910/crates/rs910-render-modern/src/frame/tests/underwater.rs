//! The underwater scene: with water detail high the seabed and the locs on
//! it are drawn below the water surface, and the shallows show them.
use super::*;

/// Water detail high, the rest of the defaults.
fn high_water() -> rs910_scene::rebuild::BuildPrefs {
    rs910_scene::rebuild::BuildPrefs {
        water_detail: 2,
        ..Default::default()
    }
}

/// The harbour's sea from above: the build has an underwater floor and
/// locs standing on it.
fn harbour(pack: &crate::cache::Pack, size: [u32; 2]) -> OfflineScene {
    OfflineScene::with_prefs(
        pack,
        (3049, 3240),
        (size[0] as i32, size[1] as i32),
        &high_water(),
        |c| {
            c.pitch = 2400.0;
            c.distance_scale = 1.2;
        },
    )
}

/// The snapshot's underwater scene changes the frame where the water
/// covers it: the seabed's floor batches and the locs' meshes are drawn
/// (the renderer's caches hold them), the frame differs from the one
/// without the underwater scene over a real share of its pixels, and the
/// underwater scene is still there the next frame (its cached meshes are
/// drawn again, none rebuilt).
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn underwater_bed_and_locs_show_through_the_water() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    let offline = harbour(&pack, size);
    let with = offline.snapshot_underwater(&pack);
    let underwater = with.underwater.as_ref().expect("an underwater scene");
    assert!(underwater.floor.vertex_count > 0, "empty bed");
    assert!(!underwater.models.is_empty(), "no underwater locs");
    let without = offline.snapshot(&pack);

    let mut a = renderer(&device, &queue, 1, ModernSettings::DEFAULT);
    let frame_with = settled(&device, &queue, &mut a, &with, size);
    let meshes = a
        .underwater
        .meshes
        .iter()
        .flatten()
        .filter(|m| m.has_mesh())
        .count();
    let bed_draws = a.underwater_bed_draws();
    let mut b = renderer(&device, &queue, 1, ModernSettings::DEFAULT);
    let frame_without = settled(&device, &queue, &mut b, &without, size);
    dump_frame("underwater-with", size, &frame_with.pixels);
    dump_frame("underwater-without", size, &frame_without.pixels);
    let changed = frame_with
        .pixels
        .chunks_exact(4)
        .zip(frame_without.pixels.chunks_exact(4))
        .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 8))
        .count();
    let share = changed as f64 / (size[0] * size[1]) as f64;
    eprintln!(
        "underwater: {} models ({meshes} meshes), {bed_draws} bed draws, {:.1}% of pixels changed",
        underwater.models.len(),
        share * 100.0
    );
    assert!(meshes > 0, "no underwater mesh was built");
    assert!(bed_draws > 0, "no seabed draw");
    assert!(
        share > 0.02,
        "only {:.2}% of the frame changed",
        share * 100.0
    );

    // A second frame builds nothing again.
    let built = a.loc_mesh_cache().1;
    render(&device, &queue, &mut a, &with, size);
    assert_eq!(a.loc_mesh_cache().1, built, "meshes were rebuilt");
    assert_eq!(
        a.underwater
            .meshes
            .iter()
            .flatten()
            .filter(|m| m.has_mesh())
            .count(),
        meshes
    );
}
