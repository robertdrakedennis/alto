//! The renderer's end-to-end tests, by subsystem: headless frames of real
//! offline scenes (the pack) or of small synthetic ones, checked for what
//! the frame must show rather than for pinned values. GPU tests are ignored
//! by default, like every GPU test of the workspace
//! (`cargo test -p rs910-render-modern -- --include-ignored`).
//!
//! Frames are compared within one process after a warm-up: the first frames
//! a process draws on Apple GPUs can differ from its later frames with
//! identical inputs by a few foliage pixels (a warm-up variant of the
//! compiled shaders; renderer plan M4/M6), so [`Noise`] measures and bounds
//! repeat differences instead of assuming bit equality.
use rs910_gpu_device::desc;

mod far;
mod lifecycle;
mod models;
mod point_shadows;
mod scale;
mod scene;
mod shadows;
mod water;

use crate::frame::{frame_uniforms, ModernRenderer, Target};
use crate::scene_snapshot::SceneSnapshot;
use crate::settings::ModernSettings;

/// Restores the thread's real clock even if a test panics.
pub(super) struct ClockReset;
impl Drop for ClockReset {
    fn drop(&mut self) {
        crate::logic_clock::set_test_now(None);
    }
}

/// The fixed clock the tests draw at (with its reset).
pub(super) fn fixed_clock() -> ClockReset {
    crate::logic_clock::set_test_now(Some(1_700_000_000_000));
    ClockReset
}

/// A renderer drawing into `Rgba8Unorm` frames (the client's non-sRGB
/// surface) with `samples` and `settings`, its far scene built in full
/// before each frame (repeatable frames).
pub(super) fn renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    samples: u32,
    settings: ModernSettings,
) -> ModernRenderer {
    let mut r = ModernRenderer::new(
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        samples,
        settings,
    );
    r.scene_resources.far.sync = true;
    r.preparation.prebuilds.sync = true;
    r.device_resources.textures.set_stream_queue(None);
    r
}

/// An offline scene around `tile` (the software toolkit's fixed-clock gate
/// builds the same one): the normal rebuild, the live scene with its
/// dynamic locs, a camera over the tile and the environment frame.
pub(super) struct OfflineScene {
    world: rs910_scene::rebuild::Rebuild,
    live: crate::live_scene::LiveScene,
    materials: crate::texture::MaterialStore,
    camera: crate::camera::SceneCamera,
    pub(super) env: rs910_scene::env::EnvFrame,
    base: (i32, i32),
}

impl OfflineScene {
    pub(super) fn new(pack: &crate::cache::Pack, tile: (i32, i32), viewport: (i32, i32)) -> Self {
        Self::with_camera(pack, tile, viewport, |_| {})
    }

    /// [`Self::new`] with the orbit camera adjusted (pitch, yaw, distance)
    /// before the plan and the environment are built from it.
    pub(super) fn with_camera(
        pack: &crate::cache::Pack,
        tile: (i32, i32),
        viewport: (i32, i32),
        adjust: impl FnOnce(&mut crate::camera::SceneCamera),
    ) -> Self {
        Self::with_prefs(pack, tile, viewport, &Default::default(), adjust)
    }

