//! The skybox config types: sky boxes (a material, decor list, fill mode and
//! model) and sky decor (sun, moon and cloud sprites, spheres and models),
//! their fill mode, and the `SKYBOXTYPE`/`SKYDECORTYPE` config groups. The
//! decoders' opcodes are revision facts and stay in the config seam; the
//! `SkyBox` state that uses them is scene state (`rs910_scene::skybox`, which
//! re-exports this module). An opcode outside a table consumes nothing and the
//! next byte is read as an opcode, as the stock client does.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::opcode_table::{at, decode_record, Entry, Input, Record, Rule, Slot, Table, Unknown};

/// Config group of sky box types.
pub const SKYBOXTYPE_GROUP: u32 = 29;

/// Config group of sky decor types.
pub const SKYDECORTYPE_GROUP: u32 = 30;

/// How a sky box material is painted: id 0 tiles it in both directions; id 1
/// draws one horizon row and fills above and below with the texture's first
/// and last pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyBoxFillMode {
    /// Id 0.
    Tiled,
    /// Id 1.
    Horizon,
}

impl SkyBoxFillMode {
    /// The mode for a stored id. An unknown id is `None`, which callers treat
    /// as "not horizon".
    #[must_use]
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::Tiled),
            1 => Some(Self::Horizon),
            _ => None,
        }
    }
}

/// One sky box type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkyBoxType {
    /// The material painted by the 2D path.
    pub material: i32,
    /// Sky decor type ids.
    pub decors: Option<Vec<i32>>,
    /// Index of the decor that lights the others.
    pub sun_decor: i32,
    /// Fill mode (default [`SkyBoxFillMode::Tiled`]).
    pub fill: Option<SkyBoxFillMode>,
    /// The 3D sky model id.
    pub model: i32,
}

impl Default for SkyBoxType {
    fn default() -> Self {
        Self {
            material: 0,
            decors: None,
            sun_decor: 0,
            fill: Some(SkyBoxFillMode::Tiled),
            model: 0,
        }
    }
}

/// Sky box opcodes of this revision.
static SKYBOX_OPCODES: Table<SkyBoxType, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Short(|t, _, v| t.material = i32::from(v))),
        Entry::new(at(2), Rule::Custom(read_decors)),
        Entry::new(at(3), Rule::Byte(|t, _, v| t.sun_decor = i32::from(v))),
        Entry::new(
            at(4),
            Rule::Byte(|t, _, v| t.fill = SkyBoxFillMode::from_id(i32::from(v))),
        ),
        Entry::new(at(5), Rule::SmartId(|t, _, v| t.model = v)),
        Entry::new(at(6), Rule::Skip(&[crate::opcode_table::Field::SmartId])),
    ],
    Unknown::Skip,
);

fn read_decors(source: Input<anyhow::Error>, t: &mut SkyBoxType, _: Slot) -> Result<()> {
    let count = source.byte()?;
    let mut ids = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        ids.push(i32::from(source.short()?));
    }
    t.decors = Some(ids);
    Ok(())
}

impl SkyBoxType {
    /// Decode one sky box entry.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let record = Record {
            kind: "skybox type",
            id: -1,
        };
        decode_record(&SKYBOX_OPCODES, "skybox", record, bytes, Self::default())
    }
}

/// One sky decor type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkyDecorType {
    /// 0 texture sprite, 1 lit sphere, 2 model.
    pub kind: i32,
    /// Size.
    pub size: i32,
    /// Position; a fixed direction when `fixed` is set.
    pub position: [i32; 3],
    /// The position is a fixed direction.
    pub fixed: bool,
    /// Colour (default `16777216`).
    pub colour: i32,
    /// Texture or model id.
    pub texture: i32,
    /// Rotation.
    pub rotation: [i32; 3],
}

impl Default for SkyDecorType {
    fn default() -> Self {
        Self {
            kind: 0,
            size: 0,
            position: [0; 3],
            fixed: false,
            colour: 16_777_216,
            texture: 0,
            rotation: [0; 3],
        }
    }
}

