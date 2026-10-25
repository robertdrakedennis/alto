//! The forward byte-stream reads the cache decoders share (landscape, light
//! and environment configs, models, particle configs) over
//! `rs910_core::reader`: big-endian widened to `i32`, `pos` public and
//! following each read, truncation errors naming the offset.

use rs910_core::reader::{Eof, Reader as CoreReader};

/// A read cursor over one cache file.
pub struct Packet<'a> {
    data: &'a [u8],
    /// Read position; decoders seek by setting it directly.
    pub pos: usize,
}

impl<'a> Packet<'a> {
    /// Read `data` from the start.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// `pos >= data.length`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// One `rs910_core::reader` read at `pos` (the arithmetic lives
    /// there). `pos` follows the read; a failure names the
    /// offset of the missing byte.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|e| anyhow::anyhow!("byte stream truncated at {}", e.pos))
    }

    /// `g1()`.
    pub fn g1(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g1).map(i32::from)
    }

    /// `g1b()`.
    pub fn g1b(&mut self) -> anyhow::Result<i8> {
        self.read(CoreReader::g1b)
    }

    /// `g2()`.
    pub fn g2(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }

    /// Signed big-endian short.
    pub fn g2s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2s).map(i32::from)
    }

    /// Signed big-endian int.
    pub fn g4s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g4s)
    }

    /// Big-endian IEEE-754 float (the bits of a signed int).
    pub fn gfloat(&mut self) -> anyhow::Result<f32> {
        self.read(CoreReader::gfloat)
    }

    /// `gSmart1or2()`: `peek < 128 ? g1() : g2() - 32768`.
    pub fn gsmart1or2(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2)
    }

    /// `gSmart1or2s()`: `peek < 128 ? g1() - 64 : g2() - 49152`.
    pub fn gsmart1or2s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2s)
    }

    /// `gSmart1or2()` minus one: `-1` encodes null.
    pub fn gsmart1or2null(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2null)
    }

    /// Unsigned big-endian 24-bit int.
    pub fn g3(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g3).map(|v| v as i32)
    }
}
