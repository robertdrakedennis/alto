//! The sky: the cube sampled by view direction, fogged by elevation with the sun's glow and the
//! exposure offset, cross-fading between cubes, and the decor sprites over it.
use super::*;
use crate::skybox::SkyLayer;

/// The sky cubes the synthetic tests name.
const CUBE_A: crate::skybox::SkyboxKey = (1, 0, 0, 0);
const CUBE_B: crate::skybox::SkyboxKey = (2, 0, 0, 0);
const CUBE_C: crate::skybox::SkyboxKey = (3, 0, 0, 0);

/// The settings of the synthetic sky frames: only the sky and the fog draw.
const SKY_ONLY: ModernSettings = ModernSettings {
    far: None,
    volumetrics: false,
    ..ModernSettings::DEFAULT
};

/// One solid colour per face in the order `+x -x +y -y +z -z` of the cube space
/// (classic `+x`, `-x`, up, down, `+z`, `-z`).
const FACES: [[f32; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [1.0, 0.0, 1.0],
];

/// A solid cube.
fn solid(c: [f32; 3]) -> [[f32; 3]; 6] {
    [c; 6]
}

/// Look along `pitch` and `yaw` (14-bit angles: pitch 0 level, 12288 straight up; yaw 0 along
/// `+z`, 4096 along `-x`) from the origin.
fn look(c: &mut crate::camera::SceneCamera, pitch: i32, yaw: i32) {
    c.legacy = Some(crate::camera::LegacyFrame {
        eye: [0, 0, 0],
        pitch,
        yaw,
        roll: 0,
    });
}

/// The fog of a frame: the distance fog on (its colour, range 12,000 to 14,844).
fn with_fog(env: &mut rs910_scene::env::EnvFrame) {
    env.fog.range = Some((12_000.0, 14_844.0));
    env.fog.distance_colour = [0.6, 0.75, 0.95];
}

const SIZE: [u32; 2] = [320, 240];

/// A synthetic sky scene: the camera's pose, the environment, and a renderer holding the cubes
/// `cubes` (the environment's cube forced to `target`).
struct Sky {
    device: wgpu::Device,
    queue: wgpu::Queue,
    camera: crate::camera::SceneCamera,
    env: rs910_scene::env::EnvFrame,
    renderer: ModernRenderer,
}

impl Sky {
    fn new(pose: impl FnOnce(&mut crate::camera::SceneCamera), fog: bool) -> Self {
        let (device, queue) = crate::test_support::require_gpu();
        let (mut camera, mut env) = bare_camera(SIZE);
        pose(&mut camera);
        if fog {
            with_fog(&mut env);
        }
        let renderer = renderer(&device, &queue, 1, SKY_ONLY);
        Self {
            device,
            queue,
            camera,
            env,
            renderer,
        }
    }

    fn cube(&mut self, key: crate::skybox::SkyboxKey, faces: [[f32; 3]; 6]) {
        self.renderer
            .insert_test_cube(&self.device, &self.queue, key, faces);
    }

    /// The environment's cube (`None`: no sky), reached through a transition or not.
    fn environment(&mut self, target: Option<crate::skybox::SkyboxKey>, transition: bool) {
        self.renderer.sky_cubes.forced = Some((target, transition, None));
    }

    fn frame(&mut self) -> Frame {
        let snapshot = bare(&self.camera, &self.env);
        render(
            &self.device,
            &self.queue,
            &mut self.renderer,
            &snapshot,
            SIZE,
        )
    }

    /// The level the sky's HDR colours are divided by.
    fn level(&self) -> f32 {
        self.renderer.look.sky_exposure()
    }

    /// The fog colour (HDR) of the frame.
    fn fog_colour(&self) -> [f32; 3] {
        self.renderer.look.fog_colour(self.env.fog.distance_colour)
    }

    /// The frame's unit view direction at pixel `(x, y)` (classic space, y down).
    fn direction(&self, x: u32, y: u32) -> glam::Vec3 {
        let u = frame_uniforms(
            &bare(&self.camera, &self.env),
            (SIZE[0] as i32, SIZE[1] as i32),
        );
        let inv = glam::Mat4::from_cols_array_2d(&u.view_proj).inverse();
        let ndc = glam::Vec4::new(
            (x as f32 + 0.5) / SIZE[0] as f32 * 2.0 - 1.0,
            1.0 - (y as f32 + 0.5) / SIZE[1] as f32 * 2.0,
            0.5,
            1.0,
        );
        let p = inv * ndc;
        (p.truncate() / p.w - glam::Vec3::from([u.eye[0], u.eye[1], u.eye[2]])).normalize()
    }
}

/// The HDR colour at pixel `(x, y)`.
fn hdr_at(frame: &Frame, x: u32, y: u32) -> [f32; 3] {
    let i = ((y * SIZE[0] + x) * 4) as usize;
    [frame.hdr[i], frame.hdr[i + 1], frame.hdr[i + 2]]
}

fn close(a: [f32; 3], b: [f32; 3], what: &str) {
    for i in 0..3 {
        assert!(
            (a[i] - b[i]).abs() < 2e-3 * b[i].abs().max(1.0),
            "{what}: {a:?} against {b:?}"
        );
    }
}

/// The cube's face is shown in the direction the camera looks along, whatever way that is.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_cube_is_sampled_by_view_direction() {
    let _clock = fixed_clock();
    let centre = (SIZE[0] / 2, SIZE[1] / 2);
    // (pitch, yaw) and the face looked at.
    let poses = [
        (12288, 0, 2), // straight up
        (0, 0, 4),     // along +z
        (0, 4096, 1),  // along -x
        (0, 8192, 5),  // along -z
        (0, 12288, 0), // along +x
        (4096, 0, 3),  // straight down
    ];
    for (pitch, yaw, face) in poses {
        let mut sky = Sky::new(|c| look(c, pitch, yaw), false);
        sky.cube(CUBE_A, FACES);
        sky.environment(Some(CUBE_A), false);
        let level = sky.level();
        let frame = sky.frame();
        let want = FACES[face].map(|v| v / level);
        close(
            hdr_at(&frame, centre.0, centre.1),
            want,
            &format!("pitch {pitch} yaw {yaw}"),
        );
    }
}

