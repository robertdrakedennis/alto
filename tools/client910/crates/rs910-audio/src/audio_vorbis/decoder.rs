//! The streaming decoder: Ogg pages in, PCM out.
//!
//! A [`VorbisDecoder`] is fed chunks of an Ogg/Vorbis stream (a sound is
//! stored as one or more chunks) and decodes ahead on its own thread
//! whenever the caller ticks it, keeping a small queue of PCM for the voice
//! that plays it. The decoder also implements the client's sample-exact
//! looping: it can skip to a loop start, cut a pass at the loop end and
//! restart from the first chunk without a gap.

use super::bits::BitReader;
use super::block::{BlockDecoder, PcmBlock};
use super::codebook::SetupCache;
use super::setup::{HeaderKind, Identification, Setup};
use super::{Endianness, SampleFormat};
use anyhow::{bail, ensure, Result};
use std::collections::VecDeque;

/// Where the decoder is in its input/output cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoderState {
    /// Freshly reset: no input queued.
    Idle,
    /// Input is queued and the decoder can be ticked.
    Ready,
    /// A tick is running.
    Decoding,
    /// The input queue ran dry and more was requested (see
    /// [`VorbisDecoder::take_input_request`]).
    Starved,
    /// The input has ended; queued PCM is still being drained.
    Draining,
    /// The input has ended and every sample has been taken.
    Ended,
    /// The decoder was reset while a tick was running.
    StopRequested,
}

/// PCM bytes taken from the decoder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PcmChunk {
    pub bytes: Vec<u8>,
}

/// How decoded samples are written out.
#[derive(Clone, Copy, Debug)]
struct OutputFormat {
    sample: SampleFormat,
    endianness: Endianness,
    /// Streams with fewer channels are expanded to this many.
    min_channels: i32,
}

/// The Ogg page buffer: input chunks are appended, whole pages are parsed
/// off the front.
#[derive(Default)]
struct PageBuffer {
    bytes: Vec<u8>,
    /// Bytes at the front that are already parsed.
    consumed: usize,
    /// There may be more pages to parse (cleared when the buffer holds no
    /// complete page, set again when a chunk is appended).
    pending: bool,
}

impl PageBuffer {
    fn append(&mut self, chunk: &[u8]) {
        self.bytes.drain(..self.consumed);
        self.consumed = 0;
        self.bytes.extend_from_slice(chunk);
    }

    fn unparsed(&self) -> usize {
        self.bytes.len() - self.consumed
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.consumed = 0;
        self.pending = false;
    }
}

/// The decoded PCM waiting to be taken.
#[derive(Default)]
struct PcmQueue {
    chunks: VecDeque<Vec<u8>>,
    /// Frames (samples of every channel) queued.
    frames: i32,
}

/// Loop bookkeeping. The sound's loop points are in output samples.
struct LoopState {
    enabled: bool,
    /// Extra passes over the loop; negative loops forever.
    count: i32,
    start: i32,
    end: i32,
    passes_done: i32,
    /// After a restart the decoder skips output (and whole leading packets)
    /// until the loop start.
    seeking: bool,
    /// Page and packet in which the loop start lies, learned during the
    /// first pass so later passes can skip straight to them. The page index
    /// counts every examination of a page header, including re-examinations
    /// of a page that straddled two input chunks.
    start_page: i32,
    start_packet: Option<i32>,
}

impl LoopState {
    fn new() -> Self {
        Self {
            enabled: false,
            count: 0,
            start: -1,
            end: -1,
            passes_done: 0,
            seeking: false,
            start_page: -1,
            start_packet: None,
        }
    }

    fn passes_remain(&self) -> bool {
        self.passes_done < self.count || self.count < 0
    }
}

/// What the page loop knows about the page it is decoding.
struct PageCursor {
    header_type: i32,
    /// The page's granule position: samples the page ends at.
    granule: i32,
    /// Samples of this page produced (or skipped) so far.
    produced: i32,
    packet_index: i32,
}

