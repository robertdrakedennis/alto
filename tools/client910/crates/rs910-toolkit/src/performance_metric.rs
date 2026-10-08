//! The performance-metric benchmark body on a native device: the lit
//! benchmark model from the graphics defaults is drawn through the measured
//! toolkit in a 16-row triangle of placements until the millisecond budget
//! expires, then the work is flushed and draws per second are returned.
//!
//! The model is built exactly as the GLX toolkit builds it (flags 2048,
//! ambient 64, contrast 768, detail 64) and its upload streams are drawn once
//! per model draw. The device is created for the measured toolkit as a
//! profiling toolkit; the surface is an offscreen canvas-sized target.
//!
//! This module is the model build (Phase 3.2: from client910's
//! `ui_preferences_metric`, which the preferences name `metric`) and the
//! seam the profiling commands reach the renderer through: [`RendererProbe`]
//! is what the shell lends the interface host while a script runs, and the
//! active renderer implements it (the faithful device half is
//! rs910-gpu-device's `ui_preferences_metric_gpu`, the modern one
//! rs910-render-modern's `frame::benchmark`). The host sees no renderer
//! types: only a message box plan to present and a benchmark to run.
use crate::ui_paint::Plan;
use anyhow::{Context, Result};
use std::path::Path;

/// What a message box is painted over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// The last game frame, which stays on screen around the box.
    LastFrame,
    /// A cleared canvas: the toolkit was just replaced, so nothing of the
    /// last frame is left to show.
    Black,
}

/// One device benchmark: the model drawn in the profiling layout (16 rows of
/// placements, a triangle of 136 per frame) at `canvas` size until `budget_ms`
/// has passed, with the camera's near and far clip distances.
#[derive(Clone, Copy)]
pub struct Benchmark<'a> {
    pub model: &'a MetricModel,
    pub canvas: [u32; 2],
    pub near: f32,
    pub far: f32,
    pub budget_ms: i64,
}

/// The renderer's side of the profiling commands (`detailget_performance_metric`
/// and the auto-setup's probes). The shell lends one to the interface host for
/// the logic cycle; a script that profiles calls it synchronously, so its
/// result is on the script's stack when the command returns, as with the
/// original client (which also blocks the thread for the measurement).
pub trait RendererProbe {
    /// The canvas the box is laid out on.
    fn canvas_size(&self) -> [u32; 2];
    /// The client frame the box's alignment is relative to.
    fn frame_size(&self) -> [i32; 2];
    /// Paints `plan` (the box) over `backdrop` and presents it at once, so it
    /// stays on screen while the logic cycle goes on measuring.
    fn present_message_box(&mut self, plan: Plan, backdrop: Backdrop) -> Result<()>;
    /// Runs `request` on the device and returns draws per second.
    fn benchmark(&mut self, request: &Benchmark<'_>) -> Result<i32>;
}

/// Upload streams of the lit benchmark model.
pub struct MetricModel {
    pub vertices: Vec<MetricVertex>,
    pub indices: Vec<u32>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MetricVertex {
    pos: [f32; 3],
    normal: [f32; 3],
    colour: u32,
}

impl MetricVertex {
    pub fn pos(&self) -> [f32; 3] {
        self.pos
    }
    pub fn normal(&self) -> [f32; 3] {
        self.normal
    }
    /// The RGBA colour, the red byte lowest.
    pub fn colour(&self) -> u32 {
        self.colour
    }
    /// A vertex at `pos` with surface `normal` and an RGBA `colour` (the red
    /// byte lowest).
    pub fn new(pos: [f32; 3], normal: [f32; 3], colour: u32) -> Self {
        Self {
            pos,
            normal,
            colour,
        }
    }
}

/// Load the unlit model from the models archive and create the toolkit model.
pub fn load_model(pack_root: &Path, id: i32) -> Result<MetricModel> {
    let pack = crate::cache::Pack::open(pack_root);
    let unlit = crate::modelunlit::ModelUnlit::load(&pack, id as u32)?;
    let materials = crate::texture::MaterialStore::load(&pack).context("materials")?;
    let billboards = crate::billboard::BillboardStore::load(&pack).context("billboards")?;
    let emitters = crate::particle::EmitterStore::load(&pack).context("emitters")?;
    let model = crate::gpumodel::GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
        },
        &unlit,
        crate::gpumodel::BuildParams {
            flags: 2048,
            ambient: 64,
            contrast: 768,
            detail: 64,
        },
    )?;
    let positions = model.position_stream();
    let normals = model.normal_stream();
    let colours = model.colour_stream(&materials)?;
    let vertices = positions
        .iter()
        .zip(&normals)
        .zip(&colours)
        .map(|((&pos, &normal), &argb)| {
            let a = (argb as u32 >> 24) & 0xff;
            let r = (argb as u32 >> 16) & 0xff;
            let g = (argb as u32 >> 8) & 0xff;
            let b = argb as u32 & 0xff;
            MetricVertex {
                pos,
                normal,
                colour: r | g << 8 | b << 16 | a << 24,
            }
        })
        .collect();
    let indices = model.index_stream().into_iter().map(u32::from).collect();
    Ok(MetricModel { vertices, indices })
}
