//! Water: the planar reflection is the mirror image (and culling it to its
//! frustum changes nothing), and the body fades with depth to its sunlit
//! opaque colour.
use super::*;

/// The river view rendered by a one-sample renderer with the water's
/// debug output `debug` (0: the frame), under `look`: the renderer and the
/// HDR texels.
fn water_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    snapshot: &SceneSnapshot<'_>,
    debug: u32,
    look: Option<crate::lighting::look::Look>,
    size: [u32; 2],
) -> (ModernRenderer, Vec<f32>) {
    // The debug outputs are read from the HDR target: nothing drawn over
    // the water (the volumetrics) or folded into it (the scattering).
    let settings = ModernSettings {
        volumetrics: debug == 0,
        ..ModernSettings::DEFAULT
    };
    let mut r = renderer(device, queue, 1, settings);
    r.water.debug = debug;
    r.atmos.test_clear_air = debug != 0;
    if let Some(look) = look {
        r.look = look;
    }
    let hdr = render(device, queue, &mut r, snapshot, size).hdr;
    (r, hdr)
}

/// A pixel's view ray direction (camera-local).
fn view_ray(inv: glam::Mat4, eye: glam::Vec3, px: [f32; 2], size: [u32; 2]) -> glam::Vec3 {
    let ndc = [
        px[0] / size[0] as f32 * 2.0 - 1.0,
        1.0 - px[1] / size[1] as f32 * 2.0,
    ];
    let far = inv.project_point3(glam::Vec3::new(ndc[0], ndc[1], 1.0));
    (far - eye).normalize()
}

/// The planar reflection is the mirror image. The reflection pass writes
/// each fragment's camera-local position (debug output
/// `DEBUG_REFLECTION_POSITIONS`) and the water shows it at its own pixel,
/// so for every covered water pixel the reflected point `w` is known:
///
/// 1. its mirror about the plane projects back onto the pixel: within 1.5
///    pixels at the 90th percentile;
/// 2. the physical reflected ray (the view ray reflected where it meets
///    the plane) points at `w`;
/// 3. the terrain does not hide `w`: marching the reflected ray over the
///    level-0 height grid, nothing rises above the ray before `w`; and where
///    the reflection shows the sky the ray does not run into terrain.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn water_reflection_is_the_mirror_image() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    for (pitch, zoom) in [(1400.0, 1.2), (2400.0, 1.6)] {
        let offline = river_scene(&pack, size, pitch, zoom);
        let snapshot = offline.snapshot(&pack);
        let (_, mask) = water_frame(&device, &queue, &snapshot, 1, None, size);
        let (r, pos) = water_frame(
            &device,
            &queue,
            &snapshot,
            crate::water_body::DEBUG_REFLECTION_POSITIONS,
            None,
            size,
        );
        let plane = r.water_stats().plane.expect("a reflection plane");
        let uniforms = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32));
        let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
        let inv = view_proj.inverse();
        let eye = glam::Vec3::from_slice(&uniforms.eye[..3]);
        let origin = glam::Vec3::new(
            (snapshot.camera.target[0] - snapshot.floor_base[0] * 512) as f32,
            snapshot.camera.target[1] as f32,
            (snapshot.camera.target[2] - snapshot.floor_base[1] * 512) as f32,
        );
        let heights = &snapshot.floors[0].as_ref().expect("level 0").heights;
        // The terrain's classic y (down) under camera-local (x, z).
        let ground = |p: glam::Vec3| {
            let (x, z) = (p.x + origin.x, p.z + origin.z);
            let (gx, gz) = (
                heights.tiles_x as f32 * 512.0,
                heights.tiles_z as f32 * 512.0,
            );
            (x >= 0.0 && z >= 0.0 && x < gx && z < gz)
                .then(|| heights.get_fine_height(x as i32, z as i32) as f32 - origin.y)
        };
        let (mut errors, mut aims) = (Vec::new(), Vec::new());
        let (mut covered, mut hidden, mut sky, mut sky_blocked) = (0, 0, 0, 0);
        for i in 0..(size[0] * size[1]) as usize {
            let m = &mask[i * 4..i * 4 + 4];
            if m[1] != 1.0 || m[3] != 1.0 {
                continue;
            }
            let px = [
                (i % size[0] as usize) as f32 + 0.5,
                (i / size[0] as usize) as f32 + 0.5,
            ];
            let d = view_ray(inv, eye, px, size);
            let s = eye + d * ((plane - eye.y) / d.y);
            let reflected = glam::Vec3::new(d.x, -d.y, d.z);
            let p = &pos[i * 4..i * 4 + 4];
            // How far the reflected ray runs before the terrain hides it
            // (classic y down), in 16-unit steps, 32 units of slack for the
            // floor's triangulation against the bilinear grid.
            let blocked_at = |limit: f32| {
                let mut t = 64.0;
                while t < limit {
                    let q = s + reflected * t;
                    if ground(q).is_some_and(|g| q.y > g + 32.0) {
                        return Some(t);
                    }
                    t += 16.0;
                }
                None
            };
            if p[3] == 0.0 {
                sky += 1;
                sky_blocked += usize::from(blocked_at(12_000.0).is_some());
                continue;
            }
            if p[3] != 1.0 {
                continue;
            }
            covered += 1;
            let w = glam::Vec3::new(p[0], p[1], p[2]);
            let (x, y) = pixel_of(&view_proj, size, [w.x, 2.0 * plane - w.y, w.z]);
            errors.push((x - px[0]).hypot(y - px[1]));
            aims.push(
                (w - s)
                    .normalize()
                    .dot(reflected)
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees(),
            );
            hidden += usize::from(blocked_at((w - s).length() - 64.0).is_some());
        }
        let pct = |v: &mut Vec<f32>, q: f32| {
            v.sort_by(f32::total_cmp);
            v[((v.len() - 1) as f32 * q) as usize]
        };
        let (e90, a90) = (pct(&mut errors, 0.9), pct(&mut aims, 0.9));
        eprintln!(
            "pitch {pitch}: plane {plane}, {covered} covered, {sky} sky; mirror error p90 {e90:.2} px; aim p90 {a90:.3} deg; {hidden} hidden, {sky_blocked} sky rays blocked"
        );
        assert!(covered > 500, "{covered}");
        assert!(e90 < 1.5, "mirror position p90 {e90} px");
        assert!(a90 < 1.0, "aim p90 {a90} deg");
        assert!(hidden * 100 <= covered, "{hidden} of {covered} hidden");
        assert!(
            sky_blocked * 100 <= sky.max(1),
            "{sky_blocked} of {sky} sky pixels"
        );
    }
}

