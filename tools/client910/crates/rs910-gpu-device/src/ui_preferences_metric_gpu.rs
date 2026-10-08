//! The device half of the performance metric:
//! draw the lit benchmark model (`ui_preferences::metric`) on a native
//! device created like a profiling toolkit switch into an offscreen
//! canvas-sized target until the budget expires, and return draws per
//! second. Split out of `ui_preferences_metric` in Phase 3.2 (wgpu stays in
//! the GPU renderer); the shell's active toolkit runs [`measure`] on its own
//! device when the interface host asks its `RendererProbe` for a benchmark.
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
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
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
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("metric draw"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(64),
            },
            count: None,
        }],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("metric draw"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform,
                offset: 0,
                size: wgpu::BufferSize::new(64),
            }),
        }],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("metric"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("metric"),
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("metric"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        })),
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
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
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("metric frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::performance_metric::MetricModel;

    /// A wall facing the camera, 400 units square and centred on the model
    /// origin.
    fn wall() -> MetricModel {
        let vertex = |x, y| MetricVertex::new([x, y, 0.0], [0.0, 0.0, -1.0], 0xffff_ffff);
        MetricModel {
            vertices: vec![
                vertex(-200.0, -200.0),
                vertex(200.0, -200.0),
                vertex(0.0, 200.0),
            ],
            indices: vec![0, 1, 2],
        }
    }

    fn read_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        size: u32,
    ) -> Vec<u8> {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("metric readback"),
            size: u64::from(size * size * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec()
    }

    /// The faithful benchmark really renders: one whole frame is 136 model
    /// draws, the model lands on the canvas, and the timed run answers a
    /// positive draws-per-second figure.
    #[test]
    #[ignore = "gpu: needs a desktop GPU adapter"]
    fn the_faithful_benchmark_draws_and_times_frames() {
        let (device, queue) = crate::test_support::require_gpu();
        let model = wall();
        let request = |budget_ms| Benchmark {
            model: &model,
            canvas: [64, 64],
            near: 200.0,
            far: 9000.0,
            budget_ms,
        };
        let once = run(&device, &queue, &request(0)).unwrap();
        assert_eq!(once.draws, PER_FRAME as i64);
        let pixels = read_back(&device, &queue, &once.colour, 64);
        let lit = pixels
            .chunks_exact(4)
            .filter(|p| p[..3] != [0, 0, 0])
            .count();
        assert!(lit > 100, "the model covers {lit} pixels");
        let rate = measure(&device, &queue, &request(50)).unwrap();
        assert!(rate > 0, "{rate} draws per second");
    }
}
