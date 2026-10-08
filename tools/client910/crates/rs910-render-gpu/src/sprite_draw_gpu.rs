//! The sprite-only GPU renderer over `ui_paint_gpu` (the default batch
//! path for [`crate::sprite_draw::Painter`] output). Split out of
//! `sprite_draw` in Phase 3.1 (it is wgpu; the painter is rs910-toolkit's).
use crate::sprite_draw::Painter;
use anyhow::Result;

/// Compatibility entry for sprite-only consumers. Fonts, sprites and toolkit
/// primitives use the same shared renderer when the full UI plan is supplied.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "sprite-only compatibility renderer; exercised by tests only"
    )
)]
pub struct Renderer(crate::ui_paint_gpu::Renderer);

impl Renderer {
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite-only compatibility renderer; exercised by tests only"
        )
    )]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self(crate::ui_paint_gpu::Renderer::new(device, format))
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite-only compatibility renderer; exercised by tests only"
        )
    )]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        painter: Painter,
    ) -> Result<()> {
        self.0.prepare(device, queue, painter.into())
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite-only compatibility renderer; exercised by tests only"
        )
    )]
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.0.draw(pass);
    }
}
