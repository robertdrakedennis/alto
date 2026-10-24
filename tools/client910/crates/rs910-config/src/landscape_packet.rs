//! The byte-stream reads the landscape loader and the landscape-side config
//! decoders (light types, environments, skybox types) use, over
//! `rs910_core::reader`.
//! Split out of client910's `maploader` (Phase 2.8), breaking the
//! `env`/`floorlight`/`maploader` cycle; `maploader` re-exports it.

use rs910_core::reader::{Eof, Reader as CoreReader};

// ---------------------------------------------------------------------------
// Landscape packet reads
// ---------------------------------------------------------------------------

/// The stream reads the landscape loader uses.
pub struct Packet<'a> {
    data: &'a [u8],
    /// Read position; the landscape loader bumps it directly.
    pub pos: usize,
}

impl<'a> Packet<'a> {
    /// Wrap a LAND stream.
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
        out.map_err(|e| anyhow::anyhow!("landscape stream truncated at {}", e.pos))
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
}
