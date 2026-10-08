//! Floor overlay and underlay configs (`flo`).
//!
//! Both are single-file-per-config groups of the shared `config` archive
//! ([`OVERLAY_GROUP`], [`UNDERLAY_GROUP`]): the config id is the file id, with
//! no `group << bits | file` composition. Landscape tiles store wire ids where
//! **0 means absent and any other value is the config id plus one**
//! ([`config_id`]).
//!
//! Colours: an overlay's `rgb` and `average_colour` are HSL-packed shorts
//! ([`colour_fudge`]; magenta `0xFF00FF` means "no colour" and stores `-1`).
//! An underlay's `colour` is plain 24-bit RGB with the HSL blend terms derived
//! by [`compute_colour`]. Unknown opcodes are a hard error naming the id and
//! the opcode, never a silent skip.
//!
//! Render contract:
//! - [`FloStore::overlay_rgb`] and [`FloStore::underlay_rgb`] take **wire** ids
//!   and return linear `0.0..=1.0` triples. Overlays decode the HSL-packed
//!   short through the palette approximation ([`hsl16_to_linear`]); underlays
//!   decode the stored RGB as `channel / 255.0` without gamma, for the
//!   unblended base. An overlay whose `rgb` is the no-colour sentinel falls
//!   back to `average_colour`, else yields `None`.
//! - `texture` holds a material id (`None` for "no material"; the underlay
//!   default is `Some(0)`). The id indexes the materials archive; each
//!   material points at texture ids for `texture::load_texture`.
//!   `material_scale` is the UV divisor.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::cache::Pack;
use crate::opcode_table::{at, decode_record, Entry, Record, Rule, Table, Unknown};

/// Js5 archive holding the flo groups.
pub const FLO_ARCHIVE: &str = "config";
/// Config-group id holding underlay entries.
pub const UNDERLAY_GROUP: u32 = 1;
/// Config-group id holding overlay entries.
pub const OVERLAY_GROUP: u32 = 4;
/// Magenta sentinel mapped to "no colour" by [`colour_fudge`].
pub const NO_COLOUR_RGB: u32 = 0xFF00FF;
/// Stored overlay `rgb`/`average_colour` meaning "no colour".
pub const NO_COLOUR: i32 = -1;

// ---------------------------------------------------------------------------
// Colour conversions
// ---------------------------------------------------------------------------

/// Split an RGB triple into `(hue_turns, saturation, lightness)`: the first
/// half of both the packed-HSL conversion and [`compute_colour`]. Channels
/// scale by `/ 256.0`, hue is in turns (divide by 6 for the packed form).
fn rgb_to_hsl_turns(rgb: u32) -> (f64, f64, f64) {
    let red = f64::from((rgb >> 16) & 0xFF) / 256.0;
    let green = f64::from((rgb >> 8) & 0xFF) / 256.0;
    let blue = f64::from(rgb & 0xFF) / 256.0;
    let mut lo = red;
    if green < red {
        lo = green;
    }
    if blue < lo {
        lo = blue;
    }
    let mut hi = red;
    if green > red {
        hi = green;
    }
    if blue > hi {
        hi = blue;
    }
    let mut hue = 0.0;
    let mut sat = 0.0;
    let light = (lo + hi) / 2.0;
    if lo != hi {
        if light < 0.5 {
            sat = (hi - lo) / (hi + lo);
        }
        if light >= 0.5 {
            sat = (hi - lo) / (2.0 - hi - lo);
        }
        if red == hi {
            hue = (green - blue) / (hi - lo);
        } else if green == hi {
            hue = (blue - red) / (hi - lo) + 2.0;
        } else if blue == hi {
            hue = (red - green) / (hi - lo) + 4.0;
        }
    }
    (hue / 6.0, sat, light)
}

/// Magenta `0xFF00FF` encodes "no colour" (`-1`); anything else packs to an
/// HSL short ([`crate::colour::rgb24_to_hsl16`], rs910-core, pinned by
/// client910's `core_goldens::colour`).
fn colour_fudge(rgb: u32) -> i32 {
    if rgb == NO_COLOUR_RGB {
        NO_COLOUR
    } else {
        crate::colour::rgb24_to_hsl16(rgb as i32)
    }
}

