//! `PayloadReader`: the packet read cursor over one server payload,
//! with `anyhow` errors naming the read (`"g2: need 2 bytes at 3 (have 1)"`).
//! The reads delegate to `rs910_core::reader` (Phase 2.1). Split from
//! `server_prot` (Phase 2.2) so `reflection_check`, which `server_prot`
//! calls, can read payloads without a module cycle; `server_prot`
//! re-exports it.

use rs910_core::reader::{Eof, Reader as CoreReader};

/// Windows-1252 byte decode per byte (unassigned bytes
/// decode to `?`): rs910-core's copy since Phase 2.1 (this module's was
/// identical on all 256 bytes).
pub use rs910_core::cp1252::cp1252_decode_byte as cp1252_byte;

/// Cursor over a fixed-size server payload with `_alt*` reads.
pub struct PayloadReader<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> PayloadReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// One `rs910_core::reader` read of `count` bytes at `pos`, all or
    /// nothing (Phase 2.1: the packet arithmetic, including the
    /// `_alt` transforms, lives there).
    fn read<T>(
        &mut self,
        count: usize,
        what: &'static str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        match r.atomic(read) {
            Ok(value) => {
                self.pos = r.pos();
                Ok(value)
            }
            Err(_) if self.pos.checked_add(count).is_none() => {
                anyhow::bail!("{what}: offset overflow at {}", self.pos)
            }
            Err(_) => anyhow::bail!(
                "{what}: need {count} bytes at {} (have {})",
                self.pos,
                self.remaining()
            ),
        }
    }

    pub fn g1(&mut self) -> anyhow::Result<u8> {
        self.read(1, "g1", CoreReader::g1)
    }

    pub fn g1s(&mut self) -> anyhow::Result<i8> {
        self.read(1, "g1", CoreReader::g1b)
    }

    pub fn g1_alt1(&mut self) -> anyhow::Result<u8> {
        self.read(1, "g1", CoreReader::g1_alt1)
    }

    pub fn g1_alt2(&mut self) -> anyhow::Result<u8> {
        self.read(1, "g1", CoreReader::g1_alt2)
    }

    pub fn g1_alt3(&mut self) -> anyhow::Result<u8> {
        self.read(1, "g1", CoreReader::g1_alt3)
    }

    pub fn g2(&mut self) -> anyhow::Result<u16> {
        self.read(2, "g2", CoreReader::g2)
    }

    pub fn g2s(&mut self) -> anyhow::Result<i16> {
        self.read(2, "g2s", CoreReader::g2s)
    }

    pub fn g2_alt1(&mut self) -> anyhow::Result<u16> {
        self.read(2, "g2_alt1", CoreReader::g2_alt1)
    }

    pub fn g2_alt2_u16(&mut self) -> anyhow::Result<u16> {
        self.read(2, "g2_alt2", CoreReader::g2_alt2)
    }

    pub fn g2_alt3(&mut self) -> anyhow::Result<u16> {
        self.read(2, "g2_alt3", CoreReader::g2_alt3)
    }

    pub fn g2s_alt1(&mut self) -> anyhow::Result<i16> {
        self.read(2, "g2s_alt1", CoreReader::g2s_alt1)
    }

    pub fn g2s_alt2(&mut self) -> anyhow::Result<i16> {
        self.read(2, "g2s_alt2", CoreReader::g2s_alt2)
    }

    pub fn g3s(&mut self) -> anyhow::Result<i32> {
        self.read(3, "g3s", CoreReader::g3s)
    }

    /// `gSmart1or2`; an empty payload fails as a `g1` (the peek reads 0).
    pub fn gsmart1or2(&mut self) -> anyhow::Result<i32> {
        if self.data.get(self.pos).copied().unwrap_or(0) < 128 {
            self.read(1, "g1", CoreReader::gsmart1or2)
        } else {
            self.read(2, "g2", CoreReader::gsmart1or2)
        }
    }

    /// `gSmart1or2s`; an empty payload fails as a `g1` (the peek reads 0).
    pub fn gsmart1or2s(&mut self) -> anyhow::Result<i32> {
        if self.data.get(self.pos).copied().unwrap_or(0) < 128 {
            self.read(1, "g1", CoreReader::gsmart1or2s)
        } else {
            self.read(2, "g2", CoreReader::gsmart1or2s)
        }
    }

    pub fn gsmart2or4s(&mut self) -> anyhow::Result<i32> {
        let first = *self
            .data
            .get(self.pos)
            .ok_or_else(|| anyhow::anyhow!("gsmart2or4s: truncated"))?;
        if first & 0x80 != 0 {
            self.read(4, "g4s", CoreReader::gsmart2or4s)
        } else {
            self.read(2, "g2", CoreReader::gsmart2or4s)
        }
    }

    pub fn g4s(&mut self) -> anyhow::Result<i32> {
        self.read(4, "g4s", CoreReader::g4s)
    }

    pub fn gfloat(&mut self) -> anyhow::Result<f32> {
        self.read(4, "g4s", CoreReader::gfloat)
    }

    pub fn g8(&mut self) -> anyhow::Result<u64> {
        self.read(8, "g8", CoreReader::g8).map(|v| v as u64)
    }

    /// `fastgstr`. Fidelity note (not changed by Phase 2.1): the reference
    /// client reads null when the first
    /// byte is `0`; this port tests `255`. Callers use `unwrap_or_default()`,
    /// so only a string starting with byte `0xFF` decodes differently.
    pub fn fastgstr(&mut self) -> anyhow::Result<Option<String>> {
        if self.data.get(self.pos).copied() == Some(255) {
            self.pos += 1;
            Ok(None)
        } else {
            Ok(Some(self.gjstr()?))
        }
    }

    pub fn g4_alt1(&mut self) -> anyhow::Result<i32> {
        self.read(4, "g4_alt1", CoreReader::g4_alt1)
    }

    pub fn g4_alt2(&mut self) -> anyhow::Result<i32> {
        self.read(4, "g4_alt2", CoreReader::g4_alt2)
    }

    pub fn g4_alt3(&mut self) -> anyhow::Result<i32> {
        self.read(4, "g4_alt3", CoreReader::g4_alt3)
    }

    pub fn gjstr(&mut self) -> anyhow::Result<String> {
        let start = self.pos;
        let mut r = CoreReader::at(self.data, self.pos);
        let bytes = r.gjstr_bytes();
        self.pos = r.pos();
        let bytes = bytes.map_err(|_| anyhow::anyhow!("gjstr: unterminated string at {start}"))?;
        Ok(bytes.iter().map(|byte| cp1252_byte(*byte)).collect())
    }

    pub fn finish(&self, what: &'static str) -> anyhow::Result<()> {
        if self.pos != self.data.len() {
            anyhow::bail!(
                "{what}: trailing {} bytes after payload",
                self.data.len().saturating_sub(self.pos)
            );
        }
        Ok(())
    }
}
