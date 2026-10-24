//! The client's sound container: a small header in front of the Ogg/Vorbis
//! stream that says how the sound loops and where each chunk of the stream
//! is stored.
//!
//! Layout (big-endian): the ASCII tag `JAGA`, then loop start, loop end,
//! sample rate, channel count and chunk count as 32-bit integers, then a
//! table of `(size, group)` pairs, one per chunk, then the bytes of the first
//! chunk. A whole-sound `vorbis` group holds all its chunks after the table;
//! a song in the `audiostreams` archive holds only the first and names the
//! archive groups of the rest (`group`).

use anyhow::{ensure, Result};
use rs910_core::reader::Reader;

/// One chunk of the Ogg stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Where the chunk starts inside the file (chunks follow the table in
    /// order).
    pub offset: usize,
    pub size: usize,
    /// The archive group that holds the chunk when it is not inline.
    pub group: i32,
}

/// The parsed header of a sound file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundFile {
    /// Loop points, in samples.
    pub loop_start: i32,
    pub loop_end: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub chunks: Vec<Chunk>,
    /// The first byte after the chunk table: the start of the first chunk.
    pub table_end: usize,
}

impl SoundFile {
    /// Parse the header at the start of `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 24 && &bytes[..4] == b"JAGA",
            "not a sound file"
        );
        let mut reader = Reader::at(bytes, 4);
        let loop_start = reader.g4s()?;
        let loop_end = reader.g4s()?;
        let sample_rate = reader.g4s()?;
        let channels = reader.g4s()?;
        let count = reader.g4s()?;
        ensure!(count >= 0, "negative chunk count");
        let mut offset = count as usize * 8 + reader.pos();
        let mut chunks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let size = reader.g4s()?;
            let group = reader.g4s()?;
            chunks.push(Chunk {
                offset,
                size: size.max(0) as usize,
                group,
            });
            offset = offset.wrapping_add(size as usize);
        }
        Ok(Self {
            loop_start,
            loop_end,
            sample_rate,
            channels,
            chunks,
            table_end: reader.pos(),
        })
    }
}
