//! Sun shadow maths (renderer plan M3, CPU): the option mapping, the fit
//! enclosing its slice, and the cascades' stability under camera moves (the fitted
//! matrices move the atlas by whole texels; the square does not change size
//! when the camera turns).
use super::*;

fn camera(target: [i32; 3], yaw: f32) -> crate::camera::SceneCamera {
    let mut camera = crate::camera::SceneCamera::new(target);
    camera.viewport = (1024, 640);
    camera.yaw = yaw;
    camera
}

/// A frame for `camera` as the renderer fits it: camera-local view and
/// projection, the scene-local origin (here the absolute target).
fn frame_of(camera: &crate::camera::SceneCamera, quality: Quality) -> (ShadowFrame, DVec3) {
    let mut local = camera.clone();
    local.target = [0; 3];
    let origin = camera.target.map(|v| v as f32);
    let frame = ShadowFrame::new(
        crate::shadows::presets::profile(quality),
        &local.view_entries(),
        &local.projection(),
        origin,
        SUN,
    );
    (frame, DVec3::from(origin.map(f64::from)))
}

/// A Lumbridge-like sun: from the south-west, 55° up (y down).
const SUN: [f32; 3] = [-0.45, -0.8, -0.4];

/// The classic `shadowQuality` option (0-4, default 1) is taken as the
/// level (0 LOW .. 4 ULTRA_PLUS); shadows go off only when nothing casts.
#[test]
fn settings_map_the_client_options() {
    let s = Settings::default();
    assert_eq!(s.quality, Some(Quality::Medium));
    assert!(s.scenery && s.characters);
    for (option, level) in [
        (0, Quality::Low),
        (1, Quality::Medium),
        (2, Quality::High),
        (3, Quality::Ultra),
        (4, Quality::UltraPlus),
        (7, Quality::Low),
    ] {
        assert_eq!(Settings::from_options(2, option, 1).quality, Some(level));
    }
    // Neither locs nor characters cast: no pass.
    assert_eq!(Settings::from_options(0, 3, 0).profile(), None);
    let only_characters = Settings::from_options(0, 3, 1);
    assert!(!only_characters.scenery && only_characters.characters);
    assert!(only_characters.profile().is_some());
    assert_eq!(Quality::parse("off"), Some(None));
    assert_eq!(Quality::parse("high"), Some(Some(Quality::High)));
    assert_eq!(Quality::parse("x"), None);
}

#[test]
fn light_basis_is_orthonormal_and_follows_the_sun() {
    for sun in [SUN, [0.0, -1.0, 0.0], [0.3, 0.2, -0.9]] {
        let b = LightBasis::new(sun);
        for (a, c) in [(b.right, b.up), (b.up, b.forward), (b.right, b.forward)] {
            assert!(a.dot(c).abs() < 1e-12);
        }
        for v in [b.right, b.up, b.forward] {
            assert!((v.length() - 1.0).abs() < 1e-12);
        }
        let s = DVec3::from(sun.map(f64::from)).normalize();
        assert!(
            (b.forward + s).length() < 1e-12,
            "light travels away from the sun"
        );
    }
}

/// Every corner of every cascade's slice lands inside its square (NDC
/// `[-1, 1]`) and depth range (`[0, 1]`), and cascades that follow each
/// other overlap at their split.
#[test]
fn fit_encloses_the_slice() {
    let camera = camera([3222 * 512, -600, 3222 * 512], 3000.0);
    for quality in [Quality::Low, Quality::Medium, Quality::High, Quality::Ultra] {
        let (frame, origin) = frame_of(&camera, quality);
        let mut local = camera.clone();
        local.target = [0; 3];
        let view_to_local = DMat4::from_cols_array(&local.view_entries().map(f64::from)).inverse();
        let rays = corner_rays(&local.projection());
        let splits = frame.profile.splits();
        for (k, fit) in frame.fits.iter().enumerate() {
            let map = fit.clip_map(&frame.basis, origin);
            let near = if k == 0 {
                0.0
            } else {
                f64::from(splits[k - 1])
            };
            for depth in [near, f64::from(splits[k])] {
                for r in &rays {
                    let local = view_to_local.transform_point3(DVec3::new(
                        r[0] * depth,
                        r[1] * depth,
                        depth,
                    ));
                    let lv = frame.basis.apply(local);
                    let ndc: [f64; 3] = std::array::from_fn(|i| {
                        lv[i] * f64::from(map.scale[i]) + f64::from(map.offset[i])
                    });
                    assert!(
                        ndc[0].abs() <= 1.0 + 1e-4 && ndc[1].abs() <= 1.0 + 1e-4,
                        "{quality:?} cascade {k}: {ndc:?}"
                    );
                    assert!(
                        (0.0..=1.0).contains(&ndc[2]),
                        "{quality:?} cascade {k}: {ndc:?}"
                    );
                }
            }
            // The pull-back: the square's depth reaches CASTER_PULL_BACK
            // past the sphere towards the sun.
            assert!(
                (fit.depth[1] - fit.depth[0] - 2.0 * fit.radius - f64::from(CASTER_PULL_BACK))
                    .abs()
                    < 1e-6
            );
        }
        // The cascades grow with their splits.
        for w in frame.fits.windows(2) {
            assert!(w[1].radius > w[0].radius);
        }
    }
}

