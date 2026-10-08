//! The output seam: a player writes its mixed bytes to a [`Sink`].
//! [`DeviceSink`] feeds the shared `cpal` output stream (the device thread
//! resamples each sink to the device rate and sums them, as an OS mixer
//! would for two lines); [`ClockSink`] consumes bytes at the line's real-time
//! rate when no device is available, so voices still drain and the audio
//! state machine keeps running; [`RecordingSink`] (tests) records every
//! byte.
//!
//! `cpal` is used here only (tools/refactor/layers.txt `[fences]`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Lock without propagating a poisoned audio-thread panic to the client.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The line a player writes to.
pub trait Sink: Send {
    /// Bytes writable without blocking.
    fn available(&mut self) -> i32;
    /// Write signed 16-bit little-endian stereo bytes.
    fn write(&mut self, bytes: &[u8]);
}

/// `DeviceSink` status values shared with the device thread.
pub(crate) const DEVICE_OPENING: u8 = 0;
pub(crate) const DEVICE_RUNNING: u8 = 1;
pub(crate) const DEVICE_FAILED: u8 = 2;

/// A line consumed by the shared `cpal` stream. Until the device thread
/// reports the stream running (CoreAudio can take seconds to open), and
/// forever if it fails, the line behaves as a [`ClockSink`].
pub struct DeviceSink {
    pub(crate) ring: Arc<Mutex<VecDeque<i16>>>,
    pub(crate) capacity: usize,
    pub(crate) status: Arc<std::sync::atomic::AtomicU8>,
    pub(crate) clock: ClockSink,
}

impl Sink for DeviceSink {
    fn available(&mut self) -> i32 {
        if self.status.load(Ordering::Acquire) != DEVICE_RUNNING {
            return self.clock.available();
        }
        let queued = lock(&self.ring).len();
        (self.capacity.saturating_sub(queued) * 2) as i32 & !3
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.status.load(Ordering::Acquire) != DEVICE_RUNNING {
            self.clock.write(bytes);
            return;
        }
        let mut ring = lock(&self.ring);
        for pair in bytes.chunks_exact(2) {
            ring.push_back(i16::from_le_bytes([pair[0], pair[1]]));
        }
    }
}

/// A line drained at its real-time byte rate when no output device exists.
pub struct ClockSink {
    bytes_per_second: f64,
    capacity: f64,
    queued: f64,
    last: Instant,
}

impl ClockSink {
    pub fn new(rate: f32, capacity: i32) -> Self {
        Self {
            bytes_per_second: f64::from(rate) * 4.0,
            capacity: f64::from(capacity),
            queued: 0.0,
            last: Instant::now(),
        }
    }
}

impl Sink for ClockSink {
    fn available(&mut self) -> i32 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.queued = (self.queued - elapsed * self.bytes_per_second).max(0.0);
        (self.capacity - self.queued).max(0.0) as i32 & !3
    }

    fn write(&mut self, bytes: &[u8]) {
        self.queued += bytes.len() as f64;
    }
}

/// A line that accepts everything and records it (tests).
#[cfg(any(test, feature = "test-hooks"))]
pub struct RecordingSink {
    pub written: Arc<Mutex<Vec<u8>>>,
    pub capacity: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
impl Sink for RecordingSink {
    fn available(&mut self) -> i32 {
        self.capacity
    }