/// Derive the HSL blend terms stored alongside an underlay's plain-RGB
/// `colour`. Returns `(hue, saturation, lightness, chroma)`; saturation and
/// lightness are clamped to `0..=255`, chroma is at least 1, hue may be
/// negative (`chroma * hue_turns` truncated toward zero, unclamped).
fn compute_colour(rgb: u32) -> (i32, u32, u32, u32) {
    let (hue_turns, sat, light) = rgb_to_hsl_turns(rgb);
    let mut saturation = (sat * 256.0) as i32;
    let mut lightness = (light * 256.0) as i32;
    saturation = saturation.clamp(0, 255);
    lightness = lightness.clamp(0, 255);
    let mut chroma = if light > 0.5 {
        ((1.0 - light) * sat * 512.0) as i32
    } else {
        (sat * light * 512.0) as i32
    };
    if chroma < 1 {
        chroma = 1;
    }
    let hue = (f64::from(chroma) * hue_turns) as i32;
    (hue, saturation as u32, lightness as u32, chroma as u32)
}

/// Packed-HSL-short → linear-`0.0..=1.0` `f32` triple, with the same formula
/// as `model.rs::hsl_to_rgb`: `hue = (h / 64 + 0.0078125) * 360`,
/// `sat = s / 8 + 0.0625`, `light = l / 128`; sector HSV to RGB blend; gamma
/// `pow(c, 0.7)`; outputs clamped to `1.0`. The lighting-stage brightness
/// remap is not applied.
fn hsl16_to_linear([hue6, sat3, light7]: [u8; 3]) -> [f32; 3] {
    let hue_deg = (f32::from(hue6) / 64.0 + 0.007_812_5) * 360.0;
    let sat = f32::from(sat3) / 8.0 + 0.0625;
    let light = f32::from(light7) / 128.0;
    let sector = (hue_deg / 60.0).floor() as i32;
    let frac = hue_deg / 60.0 - sector as f32;
    let dim = (1.0 - sat) * light;
    let fall = (1.0 - sat * frac) * light;
    let rise = (1.0 - (1.0 - frac) * sat) * light;
    let (red, green, blue) = match sector.rem_euclid(6) {
        0 => (light, rise, dim),
        1 => (fall, light, dim),
        2 => (dim, light, rise),
        3 => (dim, fall, light),
        4 => (rise, dim, light),
        _ => (light, dim, fall),
    };
    [
        red.powf(0.7).clamp(0.0, 1.0),
        green.powf(0.7).clamp(0.0, 1.0),
        blue.powf(0.7).clamp(0.0, 1.0),
    ]
}