/// The water body is the extinction towards an opaque colour lit by the
/// sun's diffuse alone (the water extinction: no ambient, no
/// point lights): with the sun's intensity at 0 the scene behind the
/// surface fades to black with the depth. Over the river's water pixels
/// binned by the shading depth (64-unit bins), each deeper bin is no
/// brighter than the one above and the deepest is well below the
/// shallowest.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn water_body_fades_to_its_sunlit_opaque_colour() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    let offline = river_scene(&pack, size, 1400.0, 1.2);
    let snapshot = offline.snapshot(&pack);
    let dark = crate::lighting::look::Look {
        sun: 0.0,
        ..crate::lighting::look::Look::CALIBRATED
    };
    // Debug outputs: 1 the water mask (green 1), 4 the body, 6 the shading
    // depth (red, depth / 1024).
    let out = |debug| water_frame(&device, &queue, &snapshot, debug, Some(dark), size).1;
    let (mask, body, depth) = (out(1), out(4), out(6));
    let mut bins = [(0.0_f64, 0usize); 6];
    for i in 0..(size[0] * size[1]) as usize {
        if mask[i * 4 + 1] != 1.0 || mask[i * 4 + 3] != 1.0 {
            continue;
        }
        let bin = ((depth[i * 4] * 1024.0 / 64.0) as usize).min(5);
        bins[bin].0 += f64::from(luma(&body[i * 4..]));
        bins[bin].1 += 1;
    }
    let bins = bins.map(|(sum, n)| (sum / n.max(1) as f64, n));
    eprintln!("sun 0, body luminance by depth bin (mean, pixels): {bins:?}");
    assert!(bins[0].1 > 200 && bins[5].1 > 200, "{bins:?}");
    for k in 1..6 {
        assert!(bins[k].0 <= bins[k - 1].0 * 1.02, "bin {k}: {bins:?}");
    }
    assert!(bins[5].0 < bins[0].0 * 0.7, "{bins:?}");
}

/// The planar reflection draws only the draws its reflected frustum can
/// show (`frame::gpu::water_reflection`): over the river from two pitches
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
                r.water.test_no_reflection_cull = true;
                let all = settled(&device, &queue, &mut r, &snapshot, size);
                let every = r.water_stats();
                r.water.test_no_reflection_cull = false;
                let culled = render(&device, &queue, &mut r, &snapshot, size);
                let stats = r.water_stats();
                let far_packets = r
                    .draws
                    .iter()
                    .enumerate()
                    .filter(|(_, draw)| matches!(draw.geometry, crate::frame::Geometry::Far { .. }))
                    .count();
                let far_kept = r
                    .water
                    .reflected
                    .iter()
                    .filter(|index| {
                        matches!(
                            r.draws[**index as usize].geometry,
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
