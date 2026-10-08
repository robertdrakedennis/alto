//! Opt-in fixed-camera frame diagnostics (`CLIENT910_PROFILE=1`).
//! CPU phases are accumulated for 120 frames. GPU timestamps sample the
//! last frame of each block; readback waits only in profiling mode.
use std::time::Instant;

pub struct FrameProfile {
    frames: u64,
    start: Instant,
    cpu_ms: [f64; 5],
    gpu: Option<GpuTimer>,
    /// `cache::stats::snapshot()` at the start of the block.
    pack_stats: (u64, u64),
}

struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}

impl FrameProfile {
    pub fn enabled() -> bool {
        crate::render_debug_flags::flags().profile
    }

    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !Self::enabled() {
            return None;
        }
        let gpu = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| GpuTimer {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("frame timing"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timing resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timing readback"),
                    size: 16,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            });
        log::info!(
            "[perf] GPU timestamps={} shared_discard={}",
            gpu.is_some(),
            crate::render_debug_flags::flags().profile_shared_discard
        );
        Some(Self {
            frames: 0,
            start: Instant::now(),
            cpu_ms: [0.0; 5],
            gpu,
            pack_stats: crate::cache::stats::snapshot(),
        })
    }

    pub fn begin(&mut self) {
        if self.frames == 0 {
            self.start = Instant::now();
        }
    }

    fn sample(&self) -> bool {
        self.frames % 120 == 119
    }

    pub fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if !self.sample() {
            return None;
        }
        self.gpu
            .as_ref()
            .map(|gpu| wgpu::RenderPassTimestampWrites {
                query_set: &gpu.queries,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            })
    }

    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.sample() {
            if let Some(gpu) = &self.gpu {
                encoder.resolve_query_set(&gpu.queries, 0..2, &gpu.resolve, 0);
                encoder.copy_buffer_to_buffer(&gpu.resolve, 0, &gpu.readback, 0, 16);
            }
        }
    }

    pub fn finish(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        phases: [f64; 5],
        draws: usize,
        size: (u32, u32),
    ) {
        for (sum, phase) in self.cpu_ms.iter_mut().zip(phases) {
            *sum += phase;
        }
        let sample = self.sample();
        self.frames += 1;
        if !sample {
            return;
        }
        let seconds = self.start.elapsed().as_secs_f64();
        let mut gpu_ms = None;
        if let Some(gpu) = &self.gpu {
            let (tx, rx) = std::sync::mpsc::channel();
            gpu.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            if matches!(rx.recv(), Ok(Ok(()))) {
                let data = gpu
                    .readback
                    .slice(..)
                    .get_mapped_range()
                    .expect("mapped range");
                let begin = u64::from_le_bytes(data[0..8].try_into().unwrap());
                let end = u64::from_le_bytes(data[8..16].try_into().unwrap());
                gpu_ms = Some(
                    end.saturating_sub(begin) as f64 * f64::from(queue.get_timestamp_period())
                        / 1e6,
                );
                drop(data);
                gpu.readback.unmap();
            }
        }
        let pack_stats = crate::cache::stats::snapshot();
        log::info!("[perf] frame={} size={}x{} fps={:.1} interval_ms={:.2} cpu_ms[prepare,acquire,encode,submit,present]={:?} gpu_sample_ms={:?} scene_batches={} pack_handles_per_frame={:.2} index_decodes_per_frame={:.2}",
            self.frames, size.0, size.1, 120.0 / seconds, seconds * 1000.0 / 120.0,
            self.cpu_ms.map(|v| (v / 120.0 * 100.0).round() / 100.0), gpu_ms, draws,
            (pack_stats.0 - self.pack_stats.0) as f64 / 120.0,
            (pack_stats.1 - self.pack_stats.1) as f64 / 120.0);
        self.pack_stats = pack_stats;
        self.cpu_ms = [0.0; 5];
        self.start = Instant::now();
    }
}