/// A frame without a cube is the flat fog colour plus the exposure (the clear colour with the
/// distance fog off), a frame with a cube is fogged at and below the horizon and the cube's
/// colour above, by the angle fog.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn no_sky_is_the_fog_colour_and_a_sky_is_fogged_by_elevation() {
    let _clock = fixed_clock();
    let cube = [0.4, 0.2, 0.1];
    // No cube, fog on: the flat fog colour, every pixel; with the exposure offset added.
    let mut bare_sky = Sky::new(|c| look(c, 0, 0), true);
    bare_sky.environment(None, false);
    let f = bare_sky.fog_colour();
    let frame = bare_sky.frame();
    for (x, y) in [(10, 10), (160, 120), (300, 230)] {
        close(hdr_at(&frame, x, y), f, "flat fog colour");
    }
    bare_sky.renderer.set_sky_exposure_offset(0.25);
    let frame = bare_sky.frame();
    close(
        hdr_at(&frame, 160, 120),
        f.map(|v| v + 0.25),
        "flat + exposure",
    );
    // With the distance fog off the flat colour is the clear colour.
    let mut unfogged = Sky::new(|c| look(c, 0, 0), false);
    unfogged.environment(None, false);
    let clear = crate::post::tonemap::display_to_hdr(unfogged.env.clear);
    close(hdr_at(&unfogged.frame(), 160, 120), clear, "clear colour");

    // A cube, fog on: the horizon and below are the fog colour; above, the cube fogged by the
    // angle fog `mix(cube, fog, angle_fog(up))` (the sun far from view: no glow).
    let mut sky = Sky::new(|c| look(c, 0, 0), true);
    sky.env.sun.dir = [0.0, 1.0, 0.0];
    sky.cube(CUBE_A, solid(cube));
    sky.environment(Some(CUBE_A), false);
    let level = sky.level();
    let f = sky.fog_colour();
    let frame = sky.frame();
    let z = crate::lighting::environment_record::angle_fog_params(
        12_000.0,
        14_844.0,
        crate::lighting::environment_record::DEFAULT_ANGLE_FOG,
    );
    let mut seen_above = 0;
    for y in (2..SIZE[1]).step_by(17) {
        let d = sky.direction(160, y);
        let up = -d.y;
        let fog = crate::atmosphere::sky::angle_fog(up, z[0], z[1]);
        let c = cube.map(|v| v / level);
        let want: [f32; 3] = std::array::from_fn(|i| c[i] + (f[i] - c[i]) * fog);
        close(hdr_at(&frame, 160, y), want, &format!("up {up:.3}"));
        if up > 0.15 {
            seen_above += 1;
            assert!(fog < 0.5, "{fog}");
        }
        if up <= 0.0 {
            close(hdr_at(&frame, 160, y), f, "below the horizon");
        }
    }
    assert!(seen_above >= 2, "the frame shows the sky above the horizon");
}

