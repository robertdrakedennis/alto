//! The wgpu half of `ui_paint`: [`Renderer`] uploads the sprites, fonts and
//! masks a [`Plan`] names and draws its quads in painter order with the
//! batch shader (scissors from the clip bounds, masks from the mask
//! sprites). Split out in Phase 3.1: the display lists (`Op`, `Recording`,
//! `Plan`, `Painter`) are rs910-toolkit's `ui_paint`.
use crate::{
    text_render::Vertex,
    text_render_gpu,
    ui_paint::{framebuffer_bounds, Image, MaskRef, Owner, Plan},
};
use anyhow::Result;
use std::{collections::HashMap, rc::Rc};

/// A run of the plan's consecutive quads drawn with one texture, scissor
/// and mask: quads `quads` of the painter's [`text_render_gpu::QuadBuffer`]
/// (the plan's quad indices).
struct Batch {
    first_quad: usize,
    image: Image,
    clip: [i32; 4],
    quads: std::ops::Range<u32>,
    mask: Option<MaskKey>,
}

/// A mask bind group's inputs: the mask sprite (`MaskRef::key`), the
/// canvas-to-target scale (`f32` bits) and the opaque-source flag.
type MaskKey = ((usize, [i32; 2]), [u32; 2], bool);

pub struct Renderer {
    pipeline: std::sync::Arc<text_render_gpu::Pipeline>,
    textures: HashMap<(u8, usize), (Owner, text_render_gpu::Texture)>,
    batches: Vec<Batch>,
    size: [u32; 2],
    /// [`Image::External`] targets by id.
    externals: HashMap<u64, text_render_gpu::Texture>,
    /// This frame's quad vertices in plan order (reused scratch).
    vertices: Vec<Vertex>,
    /// The vertex/index buffers the batches draw from (grown, never shrunk).
    quads: Option<text_render_gpu::QuadBuffer>,
    /// Mask bind groups by their inputs, kept while the mask sprite lives
    /// (the parameters are constant per key).
    masks: HashMap<MaskKey, (Owner, wgpu::BindGroup)>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_pipeline(std::sync::Arc::new(text_render_gpu::Pipeline::new(
            device, format,
        )))
    }
    /// A painter drawing with a shared pipeline set
    /// (`text_render_gpu::Pipeline::cached`).
    pub fn with_pipeline(pipeline: std::sync::Arc<text_render_gpu::Pipeline>) -> Self {
        Self {
            pipeline,
            textures: HashMap::new(),
            batches: vec![],
            size: [0; 2],
            externals: HashMap::new(),
            vertices: Vec::new(),
            quads: None,
            masks: HashMap::new(),
        }
    }
    /// Register a rendered texture so plans can draw it as [`Image::External`].
    pub fn register_external(&mut self, device: &wgpu::Device, id: u64, view: wgpu::TextureView) {
        let texture = self.pipeline.wrap(device, view);
        self.externals.insert(id, texture);
    }
    pub fn unregister_external(&mut self, id: u64) {
        self.externals.remove(&id);
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        plan: Plan,
    ) -> Result<()> {
        self.prepare_split(device, queue, plan, None)
    }
    pub fn prepare_split(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        plan: Plan,
        split: Option<usize>,
    ) -> Result<()> {
        let target = plan.size;
        self.prepare_split_target(device, queue, plan, split, target)
    }
    /// Layout/quad coordinates remain canvas pixels. GPU scissors and mask
    /// fragment positions must use the physical target instead.
    pub fn prepare_split_target(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        plan: Plan,
        split: Option<usize>,
        target: [u32; 2],
    ) -> Result<()> {
        self.prepare_boundaries_target(device, queue, plan, split.as_slice(), target)
    }
    /// Flush otherwise-compatible batches at every intervening 3D model.
    pub fn prepare_boundaries_target(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        plan: Plan,
        boundaries: &[usize],
        target: [u32; 2],
    ) -> Result<()> {
        anyhow::ensure!(
            plan.size.into_iter().chain(target).all(|v| v > 0),
            "invalid UI canvas/target size"
        );
        // Taken so that an error leaves no batches (as when they were built
        // aside and assigned at the end).
        let mut batches = std::mem::take(&mut self.batches);
        batches.clear();
        self.textures.retain(|_, (owner, _)| owner.alive());
        self.masks.retain(|_, (owner, _)| owner.alive());
        self.vertices.clear();
        self.size = target;
        let canvas = plan.size;
        let scale = [
            target[0] as f32 / canvas[0] as f32,
            target[1] as f32 / canvas[1] as f32,
        ];
        let scale_bits = scale.map(f32::to_bits);
        for (index, q) in plan.quads.into_iter().enumerate() {
            let key = q.image.key();
            if !matches!(q.image, Image::External(_)) && !self.textures.contains_key(&key) {
                let texture = match &q.image {
                    Image::Sprite(s) => {
                        anyhow::ensure!(s.size.iter().all(|v| *v > 0), "sprite upload dimensions");
                        let argb = s.argb.iter().map(|v| *v as u32).collect::<Vec<_>>();
                        self.pipeline.upload_argb(
                            device,
                            queue.queue(),
                            s.size.map(|v| v as u32),
                            &argb,
                            wgpu::FilterMode::Linear,
                        )
                    }
                    Image::Font(f) => self.pipeline.upload(device, queue.queue(), &f.atlas),
                    Image::White => self.pipeline.upload_argb(
                        device,
                        queue.queue(),
                        [1, 1],
                        &[u32::MAX],
                        wgpu::FilterMode::Linear,
                    ),
                    Image::External(_) => unreachable!(),
                };
                self.textures.insert(key, (q.image.weak(), texture));
            }
            if let Image::External(id) = q.image {
                anyhow::ensure!(
                    self.externals.contains_key(&id),
                    "external UI texture {id} is not registered"
                );
            }
            if let Some(mask) = &q.mask {
                let mkey = (0u8, Rc::as_ptr(&mask.sprite) as usize);
                if !self.textures.contains_key(&mkey) {
                    let s = &mask.sprite;
                    anyhow::ensure!(s.size.iter().all(|v| *v > 0), "mask upload dimensions");
                    let argb = s.argb.iter().map(|v| *v as u32).collect::<Vec<_>>();
                    let texture = self.pipeline.upload_argb(
                        device,
                        queue.queue(),
                        s.size.map(|v| v as u32),
                        &argb,
                        wgpu::FilterMode::Linear,
                    );
                    self.textures
                        .insert(mkey, (Owner::Sprite(Rc::downgrade(s)), texture));
                }
            }
            let mask_key = q.mask.as_ref().map(MaskRef::key);
            let quad = index as u32;
            if batches.last().is_none_or(|b| {
                b.image.key() != key
                    || b.clip != q.clip
                    || b.mask.map(|(mask, _, _)| mask) != mask_key
            }) || boundaries.contains(&index)
            {
                let opaque = matches!(q.image, Image::External(_));
                let mask = q.mask.map(|m| {
                    let mask_key = (m.key(), scale_bits, opaque);
                    self.masks.entry(mask_key).or_insert_with(|| {
                        let texture = &self.textures[&(0u8, Rc::as_ptr(&m.sprite) as usize)].1;
                        let [w, h] = m.sprite.full_size();
                        let group = self.pipeline.mask_bind_group(
                            device,
                            texture,
                            [
                                m.origin[0] as f32 * scale[0],
                                m.origin[1] as f32 * scale[1],
                                w as f32 * scale[0],
                                h as f32 * scale[1],
                            ],
                            opaque,
                        );
                        (Owner::Sprite(Rc::downgrade(&m.sprite)), group)
                    });
                    mask_key
                });
                batches.push(Batch {
                    first_quad: index,
                    image: q.image,
                    clip: q.clip,
                    quads: quad..quad,
                    mask,
                });
            }
            batches.last_mut().unwrap().quads.end = quad + 1;
            self.vertices.extend_from_slice(&q.vertices);
        }
        for batch in &mut batches {
            batch.clip = framebuffer_bounds(batch.clip, canvas, target);
        }
        text_render_gpu::QuadBuffer::upload(&mut self.quads, device, queue, &self.vertices);
        self.batches = batches;
        Ok(())
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.draw_range(pass, 0..usize::MAX);
    }
    pub fn draw_range<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        range: std::ops::Range<usize>,
    ) {
        for b in self
            .batches
            .iter()
            .filter(|b| range.contains(&b.first_quad))
        {
            let c = b.clip;
            let x = c[0].max(0) as u32;
            let y = c[1].max(0) as u32;
            let right = c[2].max(0).min(self.size[0] as i32) as u32;
            let bottom = c[3].max(0).min(self.size[1] as i32) as u32;
            if x >= right || y >= bottom {
                continue;
            }
            pass.set_scissor_rect(x, y, right - x, bottom - y);
            let texture = match b.image {
                Image::External(id) => &self.externals[&id],
                _ => &self.textures[&b.image.key()].1,
            };
            let quads = self.quads.as_ref().expect("prepared quads");
            let geometry = text_render_gpu::Geometry::Quads(quads, b.quads.clone());
            match (&b.mask, &b.image) {
                (Some(mask), _) => {
                    self.pipeline
                        .draw_masked(pass, texture, &self.masks[mask].1, geometry)
                }
                (None, Image::External(_)) => self.pipeline.draw_opaque(pass, texture, geometry),
                (None, _) => self.pipeline.draw(pass, texture, geometry),
            }
        }
    }
}

