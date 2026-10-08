//! Sun shadows: the filter's penumbra per quality, the roof-hidden casters
//! indoors, the per-cascade caster culling, and the shadow caches (kept
//! maps in a still scene, moving casters and a turning sun redrawn).
use super::*;
use crate::shadows::{Settings, ShadowFrame};

/// The cascade the lookup selects for camera-local `p` (the frame's
/// selection: by bounding sphere or by map, `sun_visibility`).
fn cascade_of(frame: &ShadowFrame, p: [f32; 3]) -> usize {
    let u = &frame.uniforms;
    let lv = glam::Mat4::from_cols_array_2d(&u.light_view) * glam::Vec4::new(p[0], p[1], p[2], 1.0);
    let lv = lv.truncate();
    let n = frame.profile.cascades;
    (0..n)
        .find(|&k| {
            if u.bias_select[1] > 1.5 {
                let c = glam::Vec4::from(u.spheres[k]);
                (lv - c.truncate()).length_squared() < c.w
            } else {
                let c = lv * glam::Vec3::from_slice(&u.tex_scale[k][..3])
                    + glam::Vec3::from_slice(&u.tex_offset[k][..3]);
                let e = u.extents[k];
                c.x >= e[0]
                    && c.y >= e[1]
                    && c.x <= e[2]
                    && c.y <= e[3]
                    && (0.0..=1.0).contains(&c.z)
            }
        })
        .unwrap_or(n)
}

/// The penumbra widens with the filter of each quality while the umbra and
/// the sunlit ground stay exact. A grey ground and a square occluder 400
/// units above it, centred 250 units to `-x` of the camera target, under a
/// sun at 45° from `-x`: the shadow's near edge crosses the ground at
/// `x = -150` (camera-local). Along `z = 0` the ground's light, as a share
/// between the umbra and the sunlit level, falls from 90% to 10% over the
/// penumbra; its width in the cascade's texels is measured for LOW (the
/// 2x2 box), MEDIUM (the approximate 4x4 box, the client's default) and
/// HIGH (the 4x4 box): the 4x4 filters' penumbrae are over 1.5 times the
/// 2x2 box's; at every level the umbra keeps none of the direct sun (the
/// ground there is the frame lit by the ambient alone) and the sunlit
/// ground is unchanged by the shadows.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn penumbra_widens_with_the_filter_and_the_umbra_stays_binary() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let size = [960, 640];
    let (mut camera, mut env) = bare_camera(size);
    camera.zoom = Some(2400);
    camera.distance_scale = 0.4;
    env.sun.dir = [
        -std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    ];
    let snapshot = bare(&camera, &env);
    let target = camera.target;
    let (cx, cz) = (target[0] as f32, target[2] as f32);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    let uniforms = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32));
    let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
    // The ground's samples along z = 0 across the edge, one unit apart.
    let xs: Vec<f32> = (0..=240).map(|i| i as f32 - 270.0).collect();
    let pixels: Vec<(f32, f32)> = xs
        .iter()
        .map(|&x| pixel_of(&view_proj, size, [x, 0.0, 0.0]))
        .collect();
    let footprint = (pixels[240].0 - pixels[0].0).hypot(pixels[240].1 - pixels[0].1) / 240.0;
    assert!(
        footprint > 2.0,
        "{footprint:.2} pixels per unit: too coarse"
    );
    // Clear air, no volumetrics: the ground's light is its surface's.
    let settings = ModernSettings {
        volumetrics: false,
        ..ModernSettings::DEFAULT
    };
    let mut r = renderer(&device, &queue, 4, settings);
    r.frame_resources.atmos.test_clear_air = true;
    let models = vec![
        (flat_quad(cx, cz, 4000.0, 0.0), identity),
        (flat_quad(cx - 250.0, cz, 300.0, -400.0), identity),
    ];
    let draw = |r: &mut ModernRenderer| {
        r.preparation.test_models = models.clone();
        render(&device, &queue, r, &snapshot, size).hdr
    };
    // The references: the ground unshadowed, and lit by the ambient alone
    // (no direct sun).
    r.set_shadow_settings(Settings::from_options(0, 0, 0));
    let off = draw(&mut r);
    let look = r.look;
    r.look.sun = 0.0;
    let ambient = draw(&mut r);
    r.look = look;
    let mut widths = Vec::new();
    for (label, option) in [("LOW 2x2", 0), ("MEDIUM 4x4 approx", 1), ("HIGH 4x4", 2)] {
        r.set_shadow_settings(Settings::from_options(2, option, 1));
        let hdr = draw(&mut r);
        let frame = r.shadow_frame().expect("shadows on");
        let k = cascade_of(frame, [-150.0, 0.0, 0.0]);
        let texel = frame.fits[k].texel;
        // The share of the direct sun each sample keeps.
        let profile: Vec<f64> = pixels
            .iter()
            .map(|&p| {
                let a = green(&ambient, size, p);
                (green(&hdr, size, p) - a) / (green(&off, size, p) - a)
            })
            .collect();
        let mean =
            |r: std::ops::Range<usize>| profile[r.clone()].iter().sum::<f64>() / r.len() as f64;
        let (lit, umbra) = (mean(0..40), mean(180..241));
        assert!(
            (lit - 1.0).abs() < 2e-3,
            "{label}: the sunlit ground keeps {lit:.4} of its light"
        );
        assert!(
            umbra.abs() < 0.01,
            "{label}: the umbra keeps {umbra:.4} of the direct sun"
        );
        let s: Vec<f64> = profile
            .iter()
            .map(|v| (v - umbra) / (lit - umbra))
            .collect();
        let cross = |level: f64| {
            let i = s.iter().position(|&v| v <= level).expect("the edge");
            let t = (s[i - 1] - level) / (s[i - 1] - s[i]);
            f64::from(xs[i - 1]) + t
        };
        let width = cross(0.1) - cross(0.9);
        eprintln!(
            "{label}: penumbra {width:.2} units = {:.2} texels of cascade {k} (texel {texel:.3}); umbra keeps {umbra:.4} of the sun",
            width / texel
        );
        widths.push(width / texel);
    }
    let [low, med, high] = widths[..] else {
        unreachable!()
    };
    assert!(med > 1.5 * low, "MEDIUM {med:.2} vs LOW {low:.2} texels");
    assert!(high > 1.5 * low, "HIGH {high:.2} vs LOW {low:.2} texels");
}

