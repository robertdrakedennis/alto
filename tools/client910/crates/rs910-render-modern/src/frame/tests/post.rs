//! Ambient occlusion: each mode darkens a crease's ambient and nothing
//! else.
use super::*;
use crate::settings::{AoMode, AoResolution};

/// A grey quad of half-width `half` standing on the ground (classic y down)
/// at `centre`, facing back along `-d` (unit `[x, z]`), `height` tall.
fn wall(
    centre: [f32; 2],
    d: [f32; 2],
    half: f32,
    height: f32,
) -> crate::models::mesh::ModelStreams {
    let side = [-d[1], d[0]];
    let corner = |s: f32, y: f32| [centre[0] + side[0] * s, y, centre[1] + side[1] * s];
    quad(
        [
            corner(-half, 0.0),
            corner(half, 0.0),
            corner(half, -height),
            corner(-half, -height),
        ],
        [-d[0], 0.0, -d[1]],
        [side[0], 0.0, side[1]],
    )
}

/// A wall standing on open ground, seen from the front: SSAO, HBAO and
/// HBAO Ultra each occlude the crease at the wall's foot, darken it by no
/// more than its ambient share of the light, leave the open ground beyond
/// the largest radius unoccluded and unchanged, and repeat; AO off draws
/// the frame without occlusion.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn ao_modes_darken_a_crease_not_open_ground() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let size = [480u32, 320];
    let (camera, mut env) = bare_camera(size);
    env.sun.dir = [-0.3, -0.9, 0.3];
    let snapshot = bare(&camera, &env);
    let uniforms = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32));
    let eye = uniforms.eye;
    let len = (eye[0] * eye[0] + eye[2] * eye[2]).sqrt();
    let d = [-eye[0] / len, -eye[2] / len];
    let (cx, cz) = (26.0 * 512.0, 26.0 * 512.0);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    let wall_at = 300.0;
    let models = vec![
        (flat_quad(cx, cz, 4000.0, 0.0), identity),
        (
            wall([cx + d[0] * wall_at, cz + d[1] * wall_at], d, 3000.0, 900.0),
            identity,
        ),
    ];
    let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
    let pixel = |along: f32| pixel_of(&view_proj, size, [d[0] * along, 0.0, d[1] * along]);
    let frame = |mode: AoMode, resolution: AoResolution| {
        let mut r = renderer(
            &device,
            &queue,
            4,
            ModernSettings {
                ao: mode,
                ao_resolution: resolution,
                ..ModernSettings::DEFAULT
            },
        );
        let mut draw = || {
            r.preparation.test_models = models.clone();
            render(&device, &queue, &mut r, &snapshot, size)
        };
        let (first, again) = (draw(), draw());
        let ao = r
            .history
            .post
            .targets
            .as_ref()
            .map(|t| halves(&read_back(&device, &queue, &t.ao_tex, 2)));
        (first, again, ao)
    };
    let (off, _, _) = frame(AoMode::Off, AoResolution::Full);
    let ambient = f64::from(uniforms.sky_ambient[1]);
    let sun = f64::from(uniforms.sun_colour[1]) * f64::from(-uniforms.sun_dir[1]).max(0.0);
    let share = ambient / (ambient + sun);
    // The open ground beyond HBAO set B's 750 units from the wall's foot.
    let (crease, open) = (pixel(wall_at - 16.0), pixel(wall_at - 800.0));
    assert!(
        open.0 < size[0] as f32 && open.1 < size[1] as f32 && open.1 >= 0.0,
        "{open:?} off screen"
    );
    for resolution in [AoResolution::Full, AoResolution::Half] {
        let ao_size = resolution.size(size);
        let divisor = resolution.divisor() as f32;
        let at = |ao: &[f32], (x, y): (f32, f32)| {
            ao[(y / divisor) as usize * ao_size[0] as usize + (x / divisor) as usize]
        };
        for mode in [AoMode::Ssao, AoMode::Hbao, AoMode::HbaoUltra] {
            let (on, again, ao) = frame(mode, resolution);
            dump_frame(&format!("ao-{resolution:?}-{mode:?}"), size, &on.pixels);
            let ao = ao.expect("the occlusion map");
            assert_eq!(on.hdr, again.hdr, "{mode:?}: the frame does not repeat");
            assert_ne!(on.hdr, off.hdr, "{mode:?}: no occlusion drawn");
            let near = green(&on.hdr, size, crease) / green(&off.hdr, size, crease);
            let far = green(&on.hdr, size, open) / green(&off.hdr, size, open);
            eprintln!(
            "{mode:?}: occlusion crease {} open {}; the crease keeps {near:.4} of its light (ambient share {share:.4})",
            at(&ao, crease),
            at(&ao, open)
        );
            assert!(
                at(&ao, crease) < 0.97,
                "{mode:?}: the crease is not occluded"
            );
            assert!(
                near >= 1.0 - share - 1e-3,
                "{mode:?}: more than the ambient removed"
            );
            assert_eq!(at(&ao, open), 1.0, "{mode:?}: the open ground is occluded");
            assert!(
                (far - 1.0).abs() < 1e-3,
                "{mode:?}: the open ground changed: {far}"
            );
        }
    }
}

