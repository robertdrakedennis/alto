//! The shell's cursor sink: `ui_cursor`'s custom/system cursor requests
//! on the winit window (the canvas cursor half of the client's cursor
//! request). Split out of `ui_cursor` in Phase 3.2 (winit stays in
//! the shell).
use crate::ui_cursor::{Image, Sink};
use anyhow::Result;
pub struct Native<'a> {
    pub event_loop: &'a winit::event_loop::ActiveEventLoop,
    pub window: &'a winit::window::Window,
}
impl Sink for Native<'_> {
    fn custom(&mut self, id: i32, image: &Image) -> Result<()> {
        anyhow::ensure!(
            image.size.iter().all(|&n| n > 0),
            "cursor image has non-positive dimensions"
        );
        let size = [u16::try_from(image.size[0])?, u16::try_from(image.size[1])?];
        let hotspot = [
            u16::try_from(image.hotspot[0])?,
            u16::try_from(image.hotspot[1])?,
        ];
        let source = winit::window::CustomCursor::from_rgba(
            image.rgba(),
            size[0],
            size[1],
            hotspot[0],
            hotspot[1],
        )?;
        self.window
            .set_cursor(self.event_loop.create_custom_cursor(source));
        if crate::debug_flags::flags().cursor_trace {
            log::info!(
                "[cursor] id={id} graphic={} size={:?} hotspot={:?}",
                image.graphic,
                image.size,
                image.hotspot
            );
        }
        Ok(())
    }
    fn system(&mut self) -> Result<()> {
        self.window.set_cursor(winit::window::CursorIcon::Default);
        if crate::debug_flags::flags().cursor_trace {
            log::info!("[cursor] system default");
        }
        Ok(())
    }
}