/// Split a packed HSL short into `[hue6, sat3, light7]` for
/// [`hsl16_to_linear`] (the split model face colours use: `(c >> 10) & 63`,
/// `(c >> 7) & 7`, `c & 127`).
fn split_hsl16(packed: i32) -> [u8; 3] {
    let raw = packed as u32;
    [
        ((raw >> 10) & 63) as u8,
        ((raw >> 7) & 7) as u8,
        (raw & 127) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

/// A floor overlay: colour and material for one tile layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Config id (the file id in [`OVERLAY_GROUP`]); folded into
    /// [`Overlay::priority`].
    pub id: u32,
    /// HSL-packed colour, or `-1` for the magenta "no colour" sentinel.
    pub rgb: i32,
    /// Material-archive id, or `None` for "no material". One opcode stores a
    /// byte id, another a short id where 65535 means none.
    pub texture: Option<u32>,
    /// Whether the overlay occludes (default `true`).
    pub occlude: bool,
    /// HSL-packed average colour, default `-1`.
    pub average_colour: i32,
    /// Material UV divisor: the stored short times four, default 512.
    pub material_scale: u32,
    /// Whether the overlay casts a hard shadow (default `true`).
    pub hard_shadow: bool,
    /// Draw priority: the stored base shifted left by 8, or'd with the id.
    pub priority: u32,
    /// Blends into neighbouring tiles (default `false`).
    pub blend: bool,
    /// Water fog colour, plain RGB, default 1190717.
    pub water_fog_colour: u32,
    /// Water fog scale: the stored byte times four, default 512.
    pub water_fog_scale: u32,
    /// Water fog offset, default 256.
    pub water_fog_offset: u32,
    /// Further water fog terms, defaults 64, 0 and 64.
    pub water_fog_extra_a: u32,
    pub water_fog_extra_b: u32,
    pub water_fog_extra_c: u32,
}

/// An overlay while its opcodes are being read.
struct OverlayDraft {
    overlay: Overlay,
    /// The stored priority base, folded into `priority` after the last opcode.
    priority_base: u32,
}

/// Overlay opcodes of this revision.
static OVERLAY_OPCODES: Table<OverlayDraft, anyhow::Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Medium(|d, _, v| d.overlay.rgb = colour_fudge(v)),
        ),
        Entry::new(
            at(2),
            Rule::Byte(|d, _, v| d.overlay.texture = Some(u32::from(v))),
        ),
        Entry::new(
            at(3),
            Rule::Short(|d, _, v| {
                d.overlay.texture = if v == 65535 { None } else { Some(u32::from(v)) };
            }),
        ),
        Entry::new(at(5), Rule::Flag(|d, _| d.overlay.occlude = false)),
        Entry::new(
            at(7),
            Rule::Medium(|d, _, v| d.overlay.average_colour = colour_fudge(v)),
        ),
        // Accepted and ignored, without payload.
        Entry::new(at(8), Rule::Skip(&[])),
        Entry::new(
            at(9),
            Rule::Short(|d, _, v| d.overlay.material_scale = u32::from(v) << 2),
        ),
        Entry::new(at(10), Rule::Flag(|d, _| d.overlay.hard_shadow = false)),
        Entry::new(at(11), Rule::Byte(|d, _, v| d.priority_base = u32::from(v))),
        Entry::new(at(12), Rule::Flag(|d, _| d.overlay.blend = true)),
        Entry::new(
            at(13),
            Rule::Medium(|d, _, v| d.overlay.water_fog_colour = v),
        ),
        Entry::new(
            at(14),
            Rule::Byte(|d, _, v| d.overlay.water_fog_scale = u32::from(v) << 2),
        ),
        Entry::new(
            at(16),
            Rule::Byte(|d, _, v| d.overlay.water_fog_offset = u32::from(v)),
        ),
        Entry::new(
            at(20),
            Rule::Short(|d, _, v| d.overlay.water_fog_extra_a = u32::from(v)),
        ),
        Entry::new(
            at(21),
            Rule::Byte(|d, _, v| d.overlay.water_fog_extra_b = u32::from(v)),
        ),
        Entry::new(
            at(22),
            Rule::Short(|d, _, v| d.overlay.water_fog_extra_c = u32::from(v)),
        ),
    ],
    Unknown::Reject,
);

/// Decode one overlay entry. An unknown opcode is an error naming the id and
/// the opcode.
pub fn decode_overlay(id: u32, data: &[u8]) -> Result<Overlay> {
    let draft = OverlayDraft {
        overlay: Overlay {
            id,
            rgb: 0,
            texture: None,
            occlude: true,
            average_colour: -1,
            material_scale: 512,
            hard_shadow: true,
            priority: 0,
            blend: false,
            water_fog_colour: 1_190_717,
            water_fog_scale: 512,
            water_fog_offset: 256,
            water_fog_extra_a: 64,
            water_fog_extra_b: 0,
            water_fog_extra_c: 64,
        },
        priority_base: 0,
    };
    let record = Record {
        kind: "overlay",
        id: i64::from(id),
    };
    let mut draft = decode_record(&OVERLAY_OPCODES, "flo", record, data, draft)?;
    draft.overlay.priority = (draft.priority_base << 8) | id;
    Ok(draft.overlay)
}

// ---------------------------------------------------------------------------
// Underlay
// ---------------------------------------------------------------------------

