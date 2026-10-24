//! NXT-only data in the revision-910 cache: side-table decoders for the
//! modern renderer (`docs/renderer/modern-renderer.md`).
//!
//! The 910 client reads none of this data, or reads it and discards
//! fields (the RT7 material decoder). The 910-faithful decoders
//! ([`crate::texture`], `rs910_scene::map`/`env`) are untouched: every table
//! here is a new type built from the same bytes, for the opt-in NXT-style
//! renderer. Formats, evidence and the list of unknowns are in
//! `docs/renderer/modern-renderer.md`.
//!
//! The reference is a decompile of the NXT rev-865 Linux client
//! (`ref/nxtlab/librs2client.c`, local only). No code is copied from it;
//! comments cite the function and line a layout was read from. Every
//! layout is proven on the 910 pack by whole-corpus consumption tests
//! (`nxt::tests`, pack-gated).
//!
//! - [`material`]: RT7 material extras (archive 26, version-1 files).
//! - [`map_terrain`]: map file 5 (NXT terrain, 66x66 tiles per level).
//! - [`map_environment`]: map file 6 (per-square environment).
//! - [`map_lights`]: map file 7 (static point lights).
//! - [`map_water`]: map file 8 (water patches).
//! - [`texture_header`]: dimensions, format and mip count of archives 52-55.
//! - [`water_type`]: `WATERTYPE` (config group 76), the types file 8 names.
//! - [`model_rt7`]: RT7 models (archive 47), the GPU-ready copies of the
//!   910 models.

pub mod map_environment;
pub mod map_lights;
pub mod map_terrain;
pub mod map_water;
pub mod material;
pub mod model_rt7;
pub mod texture_header;
pub mod water_type;

#[cfg(test)]
mod tests;

use rs910_core::reader::{Eof, Reader};

/// The map archive (`Js5Archive.MAPS`, id 5; `client.mapsv2.js5`).
pub const MAP_ARCHIVE: &str = "mapsv2";
/// The NXT terrain file.
pub const TERRAIN_FILE: u32 = 5;
/// The environment file.
pub const ENVIRONMENT_FILE: u32 = 6;
/// The static point lights file.
pub const POINT_LIGHTS_FILE: u32 = 7;
/// The water file.
pub const WATER_FILE: u32 = 8;

/// Map group of mapsquare `(x, z)`: `x | z << 7`, as 865 builds it
/// (`StaticPointLightLoader::LoadMapSquarePointLightList` L1069584,
/// `WaterLoader::LoadMapSquare` L1068701).
#[must_use]
pub const fn map_group(x: u32, z: u32) -> u32 {
    x | (z << 7)
}

/// A big-endian cursor whose errors name the record and field, over
/// `rs910_core::reader` (all-or-nothing reads).
pub(crate) struct Cur<'a> {
    reader: Reader<'a>,
    what: &'a str,
}

impl<'a> Cur<'a> {
    pub(crate) fn new(data: &'a [u8], what: &'a str) -> Self {
        Self {
            reader: Reader::new(data),
            what,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.reader.pos()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.reader.remaining()
    }

    fn map<T>(&self, field: &str, out: Result<T, Eof>) -> anyhow::Result<T> {
        out.map_err(|eof| {
            anyhow::anyhow!(
                "{}: truncated {field} at offset {} (len {})",
                self.what,
                eof.pos,
                self.reader.data().len()
            )
        })
    }

    pub(crate) fn g1(&mut self, field: &str) -> anyhow::Result<u8> {
        let out = self.reader.atomic(Reader::g1);
        self.map(field, out)
    }

    pub(crate) fn g2(&mut self, field: &str) -> anyhow::Result<u16> {
        let out = self.reader.atomic(Reader::g2);
        self.map(field, out)
    }

    pub(crate) fn g2s(&mut self, field: &str) -> anyhow::Result<i16> {
        let out = self.reader.atomic(Reader::g2s);
        self.map(field, out)
    }

    pub(crate) fn g4(&mut self, field: &str) -> anyhow::Result<u32> {
        let out = self.reader.atomic(Reader::g4s);
        self.map(field, out).map(|v| v as u32)
    }

    pub(crate) fn g4s(&mut self, field: &str) -> anyhow::Result<i32> {
        let out = self.reader.atomic(Reader::g4s);
        self.map(field, out)
    }

    pub(crate) fn gfloat(&mut self, field: &str) -> anyhow::Result<f32> {
        let out = self.reader.atomic(Reader::gfloat);
        self.map(field, out)
    }

    /// `gSmart1or2() - 1` (`gSmart1or2null`): `-1` is "none".
    pub(crate) fn gsmart_null(&mut self, field: &str) -> anyhow::Result<i32> {
        let out = self.reader.atomic(Reader::gsmart1or2null);
        self.map(field, out)
    }

    /// A `0`/`1` byte read as `== 1` by 865; any other value is an error
    /// (none occurs in the 910 pack).
    pub(crate) fn gbool(&mut self, field: &str) -> anyhow::Result<bool> {
        match self.g1(field)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(anyhow::anyhow!(
                "{}: {field} is {other}, not a 0/1 flag",
                self.what
            )),
        }
    }

    pub(crate) fn take<const N: usize>(&mut self, field: &str) -> anyhow::Result<[u8; N]> {
        let out = self.reader.atomic(|r| r.take(N));
        let bytes = self.map(field, out)?;
        let mut array = [0; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    pub(crate) fn skip(&mut self, n: usize, field: &str) -> anyhow::Result<()> {
        let out = self.reader.skip(n);
        self.map(field, out)
    }

    pub(crate) fn floats<const N: usize>(&mut self, field: &str) -> anyhow::Result<[f32; N]> {
        let mut out = [0.0; N];
        for value in &mut out {
            *value = self.gfloat(field)?;
        }
        Ok(out)
    }

    /// Fail unless every byte was consumed.
    pub(crate) fn finish(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.remaining() == 0,
            "{}: {} trailing bytes after offset {}",
            self.what,
            self.remaining(),
            self.pos()
        );
        Ok(())
    }
}

/// `Some(v)` unless `v` is the `0xFFFF` "none" sentinel.
pub(crate) fn opt_u16(v: u16) -> Option<u16> {
    (v != u16::MAX).then_some(v)
}