/// Half-size AO respects odd, inset scene viewports and clips, also when
/// the scene itself renders below the output resolution.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn half_resolution_ao_keeps_odd_viewports_and_render_scale_clipped() {
    const OUTPUT: [u32; 2] = [561, 375];
    const RECT: [i32; 4] = [17, 23, 481, 321];
    const CLIP: [i32; 4] = [29, 37, 497, 336];
    const GROUND_HALF_WIDTH: f32 = 4000.0;
    const WALL_OFFSET: f32 = 300.0;
    const WALL_HALF_WIDTH: f32 = 3000.0;
    const WALL_HEIGHT: f32 = 900.0;
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let (camera, mut env) = bare_camera([RECT[2] as u32, RECT[3] as u32]);
    env.sun.dir = [-0.3, -0.9, 0.3];
    let snapshot = bare(&camera, &env);
    let u = frame_uniforms(&snapshot, (RECT[2], RECT[3]));
    let len = (u.eye[0] * u.eye[0] + u.eye[2] * u.eye[2]).sqrt();
    let dir = [-u.eye[0] / len, -u.eye[2] / len];
    let (cx, cz) = (camera.target[0] as f32, camera.target[2] as f32);
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    let models = vec![
        (flat_quad(cx, cz, GROUND_HALF_WIDTH, 0.0), identity),
        (
            wall(
                [cx + dir[0] * WALL_OFFSET, cz + dir[1] * WALL_OFFSET],
                dir,
                WALL_HALF_WIDTH,
                WALL_HEIGHT,
            ),
            identity,
        ),
    ];
    for percent in [100, 75] {
        let mut r = renderer(
            &device,
            &queue,
            1,
            ModernSettings {
                ao_resolution: AoResolution::Half,
                render_scale: crate::settings::RenderScale::percent(percent),
                ..ModernSettings::DEFAULT
            },
        );
        let pixels = super::scale::framed(
            &device,
            &queue,
            &mut r,
            &snapshot,
            &models,
            OUTPUT,
            (RECT, CLIP),
        );
        let targets = r.history.post.targets.as_ref().expect("occlusion targets");
        let ao = halves(&read_back(&device, &queue, &targets.ao_tex, 2));
        assert!(ao.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        assert!(ao.iter().any(|v| *v < 0.95), "visible crease is occluded");
        let scene = r
            .frame_resources
            .targets
            .as_ref()
            .expect("scene targets")
            .size;
        let expected = AoResolution::Half.size(scene);
        assert_eq!([targets.ao_tex.width(), targets.ao_tex.height()], expected);
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            let (x, y) = (
                (index % OUTPUT[0] as usize) as i32,
                (index / OUTPUT[0] as usize) as i32,
            );
            if x < CLIP[0] || y < CLIP[1] || x >= CLIP[2] || y >= CLIP[3] {
                assert_eq!(pixel, MAGENTA, "outside clip at {x},{y}, scale {percent}");
            }
        }
    }
}
