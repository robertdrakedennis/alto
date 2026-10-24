//! Map file 7 (the static point light file): the static point lights of
//! one mapsquare.
//!
//! Layout (`StaticPointLightLoader::LoadMapSquarePointLightList`, L1069351,
//! reads from L1069612): `u8 count`, then `count` records in the byte
//! layout of the LAND-trailer light (`rs910_scene::env::
//! StaticLight::decode`). An empty square is the single byte `0`.
//!
//! Proven over the 910 pack (`nxt::tests`): every file is consumed exactly
//! (75,836 lights in 4,414 squares), and each square's lights equal the LAND
//! trailer's (same order, every field), except that the three coordinates
//! are pre-shifted: `nxt == (land_raw << 2) & 0xFFFF`, i.e. already in fine
//! units (1/512 tile) local to the square, where the LAND path shifts at
//! load (`shift = 2`).

use super::Cur;

/// One static point light (file order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NxtPointLight {
    /// Flag byte: bits 0-2 level (`& 7`), bit `0x8` also lights the levels
    /// above, bit `0x10` the levels below (865 `+17`/`+16`).
    pub flags: u8,
    /// Fine X (1/512 tile) in the square. 99.8% are below `0x8000`; the rest
    /// match LAND raws past the square edge (the LAND path drops them), so treating
    /// them as signed is **inferred**.
    pub x: u16,
    /// Fine Z, as [`NxtPointLight::x`].
    pub z: u16,
    /// Height above the terrain in fine units (the LAND path uses `height - y`).
    pub y: u16,
    /// `spanRadius`; the light covers `2r + 1` tile rows.
    pub span_radius: u8,
    /// `2r + 1` raw `(start << 8) | length` runs, unclamped (the LAND path and 865
    /// clamp them at load).
    pub span_runs: Vec<u16>,
    /// HSL colour index (865 `m_HSLToRGB`).
    pub colour_hsl: u16,
    /// Low 5 bits: flicker kind (`31` = from a `LightType`); high 3 bits:
    /// phase (`(b & 0xE0) << 3`).
    pub flicker_phase: u8,
    /// Light group (`g2s`, `-1` none; 865 `+68`).
    pub group: i16,
    /// `LightType` id (config group 31), present when the flicker kind is 31.
    pub light_type: Option<u16>,
}

impl NxtPointLight {
    /// Level `0..=7` (`flags & 7`).
    #[must_use]
    pub fn level(&self) -> u8 {
        self.flags & 7
    }

    /// Flicker kind (`flicker_phase & 0x1F`).
    #[must_use]
    pub fn flicker(&self) -> u8 {
        self.flicker_phase & 0x1F
    }
}

/// Decode one file-7 payload.
pub fn decode_point_lights(group: u32, data: &[u8]) -> anyhow::Result<Vec<NxtPointLight>> {
    let what = format!("map {group} file 7 (point lights)");
    let mut c = Cur::new(data, &what);
    let count = c.g1("light count")?;
    let mut lights = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        // L1069738-1070121, the 910 static point light order.
        let flags = c.g1("flags")?;
        let x = c.g2("x")?;
        let z = c.g2("z")?;
        let y = c.g2("y")?;
        let span_radius = c.g1("span radius")?;
        let runs = usize::from(span_radius) * 2 + 1;
        let mut span_runs = Vec::with_capacity(runs);
        for _ in 0..runs {
            span_runs.push(c.g2("span run")?);
        }
        let colour_hsl = c.g2("colour")?;
        let flicker_phase = c.g1("flicker")?;
        let group_id = c.g2s("group")?;
        let light_type = if flicker_phase & 0x1F == 31 {
            Some(c.g2("light type")?)
        } else {
            None
        };
        lights.push(NxtPointLight {
            flags,
            x,
            z,
            y,
            span_radius,
            span_runs,
            colour_hsl,
            flicker_phase,
            group: group_id,
            light_type,
        });
    }
    c.finish()?;
    Ok(lights)
}
