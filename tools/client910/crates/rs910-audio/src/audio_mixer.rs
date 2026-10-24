//! The mixer: two players (44.1 kHz and 22.05 kHz) that mix every playing
//! sample into a ring and write it to their output line, and the sample ring
//! buffers the voices write their PCM into.

use crate::audio_sink::{lock, Sink};
use crate::audio_vorbis::{Endianness, SampleFormat};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static SAMPLE_IDS: AtomicU64 = AtomicU64::new(1);

/// One voice's ring buffer of PCM waiting to be mixed.
struct SampleRing {
    id: u64,
    buffer: Vec<u8>,
    read: usize,
    write: usize,
    /// Free bytes in the ring.
    free: i32,
    /// Bytes mixable at the player's rate this round.
    mixable: i32,
    /// The sample's playback rate (its file rate times the sound's rate).
    rate: f32,
    bits: i32,
    channels: i32,
}

impl SampleRing {
    /// Append `len` bytes of 16-bit PCM from `data[offset..]` to the ring.
    fn write(&mut self, data: &[u8], offset: usize, len: usize) {
        let mut i = offset;
        while i < offset + len {
            self.buffer[self.write] = data[i];
            self.buffer[self.write + 1] = data[i + 1];
            i += 2;
            self.write = (self.write + 2) % self.buffer.len();
            self.free -= 2;
        }
    }

    fn short_at(&self, at: usize, width: usize) -> i16 {
        let mut value = 0_i16;
        for k in 0..width {
            value |= (i16::from(self.buffer[at + k])) << (k * 8);
        }
        value
    }
}

/// A mixer for one output line at a fixed sample rate.
pub struct LineMixer {
    line: Box<dyn Sink>,
    /// The mix ring.
    mix: Vec<u8>,
    /// Bytes the line can take now, bytes mixed but not yet written, and
    /// the ring position of the next write.
    available: i32,
    pending: i32,
    position: usize,
    rate: f32,
    bits: i32,
    channels: i32,
    samples: Vec<SampleRing>,
}

impl LineMixer {
    pub fn new(rate: f32, buffer: i32, line: Box<dyn Sink>) -> Self {
        Self {
            line,
            mix: vec![0; buffer as usize],
            available: 0,
            pending: 0,
            position: 0,
            rate,
            bits: 16,
            channels: 2,
            samples: Vec::new(),
        }
    }

    /// Ask the line how much it can take.
    pub(crate) fn refresh_available(&mut self) {
        self.available = self.line.available();
    }

    /// Mix what is ready and write it to the line.
    pub(crate) fn flush(&mut self) {
        self.pending = 0;
        self.mix_samples();
        while self.pending > 0 {
            let mut count = self.pending as usize;
            if self.position + count >= self.mix.len() {
                count = self.mix.len() - self.position;
            }
            self.line
                .write(&self.mix[self.position..self.position + count]);
            self.mix[self.position..self.position + count].fill(0);
            self.position = (self.position + count) % self.mix.len();
            self.pending -= count as i32;
        }
    }

