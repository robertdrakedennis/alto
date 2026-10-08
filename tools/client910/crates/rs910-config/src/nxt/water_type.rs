//! `WATERTYPE` (config group 76): the water
//! types map file 8's patches name (`NxtWaterPatch::water_type`). The 910
//! client declares the group and reads none of it.
//!
//! Layout: the opcode stream of `WaterType::DecodeType` (865, L161141-
//! 161270): opcodes 1-30, then `0`. 865 stores each value in a slot of the
//! type (floats already scaled: `u16 / 256`, `u8 / 32`, `u8 / 100`), and
//! `WaterPatchConfig::WaterPatchConfig` (L202400-202494) copies all of them
//! except opcode 6's colour into the per-patch config; the config offsets
//! are named per field below so later shader work can follow them.
//!
//! Proven over the 910 pack (tests below): the 27 files use exactly 865's
//! opcode set with 865's value sizes and are consumed to the last byte; the
//! material references resolve (see each field). Where 865 only stores a
//! value, the field is named by its opcode and flagged unknown. No default
//! is invented: an absent opcode is `None` (865's constructor defaults are
//! not in the decompile).

use super::Cur;

/// `Js5ConfigGroup.WATERTYPE`.
pub const WATERTYPE_GROUP: u32 = 76;

/// One water type, every opcode typed as 865 reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NxtWaterType {
    /// Op 1 `u16` (865 config `+152`): a material id; only types 21 and 22
    /// set it (material 5532 -> texture 13147, a colour texture).
    /// **Inferred** the first diffuse map (`uWaterTextureDiffuse1`, the
    /// `WaterFS` header's `WATER_DIFFUSE` samplers).
    pub diffuse_material_1: Option<u16>,
    /// Op 3 `u16` (config `+164`): as op 1, the second diffuse map
    /// (**inferred** `uWaterTextureDiffuse2`).
    pub diffuse_material_2: Option<u16>,
    /// Op 29 `u16` (config `+156`): a material whose texture is a tangent
    /// normal map in every type (texel means about `(127, 127, 252)`,
    /// proven in the tests). **Inferred** `uWaterTextureFlow1`
    /// (`WaterPatch::CheckTexturesLoaded` L1061851 loads it as texture
    /// type 1, the normal maps).
    pub normal_material_1: Option<u16>,
    /// Op 30 `u16` (config `+168`): the second normal-map material
    /// (**inferred** `uWaterTextureFlow2`).
    pub normal_material_2: Option<u16>,
    /// Op 9 `u16` (config `+180`): material 3203 in every type that sets it,
    /// whose texture (5531) is white with a varying alpha. **Inferred** the
    /// foam map (`uWaterTextureFoam`).
    pub foam_material: Option<u16>,
    /// Op 22 `u16` (config `+188`): material 3205 (texture 13145, grey).
    /// **Inferred** the mask map (`uWaterTextureMask`, the normal-and-depth
    /// pass's `maskWeight`).
    pub mask_material: Option<u16>,
    /// Op 8 `u16` (config `+92`): 865's patch constructor sizes the patch's
    /// bounding box height from it (half extent `op8 / 2`). **Inferred** a
    /// wave height (fine units).
    pub op8: Option<u16>,
    /// Op 2 `u16 / 256` (config `+160`). Unknown.
    pub op2: Option<f32>,
    /// Op 4 `u16 / 256` (config `+172`). Unknown.
    pub op4: Option<f32>,
    /// Op 5 `u16 / 256` (config `+100`). Unknown.
    pub op5: Option<f32>,
    /// Op 6 `u24` RGB (type `+120`; the only value the patch config does not
    /// copy). Unknown use (a colour).
    pub op6_rgb: Option<u32>,
    /// Op 7 two `u16` (config `+136`, `+140`). Unknown.
    pub op7: Option<[u16; 2]>,
    /// Op 10 `u16 / 256` (config `+184`). Unknown.
    pub op10: Option<f32>,
    /// Op 11 `u16` (config `+116`). Unknown.
    pub op11: Option<u16>,
    /// Op 12 `u32` (config `+88`): reads as `0xRRGGBBAA`. Unknown use.
    pub op12_rgba: Option<u32>,
    /// Op 13 `u16` (config `+120`). Unknown.
    pub op13: Option<u16>,
    /// Op 14 `u16` (config `+128`). Unknown.
    pub op14: Option<u16>,
    /// Op 15 `u32` (config `+132` and `+144`): reads as `0xRRGGBBAA`.
    /// Unknown use.
    pub op15_rgba: Option<u32>,
    /// Op 16 `u16` (config `+96`). Unknown.
    pub op16: Option<u16>,
    /// Op 17 `u16` (config `+124`; 0 or 1 in the pack). Unknown.
    pub op17: Option<u16>,
    /// Op 18 `u8 / 32` (config `+104`). Unknown.
    pub op18: Option<f32>,
    /// Op 19 `u8 / 32` (config `+108`). Unknown.
    pub op19: Option<f32>,
    /// Op 20 `u16 / 256` (config `+112`). Unknown.
    pub op20: Option<f32>,
    /// Op 21 `u16` (config `+176`). Unknown.
    pub op21: Option<u16>,
    /// Op 23 `u8` (config `+192`). Unknown; not set in the 910 pack.
    pub op23: Option<u8>,
    /// Op 24 `u8 / 100` (config `+196`). Unknown; not set in the 910 pack.
    pub op24: Option<f32>,
    /// Op 25 `u16 / 256` (config `+200`). Unknown.
    pub op25: Option<f32>,
    /// Op 26 three `u16 / 256`, in read order (865 stores them reversed at
    /// config `+212`, `+208`, `+204`). Unknown.
    pub op26: Option<[f32; 3]>,
    /// Op 27 `u16` (config `+216`). Unknown.
    pub op27: Option<u16>,
    /// Op 28 `i16` (config `+148`). Unknown.
    pub op28: Option<i16>,
}

