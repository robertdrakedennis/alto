//! Map file 6 (the environment file): one fixed 199-byte environment
//! record per mapsquare (4,414 squares in the 910 pack, all 199 bytes).
//!
//! Evidence:
//! - 865 reads the file in `EnvironmentLoader::LoadMapSquare` (L1009277)
//!   into `MapEnvironmentSettings::MapEnvironmentSettings(Packet)` (L313623,
//!   147 bytes). The 910 record is that layout with three blocks inserted
//!   (offsets 96, 128 and 170: 52 bytes, kept raw here).
//! - The fields the 910 LAND trailer also carries (the LAND
//!   environment reader) are equal in every square
//!   of the pack: sun colour, direction and the three intensities, fog
//!   colour and depth, sampler, skybox, bloom and colour remapping slot 0
//!   (`nxt::tests::map_squares_decode_and_match_the_910_land_files`).
//!
//! Field names say what is proven; `f_*`/`flag_*`/`raw_*` fields are
//! structure without a known meaning (typed as 865 reads them, or raw bytes
//! for the 910-only blocks).

use super::{opt_u16, Cur};

/// Record size in the 910 pack.
pub const ENVIRONMENT_LEN: usize = 199;

/// One decoded environment record (file order).
#[derive(Clone, Debug, PartialEq)]
pub struct MapEnvironment {
    /// @0 `u32` `0x00RRGGBB` sun colour (910 `sunColour`; 865 stores
    /// `v << 8 | 0xFF`).
    pub sun_colour: u32,
    /// @4 `i16 x3` sun direction (910 `sunDirection`, `g2s x3`; 865 negates
    /// the second component when it converts to float).
    pub sun_direction: [i16; 3],
    /// @10 `u16 x3`, each `/ 256`: sun ambient, diffuse and shadow intensity
    /// (910 order; the 910 defaults `295, 179, 307` fill squares whose
    /// trailer omits them).
    pub sun_intensity_raw: [u16; 3],
    /// @16 `f32`; 865 `+28`, default 1.0. Unknown.
    pub f_16: f32,
    /// @20 `u32` `0x00RRGGBB` fog colour (910 `fogColour`).
    pub fog_colour: u32,
    /// @24 `u16` fog depth (910 `fogDepth`).
    pub fog_depth: u16,
    /// @26 flag; 865 `+32`, default true. Unknown.
    pub flag_26: bool,
    /// @27 `f32 x4`; 865 `+44..+56`, defaults `0, 32, 8, 0`. Unknown.
    pub f_27: [f32; 4],
    /// @43 flag; 865 `+60`. Unknown.
    pub flag_43: bool,
    /// @44 `f32 x4`; 865 `+109..+121`, the head of the block
    /// `EnvScatteringSettings::SetDefaults` initialises (L313727).
    /// **Inferred**: light-scattering parameters.
    pub scattering_params: [f32; 4],
    /// @60 `f32 x3 x3`; 865 stores them over the defaults
    /// `DEF_SCATTERING_TINT`, `DEF_OUTSCATTERING_AMOUNT`,
    /// `DEF_INSCATTERING_AMOUNT` (L313721, L313707, L313719).
    /// **Inferred** from the store offsets (the decompile loses the values):
    /// `[tint, out-scattering, in-scattering]`.
    pub scattering: [[f32; 3]; 3],
    /// @96 32 bytes, 910-only (not in 865). Look like 8 `f32`; unknown.
    pub raw_96: [u8; 32],
    /// @128 8 bytes, 910-only. Look like two `u32` colours with an alpha
    /// byte of 0 or 255; unknown.
    pub raw_128: [u8; 8],
    /// @136 flag; 865 `+128`, default true. 1 in every square. Unknown.
    pub flag_136: bool,
    /// @137 `u8`; 865 `+132`, default 3. Unknown.
    pub u8_137: u8,
    /// @138 `f32 x5`; 865 `+136..+156`, defaults `0.01, 1.5, 0.55, 3, 0.5`.
    /// Unknown.
    pub f_138: [f32; 5],
    /// @158 `f32 x3`: 910 `bloomWhitePointSq, bloomIntensity,
    /// bloomThreshold` (trailer op 2). 865 `+164..+172`.
    pub bloom: [f32; 3],
    /// @170 12 bytes, 910-only. Look like 3 `f32` (mostly 0); unknown.
    pub raw_170: [u8; 12],
    /// @182 `u16` skybox type id (config group 29), `0xFFFF` none: the 910
    /// trailer op 128's `kind`. 865 `+176`.
    pub skybox: Option<u16>,
    /// @184 `u16` environment sampler material id, `0xFFFF` none: the 910
    /// environment sampler (every id is an RT5 material with the
    /// environment-cube flag `0x10`). 865 `+236`.
    pub sampler_material: Option<u16>,
    /// @186 flag; 865 `+232`. Unknown.
    pub flag_186: bool,
    /// @187 two `(u16 sprite id, f32 weight)` colour remappings, `0xFFFF`
    /// none. Slot 0 is 910 trailer op 3 (`colourRemappingMap[0]`,
    /// `colourRemappingWeight[0]`); slot 1 has no 910 source. 865 arrays at
    /// `+248`/`+288`.
    pub colour_remap: [(Option<u16>, f32); 2],
}