/// A floor underlay: base colour and material for one tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Underlay {
    /// Config id (the file id in [`UNDERLAY_GROUP`]).
    pub id: u32,
    /// Plain 24-bit RGB, default 0 (black).
    pub colour: u32,
    /// Material-archive id (a short where 65535 means none), or `None`. The
    /// default is `Some(0)`, not `None`.
    pub texture: Option<u32>,
    /// Material UV divisor: the stored short times four, default 512.
    pub material_scale: u32,
    /// Whether the underlay casts a hard shadow (default `true`).
    pub hard_shadow: bool,
    /// Whether the underlay occludes (default `true`).
    pub occlude: bool,
    /// HSL blend terms derived from `colour` by [`compute_colour`] whenever
    /// the colour opcode runs, 0 otherwise. `hue` may be negative.
    pub hue: i32,
    /// Saturation term (`0..=255`).
    pub saturation: u32,
    /// Lightness term (`0..=255`).
    pub lightness: u32,
    /// Chroma term (at least 1).
    pub chroma: u32,
}

/// Underlay opcodes of this revision.
static UNDERLAY_OPCODES: Table<Underlay, anyhow::Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Medium(|u, _, v| {
                u.colour = v;
                (u.hue, u.saturation, u.lightness, u.chroma) = compute_colour(v);
            }),
        ),
        Entry::new(
            at(2),
            Rule::Short(|u, _, v| {
                u.texture = if v == 65535 { None } else { Some(u32::from(v)) };
            }),
        ),
        Entry::new(
            at(3),
            Rule::Short(|u, _, v| u.material_scale = u32::from(v) << 2),
        ),
        Entry::new(at(4), Rule::Flag(|u, _| u.hard_shadow = false)),
        Entry::new(at(5), Rule::Flag(|u, _| u.occlude = false)),
    ],
    Unknown::Reject,
);