    fn write(&mut self, bytes: &[u8]) {
        lock(&self.written).extend_from_slice(bytes);
    }
}

/// The output device owner: a thread holding the `cpal` stream that drains
/// the player lines.
pub(crate) struct Device {
    stop: Arc<AtomicBool>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) status: Arc<std::sync::atomic::AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

struct DeviceSource {
    ring: Arc<Mutex<VecDeque<i16>>>,
    rate: f64,
    previous: [f32; 2],
    next: [f32; 2],
    phase: f64,
}

impl DeviceSource {
    fn frame(&mut self, device_rate: f64, ring: &mut VecDeque<i16>) -> [f32; 2] {
        while self.phase >= 1.0 {
            self.previous = self.next;
            if ring.len() >= 2 {
                let l = ring.pop_front().unwrap_or(0);
                let r = ring.pop_front().unwrap_or(0);
                self.next = [f32::from(l) / 32768.0, f32::from(r) / 32768.0];
            } else {
                self.next = [0.0, 0.0];
            }
            self.phase -= 1.0;
        }
        let t = self.phase as f32;
        let out = [
            self.previous[0] + (self.next[0] - self.previous[0]) * t,
            self.previous[1] + (self.next[1] - self.previous[1]) * t,
        ];
        self.phase += self.rate / device_rate;
        out
    }
}

fn fill_device(sources: &mut [DeviceSource], device_rate: f64, channels: usize, out: &mut [f32]) {
    let mut rings: Vec<_> = sources.iter().map(|s| s.ring.clone()).collect();
    let mut guards: Vec<_> = rings.iter_mut().map(|r| lock(r)).collect();
    for frame in out.chunks_mut(channels) {
        let mut sum = [0.0_f32; 2];
        for (source, ring) in sources.iter_mut().zip(guards.iter_mut()) {
            let value = source.frame(device_rate, ring);
            sum[0] += value[0];
            sum[1] += value[1];
        }
        for (c, slot) in frame.iter_mut().enumerate() {
            let value = if channels == 1 {
                (sum[0] + sum[1]) * 0.5
            } else {
                sum[c.min(1)]
            };
            *slot = value.clamp(-1.0, 1.0);
        }
    }
}

/// Open the default output device on its own thread and start the stream
/// draining `rings` (one per player, with its rate). `status` becomes
/// running or failed (logged once) when the open completes.
pub(crate) fn spawn_device(
    rings: Vec<(Arc<Mutex<VecDeque<i16>>>, f32)>,
    status: Arc<std::sync::atomic::AtomicU8>,
) -> std::io::Result<Device> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let device_status = status.clone();
    let thread = std::thread::Builder::new()
        .name("client910-audio-device".into())
        .spawn(move || {
            let build = || -> anyhow::Result<cpal::Stream> {
                let host = cpal::default_host();
                let device = host
                    .default_output_device()
                    .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
                let supported = device.default_output_config()?;
                let config: cpal::StreamConfig = supported.config();
                let device_rate = f64::from(config.sample_rate.0);
                let channels = usize::from(config.channels).max(1);
                let mut sources: Vec<DeviceSource> = rings
                    .iter()
                    .map(|(ring, rate)| DeviceSource {
                        ring: ring.clone(),
                        rate: f64::from(*rate),
                        previous: [0.0; 2],
                        next: [0.0; 2],
                        phase: 1.0,
                    })
                    .collect();
                let on_error = |error| log::warn!("[client910] audio device error: {error}");
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::F32 => device.build_output_stream(
                        &config,
                        move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            fill_device(&mut sources, device_rate, channels, out);
                        },
                        on_error,
                        None,
                    )?,
                    cpal::SampleFormat::I16 => {
                        let mut scratch = Vec::new();
                        device.build_output_stream(
                            &config,
                            move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                                scratch.resize(out.len(), 0.0);
                                fill_device(&mut sources, device_rate, channels, &mut scratch);
                                for (o, v) in out.iter_mut().zip(&scratch) {
                                    *o = (v * 32767.0) as i16;
                                }
                            },
                            on_error,
                            None,
                        )?
                    }
                    other => anyhow::bail!("unsupported device sample format {other:?}"),
                };
                stream.play()?;
                Ok(stream)
            };
            match build() {
                Ok(stream) => {
                    status.store(DEVICE_RUNNING, Ordering::Release);
                    while !thread_stop.load(Ordering::Acquire) {
                        std::thread::park_timeout(Duration::from_millis(200));
                    }
                    drop(stream);
                }
                Err(error) => {
                    status.store(DEVICE_FAILED, Ordering::Release);
                    log::warn!(
                        "[client910] audio output device unavailable ({error:#}); mixing to a silent clock"
                    );
                }
            }
        })?;
    Ok(Device {
        stop,
        status: device_status,
        thread: Some(thread),
    })
}
