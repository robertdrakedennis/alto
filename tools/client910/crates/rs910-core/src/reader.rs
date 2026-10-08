//! Packet reads (plain big-endian reads, smarts, varints and the `_alt`
//! byte transforms) over a borrowed byte slice: the one copy of the
//! byte arithmetic every client decoder uses (Phase 2.1).
//!
//! Before Phase 2.1 each decoder carried its own cursor (config, avatar,
//! anim, model, modelunlit, flo, map, maploader, particle, sprite_data,
//! texture, loading, iface, ui_bytes, ui_world_list, ui_host_builtins,
//! audio_vorbis, protocol910, session, ui_backend). They keep their types and
//! error values (the messages callers and tests see) and call this module for
//! the reads. (The per-module pre-merge digests that proved the move were
//! retired in lane T-AUDIT; the reads are pinned by this module's tests and
//! by explicit vectors for the wrappers with logic of their own.)
//!
//! # Failure semantics
//!
//! Every read checks the bound byte by byte, like a bounds-checked array access: a
//! read that runs out stops at the first missing byte, leaving [`Reader::pos`]
//! there, and returns [`Eof`] with that offset. [`Reader::take`] and
//! [`Reader::skip`] check the whole length first and consume nothing on
//! failure, and [`Reader::atomic`] gives any read that contract. The original
//! client moves `pos` before its failed read raises; no caller reads
//! on after a failure, and the ones that report positions use these.

use std::fmt;

/// A read ran past the end of the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Eof {
    /// Where the read stopped: the offset of the first missing byte, or the
    /// unchanged position for [`Reader::take`], [`Reader::skip`] and
    /// [`Reader::atomic`].
    pub pos: usize,
}

impl fmt::Display for Eof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "read past the end of the data at offset {}", self.pos)
    }
}

impl std::error::Error for Eof {}

