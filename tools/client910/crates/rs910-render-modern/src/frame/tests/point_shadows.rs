//! Point-light shadows: which lights cast, where their faces live in the
//! atlas, what they shade, that frames repeat, and that the faces a still
//! scene keeps are the faces of redrawing them.
use super::*;
use crate::lighting::point_lights::{Grid, Light, TILE};
use crate::shadows::point::{candidates, AtlasLayout, FaceRect, LightInput, RELEASE_FRAMES};
use crate::shadows::presets::point_preset;
use crate::shadows::{Quality, Settings};

/// A synthetic night scene: a wide grey ground, `lights` point lights over it
/// and above each a square occluder, seen by the bare camera. Scene-local
/// coordinates; `(cx, cz)` is the camera's target.
struct Night {
    camera: crate::camera::SceneCamera,
    env: rs910_scene::env::EnvFrame,
    lights: Vec<Light>,
    models: Vec<(crate::models::mesh::ModelStreams, [f32; 16])>,
    size: [u32; 2],
}

/// The tile grid of `lights` at level 0 over the scene around the camera
/// (four lights a tile at most, as the scene's own table).
fn grid_of(lights: &[Light]) -> Grid {
    let (nx, nz) = (64, 64);
    let mut entries = vec![[0_u16; 4]; nx * nz];
    for (id, l) in lights.iter().enumerate() {
        let span = (l.radius / TILE).ceil() as i32;
        let (tx, tz) = ((l.pos[0] / TILE) as i32, (l.pos[2] / TILE) as i32);
        for z in (tz - span).max(0)..=(tz + span).min(nz as i32 - 1) {
            for x in (tx - span).max(0)..=(tx + span).min(nx as i32 - 1) {
                let tile = &mut entries[z as usize * nx + x as usize];
                if let Some(slot) = tile.iter_mut().find(|v| **v == 0) {
                    *slot = id as u16 + 1;
                }
            }
        }
    }
    Grid {
        levels: 1,
        nx,
        nz,
        entries,
    }
}

impl Night {
    /// `count` lights of radius `radius` in a line away from the camera
    /// (`spacing` apart along the view, alternating sides), each over an
    /// occluder. The camera looks across the ground from above.
    fn new(size: [u32; 2], count: usize, radius: f32, spacing: f32) -> Self {
        let (mut camera, mut env) = bare_camera(size);
        camera.pitch = 2400.0;
        camera.distance_scale = 0.9;
        env.sun.dir = [0.0, -1.0, 0.0];
        let target = camera.target;
        let (cx, cz) = (target[0] as f32, target[2] as f32);
        // Models stand at their matrix's translation (the casters' position).
        let at = |x: f32, z: f32| {
            glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, z)).to_cols_array()
        };
        let mut models = vec![(flat_quad(0.0, 0.0, 12_000.0, 0.0), at(cx, cz))];
        let mut lights = Vec::new();
        for k in 0..count {
            let side = if k % 2 == 0 { -1.0 } else { 1.0 };
            let (x, z) = (cx + side * 500.0, cz - 200.0 + k as f32 * spacing);
            lights.push(Light {
                pos: [x, -520.0, z],
                radius,
                colour: [1.0, 0.8, 0.5],
                intensity: 1.0,
                casts_shadows: true,
            });
            models.push((flat_quad(0.0, 0.0, 180.0, -260.0), at(x, z)));
        }
        Self {
            camera,
            env,
            lights,
            models,
            size,
        }
    }

    fn snapshot(&self) -> SceneSnapshot<'_> {
        bare(&self.camera, &self.env)
    }

    /// A renderer of this scene (night: the sun's light at zero) at quality
    /// `quality` (`None`: no shadows).
    fn renderer(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: Option<Quality>,
    ) -> ModernRenderer {
        let settings = ModernSettings {
            volumetrics: false,
            ..ModernSettings::DEFAULT
        };
        let mut r = renderer(device, queue, 4, settings);
        r.frame_resources.atmos.test_clear_air = true;
        r.look.sun = 0.0;
        r.look.ambient *= 0.15;
        r.look.sky = 0.0;
        r.set_shadow_settings(match quality {
            None => Settings::from_options(0, 0, 0),
            Some(q) => Settings::from_options(2, q as i32, 1),
        });
        r.preparation.test_models = self.models.clone();
        r.preparation.test_lights = Some((self.lights.clone(), grid_of(&self.lights)));
        r
    }
}