#[cfg(test)]
#[path = "ui_paint_tests.rs"]
mod tests;

/// Draw-only painter data. CPU sprite/font owner registries stay on main;
/// this packet retains the exact GPU objects prepared for its frame.
pub struct PaintFrame {
    pipeline: std::sync::Arc<text_render_gpu::Pipeline>,
    batches: Vec<FrameBatch>,
    size: [u32; 2],
    quads: Option<text_render_gpu::QuadBuffer>,
}
struct FrameBatch {
    first_quad: usize,
    clip: [i32; 4],
    quads: std::ops::Range<u32>,
    texture: text_render_gpu::Texture,
    mask: Option<wgpu::BindGroup>,
    opaque: bool,
}
/// The same ordered painter operation for retained and owned GPU frames.
pub trait PaintDraw {
    fn draw_range<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, range: std::ops::Range<usize>);
}
impl PaintDraw for Renderer {
    fn draw_range<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, range: std::ops::Range<usize>) {
        Renderer::draw_range(self, pass, range);
    }
}
impl Renderer {
    pub fn frame(&self) -> PaintFrame {
        PaintFrame {
            pipeline: self.pipeline.clone(),
            size: self.size,
            quads: self.quads.clone(),
            batches: self
                .batches
                .iter()
                .map(|batch| FrameBatch {
                    first_quad: batch.first_quad,
                    clip: batch.clip,
                    quads: batch.quads.clone(),
                    texture: match batch.image {
                        Image::External(id) => self.externals[&id].clone(),
                        _ => self.textures[&batch.image.key()].1.clone(),
                    },
                    mask: batch.mask.map(|key| self.masks[&key].1.clone()),
                    opaque: matches!(batch.image, Image::External(_)),
                })
                .collect(),
        }
    }
}
impl PaintFrame {
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.draw_range(pass, 0..usize::MAX);
    }
}
impl PaintDraw for PaintFrame {
    fn draw_range<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, range: std::ops::Range<usize>) {
        for batch in self
            .batches
            .iter()
            .filter(|batch| range.contains(&batch.first_quad))
        {
            let [left, top, right, bottom] = batch.clip;
            let left = left.max(0) as u32;
            let top = top.max(0) as u32;
            let right = right.max(0).min(self.size[0] as i32) as u32;
            let bottom = bottom.max(0).min(self.size[1] as i32) as u32;
            if left >= right || top >= bottom {
                continue;
            }
            pass.set_scissor_rect(left, top, right - left, bottom - top);
            let quads = self.quads.as_ref().expect("prepared frame quads");
            let geometry = text_render_gpu::Geometry::Quads(quads, batch.quads.clone());
            if let Some(mask) = &batch.mask {
                self.pipeline
                    .draw_masked(pass, &batch.texture, mask, geometry);
            } else if batch.opaque {
                self.pipeline.draw_opaque(pass, &batch.texture, geometry);
            } else {
                self.pipeline.draw(pass, &batch.texture, geometry);
            }
        }
    }
}