/// A read cursor over the packet bytes `data`.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A cursor at offset 0.
    #[inline]
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A cursor at `pos`, which may lie past the end (every read then fails).
    #[inline]
    #[must_use]
    pub const fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    /// The whole buffer.
    #[inline]
    #[must_use]
    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// `Packet.pos`.
    #[inline]
    #[must_use]
    pub const fn pos(&self) -> usize {
        self.pos
    }

    /// Set the read position (decoders assign it directly).
    #[inline]
    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Bytes left after `pos` (0 when `pos` is past the end).
    #[inline]
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Run `read`; if it fails, restore the position it started at.
    #[inline]
    pub fn atomic<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T, Eof>) -> Result<T, Eof> {
        let start = self.pos;
        let out = read(self);
        if out.is_err() {
            self.pos = start;
        }
        out
    }

    /// The next `n` bytes, all or nothing.
    #[inline]
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Eof> {
        let bytes = self
            .pos
            .checked_add(n)
            .and_then(|end| self.data.get(self.pos..end))
            .ok_or(Eof { pos: self.pos })?;
        self.pos += n;
        Ok(bytes)
    }

    /// Skip `n` bytes, all or nothing.
    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<(), Eof> {
        self.take(n).map(|_| ())
    }

    /// `data[pos] & 0xFF` without advancing (the smart reads' peek).
    #[inline]
    pub fn peek(&self) -> Result<u8, Eof> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or(Eof { pos: self.pos })
    }

    /// `g1`.
    #[inline]
    pub fn g1(&mut self) -> Result<u8, Eof> {
        let value = self.peek()?;
        self.pos += 1;
        Ok(value)
    }

    /// `g1b`.
    #[inline]
    pub fn g1b(&mut self) -> Result<i8, Eof> {
        Ok(self.g1()? as i8)
    }

    /// `g2`: big-endian unsigned short.
    #[inline]
    pub fn g2(&mut self) -> Result<u16, Eof> {
        let hi = u16::from(self.g1()?);
        let lo = u16::from(self.g1()?);
        Ok((hi << 8) | lo)
    }

    /// `g2s`: big-endian signed short.
    #[inline]
    pub fn g2s(&mut self) -> Result<i16, Eof> {
        Ok(self.g2()? as i16)
    }

    /// `g3`: big-endian unsigned 24-bit.
    #[inline]
    pub fn g3(&mut self) -> Result<u32, Eof> {
        let b0 = u32::from(self.g1()?);
        let b1 = u32::from(self.g1()?);
        let b2 = u32::from(self.g1()?);
        Ok((b0 << 16) | (b1 << 8) | b2)
    }

    /// `g3s`: `g3`, minus `2^24` above `8388607`.
    #[inline]
    pub fn g3s(&mut self) -> Result<i32, Eof> {
        let value = self.g3()? as i32;
        Ok(if value > 8_388_607 {
            value - 16_777_216
        } else {
            value
        })
    }

    /// `g4s`: big-endian int.
    #[inline]
    pub fn g4s(&mut self) -> Result<i32, Eof> {
        let b0 = u32::from(self.g1()?);
        let b1 = u32::from(self.g1()?);
        let b2 = u32::from(self.g1()?);
        let b3 = u32::from(self.g1()?);
        Ok(((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) as i32)
    }

    /// `g8`: two unsigned ints, high word first.
    #[inline]
    pub fn g8(&mut self) -> Result<i64, Eof> {
        let hi = i64::from(self.g4s()?) & 0xFFFF_FFFF;
        let lo = i64::from(self.g4s()?) & 0xFFFF_FFFF;
        Ok((hi << 32).wrapping_add(lo))
    }

    /// `gFloat`: `Float.intBitsToFloat(g4s())`.
    #[inline]
    pub fn gfloat(&mut self) -> Result<f32, Eof> {
        Ok(f32::from_bits(self.g4s()? as u32))
    }

    /// `gjstr`'s byte scan: the bytes before the next
    /// NUL, with `pos` moved past it. Unterminated: `pos` stops at the end.
    /// Decoding is the caller's (`crate::cp1252`).
    #[inline]
    pub fn gjstr_bytes(&mut self) -> Result<&'a [u8], Eof> {
        let start = self.pos;
        match self
            .data
            .get(start..)
            .and_then(|rest| rest.iter().position(|&b| b == 0))
        {
            Some(len) => {
                self.pos = start + len + 1;
                Ok(&self.data[start..start + len])
            }
            None => {
                self.pos = start.max(self.data.len());
                Err(Eof { pos: self.pos })
            }
        }
    }

    /// `gSmart1or2s`.
    #[inline]
    pub fn gsmart1or2s(&mut self) -> Result<i32, Eof> {
        if self.peek()? < 128 {
            Ok(i32::from(self.g1()?) - 64)
        } else {
            Ok(i32::from(self.g2()?) - 49152)
        }
    }

    /// `gSmart1or2`.
    #[inline]
    pub fn gsmart1or2(&mut self) -> Result<i32, Eof> {
        if self.peek()? < 128 {
            Ok(i32::from(self.g1()?))
        } else {
            Ok(i32::from(self.g2()?) - 32768)
        }
    }

    /// `gSmart1or2null`: `-1` encodes null.
    #[inline]
    pub fn gsmart1or2null(&mut self) -> Result<i32, Eof> {
        if self.peek()? < 128 {
            Ok(i32::from(self.g1()?) - 1)
        } else {
            Ok(i32::from(self.g2()?) - 32769)
        }
    }

    /// `gSmart2or4s`: `g4s & MAX` when the first
    /// byte is negative, else `g2` with `32767` meaning null (`-1`).
    #[inline]
    pub fn gsmart2or4s(&mut self) -> Result<i32, Eof> {
        if (self.peek()? as i8) < 0 {
            Ok(self.g4s()? & i32::MAX)
        } else {
            let value = i32::from(self.g2()?);
            Ok(if value == 32767 { -1 } else { value })
        }
    }

    /// `gVarInt2`: little-endian 7-bit groups.
    #[inline]
    pub fn gvarint2(&mut self) -> Result<i32, Eof> {
        let mut value = 0i32;
        let mut shift = 0u32;
        loop {
            let byte = i32::from(self.g1()?);
            value |= (byte & 0x7F).wrapping_shl(shift);
            shift += 7;
            if byte <= 127 {
                return Ok(value);
            }
        }
    }

    /// `g1_alt1`: `data - 128 & 0xFF`.
    #[inline]
    pub fn g1_alt1(&mut self) -> Result<u8, Eof> {
        Ok(self.g1()?.wrapping_sub(128))
    }

    /// `g1_alt2`: `-data & 0xFF`.
    #[inline]
    pub fn g1_alt2(&mut self) -> Result<u8, Eof> {
        Ok(self.g1()?.wrapping_neg())
    }

    /// `g1_alt3`: `128 - data & 0xFF`.
    #[inline]
    pub fn g1_alt3(&mut self) -> Result<u8, Eof> {
        Ok(128u8.wrapping_sub(self.g1()?))
    }

    /// `g1b_alt1`: `(byte) (data - 128)`.
    #[inline]
    pub fn g1b_alt1(&mut self) -> Result<i8, Eof> {
        Ok(self.g1_alt1()? as i8)
    }

    /// `g1b_alt2`: `(byte) -data`.
    #[inline]
    pub fn g1b_alt2(&mut self) -> Result<i8, Eof> {
        Ok(self.g1_alt2()? as i8)
    }

    /// `g1b_alt3`: `(byte) (128 - data)`.
    #[inline]
    pub fn g1b_alt3(&mut self) -> Result<i8, Eof> {
        Ok(self.g1_alt3()? as i8)
    }

    /// `g2_alt1`: little-endian unsigned short.
    #[inline]
    pub fn g2_alt1(&mut self) -> Result<u16, Eof> {
        let lo = u16::from(self.g1()?);
        let hi = u16::from(self.g1()?);
        Ok((hi << 8) | lo)
    }

    /// `g2_alt2`: high byte, then `low + 128`.
    #[inline]
    pub fn g2_alt2(&mut self) -> Result<u16, Eof> {
        let hi = u16::from(self.g1()?);
        let lo = u16::from(self.g1_alt1()?);
        Ok((hi << 8) | lo)
    }

    /// `g2_alt3`: `low + 128`, then the high byte.
    #[inline]
    pub fn g2_alt3(&mut self) -> Result<u16, Eof> {
        let lo = u16::from(self.g1_alt1()?);
        let hi = u16::from(self.g1()?);
        Ok((hi << 8) | lo)
    }

    /// `g2s_alt1`: signed [`Self::g2_alt1`].
    #[inline]
    pub fn g2s_alt1(&mut self) -> Result<i16, Eof> {
        Ok(self.g2_alt1()? as i16)
    }

    /// `g2s_alt2`: signed [`Self::g2_alt2`].
    #[inline]
    pub fn g2s_alt2(&mut self) -> Result<i16, Eof> {
        Ok(self.g2_alt2()? as i16)
    }

    /// `g4_alt1`: little-endian int.
    #[inline]
    pub fn g4_alt1(&mut self) -> Result<i32, Eof> {
        let b = [self.g1()?, self.g1()?, self.g1()?, self.g1()?];
        Ok(i32::from_le_bytes(b))
    }

    /// `g4_alt2`: wire `[v>>8, v, v>>24, v>>16]`.
    #[inline]
    pub fn g4_alt2(&mut self) -> Result<i32, Eof> {
        let b = [self.g1()?, self.g1()?, self.g1()?, self.g1()?];
        Ok(i32::from_be_bytes([b[2], b[3], b[0], b[1]]))
    }

    /// `g4_alt3`: wire `[v>>16, v>>24, v, v>>8]`.
    #[inline]
    pub fn g4_alt3(&mut self) -> Result<i32, Eof> {
        let b = [self.g1()?, self.g1()?, self.g1()?, self.g1()?];
        Ok(i32::from_be_bytes([b[1], b[0], b[3], b[2]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-by-byte failure: a short multi-byte read stops at the first
    /// missing byte; `take`/`atomic` consume nothing.
    #[test]
    fn failed_reads_stop_at_the_first_missing_byte() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.g4s(), Err(Eof { pos: 3 }));
        assert_eq!(r.pos(), 3);
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.take(4), Err(Eof { pos: 0 }));
        assert_eq!(r.atomic(Reader::g4s), Err(Eof { pos: 3 }));
        assert_eq!(r.pos(), 0);
        assert_eq!(r.take(2), Ok(&[1, 2][..]));
        let mut r = Reader::at(&[1], 5);
        assert_eq!((r.g1(), r.remaining()), (Err(Eof { pos: 5 }), 0));
    }

    /// Hand-derived values, including the sign and smart boundaries.
    #[test]
    fn reads_hand_derived_values() {
        let mut r = Reader::new(&[0xFF, 0x80, 0x00, 0x7F, 0xFF, 0xFF, 0x80, 0x00, 0x00, 0x00]);
        assert_eq!(r.g1b(), Ok(-1));
        assert_eq!(r.g2(), Ok(0x8000));
        assert_eq!(r.g1(), Ok(0x7F));
        assert_eq!(r.g2s(), Ok(-1));
        assert_eq!(r.g4s(), Ok(i32::MIN));
        let mut r = Reader::new(&[0x80, 0x00, 0x00]);
        assert_eq!(r.g3s(), Ok(-8_388_608));
        // Smarts: 1 byte below 128, else 2 (or 4 for gSmart2or4s).
        let mut r = Reader::new(&[0x7F, 0x80, 0x05, 0x00, 0x40, 0xFF, 0xFF, 0x7F, 0xFF]);
        assert_eq!(r.gsmart1or2(), Ok(127));
        assert_eq!(r.gsmart1or2(), Ok(5));
        assert_eq!(r.gsmart1or2s(), Ok(-64));
        assert_eq!(r.gsmart1or2null(), Ok(63));
        assert_eq!(r.gsmart1or2null(), Ok(32766));
        assert_eq!(r.gsmart2or4s(), Ok(-1));
        let mut r = Reader::new(&[0x80, 0, 0, 1, 0x81, 0x01]);
        assert_eq!(r.gsmart2or4s(), Ok(1));
        assert_eq!(r.gvarint2(), Ok(0x81));
        let mut r = Reader::new(&[0x7F, 0x00, 0x01, 0x80]);
        assert_eq!(r.g8(), Err(Eof { pos: 4 }));
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1]);
        assert_eq!(r.g8(), Ok(0xFFFF_FFFF_0000_0001_u64 as i64));
        let mut r = Reader::new(b"ab\0c");
        assert_eq!(r.gjstr_bytes(), Ok(&b"ab"[..]));
        assert_eq!((r.gjstr_bytes(), r.pos()), (Err(Eof { pos: 4 }), 4));
    }

    /// The `_alt` transforms against their defining expressions evaluated on
    /// signed bytes, for every byte (and pair).
    #[test]
    fn alt_reads_match_their_signed_byte_expressions() {
        for b in 0..=255_u8 {
            let s = i32::from(b as i8); // the byte read as signed
            let one = [b];
            assert_eq!(Reader::new(&one).g1_alt1(), Ok(((s - 128) & 0xFF) as u8));
            assert_eq!(Reader::new(&one).g1_alt2(), Ok((-s & 0xFF) as u8));
            assert_eq!(Reader::new(&one).g1_alt3(), Ok(((128 - s) & 0xFF) as u8));
            assert_eq!(Reader::new(&one).g1b_alt1(), Ok((s - 128) as i8));
            assert_eq!(Reader::new(&one).g1b_alt2(), Ok((-s) as i8));
            assert_eq!(Reader::new(&one).g1b_alt3(), Ok((128 - s) as i8));
            for c in [0_u8, 1, 0x7F, 0x80, 0xFF] {
                let t = i32::from(c as i8);
                let two = [b, c];
                let (u, v) = (s & 0xFF, t & 0xFF);
                assert_eq!(Reader::new(&two).g2_alt1(), Ok(((v << 8) + u) as u16));
                assert_eq!(
                    Reader::new(&two).g2_alt2(),
                    Ok(((u << 8) + ((t - 128) & 0xFF)) as u16)
                );
                assert_eq!(
                    Reader::new(&two).g2_alt3(),
                    Ok(((v << 8) + ((s - 128) & 0xFF)) as u16)
                );
            }
        }
        let four = [0x11, 0x22, 0x33, 0x44];
        assert_eq!(Reader::new(&four).g4_alt1(), Ok(0x4433_2211));
        assert_eq!(Reader::new(&four).g4_alt2(), Ok(0x3344_1122));
        assert_eq!(Reader::new(&four).g4_alt3(), Ok(0x2211_4433));
    }
}