/// The sun's glow is a lobe around the sun's direction, added to every channel on top of the
/// fogged cube: about 1 looking at the sun, none looking away from it.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_sun_glows_where_it_is() {
    let _clock = fixed_clock();
    // The sun 30 degrees above the horizon along +z: the camera looks at it, or away.
    let sun = glam::Vec3::new(0.0, -0.5, 0.866_025_4);
    let z = crate::lighting::environment_record::angle_fog_params(
        12_000.0,
        14_844.0,
        crate::lighting::environment_record::DEFAULT_ANGLE_FOG,
    );
    let mut glows = Vec::new();
    for yaw in [0, 8192] {
        let mut sky = Sky::new(|c| look(c, 15019, yaw), true);
        sky.env.sun.dir = sun.to_array();
        sky.cube(CUBE_A, solid([0.25, 0.25, 0.25]));
        sky.environment(Some(CUBE_A), false);
        let (level, f) = (sky.level(), sky.fog_colour());
        let frame = sky.frame();
        let d = sky.direction(SIZE[0] / 2, SIZE[1] / 2);
        let fog = crate::atmosphere::sky::angle_fog(-d.y, z[0], z[1]);
        let glow = crate::atmosphere::sky::sun_glow(d.dot(sun), -d.y, z[0]);
        let c = 0.25 / level;
        let want: [f32; 3] = std::array::from_fn(|i| c + (f[i] - c) * fog + glow);
        close(
            hdr_at(&frame, SIZE[0] / 2, SIZE[1] / 2),
            want,
            &format!("yaw {yaw}"),
        );
        glows.push(glow);
    }
    assert!(glows[0] > 0.95, "at the sun: {}", glows[0]);
    assert_eq!(glows[1], 0.0, "away from it");
}

/// The exposure offset is added to the sky's colour, last, and taking it away restores the
/// frame.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_exposure_offset_shifts_the_sky() {
    let _clock = fixed_clock();
    let mut sky = Sky::new(|c| look(c, 12288, 0), false);
    sky.cube(CUBE_A, solid([0.2, 0.3, 0.4]));
    sky.environment(Some(CUBE_A), false);
    let base = hdr_at(&sky.frame(), 160, 120);
    sky.renderer.set_sky_exposure_offset(0.3);
    let lit = hdr_at(&sky.frame(), 160, 120);
    sky.renderer.set_sky_exposure_offset(0.0);
    let back = hdr_at(&sky.frame(), 160, 120);
    close(lit, base.map(|v| v + 0.3), "exposure added");
    close(back, base, "exposure removed");
}