/// The ground's share of light (over the unlit ground) at the scene-local
/// ground point beside light `k` (`side` of the camera's line, 300 units
/// towards the middle), in `hdr`.
fn ground_point(night: &Night, k: usize) -> [f32; 3] {
    let l = &night.lights[k];
    let side = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
    [l.pos[0] + side * 300.0, 0.0, l.pos[2]]
}

/// The night scene's frame at `quality` with `lights` (the scene's own or
/// none), settled.
fn night_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    night: &Night,
    quality: Option<Quality>,
    lit: bool,
) -> (Frame, ModernRenderer) {
    let snapshot = night.snapshot();
    let mut r = night.renderer(device, queue, quality);
    if !lit {
        r.preparation.test_lights = None;
    }
    let f = settled(device, queue, &mut r, &snapshot, night.size);
    (f, r)
}

/// The lights that cast are the pool's best, at most the quality's count; a
/// point beside a casting light's occluder is shaded by that light's shadow,
/// one beside any other light is exactly what it is without point shadows.
/// The atlas's depth texels (row-major, `width` x `height`).
fn atlas_depths(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &ModernRenderer,
) -> (Vec<f32>, u32, u32) {
    let texture = r.scene_resources.lights.shadow_maps.view.texture();
    let (w, h) = (texture.width(), texture.height());
    let bytes = read_back(device, queue, texture, 4);
    let depths = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    (depths, w, h)
}

/// `(min, max)` of the depths in a face rectangle.
fn rect_range(depths: &[f32], width: u32, r: FaceRect) -> (f32, f32) {
    let mut range = (f32::MAX, f32::MIN);
    for y in r.y..r.y + r.size {
        for x in r.x..r.x + r.size {
            let d = depths[(y * width + x) as usize];
            range = (range.0.min(d), range.1.max(d));
        }
    }
    range
}

#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_best_lights_cast_and_the_rest_do_not() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let mut night = Night::new([960, 640], 6, 900.0, 450.0);
    night.camera.distance_scale = 0.4;
    let snapshot = night.snapshot();
    let uniforms = frame_uniforms(&snapshot, (night.size[0] as i32, night.size[1] as i32));
    let view_proj = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
    let target = night.camera.target;
    let local = |p: [f32; 3]| [p[0] - target[0] as f32, p[1], p[2] - target[2] as f32];
    let pixel = |k: usize| pixel_of(&view_proj, night.size, local(ground_point(&night, k)));
    let (ambient, _) = night_frame(&device, &queue, &night, Some(Quality::Ultra), false);
    for quality in [Quality::Low, Quality::High, Quality::Ultra] {
        let preset = point_preset(quality);
        let (on, r) = night_frame(&device, &queue, &night, Some(quality), true);
        // The frame without point shadows: the same scene, the tests' switch.
        let snapshot = night.snapshot();
        let mut off = night.renderer(&device, &queue, Some(quality));
        off.history.shadow.point.test_off = true;
        let off = settled(&device, &queue, &mut off, &snapshot, night.size);
        // Which lights: as the selection rules give them for this camera.
        let frustum = r.history.shadow.point.frustum.clone().expect("a frustum");
        let inputs: Vec<LightInput> = night
            .lights
            .iter()
            .map(|l| LightInput {
                pos: local(l.pos),
                radius: l.radius,
                intensity: l.intensity,
                casts: true,
            })
            .collect();
        let best: Vec<usize> = candidates(&inputs, &frustum, preset.max_view_distance)
            .iter()
            .take(preset.lights)
            .map(|c| c.light)
            .collect();
        let mut active: Vec<usize> = r
            .history
            .shadow
            .point
            .active
            .iter()
            .map(|a| a.light)
            .collect();
        active.sort_unstable();
        let mut want = best.clone();
        want.sort_unstable();
        assert_eq!(
            active, want,
            "{quality:?}: the pool's first fill is the best"
        );
        assert_eq!(r.stats.point_shadow_lights, preset.lights);
        // The atlas is the layout's.
        let layout = AtlasLayout::new(preset.face, preset.levels, preset.lights);
        assert_eq!(
            r.scene_resources.lights.shadow_maps.size,
            (layout.width, layout.height),
            "{quality:?}"
        );
        for k in 0..night.lights.len() {
            let p = pixel(k);
            assert!(
                p.0 > 0.0 && p.1 > 0.0 && p.0 < 960.0 && p.1 < 640.0,
                "light {k}'s ground point is in view: {p:?}"
            );
            let amb = green(&ambient.hdr, night.size, p);
            let share =
                (green(&on.hdr, night.size, p) - amb) / (green(&off.hdr, night.size, p) - amb);
            eprintln!(
                "{quality:?}: light {k} casts {}: share {share:.3} at {p:?} (amb {amb:.4} on {:.4} off {:.4})",
                active.contains(&k),
                green(&on.hdr, night.size, p),
                green(&off.hdr, night.size, p)
            );
            if active.contains(&k) {
                assert!(
                    share < 0.1,
                    "{quality:?} light {k}: {share:.3} of its light"
                );
            } else {
                assert!(
                    (share - 1.0).abs() < 0.01,
                    "{quality:?} light {k} has no shadow: {share:.3}"
                );
            }
        }
    }
}