    /// [`Self::with_camera`] built with the graphics preferences `prefs`
    /// (water detail high builds the underwater scene).
    pub(super) fn with_prefs(
        pack: &crate::cache::Pack,
        (tx, tz): (i32, i32),
        viewport: (i32, i32),
        prefs: &rs910_scene::rebuild::BuildPrefs,
        adjust: impl FnOnce(&mut crate::camera::SceneCamera),
    ) -> Self {
        let materials = crate::texture::MaterialStore::load(pack).unwrap();
        let locs = rs910_config::config::LocStore::load(pack).unwrap();
        let flo = rs910_config::flo::FloStore::load(pack).unwrap();
        let tables = rs910_scene::maploader::FloTables::from_store(&flo);
        let mut world = rs910_scene::rebuild::rebuild_normal(
            pack,
            &tables,
            &materials,
            Some(&locs),
            tx,
            tz,
            prefs,
        )
        .unwrap();
        let base = (world.base_x, world.base_z);
        let scene = world.scene_graph.as_mut().unwrap();
        let mut live = crate::live_scene::LiveScene::new(
            scene,
            &world.scene.normal,
            world.flags.clone(),
            &world.env.lights,
        )
        .unwrap();
        live.enable_dynamic(pack, &locs, world.model_cache.clone())
            .unwrap();
        let local = [(tx - base.0) * 512 + 256, (tz - base.1) * 512 + 256];
        let ground = world.scene.normal[0]
            .as_ref()
            .unwrap()
            .heights
            .get_fine_height(local[0], local[1]);
        let mut camera =
            crate::camera::SceneCamera::new([tx * 512 + 256, ground - 200, tz * 512 + 256]);
        camera.viewport = viewport;
        adjust(&mut camera);
        let env = world.env.target_environment(tx - base.0, tz - base.1);
        let (far, near_min) = camera.fog_reference();
        let env = rs910_scene::env::EnvFrame::build(
            &env,
            rs910_scene::env::SunSettings {
                direction: world.env.sun_direction,
                brightness_pref: 3,
                anti_macro: 0.0,
            },
            true,
            rs910_scene::env::FogReference {
                far,
                near_min,
                view: &camera.view_entries(),
            },
        );
        live.update(
            world.scene_graph.as_ref().unwrap(),
            camera.clone(),
            base,
            0,
            [-1, -1],
        );
        Self {
            world,
            live,
            materials,
            camera,
            env,
            base,
        }
    }

    /// Advance the dynamic locs of the plan to logic cycle `cycle` (the
    /// client's `refresh_dynamic_scene`: the dynamic loc's model builder re-poses the
    /// animated ones); returns how many changed model.
    pub(super) fn animate(&mut self, cycle: i32) -> usize {
        let live = &mut self.live;
        let Some(dynamic) = live.dynamic.as_mut() else {
            return 0;
        };
        let scene = self.world.scene_graph.as_mut().unwrap();
        let ids: Vec<usize> = live
            .draw
            .plan
            .dispatch_opaque
            .iter()
            .chain(&live.draw.plan.dispatch_transparent)
            .copied()
            .collect();
        let mut changed = 0;
        for id in ids {
            if !dynamic.contains(id) {
                continue;
            }
            let before = dynamic.revision(id);
            dynamic
                .refresh(
                    id,
                    true,
                    cycle,
                    &mut live.entities[id],
                    rs910_scene::dynamic_scene::RefreshWorld {
                        scene,
                        floors: &mut self.world.scene.normal,
                        materials: &self.materials,
                        sun: &self.env.sun,
                    },
                )
                .unwrap();
            changed += usize::from(dynamic.revision(id) != before);
        }
        changed
    }

    /// Transient temporaries drawing copies of the scene's first `n` loc
    /// models that name their source models (NPC-like models the renderer
    /// poses every frame, `frame::posing`), a tile apart from the camera
    /// target; returns how many.
    pub(super) fn add_posed_models(&mut self, n: usize) -> usize {
        let scene = self.world.scene_graph.as_mut().unwrap();
        let models: Vec<crate::gpumodel::GpuModel> = self
            .live
            .entities
            .iter()
            .filter_map(|e| crate::dynamic_scene::model(scene, e.source))
            .filter(|m| m.source_ids.is_some() && m.draw_face_count > 0)
            .take(n)
            .cloned()
            .collect();
        let t = self.camera.target;
        let count = models.len();
        for (k, model) in models.into_iter().enumerate() {
            let local = [
                t[0] - self.base.0 * 512 + (k as i32 % 3 - 1) * 512,
                t[2] - self.base.1 * 512 + (k as i32 / 3) * 512,
            ];
            let tile = [local[0] >> 9, local[1] >> 9];
            scene.add_temporary(crate::scene::TemporaryEntity {
                player: 0,
                npc_index: None,
                location_key: None,
                pick: None,
                transient: true,
                level: 0,
                occlude_level: 0,
                position: [local[0] as f32, t[1] as f32, local[1] as f32],
                bounds: [tile[0], tile[0], tile[1], tile[1]],
                overlay_height: 0,
                transparent: false,
                spot_shadow: false,
                model: Some(Box::new(model)),
            });
            self.live.install_temporary(scene);
        }
        // Their visibility cylinders (the client's actor bounds), then the
        // plan with them.
        let statics = self.live.static_entity_count;
        for e in &mut self.live.entities[statics..] {
            let p = e.position.unwrap_or_default().map(|v| v as i32);
            e.cylinder = Some([p[0], p[1], p[2], -1024, 0, 512]);
        }
        self.live
            .update(scene, self.camera.clone(), self.base, 0, [-1, -1]);
        count
    }