/// The client's Vorbis decoder for one voice.
pub struct VorbisDecoder {
    format: OutputFormat,
    /// Seconds of decoded PCM to keep ahead of the voice.
    buffer_seconds: f32,
    setups: SetupCache,
    state: DecoderState,
    /// The input queue is empty and the owner has been asked for more.
    input_requested: bool,
    input_request_sent: bool,
    input: VecDeque<Vec<u8>>,
    ident: Option<Identification>,
    setup: Option<Setup>,
    block: BlockDecoder,
    pages: PageBuffer,
    pcm: PcmQueue,
    loops: LoopState,
    /// Index of the current page in the pass.
    page_index: i32,
    /// Output samples produced or skipped in the current pass.
    sample_position: i32,
    /// Samples decoded since the last header packet (for the end-of-stream
    /// trim).
    decoded_samples: i32,
    end_trim: i32,
}

impl VorbisDecoder {
    /// A decoder with its own codebook cache.
    pub fn new(buffer_seconds: f32) -> Self {
        Self::with_setup_cache(buffer_seconds, SetupCache::default())
    }

    /// A decoder sharing `setups` with others, so sounds from one encoder
    /// configuration parse their codebooks once.
    pub fn with_setup_cache(buffer_seconds: f32, setups: SetupCache) -> Self {
        let mut decoder = Self {
            format: OutputFormat {
                sample: SampleFormat::Signed16,
                endianness: Endianness::Little,
                min_channels: 0,
            },
            buffer_seconds,
            setups,
            state: DecoderState::Idle,
            input_requested: false,
            input_request_sent: false,
            input: VecDeque::new(),
            ident: None,
            setup: None,
            block: BlockDecoder::default(),
            pages: PageBuffer::default(),
            pcm: PcmQueue::default(),
            loops: LoopState::new(),
            page_index: -1,
            sample_position: 0,
            decoded_samples: 0,
            end_trim: 0,
        };
        decoder.reset(false);
        decoder
    }

    /// Choose the output sample format, byte order and the minimum channel
    /// count (a mono stream is expanded to `min_channels`).
    pub fn configure(&mut self, sample: SampleFormat, endianness: Endianness, min_channels: i32) {
        self.format = OutputFormat {
            sample,
            endianness,
            min_channels,
        };
    }

    /// Abandon the current stream. A tick in flight is told to stop.
    pub fn stop(&mut self) {
        let decoding = self.state == DecoderState::Decoding;
        self.reset(false);
        self.state = if decoding {
            DecoderState::StopRequested
        } else {
            DecoderState::Idle
        };
    }

    pub fn state(&self) -> DecoderState {
        self.state
    }

    /// Whether the decoder ran out of queued input since the last call. The
    /// owner answers by pushing the next chunk (or the end of input).
    pub fn take_input_request(&mut self) -> bool {
        std::mem::take(&mut self.input_requested)
    }

    /// Bytes of PCM to samples (all channels) in the output format.
    pub fn bytes_to_samples(&self, bytes: i32) -> i32 {
        bytes / (self.format.sample.bits() / 8)
    }

    /// Samples (all channels) to bytes of PCM in the output format.
    pub fn samples_to_bytes(&self, samples: i32) -> i32 {
        samples * (self.format.sample.bits() / 8)
    }

    /// The stream's sample rate; known once the setup header has been read.
    pub fn sample_rate(&self) -> Result<i32> {
        match (&self.setup, &self.ident) {
            (Some(_), Some(ident)) => Ok(ident.sample_rate),
            _ => bail!("sample rate asked for before the stream setup"),
        }
    }

    pub fn sample_format(&self) -> SampleFormat {
        self.format.sample
    }

    pub fn endianness(&self) -> Endianness {
        self.format.endianness
    }

    /// The stream's own channel count, known once the identification header
    /// has been read.
    pub fn channels(&self) -> Result<i32> {
        match &self.ident {
            Some(ident) => Ok(ident.channels),
            None => bail!("channel count asked for before the identification header"),
        }
    }

    /// The channel count of the PCM the decoder writes.
    pub fn output_channels(&self) -> Result<i32> {
        Ok(self.channels()?.max(self.format.min_channels))
    }

    /// Whether the stream setup is complete and PCM can be taken.
    pub fn ready(&self) -> bool {
        self.setup.is_some()
    }

    /// Frames of decoded PCM queued for [`Self::take_pcm`].
    pub fn buffered_frames(&self) -> i32 {
        self.pcm.frames
    }