/// The camera-move test (renderer plan M3 verification): over a walk of
/// sub-texel and larger moves and turns of the camera, each cascade's square
/// keeps its size, its centre sits on the scene's texel grid, and a fixed
/// scene point's atlas position changes only by whole texels, so the
/// rasterised shadow map of a still scene moves without resampling
/// (no shimmer).
#[test]
fn cascades_are_stable_under_camera_moves() {
    let base = [3222 * 512 + 100, -600, 3222 * 512 + 50];
    let point = DVec3::new(
        f64::from(base[0]) + 300.0,
        -120.0,
        f64::from(base[2]) + 700.0,
    );
    let moves: [([i32; 3], f32); 8] = [
        ([0, 0, 0], 0.0),
        ([1, 0, 0], 0.0),
        ([3, 0, -2], 0.0),
        ([17, 0, 9], 0.0),
        ([130, 0, -77], 0.0),
        ([130, 0, -77], 2048.0),
        ([131, 0, -76], 5000.0),
        ([900, 0, 400], 11_000.0),
    ];
    for quality in [Quality::Medium, Quality::Ultra] {
        let mut first: Option<ShadowFrame> = None;
        let mut texels: Vec<Vec<[f64; 2]>> = Vec::new();
        for (d, yaw) in moves {
            let target = [base[0] + d[0], base[1] + d[1], base[2] + d[2]];
            let (frame, origin) = frame_of(&camera(target, yaw), quality);
            let res = f64::from(frame.profile.resolution);
            let mut row = Vec::new();
            for (k, fit) in frame.fits.iter().enumerate() {
                if let Some(first) = &first {
                    assert_eq!(fit.radius, first.fits[k].radius, "cascade {k} resized");
                }
                for c in fit.centre {
                    let steps = c / fit.texel;
                    assert!((steps - steps.round()).abs() < 1e-6, "off-grid centre {c}");
                }
                let map = fit.clip_map(&frame.basis, origin);
                let lv = frame.basis.apply(point - origin);
                let ndc: [f64; 2] = std::array::from_fn(|i| {
                    lv[i] * f64::from(map.scale[i]) + f64::from(map.offset[i])
                });
                row.push(ndc.map(|v| (v * 0.5 + 0.5) * res));
            }
            texels.push(row);
            first.get_or_insert(frame);
        }
        for row in &texels[1..] {
            for (k, t) in row.iter().enumerate() {
                for i in 0..2 {
                    let shift = t[i] - texels[0][k][i];
                    assert!(
                        (shift - shift.round()).abs() < 2e-3,
                        "{quality:?} cascade {k}: the point moved {shift} texels"
                    );
                }
            }
        }
        // Without the snap the same walk moves by fractions of a texel.
        let (a, oa) = frame_of(&camera(base, 0.0), quality);
        let (b, ob) = frame_of(&camera([base[0] + 3, base[1], base[2] - 2], 0.0), quality);
        let unsnapped = |f: &ShadowFrame, o: DVec3| {
            let c = f.basis.apply(point - o);
            c.x / f.fits[0].texel
        };
        let raw = unsnapped(&b, ob) - unsnapped(&a, oa);
        assert!((raw - raw.round()).abs() > 1e-3, "{raw}");
    }
}