    pub(super) fn snapshot<'a>(&'a self, pack: &'a crate::cache::Pack) -> SceneSnapshot<'a> {
        SceneSnapshot {
            owned: None,
            time_ms: None,
            camera: self.camera.clone(),
            env: &self.env,
            live: Some(&self.live),
            scene: self.world.scene_graph.as_ref(),
            floors: &self.world.scene.normal,
            lights: &[],
            players: None,
            floor_base: [self.base.0, self.base.1],
            materials: Some(&self.materials),
            pack: Some(pack),
            blackout: false,
            local_player: None,
            particles: None,
            underwater: None,
            sky: None,
        }
    }

    /// The view's skybox as the shell resolves it (`app::environment`): the
    /// environment's box at the viewport height, its layers at the camera's
    /// pitch and yaw. The perf bench draws it.
    #[allow(dead_code)]
    pub(super) fn sky(&self, pack: &crate::cache::Pack) -> Option<crate::sky_frame::SkyCache> {
        let types = crate::skybox::SkyTypes::load(pack).ok()?;
        let mut owner = crate::skybox::SkyboxOwner::new(types);
        let t = self.camera.target;
        let env = self
            .world
            .env
            .target_environment(t[0] / 512 - self.base.0, t[2] / 512 - self.base.1);
        owner.select(env.skybox);
        let current = owner.current;
        owner.end_fade(current);
        owner.update(self.camera.viewport.1, 1);
        let layers = owner.frame(
            self.world.env.skybox_yaw_offset,
            self.camera.pitch_int(),
            self.camera.yaw_int(),
            0,
            env.fog_colour & 0xFF_FFFF,
        );
        let billboards = crate::billboard::BillboardStore::load(pack).ok()?;
        let emitters = rs910_model::particle::EmitterStore::load(pack).ok()?;
        let assets = crate::sky_frame::SkyAssets {
            pack,
            materials: &self.materials,
            billboards: &billboards,
            emitters: &emitters,
        };
        let mut cache = crate::sky_frame::SkyCache::default();
        cache.resolve(&assets, &owner, layers);
        Some(cache)
    }
}

/// The river north of Lumbridge from the east (`pitch`, `zoom`).
pub(super) fn river_scene(
    pack: &crate::cache::Pack,
    size: [u32; 2],
    pitch: f32,
    zoom: f32,
) -> OfflineScene {
    OfflineScene::with_camera(pack, (3243, 3240), (size[0] as i32, size[1] as i32), |c| {
        c.yaw = 12000.0;
        c.pitch = pitch;
        c.distance_scale = zoom;
    })
}

/// A snapshot with no live parts (the frame is the renderer's own test
/// models over the clear colour) under `env`.
pub(super) fn bare<'a>(
    camera: &crate::camera::SceneCamera,
    env: &'a rs910_scene::env::EnvFrame,
) -> SceneSnapshot<'a> {
    SceneSnapshot {
        owned: None,
        time_ms: None,
        camera: camera.clone(),
        env,
        live: None,
        scene: None,
        floors: &[],
        lights: &[],
        players: None,
        floor_base: [0, 0],
        materials: None,
        pack: None,
        blackout: false,
        local_player: None,
        particles: None,
        underwater: None,
        sky: None,
    }
}

/// A camera over map tile (26, 26) of an empty scene and its environment
/// (no fog), viewport `size`.
pub(super) fn bare_camera(
    size: [u32; 2],
) -> (crate::camera::SceneCamera, rs910_scene::env::EnvFrame) {
    let mut camera = crate::camera::SceneCamera::new([26 * 512, 0, 26 * 512]);
    camera.viewport = (size[0] as i32, size[1] as i32);
    let (far, near_min) = camera.fog_reference();
    let mut env = rs910_scene::env::EnvFrame::default_for(far, near_min, &camera.view_entries());
    env.fog.range = None;
    (camera, env)
}

