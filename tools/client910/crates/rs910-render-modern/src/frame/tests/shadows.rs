//! Sun shadows: the per-cascade caster culling leaves the frame unchanged.
use super::*;
use crate::shadows::Settings;

/// The visible casters culled per cascade give the frame of drawing every
/// visible caster into every cascade, with fewer caster draw calls: at the
/// river, the castle and Lumbridge, at the medium (select by sphere) and
/// ultra (select by map) qualities.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn cascade_culling_leaves_the_frame_unchanged() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    let views = [
        ("river", river_scene(&pack, size, 1400.0, 1.2)),
        (
            "castle",
            OfflineScene::with_camera(&pack, (3208, 3218), (480, 300), |c| {
                c.pitch = 2400.0;
                c.distance_scale = 0.7;
            }),
        ),
        (
            "lumbridge",
            OfflineScene::new(&pack, (3222, 3222), (480, 300)),
        ),
    ];
    for (name, offline) in &views {
        let snapshot = offline.snapshot(&pack);
        for quality in [1, 3] {
            // One renderer, culling toggled between frames.
            let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
            r.set_shadow_settings(Settings::from_options(2, quality, 1));
            let culled = settled(&device, &queue, &mut r, &snapshot, size).pixels;
            let n_culled = r.stats.shadow_cascade_casters;
            r.preparation.test_no_cascade_cull = true;
            let whole = render(&device, &queue, &mut r, &snapshot, size).pixels;
            let n_whole = r.stats.shadow_cascade_casters;
            let differ = Noise::of(&culled, &whole);
            eprintln!(
                "{name} quality {quality}: caster draw calls {n_whole} -> {n_culled}; {differ:?}"
            );
            assert_eq!(differ, Noise::default(), "{name} quality {quality}");
            assert!(n_culled < n_whole, "{name} quality {quality}");
        }
    }
}
