//! RT7 material extras: every field of a version-1 material file, including
//! the ones the RT7 material decoder in [`crate::texture`] reads and drops.
//!
//! Layout: the 910 RT7 order, unchanged (the 910 decoder consumes every
//! byte; this one keeps the values). NXT 865 cannot be used for it: its
//! material decoder (`RT7Core::MainLogic` L1033007-1033153) is the RT5
//! layout without the version byte, from before the 910 RT5/RT7 split.
//!
//! Proven over all 624 RT7 files of the 910 pack (`nxt::tests`): each
//! record consumes exactly its bytes; the texture ids of flags `0x20`,
//! `0x40` and `0x80` exist in all four texture archives (52-55); the byte
//! before each id is a size code `k` with DXT/ETC width `== (64 << k) + 64`
//! in 1,506 of 1,506 references. The roles of the `0x40`/`0x80` maps and of
//! the scalars are inferred or unknown (field docs; `nxt-data-formats.md`).

use std::collections::BTreeMap;

use super::Cur;
use crate::cache::Pack;
use crate::texture::MATERIALS_ARCHIVE;

/// A texture reference of an RT7 material: a size code byte, then the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rt7TextureRef {
    /// Size code `k`: the texture's atlas size is `64 << k` (`k` in 0..=4 in
    /// the pack). Proven: the DXT/ETC copies (archives 52/55) are
    /// `(64 << k) + 64` wide, a 32-pixel gutter on each side, for every
    /// reference. The PNG copy (archive 53) can be smaller (93 of 1,506).
    pub size_code: u8,
    /// Texture id (a group of archives 52-55). 910 `g4s`.
    pub texture: i32,
}

impl Rt7TextureRef {
    /// `64 << size_code`.
    #[must_use]
    pub fn size(self) -> u32 {
        64 << self.size_code
    }
}

/// Every field of one version-1 (RT7) material file, in file order.
///
/// Flag bits without a payload (`0x1`, `0x2`, `0x4`, `0x8`, `0x10`,
/// `0x400`) are kept in [`Rt7MaterialExtra::flags`] only; exact consumption
/// over the corpus proves they carry no bytes. Meanings of the scalar
/// fields are unknown unless a doc says otherwise.
#[derive(Clone, Debug, PartialEq)]
pub struct Rt7MaterialExtra {
    /// Material id (file id in group 0 of archive 26).
    pub id: u32,
    /// The `g4s` flag word.
    pub flags: u32,
    /// Flag `0x20`: the diffuse map (`diffuseAlphaMapID`, the one texture
    /// 910 keeps). Present in all 624.
    pub diffuse: Option<Rt7TextureRef>,
    /// Flag `0x40` (533 materials): **inferred** normal map. Its PNGs average
    /// about `(0, 127, 245, 128)` RGBA, a tangent-space normal with X in
    /// alpha and Y in green (DXT5nm-style), sampled over 12 materials.
    pub normal: Option<Rt7TextureRef>,
    /// Flag `0x80` (349 materials): **inferred** compound map (specular /
    /// roughness style channels; blue and alpha are 255 in the sample, red
    /// and green vary). Meaning of each channel unknown.
    pub compound: Option<Rt7TextureRef>,
    /// Flag `0x1000` float. Unknown; never set in the 910 pack.
    pub f_1000: Option<f32>,
    /// Flag `0x2000` float (55 materials: 0.75 x54, 1.5 x1). Unknown.
    pub f_2000: Option<f32>,
    /// Flag `0x4000` float (617 materials, always 0.0). Unknown.
    pub f_4000: Option<f32>,
    /// Flag `0x8000` int (1 material: `-84215041`, i.e. `0xFAFAFAFF`
    /// bytes). Unknown.
    pub i_8000: Option<i32>,
    /// The float read when flag `0x40` (the normal map) is set, after the
    /// `0x8000` int (533: 32.0 x498, 10.0 x14, ...). **Inferred**: a
    /// normal-map parameter (strength or scale); unproven.
    pub normal_param: Option<f32>,
    /// Flag `0x800` floats (910 calls the flag `specular`; 2 materials:
    /// `(0.01, 1.52, 0.01)`). Component meanings unknown.
    pub specular: Option<[f32; 3]>,
    /// Flag `0x10000` float (516: 10.0 x481, 8.0 x21, ...). Unknown.
    pub f_10000: Option<f32>,
    /// Flag `0x20000` float (162: 0.01 x155, ...). Unknown.
    pub f_20000: Option<f32>,
    /// Flag `0x100`: raw `g2s` of 910 `speedU` (`* 127 / 32767 / 64`).
    pub speed_u_raw: Option<i16>,
    /// Flag `0x200`: raw `g2s` of 910 `speedV`.
    pub speed_v_raw: Option<i16>,
    /// `repeatS | repeatT << 3` (910 masks each with 7). 9 in all 624.
    pub repeat: u8,
    /// Facet mode id (910 decodes and discards it). 1 in all 624.
    pub facet: u8,
    /// Material quality mode id (0 none, 1 HD, 2 LD). 0 in all 624.
    pub quality: u8,
    /// `AlphaMode` id (0 none, 1 alpha-tested, 2 multiply).
    pub alpha: u8,
    /// The alpha-test threshold, present when `alpha == 1` (127 in all 26).
    pub alpha_threshold: Option<u8>,
    /// Average colour (`g2`).
    pub average_colour: u16,
    /// Size code (`64 << code`, `-1` past 4).
    pub size_code: u8,
}

