//! Byte-stream reads shared by the client-config owners of the retained UI
//! engine (`g1`, `g1b`, `g2`, `g3`, `g4s`, `g8`, `gjstr`, `gSmart1or2`,
//! `gSmart2or4s`, `gVarInt2`). Truncation is an error. Unassigned
//! Windows-1252 bytes use the client charset's `?` mapping. The byte arithmetic is `rs910_core::reader`'s; this cursor
//! keeps the UI decoders' error messages.
use anyhow::{anyhow, Result};
use rs910_core::reader::{Eof, Reader as CoreReader};

pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    /// One `rs910_core::reader` read at `pos` (the arithmetic lives
    /// there). `pos` follows the read; a failure names the
    /// offset of the missing byte.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> std::result::Result<T, Eof>,
    ) -> Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|e| anyhow!("truncated data at offset {}", e.pos))
    }
    /// Unsigned byte.
    pub fn g1(&mut self) -> Result<u8> {
        self.read(CoreReader::g1)
    }
    /// Signed byte.
    pub fn g1b(&mut self) -> Result<i8> {
        self.read(CoreReader::g1b)
    }
    /// Unsigned big-endian short.
    pub fn g2(&mut self) -> Result<u16> {
        self.read(CoreReader::g2)
    }
    /// Unsigned big-endian 24-bit int.
    pub fn g3(&mut self) -> Result<u32> {
        self.read(CoreReader::g3)
    }
    /// Signed big-endian int.
    pub fn g4s(&mut self) -> Result<i32> {
        self.read(CoreReader::g4s)
    }
    /// Two unsigned ints, high word first.
    pub fn g8(&mut self) -> Result<i64> {
        self.read(CoreReader::g8)
    }
    /// NUL-terminated Windows-1252 string.
    pub fn gjstr(&mut self) -> Result<String> {
        let start = self.pos;
        let bytes = self
            .read(CoreReader::gjstr_bytes)
            .map_err(|_| anyhow!("unterminated string starting at offset {start}"))?;
        Ok(bytes
            .iter()
            .copied()
            .map(rs910_core::cp1252::cp1252_decode_byte)
            .collect())
    }
    /// `gSmart1or2`: one byte below 128, else two bytes minus 32768.
    pub fn gsmart1or2(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart1or2)
    }
    /// `gSmart2or4s`: `32767` reads as `-1`.
    pub fn gsmart2or4s(&mut self) -> Result<i32> {
        self.read(CoreReader::gsmart2or4s)
    }
    /// `gVarInt2`: little-endian 7-bit groups.
    pub fn gvarint2(&mut self) -> Result<i32> {
        self.read(CoreReader::gvarint2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint2_and_smart_reads_follow_the_packet_formats() {
        let data = [0x81, 0x01, 0x7F, 0x80, 0x10, 0x7F, 0xFF, 0x00, 0x05];
        let mut c = Cursor::new(&data);
        assert_eq!(c.gvarint2().unwrap(), 129);
        assert_eq!(c.gsmart1or2().unwrap(), 0x7F);
        assert_eq!(c.gsmart1or2().unwrap(), 0x10);
        assert_eq!(c.gsmart2or4s().unwrap(), -1);
        assert_eq!(c.gsmart2or4s().unwrap(), 5);
        assert!(c.g1().is_err());
    }

    #[test]
    fn g8_combines_unsigned_words() {
        let data = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01];
        let mut c = Cursor::new(&data);
        assert_eq!(c.g8().unwrap(), (0xFFFF_FFFFu64 << 32) as i64 + 1);
    }

    /// A missing terminator fails; unassigned bytes follow the client charset.
    #[test]
    fn gjstr_matches_client_charset_and_requires_terminator() {
        let mut c = Cursor::new(&[b'A', 0x80, 0, b'B', 0]);
        assert_eq!(c.gjstr().unwrap(), "A\u{20AC}");
        assert_eq!(c.gjstr().unwrap(), "B");
        assert_eq!(c.remaining(), 0);
        assert_eq!(Cursor::new(&[b'A', 0x81, 0]).gjstr().unwrap(), "A?");
        assert!(Cursor::new(b"AB").gjstr().is_err());
    }
}
