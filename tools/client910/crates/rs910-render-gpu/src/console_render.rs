//! Consumer of the console draw and font layout streams. Uses
//! the scene renderer's device, encoder and surface; fonts survive map rebuilds.
use crate::{
    cache::Pack,
    console_draw::{ConsoleView, Draw, Font},
    font_atlas::Atlas,
    font_layout::{self, Images, Style},
    font_metrics::Metrics,
    text_render::{self, Vertex},
    text_render_gpu::{Mesh, Pipeline, Texture},
};
#[derive(Clone)]
struct Batch {
    texture: usize,
    mesh: Mesh,
}
#[derive(Clone)]
pub struct Renderer {
    pipeline: std::sync::Arc<Pipeline>,
    fonts: Vec<Metrics>,
    textures: Vec<Texture>,
    batches: Vec<Batch>,
    last: Vec<Draw>,
    size: [u32; 2],
}
impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pack: &Pack,
    ) -> anyhow::Result<Self> {
        Self::with_pipeline(
            device,
            queue,
            std::sync::Arc::new(Pipeline::new(device, format)),
            pack,
        )
    }
    /// The console drawing with a shared 2D pipeline set
    /// (`text_render_gpu::Pipeline::cached`).
    pub fn with_pipeline(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: std::sync::Arc<Pipeline>,
        pack: &Pack,
    ) -> anyhow::Result<Self> {
        let bytes = crate::js5_fetch::fetch_file(pack, "defaults", 3)?
            .ok_or_else(|| anyhow::anyhow!("graphics defaults absent"))?;
        let defs = crate::protocol910::defaults::Graphics::decode(&bytes)
            .map_err(|e| anyhow::anyhow!("graphics defaults: {e:?}"))?
            .value
            .scalars;
        let mut fonts = Vec::new();
        let mut textures = Vec::new();
        for id in [defs.p11_full, defs.p12_full, defs.b12_full] {
            let m = Metrics::decode(
                &crate::js5_fetch::fetch_file(pack, "fontmetrics", id as u32)?
                    .ok_or_else(|| anyhow::anyhow!("font metrics {id} absent"))?,
            )?;
            let sheet = crate::sprite_sheet::SpriteSheet::decode(
                &crate::js5_fetch::fetch_file(pack, "sprites", id as u32)?
                    .ok_or_else(|| anyhow::anyhow!("font sprite {id} absent"))?,
            )?;
            let atlas = Atlas::new(&m, &sheet, true)?;
            textures.push(pipeline.upload(device, queue, &atlas));
            fonts.push(m);
        }
        textures.push(pipeline.upload(
            device,
            queue,
            &Atlas {
                width: 1,
                height: 1,
                scale: 1,
                argb: vec![0xffffffff],
            },
        ));
        Ok(Self {
            pipeline,
            fonts,
            textures,
            batches: vec![],
            last: vec![],
            size: [0, 0],
        })
    }
    pub fn line_sizes(&self) -> [i32; 2] {
        [
            self.fonts[1].ascent + self.fonts[1].descent + 2,
            self.fonts[2].ascent + self.fonts[2].descent + 2,
        ]
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        c: &dyn ConsoleView,
        size: [u32; 2],
        cycle: i32,
        focused: bool,
    ) -> anyhow::Result<()> {
        if !c.open() {
            self.batches.clear();
            self.last.clear();
            return Ok(());
        }
        let plan = c.draw(
            size[0] as i32,
            cycle,
            focused,
            [&self.fonts[0], &self.fonts[1], &self.fonts[2]],
        )?;
        if self.last == plan && self.size == size {
            return Ok(());
        }
        let mut batches: Vec<(usize, Vec<[Vertex; 4]>)> = Vec::new();
        let mut clip = [0, 0, size[0] as i32, size[1] as i32];
        let mut push = |texture, quad: Option<[Vertex; 4]>| {
            if let Some(quad) = quad {
                if batches.last().is_none_or(|b| b.0 != texture) {
                    batches.push((texture, vec![]));
                }
                batches.last_mut().unwrap().1.push(quad);
            }
        };
        for d in &plan {
            match d {
                Draw::Clip(c) => {
                    clip = [
                        c[0].max(0),
                        c[1].max(0),
                        c[2].min(size[0] as i32),
                        c[3].min(size[1] as i32),
                    ]
                }
                Draw::ResetClip => clip = [0, 0, size[0] as i32, size[1] as i32],
                Draw::Fill {
                    x,
                    y,
                    width,
                    height,
                    colour,
                    blend: _,
                } => {
                    let q = solid(
                        [
                            *x as f32,
                            *y as f32,
                            x.wrapping_add(*width) as f32,
                            y.wrapping_add(*height) as f32,
                        ],
                        *colour,
                        clip,
                        size,
                    );
                    push(3, q);
                }
                Draw::Line {
                    x,
                    y,
                    length,
                    vertical,
                    colour,
                } => push(
                    3,
                    line_quad(*x, *y, *length, *vertical, *colour, clip, size),
                ),
                Draw::Text {
                    font,
                    text,
                    x,
                    y,
                    right,
                    colour,
                    shadow,
                } => {
                    let id = match font {
                        Font::P11 => 0,
                        Font::P12 => 1,
                        Font::B12 => 2,
                    };
                    let m = &self.fonts[id];
                    let x = if *right {
                        x.wrapping_sub(m.width_utf16(text, None)?)
                    } else {
                        *x
                    };
                    let mut records = Vec::new();
                    font_layout::line(
                        m,
                        font_layout::LineAt {
                            text,
                            x,
                            baseline: *y,
                        },
                        &mut Style::new(*colour, *shadow),
                        &Images::default(),
                        false,
                        &mut records,
                    )?;
                    for record in records {
                        match record {
                            font_layout::Draw::Glyph { .. } => {
                                push(id, text_render::glyph(m, &record, clip, size))
                            }
                            font_layout::Draw::Line {
                                x,
                                y,
                                width,
                                colour,
                            } => push(3, line_quad(x, y, width, false, colour, clip, size)),
                            font_layout::Draw::Image { .. } => {
                                anyhow::bail!("console icon resource not installed")
                            }
                        }
                    }
                }
            }
        }
        self.batches = batches
            .into_iter()
            .map(|(texture, q)| Batch {
                texture,
                mesh: self.pipeline.mesh(device, &q),
            })
            .collect();
        self.last = plan;
        self.size = size;
        Ok(())
    }
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        if self.batches.is_empty() {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("developer console"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        for b in &self.batches {
            self.pipeline
                .draw(&mut pass, &self.textures[b.texture], (&b.mesh).into());
        }
    }
}
/// The normal batched profile ignores requested sprite blend modes and flushes
/// all fills, glyphs and lines with mode 1, including the console scrollbar.
fn solid(rect: [f32; 4], colour: i32, clip: [i32; 4], size: [u32; 2]) -> Option<[Vertex; 4]> {
    if clip[0] >= clip[2] || clip[1] >= clip[3] {
        return None;
    }
    text_render::quad(rect, [0., 0., 1., 1.], colour as u32, clip, size)
}
/// Line drawing specialized to console/font axis-aligned lines. The
/// client's half-pixel horizontal edges land on pixel centres. GL's
/// bottom-edge ownership differs from Metal's top-edge ownership after the
/// viewport Y flip. Cover the same integer pixel row explicitly; the OpenGL
/// console oracle checks this without relaxing pixel tolerances.
fn line_quad(
    x: i32,
    y: i32,
    len: i32,
    vertical: bool,
    colour: i32,
    clip: [i32; 4],
    size: [u32; 2],
) -> Option<[Vertex; 4]> {
    if len <= 0 {
        return None;
    }
    let rect = if vertical {
        [
            x as f32 + 0.5,
            y as f32,
            x as f32 + 1.5,
            y.wrapping_add(len).wrapping_add(1) as f32,
        ]
    } else {
        [
            x as f32,
            y as f32 + 1.0,
            x.wrapping_add(len).wrapping_add(1) as f32,
            y as f32 + 2.0,
        ]
    };
    solid(rect, colour, clip, size)
}