/// A grey quad (both windings) with the given corners and normal,
/// untextured (grey keeps the lit ground below the tonemap's shoulder).
pub(super) fn quad(
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    tangent: [f32; 3],
) -> crate::models::mesh::ModelStreams {
    crate::models::mesh::ModelStreams {
        vertices: corners
            .iter()
            .map(|&pos| crate::models::mesh::Vertex {
                pos,
                normal,
                uv: [0.0, 0.0],
                tangent: [tangent[0], tangent[1], tangent[2], 1.0],
            })
            .collect(),
        colours: vec![0xff50_5050; 4],
        indices: vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2],
        batches: vec![(-1, 0, 12)],
    }
}

/// A flat quad of half-size `half` at scene-local height `y` (classic y down)
/// around `(x, z)`.
pub(super) fn flat_quad(x: f32, z: f32, half: f32, y: f32) -> crate::models::mesh::ModelStreams {
    quad(
        [
            [x - half, y, z - half],
            [x + half, y, z - half],
            [x + half, y, z + half],
            [x - half, y, z + half],
        ],
        [0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0],
    )
}

/// The frame pixel of camera-local `p`.
pub(super) fn pixel_of(view_proj: &glam::Mat4, size: [u32; 2], p: [f32; 3]) -> (f32, f32) {
    let c = *view_proj * glam::Vec4::new(p[0], p[1], p[2], 1.0);
    (
        (c.x / c.w * 0.5 + 0.5) * size[0] as f32,
        (0.5 - c.y / c.w * 0.5) * size[1] as f32,
    )
}

/// `f16` bits to `f32`.
pub(super) fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// `f16` texels to `f32`.
pub(super) fn halves(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| half(u16::from_le_bytes([b[0], b[1]])))
        .collect()
}

/// Copy `texture` (`bytes` per texel) back to the CPU.
pub(super) fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bytes: u32,
) -> Vec<u8> {
    let (w, h) = (texture.width(), texture.height());
    let unpadded = w * bytes;
    let padded = unpadded.div_ceil(256) * 256;
    let buffer = desc::buffer(
        device,
        "read back",
        u64::from(padded * h),
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("mapped range");
    let mut out = Vec::with_capacity((unpadded * h) as usize);
    for y in 0..h {
        let row = (y * padded) as usize;
        out.extend_from_slice(&data[row..row + unpadded as usize]);
    }
    out
}

/// One frame's output pixels (RGBA8).
pub(super) struct Frame {
    pub(super) pixels: Vec<u8>,
}

/// One offscreen frame of `snapshot` at `size`.
pub(super) fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    size: [u32; 2],
) -> Frame {
    let output = device.create_texture(&desc::texture_2d(
        "offscreen frame",
        [size[0], size[1]],
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    ));
    let view = output.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    let [w, h] = size.map(|v| v as i32);
    renderer.draw(
        Target {
            device,
            queue,
            encoder: &mut encoder,
            view: &view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            size,
            rect: [0, 0, w, h],
            clip: [0, 0, w, h],
        },
        snapshot,
    );
    queue.submit(Some(encoder.finish()));
    let pixels = read_back(device, queue, &output, 4);
    Frame { pixels }
}

/// [`render`] once the renderer's light-probe capture (spread over its
/// first frames) has finished: the frame the comparisons use.
pub(super) fn settled(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut ModernRenderer,
    snapshot: &SceneSnapshot<'_>,
    size: [u32; 2],
) -> Frame {
    for _ in 0..40 {
        render(device, queue, renderer, snapshot, size);
        if !renderer.probe_capture_pending() {
            break;
        }
    }
    render(device, queue, renderer, snapshot, size)
}

/// How two frames' output pixels differ: the channel values that differ
/// and the largest difference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Noise {
    pub(super) values: usize,
    pub(super) largest: u8,
}

impl Noise {
    pub(super) fn of(a: &[u8], b: &[u8]) -> Self {
        let mut n = Self::default();
        for (p, q) in a.iter().zip(b) {
            if p != q {
                n.values += 1;
                n.largest = n.largest.max(p.abs_diff(*q));
            }
        }
        n
    }

    /// Within the repeat noise the renderers show at `size` (see the module
    /// docs): at most one channel value in 2,000. (At one sample a few
    /// alpha-tested foliage pixels flip between two states, by up to 9
    /// levels so far; with 4x MSAA repeats are exact.)
    pub(super) fn is_repeat_noise(self, size: [u32; 2]) -> bool {
        self.values * 2000 <= (size[0] * size[1] * 4) as usize
    }
}