    /// Set the loop: `count` extra passes (negative: forever) between the
    /// sound's loop `start` and `end` samples.
    pub fn set_loop(&mut self, enabled: bool, count: i32, start: i32, end: i32) {
        self.loops.enabled = enabled;
        self.loops.count = count;
        self.loops.start = start;
        self.loops.end = end;
    }

    /// Clear the stream. `restart` keeps the parsed stream setup and the
    /// loop bookkeeping and only rewinds to the first chunk (a loop pass);
    /// otherwise everything is forgotten.
    fn reset(&mut self, restart: bool) {
        if !restart {
            self.block.clear();
        }
        self.decoded_samples = 0;
        self.pages.clear();
        if !restart {
            self.pcm.chunks.clear();
            self.pcm.frames = 0;
        }
        self.input.clear();
        self.input_request_sent = false;
        self.sample_position = 0;
        self.page_index = -1;
        if restart {
            self.loops.passes_done += 1;
            self.loops.seeking = true;
            return;
        }
        self.setup = None;
        self.ident = None;
        self.loops = LoopState::new();
    }

    /// Queue the next chunk of the stream.
    pub fn push_chunk(&mut self, chunk: Vec<u8>) {
        if matches!(self.state, DecoderState::Draining | DecoderState::Ended) {
            return;
        }
        self.input.push_back(chunk);
        self.state = DecoderState::Ready;
    }

    /// Signal the end of the stream. If loop passes remain the decoder
    /// rewinds instead and asks for the first chunk again.
    pub fn push_end_of_input(&mut self) {
        if matches!(self.state, DecoderState::Draining | DecoderState::Ended) {
            return;
        }
        let restart = self.loops.enabled
            && (self.loops.count > 0 && self.loops.passes_done < self.loops.count
                || self.loops.count < 0);
        if self.pcm.chunks.is_empty() {
            if !restart {
                self.state = DecoderState::Ended;
            }
        } else if !restart {
            self.state = DecoderState::Draining;
        }
        if restart {
            self.reset(true);
        }
    }

    /// One turn of the decode thread: consume queued input and decode pages
    /// until enough PCM is buffered.
    pub fn decode_tick(&mut self) -> Result<()> {
        if matches!(
            self.state,
            DecoderState::StopRequested | DecoderState::Idle | DecoderState::Ready
        ) {
            self.state = DecoderState::Decoding;
            self.fill()?;
            if self.state == DecoderState::Decoding {
                self.state = DecoderState::Ready;
            }
        }
        Ok(())
    }

    /// Whether more than `buffer_seconds` of PCM is already queued.
    fn ahead(&self) -> Result<bool> {
        if self.setup.is_none() {
            return Ok(false);
        }
        let rate = self.sample_rate()?;
        ensure!(rate != 0, "stream has a zero sample rate");
        Ok((self.pcm.frames / rate) as f32 > self.buffer_seconds)
    }

    fn fill(&mut self) -> Result<()> {
        if matches!(
            self.state,
            DecoderState::StopRequested | DecoderState::Starved
        ) || self.ahead()?
        {
            return Ok(());
        }
        if !self.pages.pending {
            let Some(chunk) = self.input.pop_front() else {
                if !self.input_request_sent {
                    self.state = DecoderState::Starved;
                    self.input_requested = true;
                    self.input_request_sent = true;
                }
                return Ok(());
            };
            self.input_request_sent = false;
            self.pages.pending = true;
            self.pages.append(&chunk);
        }
        self.parse_pages()
    }

    /// Whether the packet just decoded lies before the loop start while a
    /// restarted pass is still seeking towards it.
    fn is_seeking(&self, packet_index: i32) -> bool {
        self.loops.seeking
            && self.page_index < self.loops.start_page
            && self
                .loops
                .start_packet
                .is_some_and(|start| packet_index < start)
    }

