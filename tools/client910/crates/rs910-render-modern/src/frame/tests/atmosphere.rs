//! The atmosphere: the haze of the light scattering grows with distance,
//! and the volumetric scattering gathers the sun's light where the
//! cascades leave it, repeatably.
use super::*;
use crate::shadows::Settings;

/// A viewer-only pose looking from `pitch` (game units, below the orbit
/// clamp as NXT's camera looks up at the sky) and `yaw`, `scale` times the
/// orbit distance of pitch 1077 from the target (a legacy pose, as
/// `Client` state 1 sets one).
fn pose(c: &mut crate::camera::SceneCamera, pitch: i32, yaw: i32, scale: f32) {
    let dist = (crate::camera::orbit_distance(1077) as f32 * scale) as i32;
    let eye = crate::camera::orbit_camera_with_profile(
        crate::camera::Orbit {
            target: [0, 0, 0],
            pitch,
            yaw,
            distance: dist,
        },
        c.viewport.1,
        c.viewport_profile,
    );
    c.legacy = Some(crate::camera::LegacyFrame {
        eye,
        pitch,
        yaw,
        roll: 0,
    });
}

/// A settled frame of `snapshot` from a fresh renderer set up by `setup`,
/// with its eye distances.
fn frame_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    snapshot: &SceneSnapshot<'_>,
    settings: ModernSettings,
    size: [u32; 2],
    setup: impl FnOnce(&mut ModernRenderer),
) -> (Frame, Vec<f32>, ModernRenderer) {
    let mut r = renderer(device, queue, 4, settings);
    setup(&mut r);
    let f = settled(device, queue, &mut r, snapshot, size);
    let d = eye_distances(device, queue, &r, snapshot);
    (f, d, r)
}

/// The light scattering over the aerial Lumbridge view (NXT's longer draw
/// distance): against the same geometry drawn through clear air (the sky
/// and the probes unchanged), nothing changes well before the record's
/// start distance, and past it the haze (the in-scattered blue) grows with
/// the distance, band after band.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn haze_grows_with_distance() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [800, 500];
    let offline = OfflineScene::with_camera(&pack, (3230, 3222), (800, 500), |c| {
        c.pitch = 2200.0;
        c.yaw = 0.0;
        c.distance_scale = 1.8;
        c.far_scale = 1.7;
    });
    let sky = offline.sky(&pack);
    let mut snapshot = offline.snapshot(&pack);
    snapshot.sky = sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
    // Without the volumetrics (they light every pixel).
    let d = ModernSettings {
        volumetrics: false,
        ..ModernSettings::DEFAULT
    };
    let (clear, dist, _) = frame_with(&device, &queue, &snapshot, d, size, |r| {
        r.frame_resources.atmos.test_clear_air = true;
    });
    let (hazy, _, r) = frame_with(&device, &queue, &snapshot, d, size, |_| {});
    let s = r.atmos_frame().scattering.expect("the scattering");
    let [_, start, _] = s.law();
    let fog_start = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32)).fog_range[0];
    let mut bands = vec![(0usize, 0.0f64); 16];
    let mut unchanged = (0usize, 0usize);
    for (i, &d) in dist.iter().enumerate() {
        if d <= 0.0 || d >= fog_start {
            continue;
        }
        let (a, b) = (&clear.hdr[i * 4..i * 4 + 4], &hazy.hdr[i * 4..i * 4 + 4]);
        // The scattering is per vertex, and the far scene's triangles are
        // coarse: pixels a little nearer than the start interpolate some.
        if d < 0.6 * start {
            unchanged.0 += 1;
            unchanged.1 += usize::from(a[..3] == b[..3]);
            continue;
        }
        if d < start {
            continue;
        }
        let band = ((d - start) / 2000.0) as usize;
        if band < bands.len() {
            bands[band].0 += 1;
            bands[band].1 += f64::from(b[2] - a[2]);
        }
    }
    let lifts: Vec<(usize, f64)> = bands
        .iter()
        .filter(|b| b.0 > 200)
        .map(|b| (b.0, b.1 / b.0 as f64))
        .collect();
    eprintln!(
        "start {start}: unchanged before 0.6 of it {}/{}; bands (pixels, mean blue lift): {lifts:?}",
        unchanged.1, unchanged.0
    );
    // (Silhouettes against farther pixels and the water, which the depth
    // does not hold, account for the rest.)
    assert!(
        unchanged.0 > 500 && unchanged.1 * 10 >= unchanged.0 * 9,
        "{unchanged:?}"
    );
    assert!(lifts.len() >= 3, "{lifts:?}");
    for w in lifts.windows(2) {
        assert!(
            w[1].1 > w[0].1,
            "the haze grows with the distance: {lifts:?}"
        );
    }
}