impl NxtWaterType {
    /// The two normal-map materials (ops 29 and 30) that are set.
    #[must_use]
    pub fn normal_materials(&self) -> [Option<u16>; 2] {
        [self.normal_material_1, self.normal_material_2]
    }
}

/// Decode one `WATERTYPE` file (865's opcode stream; an unknown opcode or a
/// byte after the terminating `0` is an error).
pub fn decode_water_type(id: u32, data: &[u8]) -> anyhow::Result<NxtWaterType> {
    let what = format!("water type {id}");
    let mut c = Cur::new(data, &what);
    let mut t = NxtWaterType::default();
    let q = |v: u16| f32::from(v) / 256.0;
    loop {
        let op = c.g1("opcode")?;
        match op {
            0 => break,
            1 => t.diffuse_material_1 = Some(c.g2("op 1")?),
            2 => t.op2 = Some(q(c.g2("op 2")?)),
            3 => t.diffuse_material_2 = Some(c.g2("op 3")?),
            4 => t.op4 = Some(q(c.g2("op 4")?)),
            5 => t.op5 = Some(q(c.g2("op 5")?)),
            6 => {
                let [r, g, b] = c.take::<3>("op 6")?;
                t.op6_rgb = Some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b));
            }
            7 => t.op7 = Some([c.g2("op 7")?, c.g2("op 7")?]),
            8 => t.op8 = Some(c.g2("op 8")?),
            9 => t.foam_material = Some(c.g2("op 9")?),
            10 => t.op10 = Some(q(c.g2("op 10")?)),
            11 => t.op11 = Some(c.g2("op 11")?),
            12 => t.op12_rgba = Some(c.g4("op 12")?),
            13 => t.op13 = Some(c.g2("op 13")?),
            14 => t.op14 = Some(c.g2("op 14")?),
            15 => t.op15_rgba = Some(c.g4("op 15")?),
            16 => t.op16 = Some(c.g2("op 16")?),
            17 => t.op17 = Some(c.g2("op 17")?),
            18 => t.op18 = Some(f32::from(c.g1("op 18")?) / 32.0),
            19 => t.op19 = Some(f32::from(c.g1("op 19")?) / 32.0),
            20 => t.op20 = Some(q(c.g2("op 20")?)),
            21 => t.op21 = Some(c.g2("op 21")?),
            22 => t.mask_material = Some(c.g2("op 22")?),
            23 => t.op23 = Some(c.g1("op 23")?),
            24 => t.op24 = Some(f32::from(c.g1("op 24")?) / 100.0),
            25 => t.op25 = Some(q(c.g2("op 25")?)),
            26 => t.op26 = Some([q(c.g2("op 26")?), q(c.g2("op 26")?), q(c.g2("op 26")?)]),
            27 => t.op27 = Some(c.g2("op 27")?),
            28 => t.op28 = Some(c.g2s("op 28")?),
            29 => t.normal_material_1 = Some(c.g2("op 29")?),
            30 => t.normal_material_2 = Some(c.g2("op 30")?),
            other => anyhow::bail!("{what}: unknown opcode {other} at offset {}", c.pos() - 1),
        }
    }
    c.finish()?;
    Ok(t)
}

/// Every water type of the pack, by id.
#[derive(Clone, Debug, Default)]
pub struct NxtWaterTypes {
    types: std::collections::BTreeMap<u32, NxtWaterType>,
}

impl NxtWaterTypes {
    /// Decode every file of config group 76.
    pub fn load(pack: &crate::cache::Pack) -> anyhow::Result<Self> {
        let files = pack.read_group(crate::flo::FLO_ARCHIVE, WATERTYPE_GROUP)?;
        let mut types = std::collections::BTreeMap::new();
        for (id, bytes) in files {
            types.insert(id, decode_water_type(id, &bytes)?);
        }
        Ok(Self { types })
    }