/// Decode one underlay entry. An unknown opcode is an error naming the id and
/// the opcode.
pub fn decode_underlay(id: u32, data: &[u8]) -> Result<Underlay> {
    let blank = Underlay {
        id,
        colour: 0,
        texture: Some(0),
        material_scale: 512,
        hard_shadow: true,
        occlude: true,
        hue: 0,
        saturation: 0,
        lightness: 0,
        chroma: 0,
    };
    let record = Record {
        kind: "underlay",
        id: i64::from(id),
    };
    decode_record(&UNDERLAY_OPCODES, "flo", record, data, blank)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Map a landscape wire id to its flo config id: wire `0` means "layer
/// absent" (`None`); any other wire id `w` addresses config id `w - 1`. This
/// applies identically to overlay and underlay ids.
#[must_use]
pub fn config_id(wire_id: u32) -> Option<u32> {
    if wire_id == 0 {
        None
    } else {
        // Wire ids are small smart ints; the only failure is `u32::MAX`
        // underflow, guarded by the zero check above in practice — use a
        // checked sub so the mapping is total.
        wire_id.checked_sub(1)
    }
}

/// All floor configs: overlays keyed by config id plus underlays keyed by
/// config id. See the module-level render-track contract for the query API.
#[derive(Clone, Debug, Default)]
pub struct FloStore {
    /// Overlay entries by config id (file id in [`OVERLAY_GROUP`]).
    pub overlays: BTreeMap<u32, Overlay>,
    /// Underlay entries by config id (file id in [`UNDERLAY_GROUP`]).
    pub underlays: BTreeMap<u32, Underlay>,
}

impl FloStore {
    /// Load every overlay (group [`OVERLAY_GROUP`]) and underlay (group
    /// [`UNDERLAY_GROUP`]) of [`FLO_ARCHIVE`], with [`decode_overlay`] and
    /// [`decode_underlay`].
    pub fn load(pack: &Pack) -> Result<Self> {
        rs910_core::profile::scope!("load flo configs");
        let overlays = load_group(pack, OVERLAY_GROUP, decode_overlay)?;
        let underlays = load_group(pack, UNDERLAY_GROUP, decode_underlay)?;
        Ok(Self {
            overlays,
            underlays,
        })
    }

    /// Look up one overlay by config id (i.e. `wire - 1`; see [`config_id`]).
    pub fn get_overlay(&self, id: u32) -> Option<&Overlay> {
        self.overlays.get(&id)
    }

    /// Look up one underlay by config id (i.e. `wire - 1`; see [`config_id`]).
    pub fn get_underlay(&self, id: u32) -> Option<&Underlay> {
        self.underlays.get(&id)
    }

    /// Number of decoded overlays.
    pub fn overlay_count(&self) -> usize {
        self.overlays.len()
    }

    /// Number of decoded underlays.
    pub fn underlay_count(&self) -> usize {
        self.underlays.len()
    }

    /// Iterate `(id, overlay)` in id order.
    pub fn iter_overlays(&self) -> impl Iterator<Item = (&u32, &Overlay)> {
        self.overlays.iter()
    }

    /// Iterate `(id, underlay)` in id order.
    pub fn iter_underlays(&self) -> impl Iterator<Item = (&u32, &Underlay)> {
        self.underlays.iter()
    }

    /// Overlay colour for a landscape **wire** id, as linear `0.0..=1.0`
    /// `f32` for the render track. Wire `0`, unknown ids, and the `-1`
    /// no-colour sentinel (with no `average_colour` fallback) yield `None`.
    pub fn overlay_rgb(&self, wire_id: u32) -> Option<[f32; 3]> {
        let overlay = self.get_overlay(config_id(wire_id)?)?;
        let packed = if overlay.rgb != NO_COLOUR {
            overlay.rgb
        } else if overlay.average_colour != NO_COLOUR {
            overlay.average_colour
        } else {
            return None;
        };
        Some(hsl16_to_linear(split_hsl16(packed)))
    }

    /// Underlay colour for a landscape **wire** id, as linear `0.0..=1.0`
    /// `f32` for the render track: the stored plain RGB decoded as
    /// `channel / 255.0` (no gamma). Wire `0` and unknown ids yield `None`.
    pub fn underlay_rgb(&self, wire_id: u32) -> Option<[f32; 3]> {
        let underlay = self.get_underlay(config_id(wire_id)?)?;
        let colour = underlay.colour;
        Some([
            ((colour >> 16) & 0xFF) as f32 / 255.0,
            ((colour >> 8) & 0xFF) as f32 / 255.0,
            (colour & 0xFF) as f32 / 255.0,
        ])
    }
}

/// Load one flo group ([`OVERLAY_GROUP`] or [`UNDERLAY_GROUP`]) of
/// [`FLO_ARCHIVE`]: every file decodes to config id == file id.
fn load_group<T>(
    pack: &Pack,
    group: u32,
    decode: fn(u32, &[u8]) -> Result<T>,
) -> Result<BTreeMap<u32, T>> {
    let files = pack
        .read_group(FLO_ARCHIVE, group)
        .with_context(|| format!("{FLO_ARCHIVE} group {group}"))?;
    let mut entries = BTreeMap::new();
    for (file_id, bytes) in &files {
        let entry = decode(*file_id, bytes)
            .with_context(|| format!("{FLO_ARCHIVE} group {group} id {file_id}"))?;
        entries.insert(*file_id, entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_colour_opcodes_pack_to_hsl_shorts() {
        // Black packs to HSL short 0 (all channels 0 -> hue/sat/light 0).
        let overlay = decode_overlay(7, &[1, 0, 0, 0, 0]).unwrap();
        assert_eq!(overlay.id, 7);
        assert_eq!(overlay.rgb, 0);
        // White packs to 127 (light byte 255 >> 1; hue/sat bytes 0).
        let overlay = decode_overlay(7, &[1, 0xFF, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(overlay.rgb, 127);
        // Magenta is the no-colour sentinel -> -1.
        let overlay = decode_overlay(7, &[1, 0xFF, 0, 0xFF, 0]).unwrap();
        assert_eq!(overlay.rgb, NO_COLOUR);
        // The average colour (opcode 7) goes through the same sentinel rule.
        let overlay = decode_overlay(7, &[7, 0xFF, 0, 0xFF, 0]).unwrap();
        assert_eq!(overlay.average_colour, NO_COLOUR);
        let overlay = decode_overlay(7, &[7, 0, 0, 0, 0]).unwrap();
        assert_eq!(overlay.average_colour, 0);
        // Untouched fields keep their defaults.
        let overlay = decode_overlay(7, &[0]).unwrap();
        assert_eq!(overlay.rgb, 0);
        assert_eq!(overlay.texture, None);
        assert!(overlay.occlude);
        assert_eq!(overlay.average_colour, -1);
        assert_eq!(overlay.material_scale, 512);
        assert!(overlay.hard_shadow);
        assert!(!overlay.blend);
        assert_eq!(overlay.water_fog_colour, 1_190_717);
        assert_eq!(overlay.water_fog_scale, 512);
        assert_eq!(overlay.water_fog_offset, 256);
        assert_eq!(overlay.water_fog_extra_a, 64);
        assert_eq!(overlay.water_fog_extra_b, 0);
        assert_eq!(overlay.water_fog_extra_c, 64);
    }

    #[test]
    fn overlay_texture_and_flag_opcodes() {
        // Opcode 2: g1 material id.
        let overlay = decode_overlay(3, &[2, 7, 0]).unwrap();
        assert_eq!(overlay.texture, Some(7));
        // Opcode 3: 16-bit material id; 65535 -> None.
        let overlay = decode_overlay(3, &[3, 0x12, 0x34, 0]).unwrap();
        assert_eq!(overlay.texture, Some(0x1234));
        let overlay = decode_overlay(3, &[3, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(overlay.texture, None);
        // Flags + scales + priority base.
        let overlay = decode_overlay(
            9,
            &[
                5, 9, 0x01, 0x00, 10, 11, 4, 12, 13, 1, 2, 3, 14, 5, 16, 6, 20, 0, 7, 21, 8, 22, 0,
                9, 0,
            ],
        )
        .unwrap();
        assert!(!overlay.occlude);
        assert_eq!(overlay.material_scale, 0x0100 << 2);
        assert!(!overlay.hard_shadow);
        assert!(overlay.blend);
        assert_eq!(overlay.water_fog_colour, 0x010203);
        assert_eq!(overlay.water_fog_scale, 5_u32 << 2);
        assert_eq!(overlay.water_fog_offset, 6);
        assert_eq!(overlay.water_fog_extra_a, 7);
        assert_eq!(overlay.water_fog_extra_b, 8);
        assert_eq!(overlay.water_fog_extra_c, 9);
        // The priority folds as base << 8 | id.
        assert_eq!(overlay.priority, (4_u32 << 8) | 9);
        // Opcode 8 is accepted and consumes no bytes.
        let overlay = decode_overlay(1, &[8, 2, 7, 0]).unwrap();
        assert_eq!(overlay.texture, Some(7));
    }

    #[test]
    fn overlay_rejects_unknown_opcode_naming_id_and_opcode() {
        let err = decode_overlay(41, &[99, 0]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("41"), "error names the id: {msg}");
        assert!(msg.contains("99"), "error names the opcode: {msg}");
        // Missing terminator is an error, not a hang.
        assert!(decode_overlay(41, &[1, 2, 3]).is_err());
        assert!(decode_overlay(41, &[]).is_err());
    }

    #[test]
    fn underlay_colour_and_texture_opcodes() {
        // Black: plain RGB 0 + blend terms (hue/sat/light 0, chroma 1).
        let underlay = decode_underlay(2, &[1, 0, 0, 0, 0]).unwrap();
        assert_eq!(underlay.id, 2);
        assert_eq!(underlay.colour, 0);
        assert_eq!(underlay.hue, 0);
        assert_eq!(underlay.saturation, 0);
        assert_eq!(underlay.lightness, 0);
        assert_eq!(underlay.chroma, 1);
        // White: lightness 255, chroma floors at 1, hue 0.
        let underlay = decode_underlay(2, &[1, 0xFF, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(underlay.colour, 0xFF_FFFF);
        assert_eq!(underlay.lightness, 255);
        assert_eq!(underlay.chroma, 1);
        // The blend terms always match the shared helper.
        let underlay = decode_underlay(2, &[1, 0x12, 0x34, 0x56, 0]).unwrap();
        assert_eq!(underlay.colour, 0x123456);
        let (hue, sat, light, chroma) = compute_colour(0x123456);
        assert_eq!(underlay.hue, hue);
        assert_eq!(underlay.saturation, sat);
        assert_eq!(underlay.lightness, light);
        assert_eq!(underlay.chroma, chroma);
        assert!(underlay.chroma >= 1);
        // Texture: 16-bit id; 65535 -> None. The default is Some(0).
        let underlay = decode_underlay(2, &[2, 0x00, 0x09, 0]).unwrap();
        assert_eq!(underlay.texture, Some(9));
        let underlay = decode_underlay(2, &[2, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(underlay.texture, None);
        let underlay = decode_underlay(2, &[0]).unwrap();
        assert_eq!(underlay.texture, Some(0));
        assert_eq!(underlay.material_scale, 512);
        assert!(underlay.hard_shadow);
        assert!(underlay.occlude);
        // Scale + flag opcodes.
        let underlay = decode_underlay(2, &[3, 0x02, 0x00, 4, 5, 0]).unwrap();
        assert_eq!(underlay.material_scale, 0x0200 << 2);
        assert!(!underlay.hard_shadow);
        assert!(!underlay.occlude);
    }

    #[test]
    fn underlay_rejects_unknown_opcode_naming_id_and_opcode() {
        let err = decode_underlay(17, &[6, 0]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("17"), "error names the id: {msg}");
        assert!(msg.contains('6'), "error names the opcode: {msg}");
        assert!(decode_underlay(17, &[]).is_err());
    }

    #[test]
    fn wire_id_zero_is_absent_rest_shift_by_one() {
        assert_eq!(config_id(0), None);
        assert_eq!(config_id(1), Some(0));
        assert_eq!(config_id(5), Some(4));

        // Store level: wire 0 -> None even with entries present; wire w reads
        // config w - 1.
        let mut store = FloStore::default();
        store
            .overlays
            .insert(4, decode_overlay(4, &[1, 0, 0, 0, 0]).unwrap());
        store
            .underlays
            .insert(6, decode_underlay(6, &[1, 0x12, 0x34, 0x56, 0]).unwrap());
        assert_eq!(store.overlay_rgb(0), None);
        assert_eq!(store.underlay_rgb(0), None);
        assert_eq!(store.overlay_rgb(2), None); // config 1 absent
        assert_eq!(store.overlay_rgb(5), Some([0.0, 0.0, 0.0])); // config 4, black
        let under = store.underlay_rgb(7).unwrap(); // config 6
        assert!((under[0] - 0x12 as f32 / 255.0).abs() < 1e-6);
        assert!((under[1] - 0x34 as f32 / 255.0).abs() < 1e-6);
        assert!((under[2] - 0x56 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(store.underlay_rgb(8), None); // config 7 absent
    }

    #[test]
    fn overlay_rgb_falls_back_to_average_colour_then_none() {
        // rgb set -> decodes rgb (black -> [0,0,0]).
        let mut store = FloStore::default();
        store.overlays.insert(
            0,
            decode_overlay(0, &[1, 0, 0, 0, 7, 0xFF, 0xFF, 0xFF, 0]).unwrap(),
        );
        assert_eq!(store.overlay_rgb(1), Some([0.0, 0.0, 0.0]));
        // rgb -1 with average set -> decodes the average slot.
        let mut store = FloStore::default();
        store.overlays.insert(
            0,
            decode_overlay(0, &[1, 0xFF, 0, 0xFF, 7, 0xFF, 0xFF, 0xFF, 0]).unwrap(),
        );
        let rgb = store.overlay_rgb(1).unwrap();
        assert!(rgb.iter().all(|c| (0.0..=1.0).contains(c)));
        assert!(
            rgb.iter().all(|&c| c > 0.9),
            "white average -> near-white: {rgb:?}"
        );
        // Both -1 -> None.
        let mut store = FloStore::default();
        store
            .overlays
            .insert(0, decode_overlay(0, &[1, 0xFF, 0, 0xFF, 0]).unwrap());
        assert_eq!(store.overlay_rgb(1), None);
    }
}
