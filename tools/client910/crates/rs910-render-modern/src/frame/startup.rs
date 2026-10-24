//! Creating the renderer off the render thread (performance plan P5).
//!
//! `ModernRenderer::new` compiles every shader module and pipeline the
//! frame draws with: about 40 ms with a warm Metal shader cache, 2 s cold
//! (the first run after a shader or driver change; it creates its
//! independent pipelines at once, `frame::compile`, which wgpu 22's Metal
//! backend could not use: it compiled under the device's lock, 6.8 s cold).
//! wgpu's `PipelineCache` is still a no-op on Metal; the OS's shader cache
//! is what makes the warm case fast. Doing the work before it is needed
//! still helps: the shell starts a [`Startup`] when the session's
//! anti-aliasing level first arrives (the login screens), and takes the
//! renderer at the first scene frame, which then waits only for what is
//! left.
//!
//! The compiles take the cores the login screens' render thread uses, so a
//! cold compile on this thread slows the login screens it overlaps for
//! about the time it runs.
//!
//! With a warm cache (the renderer made within [`WARM_MS`]) the thread then
//! builds the pipeline sets of the other sample counts the client can
//! switch to (`ModernRenderer::prepare_sample_counts`), one count at a time,
//! so an anti-aliasing change later compiles nothing; when the renderer is
//! wanted first, the thread stops after the count at hand. A cold cache
//! skips them (about a second of compiles each; a skipped count compiles at
//! its change, as before). The renderer the thread hands over
//! is the one `ModernRenderer::new` makes on the render thread: the same
//! pipelines, resources and settings, nothing drawn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::frame::ModernRenderer;

/// The longest renderer creation that counts as a warm shader cache (the
/// other counts' sets are then built too); warm takes 40-130 ms, cold
/// 2 s on the M1 Max.
pub const WARM_MS: f64 = 1000.0;

/// A renderer created on the startup thread, not drawn yet.
struct Created {
    renderer: ModernRenderer,
    /// The thread's time: the renderer, then the other counts' sets (ms).
    ms: [f64; 2],
    /// The other counts built.
    counts: usize,
}

/// A renderer being created on a background thread (see the module docs).
pub struct Startup {
    thread: Option<std::thread::JoinHandle<Created>>,
    /// Set when the renderer is wanted: the thread builds no further count.
    wanted: Arc<AtomicBool>,
    samples: u32,
    started: Instant,
}

impl Startup {
    /// Start creating the renderer `ModernRenderer::new(device, queue,
    /// output_format, samples, settings)` makes, then the pipeline sets of
    /// each count of `more` (the anti-aliasing levels the client may switch
    /// to; `samples` itself is skipped).
    #[must_use]
    pub fn spawn(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        output_format: wgpu::TextureFormat,
        samples: u32,
        more: &[u32],
        settings: crate::settings::ModernSettings,
    ) -> Self {
        let more: Vec<u32> = more.iter().copied().filter(|&n| n != samples).collect();
        let wanted = Arc::new(AtomicBool::new(false));
        let stop = wanted.clone();
        let thread = std::thread::Builder::new()
            .name("modern-startup".into())
            .spawn(move || {
                let t = Instant::now();
                let mut renderer =
                    ModernRenderer::new(&device, &queue, output_format, samples, settings);
                let created = t.elapsed().as_secs_f64() * 1000.0;
                let mut counts = 0;
                for &count in &more {
                    if created > WARM_MS || stop.load(Ordering::Acquire) {
                        break;
                    }
                    renderer.prepare_sample_counts(&device, &queue, &[count]);
                    counts += 1;
                }
                assert!(
                    renderer.scene_resources.sky_textures.is_empty(),
                    "a renderer crosses threads before it draws"
                );
                Created {
                    renderer,
                    ms: [created, t.elapsed().as_secs_f64() * 1000.0 - created],
                    counts,
                }
            })
            .expect("spawn the modern renderer's startup thread");
        Self {
            thread: Some(thread),
            wanted,
            samples,
            started: Instant::now(),
        }
    }

    /// The sample count the renderer is created with.
    #[must_use]
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Whether the renderer is ready ([`Self::finish`] will not wait).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// The renderer, drawing into a forward target of `samples` (waits for
    /// the thread if it is still at work; a panic there is raised here).
    #[must_use]
    pub fn finish(
        mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        samples: u32,
    ) -> ModernRenderer {
        let t = Instant::now();
        let ready = self.is_finished();
        self.wanted.store(true, Ordering::Release);
        let created = match self.thread.take().expect("the startup thread").join() {
            Ok(created) => created,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        let mut renderer = created.renderer;
        // A count the thread did not build (the level changed meanwhile to
        // one outside `more`) is built now, as `set_samples` would.
        renderer.prepare_sample_counts(device, queue, &[samples]);
        renderer.set_samples(device, samples);
        log::info!(
            "[modern] renderer created off the render thread: {:.1} ms, {} other sample counts {:.1} ms; taken {:.1} ms after the start ({}, waited {:.1} ms)",
            created.ms[0],
            created.counts,
            created.ms[1],
            self.started.elapsed().as_secs_f64() * 1000.0,
            if ready { "ready" } else { "not ready" },
            t.elapsed().as_secs_f64() * 1000.0
        );
        renderer
    }
}