    /// Parse and decode every complete Ogg page in the buffer.
    fn parse_pages(&mut self) -> Result<()> {
        loop {
            if !self.pages.pending {
                return Ok(());
            }
            if self.pages.unparsed() == 0 {
                self.pages.pending = false;
                return Ok(());
            }
            if self.ahead()? {
                return Ok(());
            }
            if self.pages.unparsed() < 27 {
                self.pages.pending = false;
                return Ok(());
            }
            let page = &self.pages.bytes[self.pages.consumed..];
            if &page[..4] != b"OggS" {
                bail!("input is not an Ogg page stream");
            }
            self.page_index += 1;
            if self.loops.start_packet.is_none() {
                self.loops.start_page += 1;
            }
            let header_type = i32::from(page[5]);
            let granule = i32::from(page[6])
                | i32::from(page[7]) << 8
                | i32::from(page[8]) << 16
                | i32::from(page[9]).wrapping_shl(24);
            let segments = usize::from(page[26]);
            let table_end = 27 + segments;
            if table_end > page.len() {
                self.pages.pending = false;
                return Ok(());
            }
            // Split the page into the packets that end on it. A packet that
            // continues onto the next page is dropped.
            let mut packets: Vec<(usize, usize)> = Vec::new();
            let mut end = table_end;
            let mut packet_start = table_end;
            let mut packet_len = 0_usize;
            let mut complete = true;
            for &lace in &page[27..table_end] {
                end += usize::from(lace);
                packet_len += usize::from(lace);
                if end > page.len() {
                    complete = false;
                    break;
                }
                if lace < 255 {
                    packets.push((packet_start, packet_len));
                    packet_start = end;
                    packet_len = 0;
                }
            }
            if !complete {
                self.pages.pending = false;
                return Ok(());
            }
            let base = self.pages.consumed;
            let mut cursor = PageCursor {
                header_type,
                granule,
                produced: 0,
                packet_index: -1,
            };
            let mut skipped_page = false;
            self.end_trim = 0;
            for (start, len) in packets {
                cursor.packet_index += 1;
                if self.setup.is_some() && self.is_seeking(cursor.packet_index) {
                    self.sample_position += if skipped_page { 0 } else { granule };
                    skipped_page = true;
                    continue;
                }
                let (is_audio, block) = self.decode_packet(base + start, len)?;
                if !is_audio || self.is_seeking(cursor.packet_index) {
                    if let Some(ident) = self.ident.filter(|_| self.setup.is_some()) {
                        self.block.break_overlap(&ident);
                    }
                } else if let Some(block) = block {
                    self.emit_pcm(&block, &mut cursor)?;
                }
            }
            self.pages.consumed = base + end;
        }
    }

    /// Decode the packet at `bytes[offset..offset + len]` of the page
    /// buffer. Returns whether it was an audio packet and, once a previous
    /// block exists, the overlap-added samples it completes.
    fn decode_packet(&mut self, offset: usize, len: usize) -> Result<(bool, Option<PcmBlock>)> {
        if offset + len > self.pages.bytes.len() {
            return Ok((false, None));
        }
        let data = std::mem::take(&mut self.pages.bytes);
        let result = self.decode_packet_in(&data, offset);
        self.pages.bytes = data;
        result
    }

    fn decode_packet_in(&mut self, data: &[u8], offset: usize) -> Result<(bool, Option<PcmBlock>)> {
        let mut bits = BitReader::new(data, offset);
        if bits.read_bit() != 0 {
            self.read_header(data, offset)?;
            return Ok((false, None));
        }
        let (Some(setup), Some(ident)) = (self.setup.as_mut(), self.ident) else {
            bail!("audio packet before the stream setup");
        };
        let block = self.block.decode(setup, &ident, &mut bits)?;
        Ok((true, block))
    }

    /// Handle a header packet: the identification header, the (ignored)
    /// comment header, or the setup header that completes the stream setup.
    fn read_header(&mut self, data: &[u8], offset: usize) -> Result<()> {
        self.decoded_samples = 0;
        if self.setup.is_some() {
            return Ok(());
        }
        match HeaderKind::of(data, offset)? {
            HeaderKind::Identification => {
                self.ident = Some(Identification::parse(data, offset)?);
            }
            HeaderKind::Comment => {}
            HeaderKind::Setup => {
                let Some(ident) = self.ident else {
                    bail!("setup header before the identification header");
                };
                let setup = Setup::parse(data, offset, &ident, &self.setups)?;
                self.block.install(&ident);
                self.setup = Some(setup);
            }
        }
        Ok(())
    }

