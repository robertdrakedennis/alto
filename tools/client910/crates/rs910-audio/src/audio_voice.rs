//! One voice of the backend's pool: it takes a sound's chunks, feeds them to
//! its own Vorbis decoder, and moves the decoded PCM through a small ring of
//! buffers into the mixer's sample.
//!
//! Sounds are never cached as decoded PCM: every voice decodes from the
//! Ogg/Vorbis stream. Loop points are handled inside the decoder.

use crate::audio_adjust::{AdjustContext, Adjuster};
use crate::audio_bus::BussManager;
use crate::audio_mixer::{self as mixer, Players, SampleHandle};
use crate::audio_sound_file::SoundFile;
use crate::audio_vorbis::{DecoderState, VorbisDecoder};
use anyhow::{bail, Result};
use std::sync::Arc;

/// Fetches the file of one archive group (chunk group 0 stands for the
/// stream's own header group); `None` while it is still loading.
pub type ChunkFetcher = Arc<dyn Fn(i32) -> Option<Arc<[u8]>> + Send + Sync>;

/// Where a voice reads its sound from.
#[derive(Clone)]
pub enum Source {
    /// The whole sound file, chunks inline (an effect from the `vorbis`
    /// archive).
    Group(Arc<[u8]>),
    /// A song whose chunks are fetched group by group as the decoder asks.
    Streamed(ChunkFetcher),
}

/// What a sound asks of a voice when it starts playing.
pub struct VoiceRequest {
    pub source: Option<Source>,
    pub bus: i32,
    pub volume: f32,
    pub looping: bool,
    /// Extra loop passes when `looping` (negative: forever).
    pub loop_count: i32,
    /// Playback rate relative to the file's own sample rate.
    pub rate: f32,
    /// The pool index of the sound that owns the voice.
    pub owner: usize,
}

/// The life cycle of a voice, ordered so that comparisons read as "has it
/// reached this stage yet".
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceState {
    Unused = 0,
    /// In the free pool.
    Free = 1,
    /// Claimed by a sound, not yet playing.
    Allocated = 2,
    /// Begun: header read, sample open, waiting for enough PCM.
    Started = 3,
    Reserved = 4,
    /// PCM is flowing to the mixer.
    Playing = 5,
    /// Stop requested.
    Stopping = 6,
    /// The source is exhausted, or failed.
    Finished = 7,
    /// Handed back by its sound; drains, then returns to the pool.
    Released = 8,
}

impl VoiceState {
    /// The states the backend stops when a voice has become silent.
    pub fn is_active_for_cull(self) -> bool {
        matches!(self, Self::Playing | Self::Started | Self::Reserved)
    }
}

/// A linear volume fade.
#[derive(Clone, Copy, Default)]
struct Fade {
    target: f32,
    from: f32,
    start: i64,
    end: i64,
}

/// How much of a chunk of an inline sound has been handed to the decoder.
#[derive(Clone, Copy, Default)]
struct InlineCursor {
    /// The next chunk to feed.
    chunk: i32,
    /// Bytes of that chunk already fed (a chunk cut short by the end of the
    /// group is fed in parts).
    consumed: i32,
}

/// One voice of the 50-voice pool.
pub struct Voice {
    source: Option<Source>,
    /// The parsed header of the sound being played.
    file: Option<SoundFile>,
    bus: i32,
    /// The sound's own (fading) volume.
    volume: f32,
    /// The volume of the bus chain this voice plays on.
    bus_volume: f32,
    /// Per-channel gains from the sound's positional adjuster.
    gains: Option<[f32; 2]>,
    looping: bool,
    loop_count: i32,
    rate: f32,
    /// The next chunk group to fetch for a streamed sound.
    next_stream_chunk: i32,
    inline: InlineCursor,
    /// Bytes of each PCM buffer handed to the mixer sample.
    sample_bytes: i32,
    /// The ring of PCM buffers between the decoder and the mixer (empty
    /// until the voice has begun).
    ring: Vec<Option<Vec<u8>>>,
    ring_len: usize,
    play_slot: usize,
    fill_slot: usize,
    /// Bytes of the buffer at `play_slot` already given to the mixer.
    play_offset: i32,
    sample: Option<SampleHandle>,
    /// Applying the adjuster is meaningful (set while the voice is live).
    active: bool,
    state: VoiceState,
    decoder: VorbisDecoder,
    players: Players,
    /// The decoder asked for another chunk.
    needs_data: bool,
    adjuster: Option<Adjuster>,
    owner: Option<usize>,
    /// PCM has started flowing to the mixer.
    output_started: bool,
    /// The loudness the backend ranks voices by (-1: inaudible).
    priority_volume: f32,
    volume_changed: bool,
    fade: Fade,
    paused: bool,
}