/// A capture face (the ambient capture's) shows the frame's sky: the face looking along an
/// axis has the colour the frame shows looking that way, plus the capture's exposure offset.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn a_capture_face_shows_the_frames_sky_at_the_capture_exposure() {
    let _clock = fixed_clock();
    let mut sky = Sky::new(|c| look(c, 12288, 0), false);
    sky.cube(CUBE_A, FACES);
    sky.environment(Some(CUBE_A), false);
    let level = sky.level();
    // The frame's own sky (looking up), so the cubes and the blend are bound.
    let main = hdr_at(&sky.frame(), SIZE[0] / 2, SIZE[1] / 2);
    let block = frame_uniforms(
        &bare(&sky.camera, &sky.env),
        (SIZE[0] as i32, SIZE[1] as i32),
    );
    let res = 16;
    for (face, name) in [(2, "up"), (4, "+z"), (0, "+x")] {
        let sky_face = sky.renderer.prepare_sky_face(
            &sky.device,
            &sky.queue,
            &block,
            crate::frame::gpu::sky_cube::FaceView {
                face,
                limits: (512.0, 65_536.0),
                res,
                exposure: 0.25,
            },
        );
        let target = |format, usage| {
            sky.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sky face test"),
                size: wgpu::Extent3d {
                    width: res,
                    height: res,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let colour = target(
            crate::frame::HDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(
            crate::frame::DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let mut encoder = sky.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour.create_view(&Default::default()),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth.create_view(&Default::default()),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            sky.renderer.draw_sky_face(&mut pass, &sky_face);
        }
        sky.queue.submit(Some(encoder.finish()));
        let texels = halves(&read_back(&sky.device, &sky.queue, &colour, 8));
        let centre = ((res / 2 * res + res / 2) * 4) as usize;
        let got = [texels[centre], texels[centre + 1], texels[centre + 2]];
        let want = FACES[face].map(|v| v / level + 0.25);
        close(got, want, &format!("face {name}"));
        if face == 2 {
            close(
                got,
                main.map(|v| v + 0.25),
                "the frame's sky looking up, plus the exposure",
            );
        }
    }
}

/// Cross-fade on the logic clock: the first environment's cube shows at once; later changes mix
/// the old and new cube over 5000 ms, linearly, and the new one alone after; a fade in the middle
/// of another turns back without a jump.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn a_settled_fade_mixes_the_cubes_over_the_default_time() {
    let t0: i64 = 1_700_000_000_000;
    let _clock = fixed_clock();
    let at = |ms: i64| crate::logic_clock::set_test_now(Some(t0 + ms));
    let mut sky = Sky::new(|c| look(c, 12288, 0), false);
    let (red, green, blue) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    sky.cube(CUBE_A, solid(red));
    sky.cube(CUBE_B, solid(green));
    sky.cube(CUBE_C, solid(blue));
    let level = sky.level();
    let shown = |sky: &mut Sky| hdr_at(&sky.frame(), 160, 120).map(|v| v * level);
    // The first environment: at once.
    at(0);
    sky.environment(Some(CUBE_A), false);
    close(shown(&mut sky), red, "first apply");
    // Two changes through a transition: the second fades from the first's cube to the third.
    at(1000);
    sky.environment(Some(CUBE_B), true);
    shown(&mut sky);
    at(7000);
    sky.environment(Some(CUBE_C), true);
    close(shown(&mut sky), green, "fade start");
    at(7000 + 2500);
    let half = shown(&mut sky);
    close(half, [0.0, 0.5, 0.5], "halfway");
    at(7000 + 1250);
    close(shown(&mut sky), [0.0, 0.75, 0.25], "a quarter");
    at(7000 + 5000);
    close(shown(&mut sky), blue, "fade end");
    at(7000 + 9000);
    close(shown(&mut sky), blue, "after the fade");
    // A fade running, then back to where it came from: the picture does not jump.
    at(20_000);
    sky.environment(Some(CUBE_B), true);
    shown(&mut sky);
    at(20_000 + 1000);
    let before = shown(&mut sky);
    sky.environment(Some(CUBE_C), true);
    let after = shown(&mut sky);
    close(after, before, "going back");
    // An override's explicit duration.
    at(40_000);
    sky.renderer.sky_cubes.forced = Some((Some(CUBE_A), true, Some(1000)));
    shown(&mut sky);
    at(40_000 + 500);
    let mix = shown(&mut sky);
    assert!(mix[0] > 0.2 && mix[0] < 0.8, "{mix:?}");
    at(40_000 + 1000);
    close(shown(&mut sky), red, "override ends");
}

/// Frames of the same state repeat exactly, frame to frame and from one renderer to another.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn sky_frames_repeat() {
    let _clock = fixed_clock();
    let frames: Vec<Vec<u8>> = (0..2)
        .map(|_| {
            let mut sky = Sky::new(|c| look(c, 14000, 500), true);
            sky.cube(CUBE_A, FACES);
            sky.cube(CUBE_B, solid([0.3, 0.6, 0.9]));
            sky.environment(Some(CUBE_A), false);
            sky.frame();
            sky.environment(Some(CUBE_B), true);
            sky.frame();
            let first = sky.frame();
            // A still sky binds its cubes once.
            let binds = sky.renderer.sky_cubes.binds;
            let again = sky.frame();
            assert_eq!(sky.renderer.sky_cubes.binds, binds, "no new bind group");
            assert_eq!(first.pixels, again.pixels, "frame to frame");
            first.pixels
        })
        .collect();
    assert_eq!(frames[0], frames[1], "renderer to renderer");
}

