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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each read against the expected arithmetic on hand-made bytes.
    #[test]
    fn reads_follow_the_packet_formats() {
        // g1 is unsigned, g1b signed.
        let mut p = Packet::new(&[0xFF, 0xFF]);
        assert_eq!(p.g1().unwrap(), 255);
        assert_eq!(p.g1b().unwrap(), -1);
        assert!(p.is_empty());
        // g2 big-endian unsigned; g2s subtracts 65536 above 32767.
        let mut p = Packet::new(&[0x80, 0x01, 0x80, 0x01, 0x7F, 0xFF]);
        assert_eq!(p.g2().unwrap(), 0x8001);
        assert_eq!(p.g2s().unwrap(), 0x8001 - 65536);
        assert_eq!(p.g2s().unwrap(), 32767);
        // g4s big-endian; gFloat is the float with the bits of a g4s.
        let mut p = Packet::new(&[
            0xFF, 0xFF, 0xFF, 0xFE, 0x3F, 0xC0, 0x00, 0x00, 0xC2, 0x28, 0x00, 0x00,
        ]);
        assert_eq!(p.g4s().unwrap(), -2);
        assert_eq!(p.gfloat().unwrap().to_bits(), 1.5f32.to_bits());
        assert_eq!(p.gfloat().unwrap().to_bits(), (-42.0f32).to_bits());
        assert_eq!(p.pos, 12);
        // gSmart1or2: `peek < 128 ? g1() : g2() - 32768`.
        let mut p = Packet::new(&[0x7F, 0x80, 0x00, 0xFF, 0xFF]);
        assert_eq!((p.gsmart1or2().unwrap(), p.pos), (127, 1));
        assert_eq!((p.gsmart1or2().unwrap(), p.pos), (0, 3));
        assert_eq!((p.gsmart1or2().unwrap(), p.pos), (32767, 5));
    }

    /// `readLandscape` moves `pos` itself; reads resume there, and a read
    /// past the end fails at the first missing byte instead of panicking.
    #[test]
    fn reads_resume_at_pos_and_stop_at_the_end() {
        let mut p = Packet::new(&[9, 0x12, 0x34]);
        p.pos = 1;
        assert!(!p.is_empty());
        assert_eq!(p.g2().unwrap(), 0x1234);
        assert!(p.is_empty());
        assert!(p.g1().is_err());
        p.pos = 10;
        assert!(p.is_empty());
        assert!(p.g1b().is_err());

        let mut short = Packet::new(&[1, 2, 3]);
        assert!(short.g4s().is_err());
        assert_eq!(short.pos, 3);
        let mut smart = Packet::new(&[0x80]);
        assert!(smart.gsmart1or2().is_err());
        assert!(Packet::new(&[]).gfloat().is_err());
    }
}