/// Indoors the roof removal hides the storeys above the camera's floor,
/// and they still cast: the castle kitchen's floor (the middle of the
/// frame) under its hidden storeys has lost its direct sun (against the
/// same frame without sun shadows), with the roof-hidden entities among the
/// casters.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn hidden_roofs_shade_the_kitchen_floor() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 320];
    let offline = OfflineScene::with_camera(&pack, (3208, 3218), (480, 320), |c| {
        c.pitch = 2400.0;
        c.distance_scale = 0.7;
    });
    let snapshot = offline.snapshot(&pack);
    let frame = |shadows: Settings| {
        let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
        r.set_shadow_settings(shadows);
        let f = settled(&device, &queue, &mut r, &snapshot, size);
        (f.hdr, r.interior_casters())
    };
    let (lit, _) = frame(Settings::from_options(0, 0, 0));
    let (shaded, casters) = frame(Settings::default());
    let (w, h) = (size[0] as usize, size[1] as usize);
    let centre: Vec<usize> = (h / 3..2 * h / 3)
        .flat_map(|y| (w / 3..2 * w / 3).map(move |x| y * w + x))
        .collect();
    let mean = |hdr: &[f32]| {
        centre
            .iter()
            .map(|&i| f64::from(luma(&hdr[i * 4..])))
            .sum::<f64>()
            / centre.len() as f64
    };
    let (m_lit, m_shaded) = (mean(&lit), mean(&shaded));
    eprintln!("kitchen centre luminance {m_lit:.4} without sun shadows, {m_shaded:.4} with; roof-hidden casters {casters:?}");
    assert!(casters.0 > 20 && casters.1 > 0, "{casters:?}");
    assert!(m_shaded < 0.85 * m_lit, "{m_lit} -> {m_shaded}");
}

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

/// The frame of `r` drawing every shadow map from scratch in one pass per
/// map, as before the caches (every caster, static and dynamic, into a
/// cleared atlas, every cascade freshly fitted, every point-shadow face);
/// the caches start again after it.
fn uncached(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    size: [u32; 2],
) -> Vec<u8> {
    r.history.shadow.sun.test_uncached = true;
    r.history.shadow.point.forget_faces();
    let frame = render(device, queue, r, snapshot, size).pixels;
    r.history.shadow.sun.test_uncached = false;
    frame
}

