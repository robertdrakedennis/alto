//! The performance metric on the modern renderer: the benchmark model drawn
//! through the renderer's own frame (every pass its settings enable) in the
//! profiling layout, a triangle of 136 placements a frame, until the time
//! budget has passed; the answer is placements drawn per second.
//!
//! It runs on a renderer made for the measurement over the shell's device,
//! not on the one drawing the game: the game's frame state (targets, shadow
//! and probe caches, the scene it has installed) stays as it was, and the
//! measurement starts after one warm-up frame so shader compiles are not
//! timed. The frames are offscreen; nothing is presented.
use super::*;
use rs910_toolkit::performance_metric::{Benchmark, MetricModel};

/// Placements in a frame: rows 15 down to 0 with `row + 1` models each.
pub const PER_FRAME: usize = 136;
/// The most frames a benchmark draws.
const MAX_FRAMES: usize = 500;
/// Frames the CPU may run ahead of the device before it waits for it.
const IN_FLIGHT: usize = 4;
/// Distance between placements, in scene units.
const SPACING: f32 = 512.0;

/// What a benchmark drew.
pub struct Run {
    /// Model draws (placements) in the timed frames.
    pub draws: i64,
    /// Milliseconds the timed frames took to finish on the device.
    pub elapsed_ms: i64,
    /// The last frame's colour target (`format`).
    pub frame: wgpu::Texture,
}

/// The benchmark model as the renderer's model streams: one untextured batch.
fn streams(model: &MetricModel) -> ModelStreams {
    let mut vertices: Vec<crate::models::mesh::Vertex> = model
        .vertices
        .iter()
        .map(|v| crate::models::mesh::Vertex {
            pos: v.pos(),
            normal: v.normal(),
            uv: [0.0; 2],
            tangent: [0.0; 4],
        })
        .collect();
    // The streams came from 16-bit indices.
    let indices: Vec<u16> = model.indices.iter().map(|&i| i as u16).collect();
    crate::models::mesh::add_tangents(&mut vertices, &indices);
    ModelStreams {
        colours: model.vertices.iter().map(|v| v.colour()).collect(),
        batches: vec![(-1, 0, indices.len() as u32)],
        vertices,
        indices,
    }
}

/// The 136 model matrices (column-major, scene-local) around `centre`: the
/// profiling layout's triangle, centred on the camera target.
fn placements(centre: [f32; 2]) -> Vec<[f32; 16]> {
    let mut out = Vec::with_capacity(PER_FRAME);
    for row in (0..=15).rev() {
        for col in 0..=row {
            let x = centre[0] + (col as f32 - row as f32 / 2.0) * SPACING;
            let z = centre[1] + (row as f32 - 8.0) * SPACING;
            out.push(glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, z)).to_cols_array());
        }
    }
    out
}

/// Draws the benchmark's frames on a renderer made for it over `device` and
/// waits for the device to finish them. `format` and `samples` are the
/// shell's (the surface format and forward sample count), `settings` the
/// modern quality settings the frames draw with.
pub fn run(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    format: wgpu::TextureFormat,
    samples: u32,
    settings: crate::settings::ModernSettings,
    request: &Benchmark<'_>,
) -> anyhow::Result<Run> {
    let size = [request.canvas[0].max(1), request.canvas[1].max(1)];
    let mut renderer = ModernRenderer::new(device, queue, format, samples, settings);
    let target = [26 * 512, 0, 26 * 512];
    renderer.bench_models = Some((
        streams(request.model),
        placements([target[0] as f32, target[2] as f32]),
    ));
    let mut camera = rs910_scene::camera::SceneCamera::new(target);
    camera.viewport = (size[0] as i32, size[1] as i32);
    let (far, near_min) = camera.fog_reference();
    let mut env = rs910_scene::env::EnvFrame::default_for(far, near_min, &camera.view_entries());
    env.fog.range = None;
    let snapshot = SceneSnapshot {
        owned: None,
        time_ms: None,
        camera,
        env: &env,
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
    };
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("benchmark frame"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let [w, h] = size.map(|v| v as i32);
    let mut frame = || {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.draw(
            Target {
                device,
                queue,
                encoder: &mut encoder,
                view: &view,
                format,
                size,
                rect: [0, 0, w, h],
                clip: [0, 0, w, h],
            },
            &snapshot,
        );
        queue.submit_uploads(vec![encoder.finish()]);
    };
    // The warm-up: pipelines, targets and the first uploads are not timed.
    frame();
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let start = crate::logic_clock::monotonic_millis();
    let mut frames = 0;
    while frames < MAX_FRAMES {
        frame();
        frames += 1;
        if frames % IN_FLIGHT == 0 {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        if crate::logic_clock::monotonic_millis() - start >= request.budget_ms {
            break;
        }
    }
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    Ok(Run {
        draws: (frames * PER_FRAME) as i64,
        elapsed_ms: crate::logic_clock::monotonic_millis() - start,
        frame: output,
    })
}

/// Draws per second of [`run`], or -1 when it took no measurable time.
pub fn measure(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    format: wgpu::TextureFormat,
    samples: u32,
    settings: crate::settings::ModernSettings,
    request: &Benchmark<'_>,
) -> anyhow::Result<i32> {
    let run = run(device, queue, format, samples, settings, request)?;
    if run.elapsed_ms <= 0 {
        return Ok(-1);
    }
    Ok((run.draws * 1000 / run.elapsed_ms) as i32)
}

impl ModernRenderer {
    /// The benchmark's models for this frame: the shared streams once, and a
    /// draw per placement over them (the instance stream carries the
    /// matrix), like loc meshes shared by many locs.
    pub(super) fn prepare_benchmark_models(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
    ) {
        let Some((streams, matrices)) = self.bench_models.take() else {
            return;
        };
        let (base_vertex, first) = self.arena.push(&streams);
        for matrix in &matrices {
            for &(material, start, count) in &streams.batches {
                self.textures
                    .ensure(device, queue, snapshot.pack, snapshot.materials, material);
                let instance = self.instances.len() as u32;
                let mut record =
                    self.instance(local_matrix(matrix, origin), material, 1.0, FLAG_FLOOR);
                record.p2 = [0.0; 4];
                self.instances.push(record);
                self.draws.push(Draw {
                    geometry: Geometry::Arena { base_vertex },
                    material,
                    first_index: start + first,
                    count,
                    instance,
                    pass: Pass::Opaque,
                    casts: self.shadow_frame.is_some(),
                    indirect: None,
                });
            }
        }
        self.bench_models = Some((streams, matrices));
    }
}