    /// Mix every sample with enough queued bytes into the ring, resampling
    /// linearly when the sample rate differs from the player's.
    fn mix_samples(&mut self) {
        let mut count = i32::MAX;
        let frame = f64::from(self.bits / 8 * self.channels);
        let mut any = false;
        for sample in &mut self.samples {
            let mut ready = sample.buffer.len() as i32 - sample.free;
            if self.rate != sample.rate {
                let scaled = (self.rate / sample.rate * ready as f32) as i32;
                ready = (frame * (f64::from(scaled) / frame).ceil()) as i32;
            }
            sample.mixable = ready;
            if ready > 0 && ready < count {
                any = true;
                count = ready;
            }
        }
        if count > self.available {
            count = self.available;
        }
        if count == 0 || !any {
            return;
        }
        let out_width = (self.bits / 8) as usize;
        let mix_len = self.mix.len();
        for sample in &mut self.samples {
            if sample.mixable < count {
                continue;
            }
            let mut at = self.position;
            let mut written = 0;
            let width = (sample.bits / 8) as usize;
            let mut frame_index = 0.0_f64;
            let mut channel = 0_i32;
            let out_rate = f64::from(self.rate);
            let in_rate = f64::from(sample.rate);
            let base = sample.read;
            let len = sample.buffer.len();
            while sample.mixable > 0 && written < count {
                let mut existing = 0_i16;
                for k in 0..out_width {
                    existing |= i16::from(self.mix[at + k]) << (k * 8);
                }
                let incoming;
                if self.rate == sample.rate {
                    incoming = sample.short_at(sample.read, width);
                    sample.read = (sample.read + width) % len;
                    sample.free += width as i32;
                } else {
                    let position = (frame_index / out_rate * in_rate) as f32;
                    let floor = position.floor();
                    let lo = f64::from(position).floor() as i32;
                    let hi = f64::from(position).ceil() as i32;
                    let lo_at = sample.channels * width as i32 * lo + width as i32 * channel;
                    let hi_at = sample.channels * width as i32 * hi + width as i32 * channel;
                    let lo_at = (base + lo_at as usize) % len;
                    let hi_at = (base + hi_at as usize) % len;
                    let a = sample.short_at(lo_at, width);
                    let b = sample.short_at(hi_at, width);
                    let t = position - floor;
                    debug_assert!((0.0..=1.0).contains(&t), "interpolation weight");
                    let delta = f32::from(b) - f32::from(a);
                    incoming = ((t * delta + f32::from(a)) as i32) as i16;
                    channel = (channel + 1) % sample.channels;
                    if channel == 0 {
                        frame_index += 1.0;
                    }
                }
                // The sum is narrowed to 16 bits without clamping, so a loud
                // mix wraps around.
                let mixed = existing.wrapping_add(incoming);
                self.mix[at] = mixed as u8;
                self.mix[at + 1] = (mixed >> 8) as u8;
                at = (out_width + at) % mix_len;
                written += width as i32;
            }
            if self.rate != sample.rate {
                let consumed = in_rate / out_rate * f64::from(count);
                let consumed = (frame * (consumed / frame).ceil()) as i32;
                sample.read = (sample.read + consumed as usize) % len;
                sample.free += consumed;
            }
        }
        self.pending = count;
    }
}

/// The two players (44.1 kHz and 22.05 kHz).
pub type Players = Arc<Vec<Mutex<LineMixer>>>;

/// A voice's handle to its mixer sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleHandle {
    player: usize,
    id: u64,
}

/// The player whose rate is nearest `rate`.
fn nearest_player(players: &Players, rate: f32) -> usize {
    let mut best_rate = -1.0_f32;
    let mut best_delta = f32::MAX;
    let mut best = 0;
    for (index, player) in players.iter().enumerate() {
        let player_rate = lock(player).rate;
        let delta = (player_rate - rate).abs();
        if best_rate < 0.0 || delta < best_delta {
            best_rate = player_rate;
            best_delta = delta;
            best = index;
        }
    }
    best
}

/// Open a sample for a sound of `channels` and `rate`, played at
/// `rate_scale` times that rate, on the nearest player.
pub fn play_sample(
    players: &Players,
    channels: i32,
    rate: i32,
    format: SampleFormat,
    _endianness: Endianness,
    bytes: i32,
    rate_scale: f32,
) -> SampleHandle {
    let sample_rate = rate as f32 * rate_scale;
    let player = nearest_player(players, sample_rate);
    let id = SAMPLE_IDS.fetch_add(1, Ordering::Relaxed);
    let sample = SampleRing {
        id,
        buffer: vec![0; bytes as usize],
        read: 0,
        write: 0,
        free: bytes,
        mixable: 0,
        rate: sample_rate,
        bits: format.bits(),
        channels: channels.max(2),
    };
    lock(&players[player]).samples.push(sample);
    SampleHandle { player, id }
}

/// Free bytes in the sample's ring.
pub fn sample_free(players: &Players, handle: SampleHandle) -> i32 {
    let player = lock(&players[handle.player]);
    player
        .samples
        .iter()
        .find(|s| s.id == handle.id)
        .map_or(0, |s| s.free)
}

/// Close a sample.
pub fn remove_sample(players: &Players, handle: SampleHandle) {
    lock(&players[handle.player])
        .samples
        .retain(|s| s.id != handle.id);
}

/// Append PCM to a sample's ring.
pub fn write_sample(
    players: &Players,
    handle: SampleHandle,
    data: &[u8],
    offset: usize,
    len: usize,
) {
    let mut player = lock(&players[handle.player]);
    if let Some(sample) = player.samples.iter_mut().find(|s| s.id == handle.id) {
        sample.write(data, offset, len);
    }
}
