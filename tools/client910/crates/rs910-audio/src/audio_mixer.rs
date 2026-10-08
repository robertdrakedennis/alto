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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_sink::RecordingSink;

    fn le_bytes(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// An 11.025 kHz stereo voice goes to the 22.05 kHz player, is
    /// upsampled 2:1 by linear interpolation truncated toward zero, and is
    /// summed with a same-rate voice. Every interpolation position is a
    /// multiple of 0.5, so the expected samples are exact.
    #[test]
    fn line_mixer_resamples_and_sums_voices() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let line = |written: &Arc<Mutex<Vec<u8>>>| {
            Box::new(RecordingSink {
                written: written.clone(),
                capacity: 64,
            })
        };
        let players: Players = Arc::new(vec![
            Mutex::new(LineMixer::new(44100.0, 64, line(&Arc::default()))),
            Mutex::new(LineMixer::new(22050.0, 64, line(&written))),
        ]);
        let voice = |rate, bytes| {
            play_sample(
                &players,
                2,
                rate,
                SampleFormat::Signed16,
                Endianness::Little,
                bytes,
                1.0,
            )
        };
        // Four stereo frames at 11025 Hz.
        let low = voice(11025, 32);
        let frames = le_bytes(&[1000, -1000, 2001, -2001, -3000, 3000, 4000, 0]);
        write_sample(&players, low, &frames, 0, frames.len());
        // Eight stereo frames at the player rate.
        let same = voice(22050, 64);
        let flat: Vec<i16> = (0..16).map(|i| i * 10).collect();
        let flat_bytes = le_bytes(&flat);
        write_sample(&players, same, &flat_bytes, 0, flat_bytes.len());
        assert_eq!(sample_free(&players, low), 16);
        assert_eq!(sample_free(&players, same), 32);
        {
            let mut player = lock(&players[1]);
            player.refresh_available();
            player.flush();
        }
        // Output frame f reads the voice at f/2: even frames copy a frame,
        // odd frames are midpoints; the last one blends frame 3 with the
        // unwritten (zero) frame 4 of the ring.
        let resampled = [
            1000, -1000, // f0 = F0
            1500, -1500, // f1: 1500.5, -1500.5 truncated
            2001, -2001, // f2 = F1
            -499, 499, // f3: -499.5, 499.5 truncated
            -3000, 3000, // f4 = F2
            500, 1500, // f5
            4000, 0, // f6 = F3
            2000, 0, // f7: halfway to zero
        ];
        let expected: Vec<i16> = resampled.iter().zip(&flat).map(|(r, f)| r + f).collect();
        let out: Vec<i16> = lock(&written)
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(out, expected);
        // The 11025 Hz voice advanced by `11025 / 22050 * 32` = 16 bytes,
        // the same-rate voice by the 32 it mixed.
        assert_eq!(sample_free(&players, low), 32);
        assert_eq!(sample_free(&players, same), 64);
    }

    /// FNV-1a 64 over the bytes both players write.
    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// Four voices at different rates (one per resampling case: same rate on
    /// each player, down-sampled, and a scaled rate) fed with fixed
    /// pseudo-random PCM for many mixing rounds. The digest pins the whole
    /// mix, including the resampler's rounding and the ring wrap-around; it
    /// is a self-recorded snapshot, so update it only for an intended change
    /// to the mixer.
    #[test]
    fn mixed_lines_of_fixed_voices_match_digest() {
        let written: Vec<Arc<Mutex<Vec<u8>>>> = (0..2).map(|_| Arc::default()).collect();
        let players: Players = Arc::new(
            [(44100.0, 8192), (22050.0, 4096)]
                .iter()
                .zip(&written)
                .map(|(&(rate, size), out)| {
                    Mutex::new(LineMixer::new(
                        rate,
                        size,
                        Box::new(RecordingSink {
                            written: out.clone(),
                            capacity: 1024,
                        }),
                    ))
                })
                .collect(),
        );
        // (file rate, rate scale, ring bytes).
        let voices = [
            (44100, 1.0, 4096),
            (22050, 1.0, 2048),
            (32000, 1.0, 4096),
            (22050, 1.3, 4096),
        ];
        let handles: Vec<SampleHandle> = voices
            .iter()
            .map(|&(rate, scale, bytes)| {
                play_sample(
                    &players,
                    2,
                    rate,
                    SampleFormat::Signed16,
                    Endianness::Little,
                    bytes,
                    scale,
                )
            })
            .collect();
        let mut seed = 0x1234_5678_u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 16) as i16) / 4
        };
        for _ in 0..200 {
            for &handle in &handles {
                let free = sample_free(&players, handle);
                let len = (free.min(512) / 4 * 4) as usize;
                if len > 0 {
                    let block: Vec<i16> = (0..len / 2).map(|_| noise()).collect();
                    let bytes = le_bytes(&block);
                    write_sample(&players, handle, &bytes, 0, bytes.len());
                }
            }
            for player in players.iter() {
                let mut player = lock(player);
                player.refresh_available();
                player.flush();
            }
        }
        let digests: Vec<(usize, u64)> = written
            .iter()
            .map(|w| {
                let bytes = lock(w);
                (bytes.len(), fnv1a(&bytes))
            })
            .collect();
        assert_eq!(
            digests,
            [
                (102_400, 0x8ab6_f243_ada4_c9ac),
                (70_400, 0xa6b4_1d8e_1195_aa0a)
            ],
            "{digests:#x?}"
        );
    }
}
