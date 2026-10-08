//! Big-endian packet layer — the byte discipline every 910 codec builds on.
//!
//! Mirrors the 910 client's `ByteBuf` semantics: multi-byte integers are big-endian, strings are
//! NUL-terminated raw bytes (charset mapping belongs to the format codec, not to this layer, so
//! `gstrn` borrows the raw slice and never guesses an encoding).

use crate::error::{NativeError, Result};
use encoding_rs::WINDOWS_1252;

const UNASSIGNED_CP1252: [char; 5] = ['\u{81}', '\u{8d}', '\u{8f}', '\u{90}', '\u{9d}'];

/// Cursor reader over a borrowed byte slice.
#[derive(Debug)]
pub struct Packet<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Packet<'a> {
    /// Wrap `data` with the cursor at zero.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Wrap `data` with the cursor at `pos`. Out-of-range starts are an error.
    pub fn with_pos(data: &'a [u8], pos: usize) -> Result<Self> {
        if pos > data.len() {
            return Err(NativeError::Invalid(format!(
                "packet position out of bounds: {pos} > {}",
                data.len()
            )));
        }
        Ok(Self { data, pos })
    }

    /// Current cursor position.
    #[must_use]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Bytes remaining from the cursor.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Total length of the wrapped slice.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the cursor is at the end.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Move the cursor; out-of-range positions are an error, never a wrap.
    pub fn set_pos(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(NativeError::Invalid(format!(
                "cursor position {pos} past end {}",
                self.data.len()
            )));
        }
        self.pos = pos;
        Ok(())
    }

    fn take(&mut self, count: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or(NativeError::Truncated { what })?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    /// One unsigned byte.
    pub fn g1(&mut self) -> Result<u8> {
        Ok(self.take(1, "u8")?[0])
    }

    /// One signed byte.
    pub fn g1b(&mut self) -> Result<i8> {
        Ok(self.take(1, "i8")?[0] as i8)
    }

    /// One big-endian unsigned short.
    pub fn g2(&mut self) -> Result<u16> {
        let bytes = self.take(2, "u16")?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// One big-endian signed short.
    pub fn g2s(&mut self) -> Result<i16> {
        let bytes = self.take(2, "i16")?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// One big-endian 3-byte unsigned int.
    pub fn g3(&mut self) -> Result<u32> {
        let bytes = self.take(3, "u24")?;
        Ok((u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]))
    }

    /// One big-endian signed int.
    pub fn g4s(&mut self) -> Result<i32> {
        let bytes = self.take(4, "i32")?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// One big-endian signed long.
    pub fn g8s(&mut self) -> Result<i64> {
        let bytes = self.take(8, "i64")?;
        Ok(i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Raw `length` bytes.
    pub fn gdata(&mut self, length: usize, what: &'static str) -> Result<Vec<u8>> {
        Ok(self.take(length, what)?.to_vec())
    }

    /// NUL-terminated Windows-1252 string (the 910 client's `gjstr`). A missing
    /// terminator is truncation; the five unassigned bytes decode to `?`.
    pub fn gjstr(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.data.len() {
            let value = self.data[self.pos];
            self.pos += 1;
            if value == 0 {
                let bytes = &self.data[start..self.pos - 1];
                let (decoded, _, had_errors) = WINDOWS_1252.decode(bytes);
                if had_errors {
                    return Err(NativeError::Invalid(format!(
                        "windows-1252 decode error at offset {start}"
                    )));
                }
                return Ok(decoded
                    .chars()
                    .map(|character| {
                        if UNASSIGNED_CP1252.contains(&character) {
                            '?'
                        } else {
                            character
                        }
                    })
                    .collect());
            }
        }
        Err(NativeError::Invalid(format!(
            "unterminated string at offset {start}"
        )))
    }

    /// `gjstr` that maps an empty (single NUL) value to `None` — the 910
    /// client's `gjstrnull`, used for optional script names.
    pub fn gjstrnull(&mut self) -> Result<Option<String>> {
        if self.pos >= self.data.len() {
            return Err(NativeError::Truncated {
                what: "optional string",
            });
        }
        if self.data[self.pos] == 0 {
            self.pos += 1;
            Ok(None)
        } else {
            Ok(Some(self.gjstr()?))
        }
    }

    /// `gjstr2`: a leading zero marker byte (anything else is invalid data),
    /// then a Windows-1252 string exactly like [`Packet::gjstr`].
    pub fn gjstr2(&mut self) -> Result<String> {
        let marker = self.g1()?;
        if marker != 0 {
            return Err(NativeError::Invalid(format!(
                "gjstr2 marker mismatch: expected 0, got {marker}"
            )));
        }
        self.gjstr()
    }

    /// js5 smart int: one unsigned short, or (when the high bit is set) a
    /// 4-byte int masked to 31 bits. `32767` reads as `-1` (null).
    pub fn gsmart2or4null(&mut self) -> Result<i32> {
        if self.pos >= self.data.len() {
            return Err(NativeError::Truncated { what: "smart int" });
        }
        if self.data[self.pos] as i8 >= 0 {
            let value = i32::from(self.g2()?);
            Ok(if value == 32767 { -1 } else { value })
        } else {
            Ok(self.g4s()? & i32::MAX)
        }
    }
}

/// Big-endian writer. Operands are pushed in encode order; branch/switch fixups
/// patch relative offsets with `patch_i32_at` after layout.
#[derive(Debug, Default)]
pub struct ByteWriter {
    /// Encoded bytes so far.
    pub data: Vec<u8>,
}

impl ByteWriter {
    /// Writer with pre-sized capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    /// Bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether nothing has been written yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// One unsigned byte.
    pub fn p1(&mut self, value: u8) {
        self.data.push(value);
    }

    /// One signed byte.
    pub fn p1b(&mut self, value: i8) {
        self.data.push(value as u8);
    }

    /// One big-endian unsigned short.
    pub fn p2(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// One big-endian signed short.
    pub fn p2s(&mut self, value: i16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// One big-endian 3-byte unsigned int (must fit 24 bits).
    pub fn p3(&mut self, value: u32) -> Result<()> {
        if value > 0xFF_FFFF {
            return Err(NativeError::Invalid(format!(
                "value {value} does not fit in 3 bytes"
            )));
        }
        self.data.push((value >> 16) as u8);
        self.data.push((value >> 8) as u8);
        self.data.push(value as u8);
        Ok(())
    }

    /// One big-endian signed int.
    pub fn p4s(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// One big-endian signed long.
    pub fn p8s(&mut self, value: i64) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Raw bytes.
    pub fn pdata(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    /// NUL-terminated Windows-1252 string (the 910 client's `pjstr`). Characters
    /// outside Windows-1252 are rejected: emitting raw UTF-8 would produce bytes
    /// the client mis-renders and this tool cannot re-decode.
    pub fn pjstr(&mut self, value: &str) -> Result<()> {
        if value.contains('\0')
            || value
                .chars()
                .any(|character| UNASSIGNED_CP1252.contains(&character))
        {
            return Err(NativeError::Invalid(
                "string cannot round-trip through client Windows-1252".into(),
            ));
        }
        let (encoded, _, had_errors) = WINDOWS_1252.encode(value);
        if had_errors {
            return Err(NativeError::Invalid(format!(
                "string contains characters outside Windows-1252: {value:?}"
            )));
        }
        self.data.extend_from_slice(&encoded);
        self.data.push(0);
        Ok(())
    }

    /// `pjstr` with `None` written as a single NUL — the 910 client's `pjstrnull`.
    pub fn pjstrnull(&mut self, value: Option<&str>) -> Result<()> {
        match value {
            None => {
                self.data.push(0);
                Ok(())
            }
            Some(text) => self.pjstr(text),
        }
    }

    /// Inverse of [`Packet::gjstr2`]: the zero marker byte, then the string.
    pub fn pjstr2(&mut self, value: &str) -> Result<()> {
        self.data.push(0);
        self.pjstr(value)
    }

    /// Overwrite the i32 at `pos` (big-endian). Used for branch/switch fixups
    /// after the instruction stream is laid out.
    pub fn patch_i32_at(&mut self, pos: usize, value: i32) -> Result<()> {
        let end = pos
            .checked_add(4)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| {
                NativeError::Invalid(format!(
                    "patch position {pos} out of bounds (len {})",
                    self.data.len()
                ))
            })?;
        self.data[pos..end].copy_from_slice(&value.to_be_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_roundtrip_covers_extremes() {
        let mut writer = ByteWriter::default();
        writer.p1(u8::MIN);
        writer.p1(u8::MAX);
        writer.p2(u16::MIN);
        writer.p2(u16::MAX);
        writer.p4s(i32::MIN);
        writer.p4s(0);
        writer.p4s(i32::MAX);
        writer.p8s(i64::MIN);
        writer.p8s(0);
        writer.p8s(i64::MAX);

        let mut packet = Packet::new(&writer.data);
        assert_eq!(packet.g1().unwrap(), u8::MIN);
        assert_eq!(packet.g1().unwrap(), u8::MAX);
        assert_eq!(packet.g2().unwrap(), u16::MIN);
        assert_eq!(packet.g2().unwrap(), u16::MAX);
        assert_eq!(packet.g4s().unwrap(), i32::MIN);
        assert_eq!(packet.g4s().unwrap(), 0);
        assert_eq!(packet.g4s().unwrap(), i32::MAX);
        assert_eq!(packet.g8s().unwrap(), i64::MIN);
        assert_eq!(packet.g8s().unwrap(), 0);
        assert_eq!(packet.g8s().unwrap(), i64::MAX);
        assert!(packet.is_empty());
    }

    #[test]
    fn short_reads_are_truncation_not_panic() {
        let data = [0x12, 0x34, 0x56];
        let mut packet = Packet::new(&data);
        assert_eq!(packet.g2().unwrap(), 0x1234);
        assert!(packet.g2().is_err());
        assert!(packet.g4s().is_err());
        assert!(packet.g8s().is_err());
        let mut empty = Packet::new(&[]);
        assert!(empty.g1().is_err());
        assert!(empty.gjstr().is_err());
        assert!(empty.gjstrnull().is_err());
        assert!(empty.gsmart2or4null().is_err());
    }

    #[test]
    fn cursor_discipline() {
        let data = [0xAA, 0xBB, 0xCC];
        let mut packet = Packet::new(&data);
        assert_eq!(packet.len(), 3);
        assert_eq!(packet.remaining(), 3);
        assert_eq!(packet.g1().unwrap(), 0xAA);
        assert_eq!(packet.pos(), 1);
        packet.set_pos(0).unwrap();
        assert_eq!(packet.g1().unwrap(), 0xAA);
        assert!(packet.set_pos(4).is_err());
        assert!(packet.set_pos(usize::MAX).is_err());
    }

    #[test]
    fn strings_roundtrip_byte_exact() {
        let mut writer = ByteWriter::default();
        writer.pjstr("[clientscript,bank_build_init]").unwrap();
        writer.pjstr("").unwrap();
        writer.pjstr("caf\u{e9} \u{2014}").unwrap();
        writer.pjstrnull(None).unwrap();
        writer.pjstrnull(Some("named")).unwrap();

        let mut packet = Packet::new(&writer.data);
        assert_eq!(packet.gjstr().unwrap(), "[clientscript,bank_build_init]");
        assert_eq!(packet.gjstr().unwrap(), "");
        assert_eq!(packet.gjstr().unwrap(), "caf\u{e9} \u{2014}");
        assert_eq!(packet.gjstrnull().unwrap(), None);
        assert_eq!(packet.gjstrnull().unwrap().as_deref(), Some("named"));
        assert!(packet.is_empty());
    }

    #[test]
    fn strings_reject_unrepresentable_text() {
        let mut writer = ByteWriter::default();
        // U+1F600 has no Windows-1252 mapping: reject rather than emit mojibake.
        assert!(writer.pjstr("\u{1f600}").is_err());
        assert!(writer.pjstr("a\0b").is_err());
        for character in UNASSIGNED_CP1252 {
            assert!(writer.pjstr(&character.to_string()).is_err());
        }
        assert!(writer.is_empty());
    }

    #[test]
    fn smart_int_covers_both_widths_and_null() {
        let mut writer = ByteWriter::default();
        writer.p2(300);
        writer.p2(32767);
        writer.p4s(i32::MIN | 0x1234);
        let mut packet = Packet::new(&writer.data);
        assert_eq!(packet.gsmart2or4null().unwrap(), 300);
        assert_eq!(packet.gsmart2or4null().unwrap(), -1);
        assert_eq!(
            packet.gsmart2or4null().unwrap(),
            (i32::MIN | 0x1234) & i32::MAX
        );
    }

    #[test]
    fn signed_and_wide_primitives_roundtrip() {
        let mut writer = ByteWriter::default();
        writer.p1b(i8::MIN);
        writer.p1b(-1);
        writer.p1b(i8::MAX);
        writer.p2s(i16::MIN);
        writer.p2s(-1);
        writer.p2s(i16::MAX);
        writer.p3(0).unwrap();
        writer.p3(0xFF_FFFF).unwrap();
        writer.pjstr2("hi").unwrap();
        writer.pjstr2("").unwrap();

        let mut packet = Packet::new(&writer.data);
        assert_eq!(packet.g1b().unwrap(), i8::MIN);
        assert_eq!(packet.g1b().unwrap(), -1);
        assert_eq!(packet.g1b().unwrap(), i8::MAX);
        assert_eq!(packet.g2s().unwrap(), i16::MIN);
        assert_eq!(packet.g2s().unwrap(), -1);
        assert_eq!(packet.g2s().unwrap(), i16::MAX);
        assert_eq!(packet.g3().unwrap(), 0);
        assert_eq!(packet.g3().unwrap(), 0xFF_FFFF);
        assert_eq!(packet.gjstr2().unwrap(), "hi");
        assert_eq!(packet.gjstr2().unwrap(), "");
        assert!(packet.is_empty());
    }

    #[test]
    fn wide_primitives_reject_garbage() {
        let mut writer = ByteWriter::default();
        assert!(writer.p3(0x1_000000).is_err());
        // gjstr2 with a nonzero marker is invalid data, not a string.
        let mut bad = ByteWriter::default();
        bad.p1(7);
        bad.pjstr("x").unwrap();
        let mut packet = Packet::new(&bad.data);
        assert!(packet.gjstr2().is_err());
    }

    #[test]
    fn patch_rewrites_in_place() {
        let mut writer = ByteWriter::default();
        writer.p2(0xCAFE);
        let pos = writer.len();
        writer.p4s(0);
        writer.p4s(7);
        writer.patch_i32_at(pos, -3).unwrap();
        assert_eq!(
            &writer.data,
            &[0xCA, 0xFE, 0xFF, 0xFF, 0xFF, 0xFD, 0, 0, 0, 7]
        );
        assert!(writer.patch_i32_at(writer.len(), 1).is_err());
        assert!(writer.patch_i32_at(usize::MAX, 1).is_err());
    }
}