/// The real sky box of Lumbridge bakes into a cube: a frame looking up shows its clouds where
/// the same frame without a sky is the flat fog colour, and the box bakes once.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn the_lumbridge_sky_box_is_baked_into_the_cube() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    let offline = OfflineScene::with_camera(&pack, (3222, 3222), (640, 480), |c| {
        look(c, 14500, 0);
    });
    let sky = offline.sky(&pack).expect("a sky");
    let mut with = offline.snapshot(&pack);
    with.sky = sky.frame();
    let without = offline.snapshot(&pack);
    let settings = ModernSettings {
        volumetrics: false,
        ..ModernSettings::DEFAULT
    };
    let mut a = renderer(&device, &queue, 1, settings);
    let frame_with = settled(&device, &queue, &mut a, &with, size);
    let mut b = renderer(&device, &queue, 1, settings);
    let frame_without = settled(&device, &queue, &mut b, &without, size);
    dump_frame("sky-box-with", size, &frame_with.pixels);
    dump_frame("sky-box-without", size, &frame_without.pixels);
    // The top rows of the frame, above the scene.
    let top = |f: &Frame| -> Vec<[f32; 3]> {
        (0..size[0] * 40)
            .step_by(7)
            .map(|i| {
                [
                    f.hdr[i as usize * 4],
                    f.hdr[i as usize * 4 + 1],
                    f.hdr[i as usize * 4 + 2],
                ]
            })
            .collect()
    };
    let spread = |px: &[[f32; 3]]| {
        let lum: Vec<f32> = px.iter().map(|p| luma(p)).collect();
        let mean = lum.iter().sum::<f32>() / lum.len() as f32;
        lum.iter().map(|v| (v - mean).abs()).sum::<f32>() / lum.len() as f32
    };
    let (s_with, s_without) = (spread(&top(&frame_with)), spread(&top(&frame_without)));
    eprintln!("sky spread with {s_with:.4}, without {s_without:.4}");
    assert!(
        s_without < 1e-3,
        "no sky: the flat fog colour ({s_without})"
    );
    assert!(s_with > 0.02, "the sky's clouds ({s_with})");
    // Drawn again, the box is not baked again.
    let baked = a.sky_cubes.bakes;
    settled(&device, &queue, &mut a, &with, size);
    assert_eq!(baked, 1);
    assert_eq!(a.sky_cubes.bakes, baked);
}

/// Where the frame's first decor layer lands: its centre and size.
fn decor_rect(
    sky: &crate::sky_frame::SkyCache,
    projection: [f32; 16],
    size: [u32; 2],
) -> Option<(i32, i32, i32)> {
    sky.frame()?.layers.iter().find_map(|layer| match layer {
        SkyLayer::Decor {
            size: s,
            direction,
            pitch,
            yaw,
            roll,
            ..
        } => {
            let [x, y] = crate::skybox::decor_centre(
                *direction,
                (*pitch, *yaw, *roll),
                projection,
                (size[0] as i32, size[1] as i32),
            )?;
            Some((x as i32, y as i32, *s))
        }
        _ => None,
    })
}