impl Voice {
    /// A free voice with a `ring_len`-buffer PCM ring of `sample_bytes`
    /// bytes per buffer.
    pub fn new(
        sample_bytes: i32,
        ring_len: usize,
        decoder: VorbisDecoder,
        players: Players,
    ) -> Self {
        let mut voice = Self {
            source: None,
            file: None,
            bus: 0,
            volume: 0.0,
            bus_volume: 0.0,
            gains: None,
            looping: false,
            loop_count: 0,
            rate: 1.0,
            next_stream_chunk: 0,
            inline: InlineCursor::default(),
            sample_bytes,
            ring: Vec::new(),
            ring_len,
            play_slot: 0,
            fill_slot: 0,
            play_offset: 0,
            sample: None,
            active: false,
            state: VoiceState::Free,
            decoder,
            players,
            needs_data: false,
            adjuster: None,
            owner: None,
            output_started: false,
            priority_volume: -1.0,
            volume_changed: false,
            fade: Fade::default(),
            paused: false,
        };
        voice.reset();
        voice
    }

    pub fn state(&self) -> VoiceState {
        self.state
    }

    /// The pool index of the sound that owns this voice.
    pub fn owner(&self) -> Option<usize> {
        self.owner
    }

    pub fn is_stopping(&self) -> bool {
        self.state == VoiceState::Stopping
    }

    pub fn is_finished(&self) -> bool {
        self.state == VoiceState::Finished
    }

    /// Claim a free voice.
    pub fn allocate(&mut self) {
        debug_assert!(self.state < VoiceState::Allocated, "voice already claimed");
        self.state = VoiceState::Allocated;
    }

    /// Hand the voice back: it drains its PCM, then returns to the pool.
    pub fn release(&mut self) {
        debug_assert!(self.state >= VoiceState::Allocated, "voice was not claimed");
        self.owner = None;
        self.state = VoiceState::Released;
    }

