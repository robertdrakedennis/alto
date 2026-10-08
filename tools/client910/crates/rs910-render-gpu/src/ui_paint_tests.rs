use super::*;
use crate::sprite::*;
use crate::ui_paint::*;

// Exercise actual framebuffer scissors and fragment-position mask sampling.
// Geometry-only layout tests cannot detect a canvas/target size mismatch.
fn render(target_size: [u32; 2], masked: bool) -> Vec<u8> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: Default::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let mut painter = Painter::new([32, 24]);
    painter.fill([0, 0, 32, 24], 0xff0000ffu32 as i32).unwrap();
    if masked {
        painter.masked_fill(
            [0, 0, 32, 24],
            0xff00ff00u32 as i32,
            MaskRef {
                sprite: Rc::new(Sprite {
                    paletted: None,
                    size: [8, 8],
                    padding: [0; 4],
                    argb: vec![-1; 64],
                }),
                origin: [6, 6],
            },
        );
    } else {
        painter.reset_bounds([4, 3, 12, 9]);
        // Affine geometry crosses the clip: its GPU scissor must do the cut.
        painter.affine_image(
            Image::White,
            [0., 0., 16., 0., 0., 12.],
            0xffff0000u32 as i32,
            None,
        );
    }
    let mut renderer = Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    renderer
        .prepare_split_target(&device, &queue, painter.finish(), Some(1), target_size)
        .unwrap();
    let size = wgpu::Extent3d {
        width: target_size[0],
        height: target_size[1],
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("HiDPI UI regression"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    // Same split as RetainedUi::encode, before and after the scene pass.
    for before in [true, false] {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: if before {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        renderer.draw_range(&mut pass, if before { 0..1 } else { 1..usize::MAX });
    }
    let stride = (size.width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().expect("mapped range");
    bytes
        .chunks(stride as usize)
        .flat_map(|row| row[..size.width as usize * 4].iter().copied())
        .collect()
}

fn pixel(bytes: &[u8], size: [u32; 2], x: u32, y: u32) -> &[u8] {
    let i = ((y * size[0] + x) * 4) as usize;
    &bytes[i..i + 3]
}

#[test]
fn framebuffer_edges_stay_inside_odd_sized_surfaces() {
    let canvas = [801, 601];
    let target = [1601, 1201];
    assert_eq!(
        framebuffer_bounds([0, 0, 801, 601], canvas, target),
        [0, 0, 1601, 1201]
    );
    assert_eq!(
        framebuffer_bounds([-20, -10, 900, 700], canvas, target),
        [0, 0, 1601, 1201]
    );
    let left = framebuffer_bounds([0, 0, 400, 601], canvas, target);
    let right = framebuffer_bounds([400, 0, 801, 601], canvas, target);
    assert_eq!(left[2], right[0]);
    assert_eq!(right[2], 1601);
}

#[test]
#[ignore = "requires desktop GPU; cargo test --lib ui_paint_gpu::tests -- --ignored"]
fn hidpi_scissors_cover_the_target_and_clip_components() {
    for size in [[32, 24], [64, 48], [48, 36], [65, 49]] {
        let bytes = render(size, false);
        assert_eq!(
            pixel(&bytes, size, size[0] - 1, size[1] - 1),
            [0, 0, 255],
            "full canvas at {size:?}"
        );
        for y in 0..size[1] {
            for x in 0..size[0] {
                let [l, r] = [4., 12.].map(|v| (v * size[0] as f32 / 32.).round() as u32);
                let [t, b] = [3., 9.].map(|v| (v * size[1] as f32 / 24.).round() as u32);
                let expected = if (l..r).contains(&x) && (t..b).contains(&y) {
                    [255, 0, 0]
                } else {
                    [0, 0, 255]
                };
                assert_eq!(
                    pixel(&bytes, size, x, y),
                    expected,
                    "clip at {size:?} ({x},{y})"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires desktop GPU; cargo test --lib ui_paint_gpu::tests -- --ignored"]
fn hidpi_masks_follow_the_scaled_geometry() {
    for size in [[32, 24], [64, 48], [48, 36], [65, 49]] {
        let bytes = render(size, true);
        assert_eq!(
            pixel(&bytes, size, size[0] * 10 / 32, size[1] * 10 / 24),
            [0, 255, 0],
            "mask centre at {size:?}"
        );
        for [x, y] in [[2, 10], [18, 10], [10, 2], [10, 18]] {
            assert_eq!(
                pixel(&bytes, size, size[0] * x / 32, size[1] * y / 24),
                [0, 0, 255],
                "outside mask at {size:?} ({x},{y})"
            );
        }
    }
}