impl MapEnvironment {
    /// Sun ambient, diffuse and shadow intensity (`raw / 256`, as the 910
    /// environment decoder and 865 scale them).
    #[must_use]
    pub fn sun_intensity(&self) -> [f32; 3] {
        self.sun_intensity_raw.map(|v| f32::from(v) / 256.0)
    }
}

/// Decode one file-6 record; errors unless it is exactly
/// [`ENVIRONMENT_LEN`] bytes.
pub fn decode_environment(group: u32, data: &[u8]) -> anyhow::Result<MapEnvironment> {
    let what = format!("map {group} file 6 (environment)");
    let mut c = Cur::new(data, &what);
    // MapEnvironmentSettings ctor, L313794-313991, with the 910 insertions.
    let sun_colour = c.g4("sun colour")?;
    let sun_direction = [
        c.g2s("sun direction")?,
        c.g2s("sun direction")?,
        c.g2s("sun direction")?,
    ];
    let sun_intensity_raw = [
        c.g2("sun intensity")?,
        c.g2("sun intensity")?,
        c.g2("sun intensity")?,
    ];
    let f_16 = c.gfloat("f16")?;
    let fog_colour = c.g4("fog colour")?;
    let fog_depth = c.g2("fog depth")?;
    let flag_26 = c.gbool("flag 26")?;
    let f_27 = c.floats::<4>("f27")?;
    let flag_43 = c.gbool("flag 43")?;
    let scattering_params = c.floats::<4>("scattering params")?;
    let scattering = [
        c.floats::<3>("scattering")?,
        c.floats::<3>("scattering")?,
        c.floats::<3>("scattering")?,
    ];
    let raw_96 = c.take::<32>("910 block @96")?;
    let raw_128 = c.take::<8>("910 block @128")?;
    let flag_136 = c.gbool("flag 136")?;
    let u8_137 = c.g1("byte 137")?;
    let f_138 = c.floats::<5>("f138")?;
    let bloom = c.floats::<3>("bloom")?;
    let raw_170 = c.take::<12>("910 block @170")?;
    let skybox = opt_u16(c.g2("skybox")?);
    let sampler_material = opt_u16(c.g2("sampler")?);
    let flag_186 = c.gbool("flag 186")?;
    let mut colour_remap = [(None, 0.0); 2];
    for slot in &mut colour_remap {
        *slot = (
            opt_u16(c.g2("colour remap id")?),
            c.gfloat("colour remap weight")?,
        );
    }
    c.finish()?;
    Ok(MapEnvironment {
        sun_colour,
        sun_direction,
        sun_intensity_raw,
        f_16,
        fog_colour,
        fog_depth,
        flag_26,
        f_27,
        flag_43,
        scattering_params,
        scattering,
        raw_96,
        raw_128,
        flag_136,
        u8_137,
        f_138,
        bloom,
        raw_170,
        skybox,
        sampler_material,
        flag_186,
        colour_remap,
    })
}
