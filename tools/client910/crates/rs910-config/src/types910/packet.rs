//! A read cursor with bit access over one buffer: the cursor the `*_types`
//! decoders here and `rs910_game::protocol910`'s appliers share. It reads
//! through [`Source`], so the opcode tables of [`crate::opcode_table`] drive it.
use super::{Error, Result};
use crate::opcode_table::{Record, Source};
use rs910_core::reader::{Eof, Reader as CoreReader};
type CoreResult<T> = std::result::Result<T, Eof>;
pub struct Packet<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub bit: usize,
}
impl<'a> Packet<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit: 0,
        }
    }
    pub fn bits(&mut self, n: usize) -> Result<i32> {
        if self.bit + n > self.data.len() * 8 {
            return Err(Error::Truncated { bit: self.bit });
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | ((self.data[self.bit / 8] >> (7 - self.bit % 8)) & 1) as u32;
            self.bit += 1;
        }
        Ok(v as i32)
    }
    pub fn access_bits(&mut self) {
        self.bit = self.pos * 8
    }
    pub fn access_bytes(&mut self) {
        self.pos = self.bit.div_ceil(8)
    }
    /// One `rs910_core::reader` read at `pos` (the byte arithmetic, including
    /// the `_alt` transforms, lives there). `pos` follows the read; a failure
    /// names the missing byte's bit offset.
    pub fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> CoreResult<T>,
    ) -> Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|e| Error::Truncated { bit: e.pos * 8 })
    }
    pub fn byte(&mut self) -> Result<u32> {
        self.read(CoreReader::g1).map(u32::from)
    }
    pub fn g2(&mut self) -> Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }
    pub fn g1_alt2(&mut self) -> Result<i32> {
        self.read(CoreReader::g1_alt2).map(i32::from)
    }
    pub fn g1_alt1(&mut self) -> Result<i32> {
        self.read(CoreReader::g1_alt1).map(i32::from)
    }
    pub fn g1_alt3(&mut self) -> Result<i32> {
        self.read(CoreReader::g1_alt3).map(i32::from)
    }
    pub fn g1b_alt2(&mut self) -> Result<i32> {
        self.read(CoreReader::g1b_alt2).map(i32::from)
    }
    pub fn g1b_alt3(&mut self) -> Result<i32> {
        self.read(CoreReader::g1b_alt3).map(i32::from)
    }
    pub fn g4_alt1(&mut self) -> Result<i32> {
        self.read(CoreReader::g4_alt1)
    }
    pub fn g4s(&mut self) -> Result<i32> {
        self.read(CoreReader::g4s)
    }
    pub fn g2_alt1(&mut self) -> Result<i32> {
        self.read(CoreReader::g2_alt1).map(i32::from)
    }
    pub fn g2_alt2(&mut self) -> Result<i32> {
        self.read(CoreReader::g2_alt2).map(i32::from)
    }
    pub fn g2_alt3(&mut self) -> Result<i32> {
        self.read(CoreReader::g2_alt3).map(i32::from)
    }
    pub fn g2s_alt1(&mut self) -> Result<i32> {
        self.read(CoreReader::g2s_alt1).map(i32::from)
    }
    pub fn le2(&mut self) -> Result<i32> {
        self.g2_alt1()
    }
    pub fn alt2(&mut self) -> Result<i32> {
        self.g2_alt2()
    }
    pub fn peek(&self) -> Result<u8> {
        CoreReader::at(self.data, self.pos)
            .peek()
            .map_err(|e| Error::Truncated { bit: e.pos * 8 })
    }
    pub fn smart1(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart1or2)
    }
    pub fn smart2(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart2or4s)
    }
    pub fn string(&mut self) -> Result<String> {
        let bytes = self.read(CoreReader::gjstr_bytes)?;
        // Lenient Windows-1252: unassigned bytes decode to `?`.
        Ok(bytes
            .iter()
            .map(|&b| rs910_core::cp1252::cp1252_decode_byte(b))
            .collect())
    }
    pub fn alt3(&mut self) -> Result<i32> {
        self.g2_alt3()
    }
}

impl Source for Packet<'_> {
    type Error = Error;

    fn opcode(&mut self, _: Record) -> Result<u8> {
        Ok(Packet::byte(self)? as u8)
    }

    fn unknown_opcode(&self, record: Record, opcode: u8) -> Error {
        Error::UnsupportedConfig {
            kind: record.kind,
            id: record.id as i32,
            opcode,
        }
    }

    fn byte(&mut self) -> Result<u8> {
        self.read(CoreReader::g1)
    }

    fn peek(&self) -> Result<u8> {
        Packet::peek(self)
    }

    fn text(&mut self) -> Result<String> {
        self.string()
    }

    fn short(&mut self) -> Result<u16> {
        self.read(CoreReader::g2)
    }

    fn medium(&mut self) -> Result<u32> {
        self.read(CoreReader::g3)
    }

    fn int(&mut self) -> Result<i32> {
        self.read(CoreReader::g4s)
    }

    fn smart(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart1or2)
    }

    fn smart_signed(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart1or2s)
    }

    fn smart_nullable(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart1or2null)
    }

    fn smart_id(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart2or4s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit access: bit reads start at `pos * 8`, run MSB first across byte
    /// boundaries, and leaving bit mode rounds the bit cursor up to the next
    /// byte. Expected values are worked out by hand.
    #[test]
    fn bit_access_runs_msb_first_across_bytes() {
        let data = [0x05, 0b1011_0011, 0b0101_1100, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut p = Packet::new(&data);
        assert_eq!(p.byte(), Ok(5));
        p.access_bits();
        assert_eq!(p.bit, 8);
        assert_eq!(p.bits(3), Ok(0b101));
        // Spans bytes 1 and 2: `10011` + `0101`.
        assert_eq!(p.bits(9), Ok(0b1_0011_0101));
        assert_eq!(p.bits(1), Ok(1));
        assert_eq!(p.bit, 21);
        p.access_bytes();
        assert_eq!(p.pos, 3, "(bitPos + 7) / 8");
        // A 32-bit read of all ones is -1.
        p.access_bits();
        assert_eq!(p.bits(32), Ok(-1));
        // Past the end: an error naming the bit the read started at.
        assert_eq!(p.bits(1), Err(Error::Truncated { bit: 56 }));
    }
}