    /// Turn a decoded block into output PCM, applying the end-of-stream trim
    /// and the loop start/end cuts.
    fn emit_pcm(&mut self, block: &[Vec<f32>], cursor: &mut PageCursor) -> Result<()> {
        let block_len = block[0].len() as i32;
        let mut frames = block_len;
        self.decoded_samples += frames;
        if self.decoded_samples > cursor.granule && cursor.header_type == 4 {
            // Last page: the granule position says how many samples are real.
            self.end_trim = self.decoded_samples - cursor.granule - self.end_trim;
            frames -= self.end_trim;
            if self.end_trim > block_len {
                self.end_trim = block_len;
            }
            if frames < 0 {
                frames = 0;
            }
        }
        let mut first = 0;
        let mut bytes = self.samples_to_bytes(frames) * block.len() as i32;
        if self.loops.seeking && self.sample_position < self.loops.start {
            let whole = bytes;
            bytes -= self.samples_to_bytes(self.loops.start - self.sample_position);
            if bytes <= 0 {
                self.sample_position += self.bytes_to_samples(whole);
                return Ok(());
            }
            first += self.loops.start - self.sample_position;
        }
        if self.sample_position + frames > self.loops.end
            && self.loops.passes_remain()
            && self.loops.enabled
        {
            bytes -= self.samples_to_bytes(self.sample_position + frames - self.loops.end - 1);
            if bytes <= 0 {
                return Ok(());
            }
        }
        let out_channels = self.output_channels()?;
        let channels = self.channels()?;
        let mut pcm = Vec::new();
        for frame in first..frames {
            let mut skipping = self.loops.seeking;
            if self.loops.count != 0 {
                if self.loops.start == self.sample_position {
                    if self.loops.start_packet.is_none() {
                        self.loops.start_packet = Some(cursor.packet_index);
                    }
                    self.loops.seeking = false;
                }
                if self.sample_position > self.loops.end
                    && self.loops.passes_remain()
                    && self.loops.enabled
                {
                    skipping = true;
                }
            }
            if skipping
                && (self.sample_position < self.loops.start
                    || self.sample_position > self.loops.end)
            {
                self.sample_position += 1;
                cursor.produced += 1;
                ensure!(
                    cursor.produced <= cursor.granule,
                    "page produced more samples than its granule position"
                );
            } else {
                for c in 0..out_channels {
                    let source = if (c as usize) < block.len() {
                        c as usize
                    } else {
                        (c % channels) as usize
                    };
                    let value = block[source][frame as usize];
                    self.format
                        .sample
                        .encode(value, self.format.endianness, &mut pcm);
                }
                self.sample_position += 1;
                cursor.produced += 1;
            }
        }
        self.pcm.frames += self.bytes_to_samples(pcm.len() as i32) / self.output_channels()?;
        self.pcm.chunks.push_back(pcm);
        Ok(())
    }

    /// Take up to `samples` samples (all channels) of decoded PCM.
    pub fn take_pcm(&mut self, samples: i32) -> Result<PcmChunk> {
        let mut out = Vec::with_capacity(self.samples_to_bytes(samples).max(0) as usize);
        let mut remaining = samples;
        let channels = if self.pcm.chunks.is_empty() {
            1
        } else {
            self.output_channels()?
        };
        while let Some(mut chunk) = self.pcm.chunks.pop_front() {
            self.pcm.frames -= self.bytes_to_samples(chunk.len() as i32) / channels;
            let want = self.samples_to_bytes(remaining);
            let take = (chunk.len() as i32).min(want).max(0) as usize;
            out.extend_from_slice(&chunk[..take]);
            remaining -= self.bytes_to_samples(take as i32);
            if take < chunk.len() {
                chunk.drain(..take);
                self.pcm.frames += self.bytes_to_samples(chunk.len() as i32) / channels;
                self.pcm.chunks.push_front(chunk);
            }
            if remaining <= 0 {
                break;
            }
        }
        if self.pcm.chunks.is_empty() && self.state == DecoderState::Draining {
            self.state = DecoderState::Ended;
        }
        Ok(PcmChunk { bytes: out })
    }
}