/// Per-pass GPU times for the engine profiler (`rs910_core::profile`, lane
/// E-A4), on adapters with [`PassTimer::FEATURES`]. Each mark is a render
/// pass that clears a 1x1 target of its own and writes one timestamp at its
/// start, so the time between two marks is the GPU work encoded between
/// them. (On Apple GPUs through wgpu 22 an empty compute pass, and a
/// `write_timestamp` between passes, which wgpu-hal samples in an empty
/// blit encoder, record zeros; a pass with a clear is sampled.) Each slot
/// has its own query set, so its queries start at 0 (wgpu-hal 22's Metal
/// resolve takes the range end as a length). A frame's marks resolve
/// into one of [`PassTimer::SLOTS`] readback buffers that map without
/// waiting; [`PassTimer::begin`] hands the finished ones to the profiler
/// (`rs910_core::profile::gpu_pass_times`, a few frames later). A frame
/// whose slot is still in flight is not timed. Nothing here runs unless the
/// profiler records, and it never changes what the passes draw.
pub struct PassTimer {
    resolve: wgpu::Buffer,
    /// The marks' 1x1 target.
    marker: wgpu::TextureView,
    slots: Vec<TimerSlot>,
    /// Nanoseconds per timestamp tick (`Queue::get_timestamp_period`).
    period: f64,
    next: usize,
    /// The slot of the frame being encoded.
    active: Option<usize>,
}

struct TimerSlot {
    queries: wgpu::QuerySet,
    readback: wgpu::Buffer,
    /// 0 idle, 1 mapping, 2 mapped, 3 map failed.
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    frame: u64,
    names: [&'static str; PassTimer::MARKS],
    marks: usize,
}

impl PassTimer {
    /// Timestamps per frame (the last one ends the last pass).
    pub const MARKS: usize = 8;
    /// Frames in flight.
    pub const SLOTS: usize = 4;
    /// `wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT`.
    const STRIDE: u64 = 256;
    /// The device features the timer needs.
    pub const FEATURES: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY;

    /// A timer on `device`, or `None` without [`PassTimer::FEATURES`].
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(Self::FEATURES) {
            return None;
        }
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profile pass resolve"),
            size: Self::STRIDE * Self::SLOTS as u64,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..Self::SLOTS)
            .map(|_| TimerSlot {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("profile pass marks"),
                    ty: wgpu::QueryType::Timestamp,
                    count: Self::MARKS as u32,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("profile pass readback"),
                    size: (Self::MARKS * 8) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Default::default(),
                frame: 0,
                names: [""; Self::MARKS],
                marks: 0,
            })
            .collect();
        let marker = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("profile mark target"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Some(Self {
            resolve,
            marker,
            slots,
            period: f64::from(queue.get_timestamp_period()),
            next: 0,
            active: None,
        })
    }

    /// Hand the finished readbacks to the profiler, then time this frame
    /// (profiler frame `frame`) when its slot is free; `None` (the profiler
    /// is off) times nothing.
    pub fn begin(&mut self, device: &wgpu::Device, frame: Option<u64>) {
        use std::sync::atomic::Ordering;
        self.active = None;
        let Some(frame) = frame else {
            return;
        };
        let _ = device.poll(wgpu::PollType::Poll);
        for (timed, passes) in self.collect() {
            rs910_core::profile::gpu_pass_times(timed, &passes);
        }
        let slot = &mut self.slots[self.next];
        self.active = (slot.state.load(Ordering::Acquire) == 0).then(|| {
            slot.frame = frame;
            slot.marks = 0;
            self.next
        });
    }