    #[must_use]
    pub fn get(&self, id: u32) -> Option<&NxtWaterType> {
        self.types.get(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.types.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u32, &NxtWaterType)> {
        self.types.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::require_pack;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// Water type 0 (the Lumbridge river's), all 88 bytes, by opcode.
    const TYPE_0: &str = "0c b7dbdaff 1d 15a0 02 0600 1e 15a1 04 0600 15 0096 16 0c85 \
                          09 0c83 0a 0080 05 00b3 12 60 13 30 14 0019 06 b7dbda \
                          07 1388 1b58 08 0000 10 0400 0b 0200 0d 0028 1c 001e \
                          0e 0096 11 0000 0f 261e0900 19 0100 1a 0033 0080 00c0 \
                          1b 03e8 00";

    #[test]
    fn water_type_0_decodes_by_hand() {
        let t = decode_water_type(0, &hex(TYPE_0)).unwrap();
        assert_eq!(t.op12_rgba, Some(0xb7db_daff));
        assert_eq!(t.normal_materials(), [Some(5536), Some(5537)]);
        assert_eq!((t.op2, t.op4), (Some(6.0), Some(6.0)));
        assert_eq!(
            (t.op21, t.mask_material, t.foam_material),
            (Some(150), Some(3205), Some(3203))
        );
        assert_eq!((t.op10, t.op5), (Some(0.5), Some(179.0 / 256.0)));
        assert_eq!(
            (t.op18, t.op19, t.op20),
            (Some(3.0), Some(1.5), Some(25.0 / 256.0))
        );
        assert_eq!(t.op6_rgb, Some(0xb7dbda));
        assert_eq!(t.op7, Some([5000, 7000]));
        assert_eq!(
            (t.op8, t.op16, t.op11, t.op13),
            (Some(0), Some(1024), Some(512), Some(40))
        );
        assert_eq!((t.op28, t.op14, t.op17), (Some(30), Some(150), Some(0)));
        assert_eq!(t.op15_rgba, Some(0x261e_0900));
        assert_eq!(t.op25, Some(1.0));
        assert_eq!(t.op26, Some([51.0 / 256.0, 0.5, 0.75]));
        assert_eq!(t.op27, Some(1000));
        assert_eq!(
            (t.diffuse_material_1, t.diffuse_material_2, t.op23, t.op24),
            (None, None, None, None)
        );
        // Truncated, trailing and unknown-opcode input fails.
        let bytes = hex(TYPE_0);
        assert!(decode_water_type(0, &bytes[..bytes.len() - 1]).is_err());
        let mut long = bytes.clone();
        long.push(0);
        assert!(decode_water_type(0, &long).is_err());
        assert!(decode_water_type(0, &[31, 0]).is_err());
    }

    /// Every file of group 76 decodes to its last byte (strict: an unknown
    /// opcode or trailing byte fails the load); every material it names
    /// resolves to a loadable texture; the normal-map materials (ops 29/30)
    /// look like tangent-space normal maps and the foam material (op 9) like
    /// white foam with alpha.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore)]
    fn every_water_type_decodes_and_its_maps_resolve() {
        let pack = require_pack("client.config.js5");
        let types = NxtWaterTypes::load(&pack).unwrap();
        assert!(!types.is_empty());
        let materials = crate::texture::MaterialStore::load(&pack).unwrap();
        let mean = |material: u16| {
            let m = materials.get(u32::from(material)).expect("material");
            let texture = m.diffuse_texture.expect("texture");
            let crate::texture::Texture::Single(img) =
                crate::texture::load_texture(&pack, texture).unwrap()
            else {
                panic!("material {material}: cube");
            };
            let n = img.px.len() / 4;
            let mut s = [0.0_f64; 4];
            for p in img.px.chunks(4) {
                for c in 0..4 {
                    s[c] += f64::from(p[c]);
                }
            }
            s.map(|v| v / n as f64)
        };
        for (id, t) in types.iter() {
            let named = [
                t.diffuse_material_1,
                t.diffuse_material_2,
                t.normal_material_1,
                t.normal_material_2,
                t.foam_material,
                t.mask_material,
            ];
            for m in named.into_iter().flatten() {
                mean(m);
            }
            for m in t.normal_materials().into_iter().flatten() {
                let [r, g, b, _] = mean(m);
                // A tangent-space normal map: X and Y about 0.5, Z near 1.
                assert!(
                    (r - 127.5).abs() < 40.0 && (g - 127.5).abs() < 5.0 && b > 240.0,
                    "type {id}: material {m} mean {r} {g} {b}"
                );
            }
            if let Some(m) = t.foam_material {
                let [r, g, b, a] = mean(m);
                assert!(r > 250.0 && g > 250.0 && b > 250.0 && a < 200.0, "foam {m}");
            }
        }
    }
}