/// A slotted light keeps its slot when a better one appears, and gives it up
/// only after it has left the candidate set for a while.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn slots_are_kept_until_the_light_leaves() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let mut night = Night::new([960, 640], 6, 900.0, 450.0);
    night.camera.distance_scale = 0.4;
    let snapshot = night.snapshot();
    let mut r = night.renderer(&device, &queue, Some(Quality::Low));
    let held = |r: &ModernRenderer| -> Vec<usize> {
        let mut v: Vec<usize> = r
            .history
            .shadow
            .point
            .active
            .iter()
            .map(|a| a.light)
            .collect();
        v.sort_unstable();
        v
    };
    render(&device, &queue, &mut r, &snapshot, night.size);
    let first = held(&r);
    assert_eq!(first.len(), 2, "two slots at LOW");
    // Two far lights become far brighter: they outscore the held ones and
    // still get no slot.
    let others: Vec<usize> = (0..6).filter(|k| !first.contains(k)).collect();
    for &k in &others[2..] {
        r.preparation.test_lights.as_mut().unwrap().0[k].intensity = 1.0e6;
    }
    for _ in 0..3 {
        render(&device, &queue, &mut r, &snapshot, night.size);
        assert_eq!(held(&r), first, "no eviction by a higher score");
    }
    // One held light goes out: its slot is held for a few frames, then the
    // best waiting light takes it; the other held light never moved.
    let gone = first[0];
    r.preparation.test_lights.as_mut().unwrap().0[gone].intensity = 0.0;
    for _ in 0..RELEASE_FRAMES {
        render(&device, &queue, &mut r, &snapshot, night.size);
        assert_eq!(held(&r), vec![first[1]], "the slot waits");
    }
    render(&device, &queue, &mut r, &snapshot, night.size);
    let after = held(&r);
    assert_eq!(after.len(), 2, "the freed slot is taken: {after:?}");
    assert!(after.contains(&first[1]) && !after.contains(&gone));
    assert!(
        others[2..].iter().any(|k| after.contains(k)),
        "the highest score of those waiting: {after:?}"
    );
}

