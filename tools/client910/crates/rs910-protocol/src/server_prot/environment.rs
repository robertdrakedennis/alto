//! Environment override payload decoding.

/// `PayloadReader` and `cp1252_byte` (split out in Phase 2.2).
pub use crate::payload_reader::*;

use super::{EnvironmentOverrideEvent, SkyboxRef};

pub fn parse_environment_override(payload: &[u8]) -> anyhow::Result<EnvironmentOverrideEvent> {
    let mut reader = PayloadReader::new(payload);
    let mask = reader.g8()?;
    if mask == 0 {
        let duration_ms = reader.g2()?;
        reader.finish("ENVIRONMENT_OVERRIDE")?;
        return Ok(EnvironmentOverrideEvent {
            sun_colour: None,
            sun_ambient: None,
            sun_diffuse: None,
            sun_shadow: None,
            sun_dir: None,
            fog_colour: None,
            fog_depth: None,
            skybox: None,
            bloom_intensity: None,
            bloom_threshold: None,
            bloom_white_point_sq: None,
            sampler: None,
            colour_remap: [None; 3],
            duration_ms,
            clear: true,
        });
    }
    let sun_colour = (mask & (1 << 0) != 0).then(|| reader.g4s()).transpose()?;
    let sun_ambient = (mask & (1 << 1) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    let sun_diffuse = (mask & (1 << 2) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    let sun_shadow = (mask & (1 << 3) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    let sun_dir = if mask & (1 << 4) != 0 {
        Some([reader.gfloat()?, reader.gfloat()?, reader.gfloat()?])
    } else {
        None
    };
    if mask & (1 << 5) != 0 {
        reader.gfloat()?;
    }
    let fog_colour = (mask & (1 << 6) != 0).then(|| reader.g4s()).transpose()?;
    let fog_depth = (mask & (1 << 7) != 0)
        .then(|| reader.g2())
        .transpose()?
        .map(i32::from);
    for bit in 8..12 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
        }
    }
    // Bloom intensity, threshold and white point squared (4096/8192/16384).
    let bloom_intensity = (mask & (1 << 12) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    let bloom_threshold = (mask & (1 << 13) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    let bloom_white_point_sq = (mask & (1 << 14) != 0)
        .then(|| reader.gfloat())
        .transpose()?;
    // Sampler material (32768).
    let sampler = (mask & (1 << 15) != 0)
        .then(|| reader.g2())
        .transpose()?
        .map(i32::from);
    let skybox = if mask & (1 << 16) != 0 {
        Some(SkyboxRef {
            kind: i32::from(reader.g2()?),
            a: i32::from(reader.g2s()?),
            b: i32::from(reader.g2s()?),
            c: i32::from(reader.g2s()?),
            yaw_offset: i32::from(reader.g2()?),
        })
    } else {
        None
    };
    // Colour remapping slots 0-2 (131072/262144/524288).
    let mut colour_remap = [None; 3];
    for (slot, bit) in (17..20).enumerate() {
        if mask & (1 << bit) != 0 {
            colour_remap[slot] = Some((i32::from(reader.g2()?), reader.gfloat()?));
        }
    }
    for bit in 20..22 {
        if mask & (1 << bit) != 0 {
            reader.g4s()?;
        }
    }
    for bit in 22..24 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
        }
    }
    for bit in 24..26 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
        }
    }
    if mask & (1 << 26) != 0 {
        reader.g4s()?;
    }
    for bit in 27..32 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
        }
    }
    for bit in 32..35 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
            reader.gfloat()?;
            reader.gfloat()?;
        }
    }
    for bit in 35..43 {
        if mask & (1 << bit) != 0 {
            reader.gfloat()?;
        }
    }
    for bit in 43..45 {
        if mask & (1 << bit) != 0 {
            reader.g4s()?;
        }
    }
    let duration_ms = reader.g2()?;
    reader.finish("ENVIRONMENT_OVERRIDE")?;
    Ok(EnvironmentOverrideEvent {
        sun_colour,
        sun_ambient,
        sun_diffuse,
        sun_shadow,
        sun_dir,
        fog_colour,
        fog_depth,
        skybox,
        bloom_intensity,
        bloom_threshold,
        bloom_white_point_sq,
        sampler,
        colour_remap,
        duration_ms,
        clear: false,
    })
}
