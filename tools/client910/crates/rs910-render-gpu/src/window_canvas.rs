//! Offscreen logical-canvas target used when the game canvas is bounded inside
//! a larger native window. World and UI projections remain in canvas space;
//! this compositor performs the final physical placement and clipping.

use anyhow::{ensure, Result};

/// A bounded canvas texture and the presenter that copies it into the surface.
pub struct CanvasTarget {
    pub texture: wgpu::Texture,
    #[allow(
        dead_code,
        reason = "keeps the canvas texture view alive with its texture"
    )]
    pub view: wgpu::TextureView,
    pub presenter: crate::ui_paint_gpu::Renderer,
}

impl CanvasTarget {
    /// Allocate a renderable canvas of `rect.width × rect.height` and prepare
    /// a full-surface presenter quad at `(rect.x, rect.y)`. Negative origins
    /// are intentional: wgpu scissoring clips them at the surface boundary.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        surface: [u32; 2],
        rect: [i32; 4],
    ) -> Result<Self> {
        let pipeline = std::sync::Arc::new(crate::text_render_gpu::Pipeline::new(device, format));
        Self::with_pipeline(device, queue, format, pipeline, surface, rect)
    }
    /// [`CanvasTarget::new`] with the presenter drawing through a shared 2D
    /// pipeline set (`text_render_gpu::Pipeline::cached`).
    pub fn with_pipeline(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pipeline: std::sync::Arc<crate::text_render_gpu::Pipeline>,
        surface: [u32; 2],
        rect: [i32; 4],
    ) -> Result<Self> {
        ensure!(surface.iter().all(|v| *v > 0), "invalid surface size");
        ensure!(rect[2] > 0 && rect[3] > 0, "invalid canvas rectangle");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bounded canvas"),
            size: wgpu::Extent3d {
                width: rect[2] as u32,
                height: rect[3] as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let presenter_view = texture.create_view(&Default::default());
        let mut presenter = crate::ui_paint_gpu::Renderer::with_pipeline(pipeline);
        presenter.register_external(device, 1, presenter_view);
        let mut painter = crate::ui_paint::Painter::new(surface);
        let [x, y, w, h] = rect;
        painter.affine_image(
            crate::ui_paint::Image::External(1),
            [
                x as f32,
                y as f32,
                (x + w) as f32,
                y as f32,
                x as f32,
                (y + h) as f32,
            ],
            -1,
            None,
        );
        painter.quad_count();
        presenter.prepare(
            device,
            queue,
            crate::ui_paint::Plan {
                size: surface,
                quads: painter.quads,
            },
        )?;
        Ok(Self {
            texture,
            view,
            presenter,
        })
    }

    /// Clear the native surface and composite the bounded canvas into it.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, surface_view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present bounded canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        self.presenter.draw(&mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn bounded_canvas_pixels_preserve_scale_and_clip() -> Result<()> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|err| anyhow::anyhow!("GPU unavailable: {err}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        for rect in [[2, 0, 8, 6], [-2, 0, 16, 6], [0, 0, 12, 12]] {
            let canvas = CanvasTarget::new(&device, &queue, format, [12, 8], rect)?;
            let source: Vec<u8> = (0..rect[3])
                .flat_map(|y| {
                    (0..rect[2]).flat_map(move |x| [(x * 13) as u8, (y * 17) as u8, 80, 255])
                })
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &canvas.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &source,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(rect[2] as u32 * 4),
                    rows_per_image: Some(rect[3] as u32),
                },
                canvas.texture.size(),
            );
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("canvas test surface"),
                size: wgpu::Extent3d {
                    width: 12,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            canvas.encode(&mut encoder, &target.create_view(&Default::default()));
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 8,
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
                        bytes_per_row: Some(256),
                        rows_per_image: Some(8),
                    },
                },
                target.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            rx.recv()??;
            let pixels = buffer.slice(..).get_mapped_range().expect("mapped range");
            for y in 0..8i32 {
                for x in 0..12i32 {
                    let (cx, cy) = (x - rect[0], y - rect[1]);
                    let expected = if cx >= 0 && cy >= 0 && cx < rect[2] && cy < rect[3] {
                        [(cx * 13) as u8, (cy * 17) as u8, 80, 0]
                    } else {
                        [0, 0, 0, 255]
                    };
                    let actual =
                        &pixels[(y * 256 + x * 4) as usize..(y * 256 + x * 4 + 4) as usize];
                    assert_eq!(actual, expected, "rect={rect:?} pixel={x},{y}");
                }
            }
        }
        Ok(())
    }
}
