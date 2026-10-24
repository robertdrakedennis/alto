//! The client's Ogg/Vorbis decoder. Every sound the client plays (effects,
//! jingles and songs) is stored as Ogg/Vorbis in its sound archives; there
//! is no MIDI or synthesiser path.
//!
//! The decoder is built from the stages of the Vorbis specification, each in
//! its own module:
//!
//! - `bits`: the bit reader and the small integer helpers;
//! - `setup`: the identification and setup headers, and the [`SetupCache`]
//!   (`codebook`) that lets sounds from one encoder share their codebooks;
//! - `floor` and `residue`: the two halves of a block's spectrum;
//! - `mapping`: channel coupling and the floor/residue routing per channel;
//! - `imdct`: the inverse MDCT and the block window;
//! - `block`: one audio packet, from bits to overlap-added samples;
//! - `decoder`: the streaming [`VorbisDecoder`] (Ogg pages, PCM queue and
//!   the sample-exact loop handling).
//!
//! The client's decoder differs from a general Vorbis library in details
//! that the recorded audio depends on: floor type 1 only, output samples
//! truncated (not rounded) to 16 bits, residue type 2 decoded over a working
//! buffer that is not cleared first, and channel buffers shared between the
//! channels of multi-submap mappings. Those are kept and noted where they
//! occur.
//!
//! Where these differ from a reference library the output is the original
//! client's, not the library's. Two recordings pin this
//! (`fixtures/recorded/audio-vorbis`, effect 356 and song 2): the
//! working-buffer quirk makes stereo residue-type-2 sounds differ from ffmpeg
//! by small errors on some blocks (rms 17 on song 2), and a sound that is a
//! chain of Ogg streams (effect 356 has fourteen, each with its own headers)
//! is cut to each stream's last granule position, with nothing carried over,
//! so a library that plays the streams back to back yields extra frames at
//! every join. Compare against ffmpeg only per stream.

mod bits;
mod block;
mod codebook;
mod decoder;
mod floor;
mod imdct;
mod mapping;
mod residue;
mod setup;
mod windows;

pub use codebook::SetupCache;
pub use decoder::{DecoderState, PcmChunk, VorbisDecoder};

/// The sample format of the PCM the decoder writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    Signed16,
    Unsigned16,
    Signed8,
    Unsigned8,
}

/// The byte order of 16-bit output samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endianness {
    Little,
    Big,
}

impl SampleFormat {
    /// Bits per sample.
    pub fn bits(self) -> i32 {
        match self {
            Self::Signed16 | Self::Unsigned16 => 16,
            Self::Signed8 | Self::Unsigned8 => 8,
        }
    }

    /// Append `value` (nominally -1.0 to 1.0) to `out`: scaled, truncated
    /// towards zero and clamped.
    fn encode(self, value: f32, endianness: Endianness, out: &mut Vec<u8>) {
        let put16 = |out: &mut Vec<u8>, v: i32| {
            let bytes = if endianness == Endianness::Little {
                (v as u16).to_le_bytes()
            } else {
                (v as u16).to_be_bytes()
            };
            out.extend_from_slice(&bytes);
        };
        match self {
            Self::Signed16 => put16(out, ((value * 32767.0) as i32).clamp(-32768, 32767)),
            Self::Unsigned16 => put16(out, ((value * 32767.0 + 32768.0) as i32).clamp(0, 65535)),
            Self::Signed8 => out.push(((value * 127.0) as i32).clamp(-128, 127) as u8),
            Self::Unsigned8 => out.push(((value * 127.0 + 128.0) as i32).clamp(0, 255) as u8),
        }
    }
}