/// Each slotted light's faces are in its slot's block at its level: the
/// face that looks down holds the occluder's depth (squared distance over
/// squared radius), the face that looks up nothing, the levels it is not at
/// and the slots nobody holds stay cleared.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn faces_land_in_their_atlas_rectangles() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let mut night = Night::new([960, 640], 6, 900.0, 450.0);
    night.camera.distance_scale = 0.3;
    for quality in [Quality::Low, Quality::Ultra] {
        let preset = point_preset(quality);
        let (_, r) = night_frame(&device, &queue, &night, Some(quality), true);
        let (depths, w, _) = atlas_depths(&device, &queue, &r);
        let layout = AtlasLayout::new(preset.face, preset.levels, preset.lights);
        assert_eq!(r.history.shadow.point.active.len(), preset.lights);
        let mut levels = Vec::new();
        for a in &r.history.shadow.point.active {
            levels.push(a.level);
            for level in 0..preset.levels {
                for face in 0..6 {
                    let (lo, hi) = rect_range(&depths, w, layout.rect(a.slot, level, face));
                    let here = (a.level == level, face);
                    match here {
                        (true, 2) => {
                            // The occluder 260 below a light of radius 900.
                            let d = (260.0_f32 / 900.0).powi(2);
                            assert!((lo - d).abs() < 0.01, "{quality:?} slot {} down face: {lo}", a.slot);
                        }
                        (true, 3) => assert_eq!((lo, hi), (1.0, 1.0), "{quality:?}: nothing above the light"),
                        (true, _) => assert!(lo < 1.0, "{quality:?}: a side face sees the ground"),
                        (false, _) => assert_eq!(
                            (lo, hi),
                            (1.0, 1.0),
                            "{quality:?} slot {} level {level} face {face} is not this light's level",
                            a.slot
                        ),
                    }
                }
            }
        }
        assert!(levels.iter().all(|&l| l < preset.levels));
        // The rectangles of unused slots (none at these qualities when all
        // slots are held) and everything outside every block is cleared.
        let held: Vec<usize> = r
            .history
            .shadow
            .point
            .active
            .iter()
            .map(|a| a.slot)
            .collect();
        for slot in (0..preset.lights).filter(|s| !held.contains(s)) {
            let [x, y, bw, bh] = layout.block(slot);
            let (lo, hi) = rect_range(
                &depths,
                w,
                FaceRect {
                    x,
                    y,
                    size: bw.min(bh),
                },
            );
            assert_eq!((lo, hi), (1.0, 1.0));
        }
        eprintln!(
            "{quality:?}: atlas {}x{} levels {levels:?}",
            layout.width, layout.height
        );
    }
}

/// Frames repeat: the same scene drawn again, and by a second renderer,
/// gives the same pixels (the casters here are redrawn every frame).
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn point_shadow_frames_repeat() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let mut night = Night::new([960, 640], 6, 900.0, 450.0);
    night.camera.distance_scale = 0.4;
    let snapshot = night.snapshot();
    let mut r = night.renderer(&device, &queue, Some(Quality::Ultra));
    let first = settled(&device, &queue, &mut r, &snapshot, night.size).pixels;
    let again = render(&device, &queue, &mut r, &snapshot, night.size).pixels;
    let (other, _) = night_frame(&device, &queue, &night, Some(Quality::Ultra), true);
    assert_eq!(Noise::of(&first, &again), Noise::default());
    assert_eq!(Noise::of(&first, &other.pixels), Noise::default());
    let (off, _) = {
        let mut r = night.renderer(&device, &queue, Some(Quality::Ultra));
        r.history.shadow.point.test_off = true;
        (settled(&device, &queue, &mut r, &snapshot, night.size), ())
    };
    assert_ne!(
        Noise::of(&first, &off.pixels),
        Noise::default(),
        "the shadows show"
    );
}

