//! Map file 8 (the water file): the water patches of one mapsquare.
//!
//! Layout (`WaterLoader::LoadMapSquare`, L1068507; record read at
//! L1069165-1069229): `u8 count`, then `count` fixed 28-byte records. An
//! empty square is the single byte `0`.
//!
//! Proven over the 910 pack (`nxt::tests`): every file is consumed exactly
//! (7,299 patches in 4,414 squares; file sizes are all `1 + 28n`). Field
//! meanings follow what 865 does with each value; where 865 only stores a
//! value, the doc says so.

use super::Cur;

/// Record size.
pub const WATER_PATCH_LEN: usize = 28;

/// One water patch (file order).
#[derive(Clone, Debug, PartialEq)]
pub struct NxtWaterPatch {
    /// @0 tile X; 865 places the patch at `x * 512 + 256` fine units (the
    /// tile centre). 9 of 7,299 are 64 or more; unknown whether they are
    /// meant as signed.
    pub tile_x: u8,
    /// @1 tile Z, as [`NxtWaterPatch::tile_x`].
    pub tile_z: u8,
    /// @2 extent in tiles along X; 865 stores `v << 9` (fine units).
    pub size_x: u8,
    /// @3 extent in tiles along Z; 865 stores `v << 9`.
    pub size_z: u8,
    /// @4 `u16` height; 865 stores it as the patch's Y. Units **inferred**
    /// (fine units, like the light heights).
    pub height: u16,
    /// @6 `f32 x3` rotation axis and @18 `f32` angle in turns: 865 builds
    /// the quaternion `(axis * sin(t * pi), cos(t * pi))` and normalises
    /// it (L1069207-1069222). Mostly `(0, 1, 0)` with small angles.
    pub rotation_axis: [f32; 3],
    /// @18 see [`NxtWaterPatch::rotation_axis`].
    pub rotation_turns: f32,
    /// @22 `u16`; 865 stores it at `+72` of `WaterPatchConfig` and nothing
    /// in the loader reads it. Values: 10000 (5,538), `0xFFFF` (1,756),
    /// five others. Unknown.
    pub u16_22: u16,
    /// @24 two bytes, 865 divides each by 50 and builds a rotation from the
    /// direction `(a, 0, b)` (L1069226-1069328). **Inferred** a flow
    /// direction; read as signed (`(0, -50)` gives the unit vector
    /// `(0, -1)`); 865's byte signedness is not visible in the decompile.
    pub flow: [i8; 2],
    /// @26 `u16` water type id (config group 76, `WATERTYPE`, 27 entries):
    /// 865 collects it into a separate `u16` list for type loading. Values
    /// 0..=26 in the pack (**inferred** from the range match).
    pub water_type: u16,
}

impl NxtWaterPatch {
    /// The flow direction as 865 scales it (`/ 50`).
    #[must_use]
    pub fn flow_scaled(&self) -> [f32; 2] {
        self.flow.map(|v| f32::from(v) / 50.0)
    }
}

/// Decode one file-8 payload.
pub fn decode_water(group: u32, data: &[u8]) -> anyhow::Result<Vec<NxtWaterPatch>> {
    let what = format!("map {group} file 8 (water)");
    let mut c = Cur::new(data, &what);
    let count = c.g1("patch count")?;
    let mut patches = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let [tile_x, tile_z, size_x, size_z] = c.take::<4>("patch box")?;
        let height = c.g2("height")?;
        let rotation_axis = c.floats::<3>("rotation axis")?;
        let rotation_turns = c.gfloat("rotation angle")?;
        let u16_22 = c.g2("u16 @22")?;
        let [a, b] = c.take::<2>("flow")?;
        let water_type = c.g2("water type")?;
        patches.push(NxtWaterPatch {
            tile_x,
            tile_z,
            size_x,
            size_z,
            height,
            rotation_axis,
            rotation_turns,
            u16_22,
            flow: [a as i8, b as i8],
            water_type,
        });
    }
    c.finish()?;
    Ok(patches)
}