/// A still scene keeps its shadow maps: after the warm-up a frame redraws
/// no caster (sun cascades and point-light faces) and is the frame of
/// redrawing every map from scratch. With its dynamic locs animating, the
/// kept static casters under the redrawn animated ones still give the
/// frame of a full redraw. Lumbridge and the river at MED with point
/// shadows and at ULTRA without (a frame keeping its maps then skips the
/// off-screen static casters' draws), each against the frame of drawing
/// every map from scratch as before the caches.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn still_scenes_keep_their_shadow_maps() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 300];
    for (name, mut offline) in [
        (
            "lumbridge",
            OfflineScene::new(&pack, (3222, 3222), (480, 300)),
        ),
        ("river", river_scene(&pack, size, 1400.0, 1.2)),
    ] {
        for quality in [1, 3] {
            let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
            r.set_shadow_settings(Settings::from_options(2, quality, 1));
            let snapshot = offline.snapshot(&pack);
            settled(&device, &queue, &mut r, &snapshot, size);
            let kept = render(&device, &queue, &mut r, &snapshot, size).pixels;
            let (met, drawn, faces) = (
                r.stats.shadow_cascade_casters,
                r.stats.shadow_draws,
                r.stats.point_shadow_draws,
            );
            let redrawn = uncached(&device, &queue, &mut r, &snapshot, size);
            let full = r.stats.shadow_draws;
            eprintln!(
                "{name} quality {quality}: {met} caster draws meet the cascades; kept frame draws {drawn} (+{faces} point faces), a full redraw {full}"
            );
            // What is left is the few dynamic casters (scrolling
            // materials, animated locs).
            assert!(
                met > 1000 && full >= met / 2,
                "{name} {quality}: {met} / {full}"
            );
            assert!(drawn * 100 <= full && faces == 0, "{name} {quality}");
            assert_eq!(
                Noise::of(&kept, &redrawn),
                Noise::default(),
                "{name} {quality}"
            );
            // The dynamic locs animate: they are redrawn every frame over
            // the kept static casters.
            drop(snapshot);
            let mut changed = 0;
            for cycle in 1..=4 {
                changed += offline.animate(5 * cycle);
                let snapshot = offline.snapshot(&pack);
                render(&device, &queue, &mut r, &snapshot, size);
            }
            let snapshot = offline.snapshot(&pack);
            let animated = render(&device, &queue, &mut r, &snapshot, size).pixels;
            let drawn = r.stats.shadow_draws;
            let redrawn = uncached(&device, &queue, &mut r, &snapshot, size);
            eprintln!(
                "{name} quality {quality}: {changed} loc poses changed; the animated frame draws {drawn} casters, a full redraw {}",
                r.stats.shadow_draws
            );
            assert!(
                changed > 0 && drawn < r.stats.shadow_draws,
                "{name} {quality}"
            );
            assert_eq!(
                Noise::of(&animated, &redrawn),
                Noise::default(),
                "{name} {quality}"
            );
        }
    }
}

/// Moving casters and a turning sun move their shadows at once: a caster
/// over a ground quad moves across, and the next frame of the renderer that
/// drew it at its old place is the frame of redrawing every shadow map from
/// scratch (the old shadow gone); likewise the frame after the sun turns
/// (a turned light refits every cascade, the far ones included), at LOW,
/// MED and ULTRA.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn moving_casters_and_a_turning_sun_move_their_shadows() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480, 320];
    let (mut camera, mut env) = bare_camera(size);
    camera.zoom = Some(2400);
    camera.distance_scale = 0.4;
    env.sun.dir = [-0.6, -0.7, 0.2];
    let mut turned = env;
    turned.sun.dir = [0.5, -0.8, -0.3];
    let (cx, cz) = (camera.target[0] as f32, camera.target[2] as f32);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    let scene = |x: f32| {
        vec![
            (flat_quad(cx, cz, 4000.0, 0.0), identity),
            (flat_quad(cx + x, cz, 150.0, -400.0), identity),
        ]
    };
    let settings = ModernSettings {
        volumetrics: false,
        ..ModernSettings::DEFAULT
    };
    let draw = |r: &mut ModernRenderer, env: &rs910_scene::env::EnvFrame, x: f32| {
        r.preparation.test_models = scene(x);
        render(&device, &queue, r, &bare(&camera, env), size).pixels
    };
    // The frame of `r` and the same frame drawn without the caches.
    let with_redrawn = |r: &mut ModernRenderer, env: &rs910_scene::env::EnvFrame, x: f32| {
        let kept = draw(r, env, x);
        r.preparation.test_models = scene(x);
        (
            kept,
            uncached(&device, &queue, r, &bare(&camera, env), size),
        )
    };
    for quality in [0, 1, 3, 4] {
        crate::logic_clock::set_test_now(Some(1_700_000_000_000));
        let mut r = renderer(&device, &queue, 4, settings);
        r.set_shadow_settings(Settings::from_options(2, quality, 1));
        r.preparation.test_models = scene(-250.0);
        let before = settled(&device, &queue, &mut r, &bare(&camera, &env), size).pixels;
        draw(&mut r, &env, 250.0);
        let (moved, redrawn) = with_redrawn(&mut r, &env, 250.0);
        let shifted = Noise::of(&before, &moved);
        eprintln!("quality {quality}: the moved caster changes {shifted:?}");
        assert!(shifted.values > 1000, "quality {quality}: {shifted:?}");
        assert_eq!(
            Noise::of(&moved, &redrawn),
            Noise::default(),
            "quality {quality}"
        );
        // The sun starts turning, then its turn time passes in one frame.
        draw(&mut r, &turned, 250.0);
        crate::logic_clock::set_test_now(Some(1_700_000_100_000));
        draw(&mut r, &turned, 250.0);
        let (after, redrawn) = with_redrawn(&mut r, &turned, 250.0);
        let shifted = Noise::of(&moved, &after);
        eprintln!("quality {quality}: the turned sun changes {shifted:?}");
        assert!(shifted.values > 1000, "quality {quality}: {shifted:?}");
        assert_eq!(
            Noise::of(&after, &redrawn),
            Noise::default(),
            "quality {quality}"
        );
    }
}