/// Lumbridge at night by its east lamps: the real scene's lights shade their surroundings
/// (the frame differs from the frame without point shadows), a still scene
/// keeps its faces (a later frame draws none) and the kept faces are the
/// faces of drawing them from scratch.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn a_still_night_scene_reuses_faces_that_redraw_identically() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [800, 500];
    // The lamps east of the castle, close in.
    let offline = OfflineScene::with_camera(&pack, (3227, 3225), (800, 500), |c| {
        c.pitch = 1700.0;
        c.distance_scale = 0.35;
    });
    let snapshot = offline.snapshot(&pack);
    let night = |r: &mut ModernRenderer| {
        r.look.sun = 0.0;
        r.look.ambient *= 0.2;
        r.set_shadow_settings(Settings::from_options(2, 3, 1));
    };
    let mut r = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    night(&mut r);
    settled(&device, &queue, &mut r, &snapshot, size);
    let kept = render(&device, &queue, &mut r, &snapshot, size);
    assert!(
        !r.history.shadow.point.active.is_empty(),
        "lights cast in this view"
    );
    assert_eq!(r.stats.point_shadow_draws, 0, "a still scene draws no face");
    r.history.shadow.point.forget_faces();
    let redrawn = render(&device, &queue, &mut r, &snapshot, size);
    assert!(r.stats.point_shadow_draws > 0, "every visible face redrawn");
    assert_eq!(Noise::of(&kept.pixels, &redrawn.pixels), Noise::default());
    // Preparing every off-screen static caster for the faces (not only those
    // within a light's reach) draws the same frame.
    let mut all = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    night(&mut all);
    all.history.shadow.point.test_all_casters = true;
    let everything = settled(&device, &queue, &mut all, &snapshot, size);
    // (Two renderers: within the repeat noise of a process.)
    let spread = Noise::of(&kept.pixels, &everything.pixels);
    assert!(
        spread.is_repeat_noise(size) && spread.largest <= 1,
        "{spread:?}"
    );
    let mut off = renderer(&device, &queue, 4, ModernSettings::DEFAULT);
    night(&mut off);
    off.history.shadow.point.test_off = true;
    let without = settled(&device, &queue, &mut off, &snapshot, size);
    let differ = Noise::of(&kept.pixels, &without.pixels);
    eprintln!(
        "{} lights cast, {} face draws redrawn; the frame differs from the one without in {differ:?}",
        r.history.shadow.point.active.len(),
        r.stats.point_shadow_draws
    );
    assert!(differ.values > 5000, "point shadows change the frame");
}

/// The shadow fades to lit towards the quality's maximum view distance: at
/// LOW (2x2 box, linear fade from 8500 to 10000) the ground beside a
/// casting light is fully shadowed before the fade, half way through it
/// about half lit, and unshadowed past 10000; the same ground is fully
/// shadowed at ULTRA (maximum 25000) at every one of those distances.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_shadow_fades_to_lit_by_the_maximum_distance() {
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let size = [960, 640];
    let mut night = Night::new(size, 2, 900.0, 450.0);
    let point = ground_point(&night, 0);
    // The camera distance that puts the ground beside light 0 at view depth
    // `depth` (the depth grows with the zoom).
    let depth_at = |night: &Night| {
        let snapshot = night.snapshot();
        let u = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32));
        let view = glam::Mat4::from_cols_array_2d(&u.view);
        let t = night.camera.target;
        let local = glam::Vec3::new(point[0] - t[0] as f32, point[1], point[2] - t[2] as f32);
        (view * local.extend(1.0)).z
    };
    let scale_for = |night: &mut Night, depth: f32| {
        let (mut lo, mut hi) = (0.2_f32, 6.0_f32);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            night.camera.distance_scale = mid;
            if depth_at(night) < depth {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        night.camera.distance_scale = (lo + hi) / 2.0;
    };
    let share_at = |night: &Night, quality: Quality| {
        let snapshot = night.snapshot();
        let u = frame_uniforms(&snapshot, (size[0] as i32, size[1] as i32));
        let view_proj = glam::Mat4::from_cols_array_2d(&u.view_proj);
        let t = night.camera.target;
        let p = pixel_of(
            &view_proj,
            size,
            [point[0] - t[0] as f32, 0.0, point[2] - t[2] as f32],
        );
        assert!(
            p.0 > 0.0 && p.1 > 0.0 && p.0 < size[0] as f32 && p.1 < size[1] as f32,
            "{p:?}"
        );
        let (ambient, _) = night_frame(&device, &queue, night, Some(quality), false);
        let (on, r) = night_frame(&device, &queue, night, Some(quality), true);
        assert!(
            !r.history.shadow.point.active.is_empty(),
            "light 0 casts at this distance"
        );
        let mut off = night.renderer(&device, &queue, Some(quality));
        off.history.shadow.point.test_off = true;
        let off = settled(&device, &queue, &mut off, &snapshot, size);
        let amb = green(&ambient.hdr, size, p);
        (green(&on.hdr, size, p) - amb) / (green(&off.hdr, size, p) - amb)
    };
    let mut low = Vec::new();
    for depth in [7_500.0, 9_250.0, 10_400.0] {
        scale_for(&mut night, depth);
        let got = depth_at(&night);
        assert!((got - depth).abs() < 60.0, "{got} for {depth}");
        let (lo, ultra) = (
            share_at(&night, Quality::Low),
            share_at(&night, Quality::Ultra),
        );
        eprintln!("depth {got:.0}: LOW keeps {lo:.3} of the light, ULTRA {ultra:.3}");
        assert!(ultra < 0.02, "ULTRA shadows at {depth}: {ultra:.3}");
        low.push(lo);
    }
    assert!(low[0] < 0.02, "before the fade: {:.3}", low[0]);
    assert!((low[1] - 0.5).abs() < 0.15, "half way: {:.3}", low[1]);
    assert!(low[2] > 0.98, "past the maximum distance: {:.3}", low[2]);
}

