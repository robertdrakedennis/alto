//! The identification and setup headers of a Vorbis stream.

use super::bits::BitReader;
use super::codebook::{Codebook, SetKey, SetupCache};
use super::floor::Floor;
use super::imdct::MdctTables;
use super::mapping::Mapping;
use super::residue::Residue;
use anyhow::{bail, ensure, Result};
use std::sync::Arc;

/// Which of the three Vorbis header packets a packet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeaderKind {
    Identification,
    Comment,
    Setup,
}

impl HeaderKind {
    /// Classify the header packet at `data[offset..]` by its type byte and
    /// the `vorbis` signature that follows it.
    pub(super) fn of(data: &[u8], offset: usize) -> Result<Self> {
        let is = |kind: u8| {
            data.get(offset..offset + 7)
                .is_some_and(|h| h[0] == kind && &h[1..] == b"vorbis")
        };
        if is(1) {
            Ok(Self::Identification)
        } else if is(3) {
            Ok(Self::Comment)
        } else if is(5) {
            Ok(Self::Setup)
        } else {
            bail!("unknown Vorbis header packet")
        }
    }
}

/// The stream properties of the identification header.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Identification {
    pub(super) channels: i32,
    pub(super) sample_rate: i32,
    pub(super) bitrate_upper: i32,
    pub(super) bitrate_nominal: i32,
    pub(super) bitrate_lower: i32,
    /// Sizes of the short and long blocks, in samples.
    pub(super) short_block: i32,
    pub(super) long_block: i32,
}

impl Identification {
    /// Parse the identification header packet at `data[offset..]`.
    pub(super) fn parse(data: &[u8], offset: usize) -> Result<Self> {
        let mut bits = BitReader::new(data, offset + 7);
        let version = bits.read(32);
        let channels = bits.read(8);
        let sample_rate = bits.read(32);
        let bitrate_upper = bits.read(32);
        let bitrate_nominal = bits.read(32);
        let bitrate_lower = bits.read(32);
        let short_block = 1 << bits.read(4);
        let long_block = 1 << bits.read(4);
        bits.read(1); // framing flag (not checked)
        ensure!(version == 0, "unsupported Vorbis version {version}");
        ensure!(
            channels > 0 && short_block >= 64 && short_block <= long_block,
            "unusable Vorbis identification header"
        );
        Ok(Self {
            channels,
            sample_rate,
            bitrate_upper,
            bitrate_nominal,
            bitrate_lower,
            short_block,
            long_block,
        })
    }

    fn set_key(&self) -> SetKey {
        SetKey {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bitrate_upper: self.bitrate_upper,
            bitrate_lower: self.bitrate_lower,
            bitrate_nominal: self.bitrate_nominal,
        }
    }
}

/// A block mode: its size class and the mapping that decodes it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Mode {
    pub(super) long_block: bool,
    pub(super) mapping: usize,
}

/// Everything the setup header defines.
pub(super) struct Setup {
    pub(super) books: Arc<Vec<Codebook>>,
    pub(super) floors: Vec<Floor>,
    pub(super) residues: Vec<Residue>,
    pub(super) mappings: Vec<Mapping>,
    pub(super) modes: Vec<Mode>,
    /// Transform tables for the short and long block sizes.
    pub(super) tables: [MdctTables; 2],
}

impl Setup {
    /// Parse the setup header packet at `data[offset..]` for a stream with
    /// the given identification.
    pub(super) fn parse(
        data: &[u8],
        offset: usize,
        ident: &Identification,
        cache: &SetupCache,
    ) -> Result<Self> {
        let mut bits = BitReader::new(data, offset + 7);
        let tables = [
            MdctTables::new(ident.short_block),
            MdctTables::new(ident.long_block),
        ];
        let book_count = (bits.read(8) + 1) as usize;
        let books = cache.read_books(&mut bits, ident.set_key(), book_count)?;
        let time_entries = bits.read(6) + 1;
        for _ in 0..time_entries {
            bits.read(16); // time domain transforms (unused placeholders)
        }
        let floor_count = bits.read(6) + 1;
        let floors = (0..floor_count)
            .map(|_| Floor::parse(&mut bits, ident.channels))
            .collect::<Result<Vec<_>>>()?;
        let residue_count = bits.read(6) + 1;
        let residues = (0..residue_count)
            .map(|_| Residue::parse(&mut bits))
            .collect();
        let mapping_count = bits.read(6) + 1;
        let mappings = (0..mapping_count)
            .map(|_| Mapping::parse(&mut bits, ident.channels))
            .collect();
        let mode_count = bits.read(6) + 1;
        let modes = (0..mode_count)
            .map(|_| {
                let long_block = bits.read_bit() != 0;
                bits.read(16); // window type
                bits.read(16); // transform type
                let mapping = bits.read(8) as usize;
                Mode {
                    long_block,
                    mapping,
                }
            })
            .collect();
        Ok(Self {
            books,
            floors,
            residues,
            mappings,
            modes,
            tables,
        })
    }
}
