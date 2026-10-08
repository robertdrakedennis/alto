//! `rs910-gpu-device`: the GPU device layer of the 910 client port
//! (`docs/architecture.md`). The
//! faithful GPU toolkit and the modern renderer both sit on it; it names no windowing crate (the shell hands its
//! window in as a `wgpu::SurfaceTarget`).
//!
//! - [`gpu_device`]: `Device` (instance, adapter, device, queue, surface and
//!   its configuration, screenshot readback) and its capability answers
//!   (`scene_sample_counts`).
//! - [`uploads`]: the `Uploader` buffer writes (`Device`'s staging belt,
//!   submitted by `Device::submit`; `wgpu::Queue` writes directly).
//! - [`client_watch_gpu`], [`ui_preferences_metric_gpu`]: the device halves
//!   of the UI's capability queries (the input telemetry's texture formats,
//!   the performance metric; the seam is
//!   `rs910_toolkit::performance_metric::RendererProbe`, which the shell's
//!   active toolkit implements with [`ui_preferences_metric_gpu::measure`]).
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::gpu_device::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod client_watch_gpu;
pub mod gpu_device;
pub mod health;
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;
pub mod ui_preferences_metric_gpu;
pub mod uploads;

// The moved code names these through `crate::` (like client910's facades).
use rs910_core::{logic_clock, png_out};
use rs910_toolkit::{compressed_texture_format, performance_metric};