/// Motion inside the old preparation margin but beyond the actual light
/// radius must not redraw faces. Entering casts, departure clears once.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn dynamic_caster_overlap_and_departure_clear_the_actual_faces() {
    const SIZE: [u32; 2] = [480, 320];
    const LIGHT_COUNT: usize = 1;
    const LIGHT_RADIUS: f32 = 900.0;
    const LIGHT_SPACING: f32 = 0.0;
    const OUTSIDE_OFFSET: f32 = LIGHT_RADIUS + 500.0;
    const SMALL_MOTION: f32 = 32.0;
    let _clock = fixed_clock();
    let (device, queue) = crate::test_support::require_gpu();
    let night = Night::new(SIZE, LIGHT_COUNT, LIGHT_RADIUS, LIGHT_SPACING);
    let snapshot = night.snapshot();
    let inside = night.models[LIGHT_COUNT].clone();
    let mut outside = inside.clone();
    outside.1[12] += OUTSIDE_OFFSET;
    let mut r = night.renderer(&device, &queue, Some(Quality::High));
    r.preparation.test_models = vec![outside.clone()];
    settled(&device, &queue, &mut r, &snapshot, SIZE);
    let (empty, _, _) = atlas_depths(&device, &queue, &r);
    assert!(empty.iter().all(|depth| *depth == 1.0));
    outside.1[12] += SMALL_MOTION;
    r.preparation.test_models = vec![outside];
    render(&device, &queue, &mut r, &snapshot, SIZE);
    assert_eq!(
        r.stats.point_shadow_draws, 0,
        "margin-only motion is irrelevant"
    );
    assert!(
        !r.encoding_inputs().point_shadows_record(),
        "cached empty faces remain untouched"
    );
    r.preparation.test_models = vec![inside];
    render(&device, &queue, &mut r, &snapshot, SIZE);
    let (cast, _, _) = atlas_depths(&device, &queue, &r);
    assert!(r.stats.point_shadow_draws > 0);
    assert!(
        cast.iter().any(|depth| *depth < 1.0),
        "actual shadow depth was written"
    );
    r.preparation.test_models.clear();
    render(&device, &queue, &mut r, &snapshot, SIZE);
    assert!(
        r.encoding_inputs().point_shadows_record(),
        "departure clears old depth once"
    );
    let (cleared, _, _) = atlas_depths(&device, &queue, &r);
    assert_eq!(cleared, empty, "departure leaves no stale caster depth");
    render(&device, &queue, &mut r, &snapshot, SIZE);
    assert!(
        !r.encoding_inputs().point_shadows_record(),
        "empty faces are retained after clearing"
    );
}
