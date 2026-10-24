//! The device half of the performance metric:
//! draw the lit benchmark model (`ui_preferences::metric`) on a native
//! device created like a profiling toolkit switch into an offscreen
//! canvas-sized target until the budget expires, and return draws per
//! second. Split out of `ui_preferences_metric` in Phase 3.2 (wgpu stays in
//! the GPU renderer); the shell's active toolkit runs [`measure`] on its own
//! device when the interface host asks its `RendererProbe` for a benchmark.
use crate::desc;
use crate::performance_metric::{Benchmark, MetricVertex};
use anyhow::Result;
const SHADER: &str = r#"
struct Draw { mvp: mat4x4<f32> };
@group(0) @binding(0) var<uniform> draw: Draw;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) colour: vec4<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>) -> Out {
    // Ambient intensity 1.0; white sun, direction (20, -50, 30).
    let sun = normalize(vec3<f32>(20.0, -50.0, 30.0));
    let light = 1.0 + 0.5 * max(dot(n, -sun), 0.0);
    var o: Out;
    o.pos = draw.mvp * vec4<f32>(p, 1.0);
    o.colour = vec4<f32>(c.rgb * light, c.a);
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return i.colour; }
"#;

/// Uniform stride: `minUniformBufferOffsetAlignment` upper bound.
const STRIDE: u64 = 256;
/// Placements per frame: rows 15..0 with `row + 1` models each.
pub const PER_FRAME: usize = 136;

/// The benchmark's view: a translation by `(0, 256, 0)` and a pixel-space
/// projection centred on `(w/2, h/2)` with a 512 focal length.
fn projection(size: [u32; 2], near: f32, far: f32) -> glam::Mat4 {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let (cx, cy, f) = (w / 2.0, h / 2.0, 512.0);
    // clip.x = x*2f/w + z*(2cx/w - 1); clip.y = -(y*2f/h + z*(2cy/h - 1))
    // (client y grows downward); depth maps [near, far] to [0, 1].
    glam::Mat4::from_cols(
        glam::Vec4::new(2.0 * f / w, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, -2.0 * f / h, 0.0, 0.0),
        glam::Vec4::new(
            2.0 * cx / w - 1.0,
            -(2.0 * cy / h - 1.0),
            far / (far - near),
            1.0,
        ),
        glam::Vec4::new(0.0, 0.0, -near * far / (far - near), 0.0),
    ) * glam::Mat4::from_translation(glam::Vec3::new(0.0, 256.0, 0.0))
}

/// Returns `draws * 1000 / elapsed` for the benchmark `request` run on the
/// shell's own device (the one the game draws with), or `-1` on any failure
/// (including a zero elapsed time).
pub fn measure(device: &wgpu::Device, queue: &wgpu::Queue, request: &Benchmark<'_>) -> Result<i32> {
    let run = run(device, queue, request)?;
    if run.elapsed_ms <= 0 {
        return Ok(-1);
    }
    Ok((run.draws * 1000 / run.elapsed_ms) as i32)
}

/// What a benchmark drew: the model draws submitted, the milliseconds they
/// took to finish on the device, and the last frame's colour target.
pub struct Run {
    pub draws: i64,
    pub elapsed_ms: i64,
    pub colour: wgpu::Texture,
}

/// Draws the benchmark's frames (whole frames of [`PER_FRAME`] placements,
/// at most 500, until the budget has passed after one) and waits for the
/// device to finish them.
pub fn run(device: &wgpu::Device, queue: &wgpu::Queue, request: &Benchmark<'_>) -> Result<Run> {
    use wgpu::util::DeviceExt;
    let Benchmark {
        model,
        canvas,
        near,
        far,
        budget_ms,
    } = *request;
    let size = [canvas[0].max(1), canvas[1].max(1)];
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = |format, usage, label| {
        let texture =
            device.create_texture(&desc::texture_2d(label, [size[0], size[1]], format, usage));
        let view = texture.create_view(&Default::default());
        (texture, view)
    };
    let (colour_texture, colour) = target(
        format,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        "metric canvas",
    );
    let (_, depth) = target(
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
        "metric depth",
    );
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("metric vertices"),
        contents: bytemuck::cast_slice(&model.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("metric indices"),
        contents: bytemuck::cast_slice(&model.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    // The 136 placement matrices are identical every frame.
    let proj = projection(size, near, far);
    let mut uniforms = vec![0u8; STRIDE as usize * PER_FRAME];
    let mut slot = 0;
    for row in (0..=15).rev() {
        for col in 0..=row {
            let t = glam::Vec3::new(
                (col as f32 - row as f32 / 2.0) * 512.0,
                0.0,
                ((row + 1) * 512) as f32,
            );
            let mvp = proj * glam::Mat4::from_translation(t);
            let at = slot * STRIDE as usize;
            uniforms[at..at + 64].copy_from_slice(bytemuck::cast_slice(&mvp.to_cols_array()));
            slot += 1;
        }
    }
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("metric placements"),
        contents: &uniforms,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let layout = desc::layout(
        device,
        "metric draw",
        &[desc::uniform_sized(0, wgpu::ShaderStages::VERTEX, true, 64)],
    );
    let bind = desc::bind_group(
        device,
        "metric draw",
        &layout,
        &[(
            0,
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform,
                offset: 0,
                size: wgpu::BufferSize::new(64),
            }),
        )],
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("metric"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("metric"),
        layout: Some(&desc::pipeline_layout(device, "metric", &[Some(&layout)])),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<MetricVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4],
            })],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(desc::depth_state(wgpu::TextureFormat::Depth32Float, true, wgpu::CompareFunction::LessEqual)),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let index_count = model.indices.len() as u32;
    let start = crate::logic_clock::monotonic_millis();
    let mut draws: i64 = 0;
    for _ in 0..500 {
        // toolkit.clear(3, 0)
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = desc::pass(
                &mut encoder,
                "metric frame",
                &[desc::colour(
                    &colour,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                )],
                desc::depth(&depth, wgpu::LoadOp::Clear(1.0), wgpu::StoreOp::Store),
            );
            pass.set_pipeline(&pipeline);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for slot in 0..PER_FRAME {
                // One model draw per placement.
                pass.set_bind_group(0, &bind, &[(slot as u64 * STRIDE) as u32]);
                pass.draw_indexed(0..index_count, 0, 0..1);
            }
        }
        draws += PER_FRAME as i64;
        queue.submit(Some(encoder.finish()));
        if crate::logic_clock::monotonic_millis() - start >= budget_ms {
            break;
        }
    }
    // Wait for the submitted work.
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    Ok(Run {
        draws,
        elapsed_ms: crate::logic_clock::monotonic_millis() - start,
        colour: colour_texture,
    })
}