/// Decode one material file when it is RT7 (first byte `1`); `Ok(None)` for
/// RT5 (first byte `0`), which carries no NXT-only fields here.
pub fn decode_rt7_extra(id: u32, data: &[u8]) -> anyhow::Result<Option<Rt7MaterialExtra>> {
    let what = format!("rt7 material {id}");
    let mut c = Cur::new(data, &what);
    match c.g1("version")? {
        0 => return Ok(None),
        1 => {}
        other => anyhow::bail!("{what}: unknown version {other} (only 0 RT5 / 1 RT7)"),
    }
    // Same field order as the RT7 layout above.
    let flags = c.g4("flags")?;
    let tex = |c: &mut Cur<'_>, bit: u32, field: &str| -> anyhow::Result<Option<Rt7TextureRef>> {
        if flags & bit == 0 {
            return Ok(None);
        }
        Ok(Some(Rt7TextureRef {
            size_code: c.g1(field)?,
            texture: c.g4s(field)?,
        }))
    };
    let float = |c: &mut Cur<'_>, bit: u32, field: &str| -> anyhow::Result<Option<f32>> {
        if flags & bit == 0 {
            Ok(None)
        } else {
            c.gfloat(field).map(Some)
        }
    };
    let diffuse = tex(&mut c, 0x20, "diffuse")?;
    let normal = tex(&mut c, 0x40, "normal map")?;
    let compound = tex(&mut c, 0x80, "compound map")?;
    let f_1000 = float(&mut c, 0x1000, "0x1000 float")?;
    let f_2000 = float(&mut c, 0x2000, "0x2000 float")?;
    let f_4000 = float(&mut c, 0x4000, "0x4000 float")?;
    let i_8000 = if flags & 0x8000 == 0 {
        None
    } else {
        Some(c.g4s("0x8000 int")?)
    };
    let normal_param = float(&mut c, 0x40, "normal-map float")?;
    let specular = if flags & 0x800 == 0 {
        None
    } else {
        Some(c.floats::<3>("specular floats")?)
    };
    let f_10000 = float(&mut c, 0x10000, "0x10000 float")?;
    let f_20000 = float(&mut c, 0x20000, "0x20000 float")?;
    let speed_u_raw = if flags & 0x100 == 0 {
        None
    } else {
        Some(c.g2s("speed u")?)
    };
    let speed_v_raw = if flags & 0x200 == 0 {
        None
    } else {
        Some(c.g2s("speed v")?)
    };
    let repeat = c.g1("repeat")?;
    let facet = c.g1("facet")?;
    let quality = c.g1("quality")?;
    let alpha = c.g1("alpha mode")?;
    let alpha_threshold = if alpha == 1 {
        Some(c.g1("alpha threshold")?)
    } else {
        None
    };
    let average_colour = c.g2("average colour")?;
    let size_code = c.g1("size code")?;
    c.finish()?;
    Ok(Some(Rt7MaterialExtra {
        id,
        flags,
        diffuse,
        normal,
        compound,
        f_1000,
        f_2000,
        f_4000,
        i_8000,
        normal_param,
        specular,
        f_10000,
        f_20000,
        speed_u_raw,
        speed_v_raw,
        repeat,
        facet,
        quality,
        alpha,
        alpha_threshold,
        average_colour,
        size_code,
    }))
}

/// The RT7 side table: material id -> extras, for version-1 files only.
#[derive(Clone, Debug, Default)]
pub struct Rt7MaterialExtraStore {
    entries: BTreeMap<u32, Rt7MaterialExtra>,
}

impl Rt7MaterialExtraStore {
    /// Decode every RT7 file of group 0 of the materials archive (strict:
    /// any undecodable RT7 file is an error).
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        let files = pack
            .read_group(MATERIALS_ARCHIVE, 0)
            .map_err(|error| anyhow::anyhow!("materials: group 0: {error}"))?;
        let mut entries = BTreeMap::new();
        for (id, bytes) in &files {
            if let Some(extra) = decode_rt7_extra(*id, bytes)? {
                entries.insert(*id, extra);
            }
        }
        Ok(Self { entries })
    }

    /// Number of RT7 materials.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Extras of material `id` (`None` for RT5 or absent ids).
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Rt7MaterialExtra> {
        self.entries.get(&id)
    }

    /// Iterate in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Rt7MaterialExtra> {
        self.entries.values()
    }
}