/// Sky decor opcodes of this revision.
static SKYDECOR_OPCODES: Table<SkyDecorType, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Short(|t, _, v| t.size = i32::from(v))),
        Entry::new(at(2), Rule::Flag(|t, _| t.fixed = true)),
        Entry::new(
            at(3),
            Rule::Custom(|s, t, _| {
                t.position = read_triple(s)?;
                Ok(())
            }),
        ),
        Entry::new(at(4), Rule::Byte(|t, _, v| t.kind = i32::from(v))),
        Entry::new(at(5), Rule::SmartId(|t, _, v| t.texture = v)),
        Entry::new(at(6), Rule::Medium(|t, _, v| t.colour = v as i32)),
        Entry::new(
            at(7),
            Rule::Custom(|s, t, _| {
                t.rotation = read_triple(s)?;
                Ok(())
            }),
        ),
    ],
    Unknown::Skip,
);

/// Three signed shorts.
fn read_triple(source: Input<anyhow::Error>) -> Result<[i32; 3]> {
    Ok([
        i32::from(source.signed_short()?),
        i32::from(source.signed_short()?),
        i32::from(source.signed_short()?),
    ])
}

impl SkyDecorType {
    /// Decode one sky decor entry.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let record = Record {
            kind: "sky decor type",
            id: -1,
        };
        decode_record(
            &SKYDECOR_OPCODES,
            "skydecor",
            record,
            bytes,
            Self::default(),
        )
    }
}

/// The sky box and sky decor type lists.
#[derive(Clone, Debug, Default)]
pub struct SkyTypes {
    boxes: BTreeMap<u32, SkyBoxType>,
    decors: BTreeMap<u32, SkyDecorType>,
}

impl SkyTypes {
    /// Decode config groups 29 and 30. A group the cache does not carry (the
    /// 910 cache has no group 30) lists as empty; every lookup then returns
    /// the default type.
    pub fn load(pack: &crate::cache::Pack) -> anyhow::Result<Self> {
        let index = pack.read_archive_index(crate::flo::FLO_ARCHIVE)?;
        let mut out = Self::default();
        for (group, into_boxes) in [(SKYBOXTYPE_GROUP, true), (SKYDECORTYPE_GROUP, false)] {
            if !index.group_id.contains(&group) {
                continue;
            }
            for (id, bytes) in pack.read_group(crate::flo::FLO_ARCHIVE, group)? {
                if into_boxes {
                    out.boxes.insert(
                        id,
                        SkyBoxType::decode(&bytes)
                            .map_err(|e| anyhow::anyhow!("skybox type {id}: {e:#}"))?,
                    );
                } else {
                    out.decors.insert(
                        id,
                        SkyDecorType::decode(&bytes)
                            .map_err(|e| anyhow::anyhow!("sky decor type {id}: {e:#}"))?,
                    );
                }
            }
        }
        Ok(out)
    }

    /// The sky box type for `id`, or the default.
    #[must_use]
    pub fn skybox(&self, id: i32) -> SkyBoxType {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.boxes.get(&id).cloned())
            .unwrap_or_default()
    }

    /// The sky decor type for `id`, or the default.
    #[must_use]
    pub fn decor(&self, id: i32) -> SkyDecorType {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.decors.get(&id).cloned())
            .unwrap_or_default()
    }

    /// Give every sky box `decors` as its decors, the first lighting the
    /// others. For a diagnostic only: no 910 sky box carries a decor of its
    /// own.
    pub fn attach_decors_to_all(&mut self, decors: Vec<SkyDecorType>) {
        let first = self.decors.keys().next_back().map_or(0, |last| last + 1);
        let ids: Vec<i32> = (first..)
            .zip(decors)
            .map(|(id, decor)| {
                self.decors.insert(id, decor);
                id as i32
            })
            .collect();
        for sky in self.boxes.values_mut() {
            sky.decors = Some(ids.clone());
            sky.sun_decor = 0;
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    #[must_use]
    pub fn skybox_count(&self) -> usize {
        self.boxes.len()
    }

    /// The decoded skybox types, for the skybox tests' fixtures.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn boxes_mut(&mut self) -> &mut BTreeMap<u32, SkyBoxType> {
        &mut self.boxes
    }

    /// The decoded decor types, for the skybox tests' fixtures.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn decors_mut(&mut self) -> &mut BTreeMap<u32, SkyDecorType> {
        &mut self.decors
    }
}