/// The volumetric scattering over the castle from the south-east: it adds
/// the sun's light along the view rays (most pixels change), less of it
/// where the cascades shadow the rays than with no sun shadows, and its
/// frames repeat (no temporal jitter): frame to frame of one renderer and
/// renderer to renderer within the repeat noise every frame shows.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn volumetrics_gather_the_unshadowed_sun_repeatably() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [800, 500];
    let offline = OfflineScene::with_camera(&pack, (3219, 3215), (800, 500), |c| {
        pose(c, 1000, 2048, 2.4);
        c.far_scale = 1.5;
    });
    let sky = offline.sky(&pack);
    let mut snapshot = offline.snapshot(&pack);
    snapshot.sky = sky.as_ref().and_then(crate::sky_frame::SkyCache::frame);
    let on = ModernSettings::DEFAULT;
    let off = ModernSettings {
        volumetrics: false,
        ..on
    };
    let no_shadows =
        |r: &mut ModernRenderer| r.set_shadow_settings(Settings::from_options(0, 0, 0));
    // Warm-up (the module docs), then the frames.
    frame_with(&device, &queue, &snapshot, on, size, |_| {});
    let (vol, _, mut r) = frame_with(&device, &queue, &snapshot, on, size, |_| {});
    let again = render(&device, &queue, &mut r, &snapshot, size);
    let (other, _, _) = frame_with(&device, &queue, &snapshot, on, size, |_| {});
    let (plain, _, _) = frame_with(&device, &queue, &snapshot, off, size, |_| {});
    let (vol_lit, _, _) = frame_with(&device, &queue, &snapshot, on, size, no_shadows);
    let (plain_lit, _, _) = frame_with(&device, &queue, &snapshot, off, size, no_shadows);
    let (n_again, n_other) = (
        Noise::of(&vol.pixels, &again.pixels),
        Noise::of(&vol.pixels, &other.pixels),
    );
    let added = |on: &[f32], off: &[f32]| {
        let (mut sum, mut changed) = (0.0f64, 0usize);
        for (p, q) in on.chunks_exact(4).zip(off.chunks_exact(4)) {
            sum += f64::from(luma(p) - luma(q));
            changed += usize::from(p[..3] != q[..3]);
        }
        (sum / (on.len() / 4) as f64, changed)
    };
    let (shadowed, changed) = added(&vol.hdr, &plain.hdr);
    let (lit, _) = added(&vol_lit.hdr, &plain_lit.hdr);
    eprintln!(
        "in-scatter (mean luma added): {shadowed:.5} with the cascades, {lit:.5} without; {changed} pixels changed; frame to frame {n_again:?}, renderer to renderer {n_other:?}"
    );
    assert_eq!(n_again, Noise::default(), "frame to frame");
    assert!(
        n_other.is_repeat_noise(size),
        "renderer to renderer {n_other:?}"
    );
    assert!(changed * 2 > vol.hdr.len() / 4);
    assert!(shadowed > 1e-4, "the rays gather light: {shadowed}");
    assert!(
        lit > shadowed,
        "shadowed rays gather less light: {lit} vs {shadowed}"
    );
}
