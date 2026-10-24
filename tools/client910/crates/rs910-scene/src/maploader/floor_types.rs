//! Floor configuration snapshots used during terrain meshing.

#[cfg(test)]
use std::sync::OnceLock;

use crate::flo::{FloStore, Overlay, Underlay};

// ---------------------------------------------------------------------------
// Config views (floor overlay / underlay types as the loader sees them)
// ---------------------------------------------------------------------------

/// Floor overlay type fields in integer form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ov {
    /// `rgb` (HSL16 or `-1`).
    pub rgb: i32,
    /// `material` (`-1` = none).
    pub material: i32,
    /// `occlude`.
    pub occlude: bool,
    /// `averagecolour` (HSL16 or `-1`).
    pub averagecolour: i32,
    /// `materialscale`.
    pub materialscale: i32,
    /// `hardshadow`.
    pub hardshadow: bool,
    /// `priority` (`base << 8 | id`).
    pub priority: i32,
    /// `blend`.
    pub blend: bool,
    /// `waterfogcolour`.
    pub waterfogcolour: i32,
    /// `waterfogscale`.
    pub waterfogscale: i32,
    /// `waterfogoffset`.
    pub waterfogoffset: i32,
    /// `waterFogExtraA`.
    pub water_fog_extra_a: i32,
    /// `waterFogExtraB`.
    pub water_fog_extra_b: i32,
    /// `waterFogExtraC`.
    pub water_fog_extra_c: i32,
}

impl Ov {
    pub(super) fn from_overlay(o: &Overlay) -> Self {
        Self {
            rgb: o.rgb,
            material: o.texture.map_or(-1, |t| t as i32),
            occlude: o.occlude,
            averagecolour: o.average_colour,
            materialscale: o.material_scale as i32,
            hardshadow: o.hard_shadow,
            priority: o.priority as i32,
            blend: o.blend,
            waterfogcolour: o.water_fog_colour as i32,
            waterfogscale: o.water_fog_scale as i32,
            waterfogoffset: o.water_fog_offset as i32,
            water_fog_extra_a: o.water_fog_extra_a as i32,
            water_fog_extra_b: o.water_fog_extra_b as i32,
            water_fog_extra_c: o.water_fog_extra_c as i32,
        }
    }

    /// An overlay type that was never decoded (absent file): the field defaults with the
    /// post-decode step applied (`priority = 0 << 8 | id`).
    pub(super) fn default_for(id: u32) -> Self {
        Self {
            rgb: 0,
            material: -1,
            occlude: true,
            averagecolour: -1,
            materialscale: 512,
            hardshadow: true,
            priority: id as i32,
            blend: false,
            waterfogcolour: 1_190_717,
            waterfogscale: 512,
            waterfogoffset: 256,
            water_fog_extra_a: 64,
            water_fog_extra_b: 0,
            water_fog_extra_c: 64,
        }
    }
}

/// Floor underlay type fields in integer form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ul {
    /// `material` (default 0, `-1` = none).
    pub material: i32,
    /// `materialscale`.
    pub materialscale: i32,
    /// `hardshadow`.
    pub hardshadow: bool,
    /// `occlude`.
    pub occlude: bool,
    /// `hue` (chroma-premultiplied).
    pub hue: i32,
    /// `saturation`.
    pub saturation: i32,
    /// `lightness`.
    pub lightness: i32,
    /// `chroma`.
    pub chroma: i32,
}

impl Ul {
    pub(super) fn from_underlay(u: &Underlay) -> Self {
        Self {
            material: u.texture.map_or(-1, |t| t as i32),
            materialscale: u.material_scale as i32,
            hardshadow: u.hard_shadow,
            occlude: u.occlude,
            hue: u.hue,
            saturation: u.saturation as i32,
            lightness: u.lightness as i32,
            chroma: u.chroma as i32,
        }
    }

    /// Never-decoded underlay: the field defaults.
    const DEFAULT: Self = Self {
        material: 0,
        materialscale: 512,
        hardshadow: true,
        occlude: true,
        hue: 0,
        saturation: 0,
        lightness: 0,
        chroma: 0,
    };
}

/// Flo configs indexed by config id, with default-initialised fill for gaps (an absent
/// config resolves to its defaults).
#[derive(Clone, Debug)]
pub struct FloTables {
    overlays: Vec<Ov>,
    underlays: Vec<Ul>,
}

impl FloTables {
    /// Build from a decoded [`FloStore`].
    #[must_use]
    pub fn from_store(flo: &FloStore) -> Self {
        let max_ov = flo
            .overlays
            .keys()
            .next_back()
            .map_or(0, |&k| k as usize + 1);
        let max_ul = flo
            .underlays
            .keys()
            .next_back()
            .map_or(0, |&k| k as usize + 1);
        let overlays = (0..max_ov)
            .map(|id| {
                flo.overlays
                    .get(&(id as u32))
                    .map_or_else(|| Ov::default_for(id as u32), Ov::from_overlay)
            })
            .collect();
        let underlays = (0..max_ul)
            .map(|id| {
                flo.underlays
                    .get(&(id as u32))
                    .map_or(Ul::DEFAULT, Ul::from_underlay)
            })
            .collect();
        Self {
            overlays,
            underlays,
        }
    }

    /// `overlays.list(id)`.
    pub(super) fn overlay(&self, id: i32) -> Ov {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.overlays.get(i).copied())
            .unwrap_or_else(|| Ov::default_for(id.max(0) as u32))
    }

    /// `underlays.list(id)`.
    pub(super) fn underlay(&self, id: i32) -> Ul {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.underlays.get(i).copied())
            .unwrap_or(Ul::DEFAULT)
    }
}

#[cfg(test)]
pub(super) fn default_flo_tables() -> &'static FloTables {
    static EMPTY: OnceLock<FloTables> = OnceLock::new();
    EMPTY.get_or_init(|| FloTables {
        overlays: Vec::new(),
        underlays: Vec::new(),
    })
}
