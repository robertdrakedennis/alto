//! Point-light shadows: the faces a still scene keeps are the faces of
//! redrawing them, and a moving caster clears the faces it leaves.
use super::*;
use crate::lighting::point_lights::{PointLightGrid, StaticPointLight, TILE};
use crate::shadows::{Quality, Settings};

/// A synthetic night scene: a wide grey ground, `lights` point lights over it
/// and above each a square occluder, seen by the bare camera. Scene-local
/// coordinates; `(cx, cz)` is the camera's target.
struct Night {
    camera: crate::camera::SceneCamera,
    env: rs910_scene::env::EnvFrame,
    lights: Vec<StaticPointLight>,
    models: Vec<(crate::models::mesh::ModelStreams, [f32; 16])>,
}

/// The tile grid of `lights` at level 0 over the scene around the camera
/// (four lights a tile at most, as the scene's own table).
fn grid_of(lights: &[StaticPointLight]) -> PointLightGrid {
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
    PointLightGrid {
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
            lights.push(StaticPointLight {
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