    /// The frames whose readback has mapped: each frame's passes as
    /// `(name, start_ns, dur_ns)`, start relative to its first mark.
    pub fn collect(&mut self) -> Vec<(u64, Vec<rs910_core::profile::GpuPass>)> {
        use std::sync::atomic::Ordering;
        let mut out = Vec::new();
        for slot in &mut self.slots {
            match slot.state.load(Ordering::Acquire) {
                2 => {
                    let data = slot
                        .readback
                        .slice(..)
                        .get_mapped_range()
                        .expect("mapped range");
                    let ticks: Vec<u64> = data
                        .chunks_exact(8)
                        .take(slot.marks)
                        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                        .collect();
                    drop(data);
                    slot.readback.unmap();
                    let ns =
                        |from: u64, to: u64| (to.saturating_sub(from) as f64 * self.period) as u64;
                    let passes = (0..slot.marks.saturating_sub(1))
                        .map(|i| {
                            (
                                slot.names[i],
                                ns(ticks[0], ticks[i]),
                                ns(ticks[i], ticks[i + 1]),
                            )
                        })
                        .collect();
                    out.push((slot.frame, passes));
                    slot.state.store(0, Ordering::Release);
                }
                3 => slot.state.store(0, Ordering::Release),
                _ => {}
            }
        }
        out
    }

    /// Write the next timestamp: the pass `name` starts here (the frame's
    /// last mark ends the previous pass; its name is unused).
    pub fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, name: &'static str) {
        let Some(active) = self.active else {
            return;
        };
        let slot = &mut self.slots[active];
        if slot.marks == Self::MARKS {
            return;
        }
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("profile mark"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.marker,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                query_set: &slot.queries,
                beginning_of_pass_write_index: Some(slot.marks as u32),
                end_of_pass_write_index: None,
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        slot.names[slot.marks] = name;
        slot.marks += 1;
    }

    /// Resolve this frame's marks into its readback buffer (before
    /// `encoder.finish()`).
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(active) = self.active else {
            return;
        };
        let slot = &self.slots[active];
        if slot.marks < 2 {
            self.active = None;
            return;
        }
        let offset = active as u64 * Self::STRIDE;
        encoder.resolve_query_set(&slot.queries, 0..slot.marks as u32, &self.resolve, offset);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            offset,
            &slot.readback,
            0,
            (slot.marks * 8) as u64,
        );
    }

    /// Map this frame's readback (after the submit that resolved it).
    pub fn submitted(&mut self) {
        use std::sync::atomic::Ordering;
        let Some(active) = self.active.take() else {
            return;
        };
        let slot = &self.slots[active];
        slot.state.store(1, Ordering::Release);
        let state = slot.state.clone();
        slot.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 2 } else { 3 }, Ordering::Release);
            });
        self.next = (active + 1) % Self::SLOTS;
    }
}

#[cfg(test)]
mod tests {
    use super::PassTimer;

    /// The pass timer on the real adapter (GPU test, ignored by default):
    /// marks around two clears of a large target give that frame's two
    /// passes, with a positive total, once the readback maps.
    #[test]
    #[ignore = "needs a GPU with TIMESTAMP_QUERY"]
    fn pass_timer_times_passes_between_marks() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("no wgpu adapter: this test needs a desktop GPU");
        assert!(
            adapter.features().contains(PassTimer::FEATURES),
            "adapter without timestamp queries"
        );
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: PassTimer::FEATURES,
            ..Default::default()
        }))
        .unwrap();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4096,
                height: 4096,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut timer = PassTimer::new(&device, &queue).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        timer.begin(&device, Some(7));
        for name in ["first", "second"] {
            timer.mark(&mut encoder, name);
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
        }
        timer.mark(&mut encoder, "");
        timer.resolve(&mut encoder);
        queue.submit(Some(encoder.finish()));
        timer.submitted();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let frames = timer.collect();
        eprintln!("pass timer: {frames:?}");
        assert_eq!(frames.len(), 1);
        let (frame, passes) = &frames[0];
        assert_eq!(*frame, 7);
        let names: Vec<&str> = passes.iter().map(|p| p.0).collect();
        assert_eq!(names, ["first", "second"]);
        assert_eq!(passes[0].1, 0);
        assert!(passes.iter().map(|p| p.2).sum::<u64>() > 0, "{passes:?}");
    }
}
