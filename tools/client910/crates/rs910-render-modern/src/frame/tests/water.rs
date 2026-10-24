//! Water: culling the planar reflection to its frustum changes nothing.
use super::*;

/// The planar reflection draws only the draws its reflected frustum can
/// show (`frame::gpu::planar_reflection`): over the river from two pitches
/// and four cardinal headings, at 4x MSAA (exact repeats) and one sample, the culled reflection leaves
/// draws out and the frame equals the one whose reflection draws every
/// draw. The frame also runs the water's single resolves (group 0 into the
/// scene copy, group 2 into the frame) and the clipped scene copy.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn water_reflection_culling_leaves_the_frame_unchanged() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    const QUARTER_TURN: f32 = 4096.0;
    const HEADINGS: usize = 4;
    let mut far_culled = 0;
    for heading in 0..HEADINGS {
        for (pitch, zoom) in [(1400.0, 1.2), (2400.0, 1.6)] {
            let mut offline = river_scene(&pack, size, pitch, zoom);
            offline.camera.yaw += heading as f32 * QUARTER_TURN;
            let snapshot = offline.snapshot(&pack);
            for samples in [4, 1] {
                let mut r = renderer(&device, &queue, samples, ModernSettings::DEFAULT);
                r.frame_resources.water.test_no_reflection_cull = true;
                let all = settled(&device, &queue, &mut r, &snapshot, size);
                let every = r.water_stats();
                r.frame_resources.water.test_no_reflection_cull = false;
                let culled = render(&device, &queue, &mut r, &snapshot, size);
                let stats = r.water_stats();
                let far_packets = r
                    .frame_resources
                    .draws
                    .iter()
                    .enumerate()
                    .filter(|(_, draw)| matches!(draw.geometry, crate::frame::Geometry::Far { .. }))
                    .count();
                let far_kept = r
                    .frame_resources
                    .water
                    .reflected
                    .iter()
                    .filter(|index| {
                        matches!(
                            r.frame_resources.draws[**index as usize].geometry,
                            crate::frame::Geometry::Far { .. }
                        )
                    })
                    .count();
                far_culled += far_packets - far_kept;
                eprintln!(
                    "heading {heading} far reflection: {far_kept}/{far_packets} packets retained"
                );
                let noise = Noise::of(&all.pixels, &culled.pixels);
                eprintln!(
                    "pitch {pitch} {samples}x: reflection {} of {} draws ({} culled); {noise:?}",
                    stats.reflection_draws, every.reflection_draws, stats.reflection_culled
                );
                assert!(stats.plane.is_some(), "no reflection plane");
                assert!(stats.reflection_culled > 0, "nothing culled");
                assert_eq!(
                    stats.reflection_draws + stats.reflection_culled,
                    every.reflection_draws
                );
                if samples > 1 {
                    assert_eq!(noise.values, 0, "pitch {pitch}: {noise:?}");
                } else {
                    assert!(noise.is_repeat_noise(size), "pitch {pitch}: {noise:?}");
                }
            }
        }
    }
    assert!(
        far_culled > 0,
        "real far container bounds remove reflected draws"
    );
}