    /// Fade the sound volume to `target` over `ms` milliseconds (at once
    /// when `ms` is not positive).
    pub fn fade(&mut self, target: f32, ms: i32, now: i64) {
        if ms <= 0 {
            self.volume = target;
            self.fade.target = self.volume;
            self.fade.start = 0;
            self.fade.end = 0;
        } else {
            self.fade.from = self.volume;
            self.fade.target = target;
            self.fade.start = now;
            self.fade.end = self.fade.start + i64::from(ms);
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Parse the sound header, open the mixer sample and queue the first
    /// chunk. Returns `false` while the sound's data is not loaded yet.
    pub fn begin(&mut self, request: VoiceRequest) -> Result<bool> {
        self.owner = Some(request.owner);
        self.bus = request.bus;
        self.volume = request.volume;
        self.looping = request.looping;
        self.loop_count = request.loop_count;
        self.rate = request.rate;
        self.source = request.source;
        let bytes = match &self.source {
            Some(Source::Group(bytes)) => Some(bytes.clone()),
            Some(Source::Streamed(fetch)) => {
                self.next_stream_chunk = 0;
                fetch(self.next_stream_chunk)
            }
            None => None,
        };
        let Some(bytes) = bytes else {
            return Ok(false);
        };
        let file = SoundFile::parse(&bytes)?;
        if self.sample.is_none() {
            self.sample = Some(mixer::play_sample(
                &self.players,
                file.channels,
                file.sample_rate,
                self.decoder.sample_format(),
                self.decoder.endianness(),
                self.sample_bytes,
                self.rate,
            ));
        }
        self.ring = (0..self.ring_len).map(|_| None).collect();
        self.gains = Some([1.0, 1.0]);
        self.decoder.set_loop(
            true,
            if self.looping { self.loop_count } else { 0 },
            file.loop_start,
            file.loop_end,
        );
        if matches!(self.source, Some(Source::Group(_))) {
            self.needs_data = true;
        } else {
            // A streamed sound's first chunk follows the header in the same
            // group.
            self.decoder.push_chunk(bytes[file.table_end..].to_vec());
            self.next_stream_chunk += 1;
        }
        self.file = Some(file);
        Ok(true)
    }

    /// Start output once the sound has begun.
    pub fn start_playing(&mut self, buss: &BussManager, ctx: &AdjustContext) {
        if self.state != VoiceState::Stopping && self.state < VoiceState::Started {
            self.state = VoiceState::Started;
            self.active = true;
            self.apply_adjuster(ctx);
            self.update_volume(buss);
        }
    }

    pub fn stop(&mut self) {
        if self.state != VoiceState::Stopping && self.state >= VoiceState::Allocated {
            self.state = VoiceState::Stopping;
        }
    }

    /// A decode failure ends the voice, and with it the sound, instead of
    /// taking the audio thread down.
    pub fn fail(&mut self) {
        if self.state >= VoiceState::Allocated && self.state < VoiceState::Finished {
            self.state = VoiceState::Finished;
            self.active = false;
        }
    }

    pub fn set_adjuster(&mut self, adjuster: Option<Adjuster>, ctx: &AdjustContext) {
        self.adjuster = adjuster;
        if let (Some(adjuster), Some(gains)) = (self.adjuster, self.gains.as_mut()) {
            adjuster.apply(gains, ctx);
        }
    }

    /// A positioned sound's owner moved it: the adjuster reads the new
    /// position on its next pass.
    pub fn set_adjuster_position(&mut self, position: [f32; 3]) {
        if let Some(adjuster) = self.adjuster.as_mut() {
            adjuster.position = position;
        }
    }

    /// Whether the voice is loud enough to be worth mixing.
    fn audible(&self) -> bool {
        let Some(gains) = &self.gains else {
            return false;
        };
        let mut weighted = 0.0_f32;
        let mut peak = 0.0_f32;
        for &gain in gains {
            if self.volume * gain > weighted {
                weighted = gain;
            }
            if gain > peak {
                peak = gain;
            }
        }
        weighted >= 1.0e-5 || peak >= 1.0e-5 && self.fade.target >= 1.0e-5
    }

    /// Recompute the bus volume and the priority the backend ranks voices
    /// by.
    fn update_volume(&mut self, buss: &BussManager) {
        self.bus_volume = buss.volume(self.bus).unwrap_or(1.0);
        let priority = buss.priority(self.bus).unwrap_or(0.1);
        let scaled = self.volume * priority;
        let gains = self.gains.unwrap_or([1.0, 1.0]);
        let mut peak = 0.0_f32;
        for gain in gains {
            if gain > peak {
                peak = gain;
            }
        }
        let mut value = scaled * peak;
        if !self.audible() {
            value = -1.0;
        }
        if self.priority_volume != value {
            self.priority_volume = value;
            self.volume_changed = true;
        }
    }

    pub fn priority_volume(&self) -> f32 {
        self.priority_volume
    }

    pub fn volume_changed(&self) -> bool {
        self.volume_changed
    }

    /// The per-frame update from the client thread: feed the decoder, ease
    /// the fade, refresh the volume and the positional gains.
    pub fn client_update(&mut self, now: i64, buss: &BussManager, ctx: &AdjustContext) {
        if self.state < VoiceState::Started {
            return;
        }
        if self.state == VoiceState::Released {
            self.recycle();
            return;
        }
        if let Err(error) = self.feed() {
            rs910_core::log_repeat::warn_repeated!(
                "[client910] audio voice feed failed: {error:#}"
            );
            self.fail();
        }
        if self.fade.target != self.volume {
            if now > self.fade.end {
                self.volume = self.fade.target;
            } else {
                let delta = self.fade.target - self.fade.from;
                let span = self.fade.end - self.fade.start;
                let slope = delta / span as f32;
                self.volume = (now - self.fade.start) as f32 * slope + self.fade.from;
                self.volume = self.volume.clamp(0.0, 1.0);
            }
        }
        self.update_volume(buss);
        if self.state < VoiceState::Stopping {
            self.apply_adjuster(ctx);
        }
    }

    /// Hand the decoder its next chunk when it asked for one.
    fn feed(&mut self) -> Result<()> {
        if !self.needs_data || self.decoder.state() == DecoderState::Draining {
            return Ok(());
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let chunk_count = file.chunks.len() as i32;
        match self.source.clone() {
            Some(Source::Streamed(fetch)) => {
                if self.next_stream_chunk >= chunk_count {
                    self.next_stream_chunk = 0;
                    self.decoder.push_end_of_input();
                } else if let Some(bytes) =
                    fetch(file.chunks[self.next_stream_chunk as usize].group)
                {
                    self.decoder.push_chunk(bytes.to_vec());
                    self.needs_data = false;
                    self.next_stream_chunk += 1;
                }
                Ok(())
            }
            Some(Source::Group(group)) => {
                if self.inline.chunk >= chunk_count {
                    self.decoder.push_end_of_input();
                    self.inline.chunk = 0;
                    return Ok(());
                }
                let chunk = self.inline.chunk as usize;
                let chunk_start = file.chunks[chunk].offset as i32;
                let size = file.chunks[chunk].size as i32;
                let from = self.inline.consumed + chunk_start;
                let end = if self.inline.consumed + size > size {
                    chunk_start + size
                } else {
                    from + size
                };
                self.inline.consumed += if from + size > group.len() as i32 {
                    group.len() as i32 - from
                } else {
                    size
                };
                let Some(data) = group.get(from as usize..end.max(from) as usize) else {
                    bail!("chunk {chunk} lies beyond the sound group");
                };
                self.decoder.push_chunk(data.to_vec());
                self.needs_data = false;
                if self.inline.consumed >= size {
                    self.inline.chunk += 1;
                    self.inline.consumed = 0;
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Return a released voice to the free pool once its sample has drained.
    fn recycle(&mut self) {
        if self.decoder.state() == DecoderState::Idle {
            match self.sample {
                None => self.state = VoiceState::Free,
                Some(sample) => {
                    if mixer::sample_free(&self.players, sample) >= self.sample_bytes {
                        mixer::remove_sample(&self.players, sample);
                        self.sample = None;
                        self.state = VoiceState::Free;
                    }
                }
            }
        }
        self.reset();
    }

    fn apply_adjuster(&mut self, ctx: &AdjustContext) {
        if self.active
            && self.state >= VoiceState::Started
            && self.state < VoiceState::Finished
            && self.fade.target == self.volume
        {
            if let (Some(adjuster), Some(gains)) = (self.adjuster, self.gains.as_mut()) {
                adjuster.apply(gains, ctx);
            }
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// The decode thread's turn: decode ahead and refill the PCM ring.
    pub fn decode_tick(&mut self) -> Result<()> {
        if self.state < VoiceState::Started || self.state >= VoiceState::Stopping || !self.audible()
        {
            return Ok(());
        }
        self.decoder.decode_tick()?;
        if self.decoder.take_input_request() {
            self.needs_data = true;
        }
        self.fill_ring()
    }

    /// Move decoded PCM into the next free ring buffer, start output once
    /// the ring is full (or the sound is short), and finish the voice when
    /// the sound has ended and every buffer has played.
    fn fill_ring(&mut self) -> Result<()> {
        if self.ring.is_empty() {
            return Ok(());
        }
        let state = self.decoder.state();
        if self.decoder.ready() {
            let buffered = self.decoder.buffered_frames();
            let want = self.decoder.bytes_to_samples(self.sample_bytes);
            if self.ring[self.fill_slot].is_none() && state != DecoderState::Ended && buffered > 0 {
                let pcm = self.decoder.take_pcm(want)?.bytes;
                let channels = self.decoder.output_channels()?;
                let frames = self.decoder.bytes_to_samples(pcm.len() as i32 / channels);
                if frames > 0 {
                    self.ring[self.fill_slot] = Some(pcm);
                    self.fill_slot = (self.fill_slot + 1) % self.ring.len();
                }
            }
        }
        let filled = self.ring.iter().filter(|slot| slot.is_some()).count();
        if !self.output_started
            && self.priority_volume >= 0.0
            && (filled >= self.ring_len
                || state == DecoderState::Ended
                || state == DecoderState::Draining)
        {
            self.output_started = true;
            self.state = VoiceState::Playing;
        }
        // A looping voice never finishes on its own.
        if !self.decoder.ready() || filled > 0 || state != DecoderState::Ended || self.looping {
            return Ok(());
        }
        self.state = VoiceState::Finished;
        self.active = false;
        Ok(())
    }

    /// The output thread's turn: scale the PCM at the front of the ring by
    /// the bus, sound and positional volumes and give as much as fits to
    /// the mixer sample.
    pub fn output(&mut self) {
        if self.paused {
            return;
        }
        let Some(sample) = self.sample else {
            return;
        };
        let free = mixer::sample_free(&self.players, sample);
        if free <= 0 || !self.output_started {
            return;
        }
        let play = self.play_slot;
        let Some(slot) = self.ring.get_mut(play).and_then(Option::as_mut) else {
            return;
        };
        let len = slot.len() as i32;
        let count = if self.play_offset + free > len {
            len - self.play_offset
        } else {
            free
        };
        let from = self.play_offset as usize;
        let to = (self.play_offset + count) as usize;
        // Each sample is scaled by (bus * volume * channel gain) squared.
        let gains = self.gains.unwrap_or([1.0, 1.0]);
        let mut at = from;
        let mut channel = 0;
        while at < slot.len() && at < to {
            let gain = self.bus_volume * self.volume * gains[channel];
            let value = (i32::from(slot[at + 1] as i8) << 8) + i32::from(slot[at]);
            let scaled = (gain * gain * value as f32) as i32;
            slot[at] = scaled as u8;
            slot[at + 1] = (scaled >> 8) as u8;
            at += 2;
            channel = (channel + 1) % gains.len();
        }
        mixer::write_sample(&self.players, sample, slot, from, count.max(0) as usize);
        self.play_offset += count;
        if self.play_offset >= len {
            self.ring[play] = None;
            self.play_slot = (self.play_slot + 1) % self.ring.len();
            self.play_offset = 0;
        }
    }

    /// Forget the sound and stop the decoder, ready for the next one.
    fn reset(&mut self) {
        self.source = None;
        self.file = None;
        self.bus = 0;
        self.volume = 0.0;
        self.bus_volume = 0.0;
        self.looping = false;
        self.loop_count = 0;
        self.rate = 1.0;
        self.next_stream_chunk = 0;
        self.ring = Vec::new();
        self.play_slot = 0;
        self.active = false;
        self.decoder.stop();
        self.needs_data = false;
        self.adjuster = None;
        self.owner = None;
        self.play_offset = 0;
        self.inline = InlineCursor::default();
        self.fill_slot = 0;
        self.output_started = false;
        self.priority_volume = -1.0;
        self.volume_changed = false;
        self.fade = Fade::default();
    }
}