/// A texture decor, attached to every sky box, is drawn: the frame differs
/// from the one without it inside the sprite's square and only there.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn sky_decor_is_drawn_where_its_direction_projects() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 480];
    // Looking up at the sky (a legacy pose below the orbit camera's clamp).
    let offline = OfflineScene::with_camera(&pack, (3230, 3222), (640, 480), |c| {
        let (pitch, yaw) = (300, 0);
        let eye = crate::camera::orbit_camera_with_profile(
            crate::camera::Orbit {
                target: [0, 0, 0],
                pitch,
                yaw,
                distance: crate::camera::orbit_distance(1077),
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
    });
    let materials = crate::texture::MaterialStore::load(&pack).unwrap();
    let texture = materials
        .get(2707)
        .and_then(|m| m.diffuse_texture)
        .expect("the sky material's texture") as i32;
    let projection = offline.camera.projection();
    // A direction that lands inside the frame: the view's own axes, turned
    // back into the sky's (a direction in eye space `e` is `e * R^T`).
    let view = crate::skybox::model_view(offline.camera.pitch_int(), offline.camera.yaw_int(), 0);
    let to_sky = |eye: [f32; 3]| -> [i32; 3] {
        std::array::from_fn(|j| {
            (0..3)
                .map(|i| eye[i] * view.e[j * 3 + i])
                .sum::<f32>()
                .round() as i32
        })
    };
    // Near the top of the view, where the scene leaves the sky open.
    let mut found = None;
    'search: for lift in (60..=300).step_by(20) {
        for sign in [-1.0, 1.0] {
            let direction = to_sky([0.0, sign * lift as f32, 240.0]);
            let decor = crate::skybox::SkyDecorType {
                kind: 0,
                texture,
                position: direction,
                size: 60,
                fixed: true,
                ..Default::default()
            };
            let sky = offline
                .sky_with_types(&pack, |types| types.attach_decors_to_all(vec![decor]))
                .expect("a sky");
            if let Some((x, y, s)) = decor_rect(&sky, projection, size) {
                if y > s && y < 60 && x > s && x + s < size[0] as i32 {
                    found = Some((sky, (x, y, s)));
                    break 'search;
                }
            }
        }
    }
    let (sky, (cx, cy, decor_size)) = found.expect("a decor direction inside the view");
    let plain = offline.sky(&pack).expect("a sky");
    let mut with = offline.snapshot(&pack);
    with.sky = sky.frame();
    let mut without = offline.snapshot(&pack);
    without.sky = plain.frame();
    let mut a = renderer(&device, &queue, 1, ModernSettings::DEFAULT);
    let frame_with = settled(&device, &queue, &mut a, &with, size);
    let mut b = renderer(&device, &queue, 1, ModernSettings::DEFAULT);
    let frame_without = settled(&device, &queue, &mut b, &without, size);
    dump_frame("sky-decor-with", size, &frame_with.pixels);
    dump_frame("sky-decor-without", size, &frame_without.pixels);
    let half = decor_size / 2;
    let (mut inside, mut outside) = (0, 0);
    for (i, (p, q)) in frame_with
        .pixels
        .chunks_exact(4)
        .zip(frame_without.pixels.chunks_exact(4))
        .enumerate()
    {
        if p.iter().zip(q).all(|(x, y)| x.abs_diff(*y) <= 8) {
            continue;
        }
        let (x, y) = ((i % size[0] as usize) as i32, (i / size[0] as usize) as i32);
        if (x - cx).abs() <= half + 1 && (y - cy).abs() <= half + 1 {
            inside += 1;
        } else {
            outside += 1;
        }
    }
    eprintln!("decor at ({cx}, {cy}) size {decor_size}: {inside} pixels changed inside, {outside} outside");
    assert!(inside > (decor_size * decor_size) as usize / 4, "{inside}");
    // Elsewhere only the repeat noise of the frames differs.
    assert!(outside * 500 < (size[0] * size[1]) as usize, "{outside}");
}
