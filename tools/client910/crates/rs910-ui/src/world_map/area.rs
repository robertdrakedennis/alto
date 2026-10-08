//! Placed map elements, loc records and source/display coordinate conversion.

use crate::{cache::Pack, ui_bytes::Cursor};

use anyhow::Result;

/// The locs of one ground tile: a single loc (with its shape/angle byte) or
/// several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileLocs {
    One(i32, i8),
    Many(Vec<i32>, Vec<i8>),
}

impl TileLocs {
    pub(super) fn split(&self) -> (Vec<i32>, Vec<i8>) {
        match self {
            Self::One(id, angle) => (vec![*id], vec![*angle]),
            Self::Many(ids, angles) => (ids.clone(), angles.clone()),
        }
    }
}

/// One tile of an upper level (levels 1-3), kept per 64x64 chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpperTile {
    /// The tile within its 64x64 chunk.
    pub x: u8,
    pub z: u8,
    /// The underlay id until the level blend replaces it by the blended ARGB
    /// colour (0 = none).
    pub colour: i32,
    /// Overlay id.
    pub overlay: i16,
    /// Overlay shape/rotation byte.
    pub shape: i8,
    /// Locs and their shape/angle bytes.
    pub locs: Option<(Vec<i32>, Vec<i8>)>,
}

/// One placed map element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// The map element type id.
    pub id: i32,
    /// Never assigned by the 910 client, so always 0.
    pub level: i32,
    /// Display tile relative to the area origin.
    pub x: i32,
    pub z: i32,
    /// The projected screen point (see [`super::View::project`]).
    pub screen: [i32; 2],
    /// The mouse is over the element.
    pub hovered: bool,
}

impl Element {
    pub fn new(id: i32, x: i32, z: i32) -> Self {
        Self {
            id,
            level: 0,
            x,
            z,
            screen: [0, 0],
            hovered: false,
        }
    }
}

/// File 1 of the area's `worldmapareas` group: packed display coords and
/// element ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticElements {
    pub coords: Vec<i32>,
    pub elements: Vec<i32>,
}

impl StaticElements {
    /// Loads the static elements of area `name`.
    pub fn load(pack: &Pack, name: &str, members: bool) -> Result<Self> {
        let Some(group) = pack.group_id_by_name("worldmapareas", name)? else {
            return Ok(Self::default());
        };
        let mut files = pack.read_group("worldmapareas", group)?;
        let Some(bytes) = files.remove(&1) else {
            return Ok(Self::default());
        };
        Self::decode(&bytes, members)
    }
    /// Every members-only entry a non-member skips decrements the entry
    /// count, so the loop stops `skipped` entries early.
    pub fn decode(bytes: &[u8], members: bool) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut count = usize::from(c.g2()?);
        let mut out = Self::default();
        while out.coords.len() < count {
            let coord = c.g4s()?;
            let element = i32::from(c.g2()?);
            let members_only = c.g1()?;
            if !members && members_only == 1 {
                count -= 1;
            } else {
                out.coords.push(coord);
                out.elements.push(element);
            }
        }
        Ok(out)
    }
}

/// A source tile's display tile `[x, z]` through the area's subareas.
pub fn source_to_display(
    area: &crate::minimap::AreaMetadata,
    level: i32,
    x: i32,
    z: i32,
) -> Option<[i32; 2]> {
    area.subareas.iter().find_map(|a| {
        (level >= a[0] && x >= a[1] && x <= a[3] && z >= a[2] && z <= a[4])
            .then_some([a[5] - a[1] + x, a[6] - a[2] + z])
    })
}

/// [`source_to_display`] without the level test.
pub fn source_to_display_any_level(
    area: &crate::minimap::AreaMetadata,
    x: i32,
    z: i32,
) -> Option<[i32; 2]> {
    area.subareas.iter().find_map(|a| {
        (x >= a[1] && x <= a[3] && z >= a[2] && z <= a[4])
            .then_some([a[5] - a[1] + x, a[6] - a[2] + z])
    })
}

/// A display tile's `[level, x, z]` source tile.
pub fn display_to_source(area: &crate::minimap::AreaMetadata, x: i32, z: i32) -> Option<[i32; 3]> {
    area.subareas.iter().find_map(|a| {
        (x >= a[5] && x <= a[7] && z >= a[6] && z <= a[8]).then_some([
            a[0],
            a[1] - a[5] + x,
            a[2] - a[6] + z,
        ])
    })
}
