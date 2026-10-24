//! Minimap state, base-sprite plan and overlay resolution, plus the configs it reads:
//! map scene icons (config group 34), map elements (config group 36) and the minimap
//! sprite ids of the graphics defaults.
//!
//! Covers the minimap state fields and their resets on rebuild and level change, the tile
//! visibility rule, the base-sprite plan (which level/tile subset the floor pass draws, and
//! the wall/loc marks with their scene icons) and the composite parameters of the per-frame
//! draw. The GPU side (the top-down floor pass and the rotated, masked composite) lives in
//! `render.rs` / `ui_backend.rs`.
//!
//! Overlays: the loc icon queue, the map-element queue built from the current world-map area,
//! and the per-frame overlay list (NPC, player and map-element markers) are resolved here
//! into `Overlay`s that `ui_backend::draw_minimap` draws.
//!
//! A runtime loc change refreshes the minimap; the app passes the changed occupants as
//! [`LocOverrides`]. The base sprite is not redrawn then (only a level change does that), and
//! `base_plan` still reads the scene graph's own locs.
//!
//! Gaps, each documented at its site: the wait for loc icons to be ready. Hint-arrow
//! targets, including the edge/rotated presentation path,
//! now have a retained packet/minimap owner.
//! `MINIMAP_TOGGLE` is parsed into the retained engine and
//! copied to this renderer's `toggle` field each frame.
use crate::{
    cache::Pack,
    config::{LocStore, NpcStore},
    scene::Scene,
    sprite::Sprite,
    sprite_data::Data,
    ui_bytes::Cursor,
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, rc::Rc};

/// Map scene icon config: sprite id (opcode 1, -1 on opcode 4), tint (opcode 2) and
/// scale-to-loc (opcode 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MsiType {
    pub sprite: i32,
    pub tint: i32,
    pub scale_to_loc: bool,
}
impl MsiType {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut t = Self::default();
        loop {
            match c.g1()? {
                0 => return Ok(t),
                1 => t.sprite = c.gsmart2or4s()?,
                2 => t.tint = c.g3()? as i32,
                3 => t.scale_to_loc = true,
                4 => t.sprite = -1,
                5 => {}
                other => anyhow::bail!("map scene icon opcode {other}"),
            }
        }
    }
}

/// The map scene icon configs (group 34) plus the rotated / mirrored / tinted sprite cache.
#[derive(Default)]
pub struct MsiStore {
    pub types: BTreeMap<i32, MsiType>,
    sprites: BTreeMap<(i32, i32, bool), Option<Rc<Sprite>>>,
}
impl MsiStore {
    pub fn load(pack: &Pack) -> Result<Self> {
        let mut s = Self::default();
        for (id, bytes) in pack
            .read_group("config", 34)
            .context("msitype config group 34")?
        {
            s.types.insert(
                id as i32,
                MsiType::decode(&bytes).with_context(|| format!("msitype {id}"))?,
            );
        }
        Ok(s)
    }
    /// An absent file decodes to the defaults.
    pub fn get(&self, id: i32) -> MsiType {
        self.types.get(&id).cloned().unwrap_or_default()
    }
    /// The group's first frame, mirrored then rotated `rotation` times, tinted by the
    /// tint colour over its alpha.
    pub fn sprite(
        &mut self,
        pack: &Pack,
        id: i32,
        rotation: i32,
        mirror: bool,
    ) -> Option<Rc<Sprite>> {
        let t = self.get(id);
        if t.sprite == -1 {
            return None;
        }
        let key = (t.sprite, rotation & 3, mirror);
        if let Some(s) = self.sprites.get(&key) {
            return s.clone();
        }
        let sprite = (|| -> Result<Option<Rc<Sprite>>> {
            let Ok(mut files) = pack.read_group("sprites", t.sprite as u32) else {
                return Ok(None);
            };
            let Some(bytes) = files.remove(&0) else {
                return Ok(None);
            };
            let mut all = Data::decode(&bytes)?;
            if all.is_empty() {
                return Ok(None);
            }
            let mut data = all.remove(0);
            if mirror {
                data.flip(true);
            }
            for _ in 0..(rotation & 3) {
                data.rotate();
            }
            let mut sprite = Sprite::new(&data)?;
            if t.tint != 0 {
                // Multiply each opaque pixel by the tint, blending by alpha.
                let (r, g, b) = (
                    (t.tint >> 16 & 0xFF) as i64,
                    (t.tint >> 8 & 0xFF) as i64,
                    (t.tint & 0xFF) as i64,
                );
                for p in &mut sprite.argb {
                    let px = *p as i64 as u32 as i64;
                    let a = (px >> 24) & 0xFF;
                    if a == 0 {
                        continue;
                    }
                    // Tint product in wrapping 32-bit arithmetic, then a logical shift right by 8.
                    let red_scaled =
                        (((px & 0xFF0000) as i32).wrapping_mul(r as i32)).wrapping_mul(-16777216);
                    let green_scaled =
                        (((px & 0xFF00) as i32).wrapping_mul(g as i32)).wrapping_mul(16711680);
                    let blue_scaled =
                        (((px & 0xFF) as i32).wrapping_mul(b as i32)).wrapping_mul(65280);
                    let tinted = ((red_scaled | green_scaled | blue_scaled) as u32 >> 8) as i32;
                    if a == 255 {
                        *p = tinted;
                    } else {
                        let inverse_alpha = (256 - a) as i32;
                        let old = px as i32;
                        *p = ((a as i32) << 24)
                            | ((((tinted & 0xFF00FF)
                                .wrapping_mul(inverse_alpha)
                                .wrapping_add((old & 0xFF00FF).wrapping_mul(a as i32)))
                                & 0xFF00FF00u32 as i32)
                                .wrapping_add(
                                    ((tinted & 0xFF00)
                                        .wrapping_mul(inverse_alpha)
                                        .wrapping_add((old & 0xFF00).wrapping_mul(a as i32)))
                                        & 0xFF0000,
                                ))
                                >> 8;
                    }
                }
            }
            Ok(Some(Rc::new(sprite)))
        })()
        .unwrap_or(None);
        self.sprites.insert(key, sprite.clone());
        sprite
    }
}

/// A map element config (group 36), decoded opcode by opcode, with the polygon bounds
/// derived after decoding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapElement {
    /// Opcode 1 `sprite` (-1 none).
    pub sprite: i32,
    /// Opcode 2: the alternate sprite.
    pub sprite2: i32,
    /// Opcode 25: the sprite used for a flashing element.
    pub flash_sprite: i32,
    /// Opcode 3 `text`.
    pub text: Option<String>,
    /// Opcodes 10-14: the five world-map element operations.
    pub ops: Vec<Option<String>>,
    /// Opcode 17: the operation target text.
    pub target: Option<String>,
    /// Opcode 4: the label colour.
    pub text_colour: i32,
    /// Opcode 5: the label colour while hovered (-1 none).
    pub hover_text_colour: i32,
    /// Opcode 7 bit 1 clear: hidden on the world map.
    pub drawn_on_world_map: bool,
    /// Opcode 18: the off-screen arrow sprite.
    pub arrow_sprite: i32,
    /// Opcodes 22/21: label fill and label outline colours.
    pub text_fill: i32,
    pub text_outline: i32,
    /// Opcode 24: the label offset in tiles.
    pub text_offset: [i32; 2],
    /// Opcode 6: the label text size.
    pub text_size: i32,
    /// Opcode 7 bit 2: shown on the minimap.
    pub show_on_minimap: bool,
    /// Opcode 16: whether this element participates in the full
    /// world-map element list/interaction owner.
    pub show_on_world_map: bool,
    /// Opcode 19 `category` used by map-element trigger/filter queries.
    pub category: i32,
    /// Opcodes 9/26/27: varbit, varp and (opcode 9 only) the value range.
    pub varbit: i32,
    pub varp: i32,
    pub range: [i32; 2],
    /// Opcodes 26/27 `multime`: empty when null; last entry = default (-1 none).
    pub multime: Vec<i32>,
    /// Opcode 15 `polygon` (tile offsets, x/z pairs), fill colour, line colours and
    /// per-edge colour indices.
    pub polygon: Vec<i32>,
    pub polygon_fill: i32,
    pub line_colours: Vec<i32>,
    pub line_index: Vec<i8>,
    /// Min x, min z, max x, max z of the polygon.
    pub bounds: [i32; 4],
    /// Opcode 20: the second varbit, varp and value range.
    pub varbit2: i32,
    pub varp2: i32,
    pub range2: [i32; 2],
    /// Opcode 23: line style (dash length, gap length, dash phase).
    pub line_style: [i32; 3],
    /// Opcode 28: sprite scale percent (-1 = graphics default).
    pub scale: i32,
    /// Opcodes 29/30: the x/y alignment index (serial 0 -> index 1, 1 -> 2, 2 -> 0;
    /// default 2).
    pub align: [i32; 2],
    /// Opcode 249 `params` in file order; read by key for `mec_param`.
    pub params: Vec<(i32, crate::config::ParamValue)>,
}
impl MapElement {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut t = Self {
            sprite: -1,
            sprite2: -1,
            flash_sprite: -1,
            hover_text_colour: -1,
            drawn_on_world_map: true,
            arrow_sprite: -1,
            varbit: -1,
            varp: -1,
            range: [-1, -1],
            bounds: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
            varbit2: -1,
            varp2: -1,
            line_style: [-1, -1, -1],
            scale: -1,
            align: [2, 2],
            show_on_world_map: true,
            category: -1,
            ops: vec![None; 5],
            ..Self::default()
        };
        let id16 = |c: &mut Cursor| -> Result<i32> {
            Ok(match c.g2()? {
                65535 => -1,
                v => i32::from(v),
            })
        };
        loop {
            match c.g1()? {
                0 => break,
                1 => t.sprite = c.gsmart2or4s()?,
                2 => t.sprite2 = c.gsmart2or4s()?,
                3 => t.text = Some(c.gjstr()?),
                4 => t.text_colour = c.g3()? as i32,
                5 => t.hover_text_colour = c.g3()? as i32,
                6 => t.text_size = i32::from(c.g1()?),
                7 => {
                    let flags = c.g1()?;
                    // bit 1 clear hides it on the world map; bit 2 shows it on the minimap.
                    if flags & 1 == 0 {
                        t.drawn_on_world_map = false;
                    }
                    if flags & 2 == 2 {
                        t.show_on_minimap = true;
                    }
                }
                8 => {
                    let _ = c.g1()?;
                }
                9 => {
                    t.varbit = id16(&mut c)?;
                    t.varp = id16(&mut c)?;
                    t.range = [c.g4s()?, c.g4s()?];
                }
                op @ 10..=14 => {
                    t.ops[(op - 10) as usize] = Some(c.gjstr()?);
                }
                15 => {
                    let n = usize::from(c.g1()?);
                    t.polygon = (0..n * 2)
                        .map(|_| c.g2().map(|v| i32::from(v as i16)))
                        .collect::<Result<_>>()?;
                    t.polygon_fill = c.g4s()?;
                    let m = usize::from(c.g1()?);
                    t.line_colours = (0..m).map(|_| c.g4s()).collect::<Result<_>>()?;
                    t.line_index = (0..n).map(|_| c.g1b()).collect::<Result<_>>()?;
                }
                16 => t.show_on_world_map = false,
                17 => t.target = Some(c.gjstr()?),
                18 => t.arrow_sprite = c.gsmart2or4s()?,
                25 => t.flash_sprite = c.gsmart2or4s()?,
                19 => t.category = i32::from(c.g2()?),
                20 => {
                    t.varbit2 = id16(&mut c)?;
                    t.varp2 = id16(&mut c)?;
                    t.range2 = [c.g4s()?, c.g4s()?];
                }
                21 => t.text_outline = c.g4s()?,
                22 => t.text_fill = c.g4s()?,
                23 => t.line_style = [i32::from(c.g1()?), i32::from(c.g1()?), i32::from(c.g1()?)],
                24 => t.text_offset = [i32::from(c.g2()? as i16), i32::from(c.g2()? as i16)],
                op @ (26 | 27) => {
                    t.varbit = id16(&mut c)?;
                    t.varp = id16(&mut c)?;
                    let fallback = if op == 27 { id16(&mut c)? } else { -1 };
                    let n = usize::from(c.g1()?);
                    t.multime = (0..=n).map(|_| id16(&mut c)).collect::<Result<_>>()?;
                    t.multime.push(fallback);
                }
                28 => t.scale = i32::from(c.g1()?),
                op @ (29 | 30) => {
                    let index = match c.g1()? {
                        0 => 1,
                        1 => 2,
                        2 => 0,
                        other => anyhow::bail!("map element alignment serial {other}"),
                    };
                    t.align[(op - 29) as usize] = index;
                }
                249 => {
                    let n = c.g1()?;
                    for _ in 0..n {
                        let is_string = c.g1()? == 1;
                        let key = c.g3()? as i32;
                        let value = if is_string {
                            crate::config::ParamValue::Str(c.gjstr()?)
                        } else {
                            crate::config::ParamValue::Int(c.g4s()?)
                        };
                        t.params.push((key, value));
                    }
                }
                other => anyhow::bail!("map element opcode {other}"),
            }
        }
        // Polygon bounds (note the `else if`: a point that sets a new
        // minimum never updates the maximum on that axis).
        for pair in t.polygon.chunks(2) {
            if pair[0] < t.bounds[0] {
                t.bounds[0] = pair[0];
            } else if pair[0] > t.bounds[2] {
                t.bounds[2] = pair[0];
            }
            if pair[1] < t.bounds[1] {
                t.bounds[1] = pair[1];
            } else if pair[1] > t.bounds[3] {
                t.bounds[3] = pair[1];
            }
        }
        Ok(t)
    }
    /// Variable test over the local player's vars; `read(is_varbit, id)`.
    pub fn variable_test(&self, read: &dyn Fn(bool, i32) -> Result<i32>) -> Result<bool> {
        let value = if self.varp == -1 {
            if self.varbit == -1 {
                return Ok(true);
            }
            read(true, self.varbit)?
        } else {
            read(false, self.varp)?
        };
        if !self.multime.is_empty() {
            let last = self.multime.len() as i32 - 1;
            if value < 0 || value >= last {
                if self.multime[last as usize] == -1 {
                    return Ok(false);
                }
                if self.range[0] != -1
                    && self.range[1] != -1
                    && (value < self.range[0] || value > self.range[1])
                {
                    return Ok(false);
                }
            }
            // `multime[value]` is indexed unguarded here: an out-of-range value is an error.
            let entry = self
                .multime
                .get(value as usize)
                .copied()
                .context("map element multime index")?;
            if entry == -1 {
                return Ok(false);
            }
        } else if value < self.range[0] || value > self.range[1] {
            return Ok(false);
        }
        let value2 = if self.varp2 == -1 {
            if self.varbit2 == -1 {
                return Ok(true);
            }
            read(true, self.varbit2)?
        } else {
            read(false, self.varp2)?
        };
        Ok(value2 >= self.range2[0] && value2 <= self.range2[1])
    }
    /// The element id this one resolves to (None = no element).
    pub fn multi(&self, read: &dyn Fn(bool, i32) -> Result<i32>) -> Result<Option<i32>> {
        let value = if self.varbit != -1 {
            read(true, self.varbit)?
        } else if self.varp != -1 {
            read(false, self.varp)?
        } else {
            -1
        };
        let last = self.multime.len() - 1;
        let id = if value >= 0 && (value as usize) < last {
            self.multime[value as usize]
        } else {
            self.multime[last]
        };
        Ok((id != -1).then_some(id))
    }
}

/// The map element configs (group 36) plus the element sprite cache (group `sprite`,
/// file 0, frame 0).
#[derive(Default)]
pub struct MapElementStore {
    pub types: BTreeMap<i32, MapElement>,
    sprites: BTreeMap<i32, Option<Rc<Sprite>>>,
}
impl MapElementStore {
    pub fn load(pack: &Pack) -> Result<Self> {
        let mut s = Self::default();
        for (id, bytes) in pack
            .read_group("config", 36)
            .context("meltype config group 36")?
        {
            s.types.insert(
                id as i32,
                MapElement::decode(&bytes).with_context(|| format!("meltype {id}"))?,
            );
        }
        Ok(s)
    }
    /// `None` when there is no config file for `id`.
    pub fn get(&self, id: i32) -> Option<&MapElement> {
        self.types.get(&id)
    }
    pub fn sprite(&mut self, pack: &Pack, sprite: i32) -> Option<Rc<Sprite>> {
        if sprite == -1 {
            return None;
        }
        if let Some(s) = self.sprites.get(&sprite) {
            return s.clone();
        }
        let loaded = first_frame(pack, sprite).unwrap_or_else(|error| {
            log::warn!("[client910] map element sprite {sprite} unavailable: {error:#}");
            None
        });
        self.sprites.insert(sprite, loaded.clone());
        loaded
    }
}

/// File 0 of the sprite group, first frame.
fn first_frame(pack: &Pack, group: i32) -> Result<Option<Rc<Sprite>>> {
    let Ok(mut files) = pack.read_group("sprites", group as u32) else {
        return Ok(None);
    };
    let Some(bytes) = files.remove(&0) else {
        return Ok(None);
    };
    let mut all = Data::decode(&bytes)?;
    anyhow::ensure!(!all.is_empty(), "empty sprite group {group}");
    Ok(Some(Rc::new(Sprite::new(&all.remove(0))?)))
}
/// File 0 of the sprite group, every frame. The original applies random colour
/// jitter to the mapflag/mapdots frames (noise around the packed pixels); it is
/// replaced by its expected value 0.
pub fn sprite_frames(pack: &Pack, group: i32) -> Result<Vec<Rc<Sprite>>> {
    if group == -1 {
        return Ok(Vec::new());
    }
    let mut files = pack
        .read_group("sprites", group as u32)
        .with_context(|| format!("sprite group {group}"))?;
    let bytes = files
        .remove(&0)
        .with_context(|| format!("sprite group {group} file 0"))?;
    Data::decode(&bytes)?
        .iter()
        .map(|d| Sprite::new(d).map(Rc::new))
        .collect()
}

/// World map area metadata: one file of group 0 of the `worldmap` archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaMetadata {
    pub id: u32,
    /// The group name in the `worldmapareas` archive.
    pub name: String,
    pub map_name: String,
    /// The configured origin and the four configured bounds of the area.
    pub config_origin: i32,
    pub config_bounds: [i32; 4],
    /// The background colour of out-of-area chunks (-1 = black).
    pub background: i32,
    /// Whether the area is active.
    pub active: bool,
    /// The configured zoom (255 is normalised to zero).
    pub config_zoom: i32,
    /// Subarea records: the nine decoded values of each, in file order.
    pub subareas: Vec<[i32; 9]>,
}
impl AreaMetadata {
    pub fn decode(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let name = c.gjstr()?;
        let map_name = c.gjstr()?;
        let config_origin = c.g4s()?;
        let background = c.g4s()?;
        let active = c.g1()? == 1;
        let zoom = c.g1()?;
        let _build_area = c.g1()?;
        let n = usize::from(c.g1()?);
        let mut subareas = Vec::with_capacity(n);
        for _ in 0..n {
            let mut a = [0; 9];
            a[0] = i32::from(c.g1()?);
            for v in &mut a[1..] {
                *v = i32::from(c.g2()?);
            }
            subareas.push(a);
        }
        let mut config_bounds = [16384, 0, 16384, 0];
        for a in &subareas {
            config_bounds[0] = config_bounds[0].min(a[5]);
            config_bounds[1] = config_bounds[1].max(a[7]);
            config_bounds[2] = config_bounds[2].min(a[6]);
            config_bounds[3] = config_bounds[3].max(a[8]);
        }
        Ok(Self {
            id,
            name,
            map_name,
            config_origin,
            config_bounds,
            background,
            active,
            config_zoom: if zoom == 255 { 0 } else { i32::from(zoom) },
            subareas,
        })
    }
    /// Whether any subarea contains the tile.
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.subareas
            .iter()
            .any(|a| x >= a[1] && x <= a[3] && z >= a[2] && z <= a[4])
    }
}
/// Every file of group 0.
pub fn load_area_metadata(pack: &Pack) -> Result<Vec<AreaMetadata>> {
    pack.read_group("worldmap", 0)
        .context("worldmap group 0")?
        .into_iter()
        .map(|(id, bytes)| {
            AreaMetadata::decode(id, &bytes).with_context(|| format!("world map area {id}"))
        })
        .collect()
}
/// The first active area containing the tile. The original walks its areas (a hash
/// table keyed by id) in bucket order; file order is used here (`TODO(#gap-worldmap-hash-order)`: only matters
/// when two active areas overlap the same tile).
pub fn area_for(areas: &[AreaMetadata], x: i32, z: i32) -> Option<&AreaMetadata> {
    areas.iter().find(|a| a.active && a.contains(x, z))
}

/// The map elements of one area: packed coords (`level << 28 | x << 14 | z`) and
/// element ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorldMapElements {
    pub coords: Vec<i32>,
    pub elements: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct WorldMapLabel {
    pub text: String,
    pub colour: i32,
    pub size: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldMapPolygon {
    /// Display-coordinate tile points; the UI owner applies the current map
    /// center and zoom when painting them.
    pub points: Vec<[i32; 2]>,
    pub fill: i32,
    pub lines: Vec<WorldMapLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldMapLine {
    pub from: [i32; 2],
    pub to: [i32; 2],
    pub colour: i32,
    /// Dash length.
    /// Values <= 0 use the one-pixel solid line path.
    pub dash: i32,
    /// Gap length.
    pub gap: i32,
    /// Dash phase.
    pub phase: i32,
}

impl WorldMapElements {
    /// Reads the `worldmapareas` archive: file 1 of the group named
    /// `name`; a missing group or file is the empty list.
    pub fn load(pack: &Pack, name: &str, members: bool) -> Result<Self> {
        let Some(group) = pack.group_id_by_name("worldmapareas", name)? else {
            return Ok(Self::default());
        };
        let mut files = pack.read_group("worldmapareas", group)?;
        let Some(bytes) = files.remove(&1) else {
            return Ok(Self::default());
        };
        let mut c = Cursor::new(&bytes);
        let count = usize::from(c.g2()?);
        let mut out = Self::default();
        for _ in 0..count {
            let coord = c.g4s()?;
            let element = i32::from(c.g2()?);
            let members_only = c.g1()?;
            if !members && members_only == 1 {
                continue;
            }
            out.coords.push(coord);
            out.elements.push(element);
        }
        Ok(out)
    }
}

/// Sine and cosine table entries for `angle` (`trig.rs`).
pub fn trig1(angle: i32) -> (i32, i32) {
    (crate::trig::sin(angle), crate::trig::cos(angle))
}

/// Whether a tile of `level` is drawn when the player is on `player_level`, over the
/// CPU terrain flags.
pub fn tile_visible(
    terrain: &crate::protocol910::terrain::Terrain,
    player_level: i32,
    level: i32,
    x: i32,
    z: i32,
) -> bool {
    let flags = |l: i32| -> i32 {
        i32::from(terrain.tiles[terrain.tile(l as usize, x as usize, z as usize)].flags)
    };
    if flags(0) & 0x2 != 0 {
        return true;
    }
    if flags(level) & 0x10 != 0 {
        return false;
    }
    let effective = if flags(level) & 0x8 == 0 {
        if level <= 0 || flags(1) & 0x2 == 0 {
            level
        } else {
            level - 1
        }
    } else {
        0
    };
    effective == player_level
}
/// Whether the level-1 tile at (x, z) has the bridge flag (0x2).
pub fn link_below(terrain: &crate::protocol910::terrain::Terrain, x: i32, z: i32) -> bool {
    x >= 0
        && z >= 0
        && (x as usize) < terrain.width
        && (z as usize) < terrain.height
        && terrain.tiles[terrain.tile(1, x as usize, z as usize)].flags & 0x2 != 0
}

/// The GPU base sprite: a registered external texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub texture: u64,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintArrow {
    pub hint_type: u8,
    pub sprite: u8,
    pub npc_index: Option<usize>,
    pub player_index: Option<usize>,
    pub fine: Option<[i32; 2]>,
    pub level: Option<i32>,
    pub distance_tiles: i32,
    /// NPC target blink rate (0 = steady).
    pub blink: i32,
    /// Tile target height (`g1 << 2`), drawn at `* 2` above ground.
    pub height: i32,
    /// The 3D arrow model drawn around the local player, -1 for none.
    pub model: i32,
}

/// `(level, layer, x, z)` -> the `(loc id, shape)` a runtime loc replacement left in
/// the live scene (`None`: removed), for slots whose occupant differs from the scene
/// graph's own loc.
pub type LocOverrides = std::collections::HashMap<(i32, i32, i32, i32), Option<(u32, i32)>>;

/// The minimap's flag and toggle: logic state, owned by the client core
/// (`ClientCore::minimap`, lane E-A1). A walk click sets the flag in the logic, the
/// scene redraw's arrival test clears it; the minimap draw only reads it
/// ([`OverlayInput::flag`], [`Frame::toggle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinimapFlag {
    /// `MINIMAP_TOGGLE` (2/5 draw black, >= 3 hides the compass).
    pub toggle: i32,
    /// The flag tile x/z (-1 = none) and the map-flag switch.
    pub flag: [i32; 2],
    pub map_flag: bool,
}
impl Default for MinimapFlag {
    fn default() -> Self {
        Self {
            toggle: 0,
            flag: [-1, -1],
            map_flag: true,
        }
    }
}
impl MinimapFlag {
    /// A walk click's target tile, drawn with the first mapflag sprite.
    pub fn set(&mut self, tile: [i32; 2]) {
        self.flag = tile;
        self.map_flag = false;
    }
    /// The flag clears when the local player's tile (`trans - (size - 1) * 256 >> 9`) reaches it.
    pub fn arrive(&mut self, fine: [i32; 2], size: i32) {
        let half = (size - 1) * 256;
        if (fine[0] - half) >> 9 == self.flag[0] && (fine[1] - half) >> 9 == self.flag[1] {
            self.flag = [-1, -1];
        }
    }
}

/// The minimap's static state.
pub struct Minimap {
    /// The level the base sprite was last built for.
    pub cached_level: i32,
    /// The base sprite.
    pub base: Option<Base>,
    /// Per 8x8 tile block: force the ground level.
    pub force_ground_level: Vec<Vec<bool>>,
    /// Minimap zoom and the anti-cheat angle offset.
    pub zoom: i32,
    pub anticheat_angle: i32,
    pub msi: MsiStore,
    pub locs: Option<LocStore>,
    /// The NPC configs, as the NPC overlay and the transmog check read them.
    pub npcs: Option<NpcStore>,
    pub elements: MapElementStore,
    pub defaults: Option<crate::avatar::GraphicsDefaults>,
    /// The compass sprite.
    pub compass: Option<Rc<Sprite>>,
    /// The map dot and map flag sprites.
    pub mapdots: Vec<Rc<Sprite>>,
    pub mapflag: Vec<Rc<Sprite>>,
    /// The HINT_ARROW sprite range check and the in-world tile/NPC/player arrows.
    pub hintarrows: Vec<Rc<Sprite>>,
    /// The minimap hint-arrow sprites.
    pub hintarrow_minimap: Vec<Rc<Sprite>>,
    /// The sprites used for offscreen/rotated hint arrows.
    pub hintarrow_edges: Vec<Rc<Sprite>>,
    pub hint_arrow_state: [Option<HintArrow>; 9],
    /// Scene base the tile targets' offsets are relative to; a new base shifts them
    /// by the base delta.
    pub hint_arrow_base: Option<[i32; 2]>,
    pub pack: Option<Pack>,
    /// The world map area metadata list.
    pub areas: Vec<AreaMetadata>,
    /// The current area's map elements, keyed by the scene base they were loaded for.
    pub world_map: Option<([i32; 2], WorldMapElements)>,
    /// Whether the account is a member, as the loc queue and the map-element load read it.
    pub logged_in_members: bool,
    /// Queued map elements (indices into `world_map`).
    pub queued_elements: Vec<usize>,
    /// Queued loc ids with their x/z.
    pub queued_locs: Vec<(u32, i32, i32)>,
    /// Bounded diagnostics for the overlay gaps.
    pub skipped_overlays: BTreeMap<&'static str, usize>,
}
impl Default for Minimap {
    fn default() -> Self {
        Self {
            cached_level: -1,
            base: None,
            force_ground_level: Vec::new(),
            zoom: 0,
            anticheat_angle: 0,
            msi: MsiStore::default(),
            locs: None,
            npcs: None,
            elements: MapElementStore::default(),
            defaults: None,
            compass: None,
            mapdots: Vec::new(),
            mapflag: Vec::new(),
            hintarrows: Vec::new(),
            hintarrow_minimap: Vec::new(),
            hintarrow_edges: Vec::new(),
            hint_arrow_state: std::array::from_fn(|_| None),
            hint_arrow_base: None,
            pack: None,
            areas: Vec::new(),
            world_map: None,
            logged_in_members: false,
            queued_elements: Vec::new(),
            queued_locs: Vec::new(),
            skipped_overlays: BTreeMap::new(),
        }
    }
}

/// One 2D wall/loc mark on the base sprite, in base pixels.
#[derive(Clone, Debug)]
pub enum Mark {
    /// A filled rectangle.
    Fill { rect: [i32; 4], colour: i32 },
    /// A line (the diagonal walls).
    Line {
        from: [i32; 2],
        to: [i32; 2],
        colour: i32,
    },
    /// A tinted, scaled map scene icon.
    Icon { sprite: Rc<Sprite>, rect: [i32; 4] },
    /// A filled rectangle with the additive blend.
    Add { rect: [i32; 4], colour: i32 },
}

/// The colours of the wall marks of the base map: ordinary walls and walls
/// of active (interactive) locs.
#[derive(Clone, Copy)]
struct MarkColours {
    wall: i32,
    active: i32,
}

/// What the base-sprite rebuild needs the GPU to draw: per level the
/// tile indices (`z * tiles_x + x`) whose floor is visible, then the marks.
pub struct BasePlan {
    pub size: u32,
    /// The lowest level drawn (forced ground levels can pull it to 0).
    pub first_level: i32,
    pub level_tiles: Vec<(usize, Vec<usize>)>,
    pub marks: Vec<Mark>,
}

impl Minimap {
    /// Loads the configs and sprites the minimap reads.
    pub fn install(&mut self, pack: &Pack) {
        self.pack = Some(pack.clone());
        match MsiStore::load(pack) {
            Ok(s) => self.msi = s,
            Err(error) => log::warn!("[client910] msi types unavailable: {error:#}"),
        }
        match LocStore::load(pack) {
            Ok(s) => self.locs = Some(s),
            Err(error) => log::warn!("[client910] minimap loc types unavailable: {error:#}"),
        }
        match NpcStore::load(pack) {
            Ok(s) => self.npcs = Some(s),
            Err(error) => log::warn!("[client910] minimap npc types unavailable: {error:#}"),
        }
        match MapElementStore::load(pack) {
            Ok(s) => self.elements = s,
            Err(error) => log::warn!("[client910] map element types unavailable: {error:#}"),
        }
        match load_area_metadata(pack) {
            Ok(a) => self.areas = a,
            Err(error) => log::warn!("[client910] world map areas unavailable: {error:#}"),
        }
        match crate::avatar::GraphicsDefaults::load(pack) {
            Ok(d) => {
                // The compass group's first frame.
                if d.compass != -1 {
                    self.compass = first_frame(pack, d.compass).unwrap_or_else(|error| {
                        log::warn!("[client910] compass sprite unavailable: {error:#}");
                        None
                    });
                }
                // Every frame of mapflag / mapdots.
                self.mapflag = sprite_frames(pack, d.mapflag).unwrap_or_else(|error| {
                    log::warn!("[client910] mapflag sprites unavailable: {error:#}");
                    Vec::new()
                });
                self.mapdots = sprite_frames(pack, d.mapdots).unwrap_or_else(|error| {
                    log::warn!("[client910] mapdots sprites unavailable: {error:#}");
                    Vec::new()
                });
                self.hintarrows = sprite_frames(pack, d.hintarrows).unwrap_or_else(|error| {
                    log::warn!("[client910] hint-arrow sprites unavailable: {error:#}");
                    Vec::new()
                });
                self.hintarrow_minimap =
                    sprite_frames(pack, d.hintarrow_minimap).unwrap_or_else(|error| {
                        log::warn!("[client910] minimap hint-arrow sprites unavailable: {error:#}");
                        Vec::new()
                    });
                self.hintarrow_edges =
                    sprite_frames(pack, d.hintarrow_edges).unwrap_or_else(|error| {
                        log::warn!("[client910] edge hint-arrow sprites unavailable: {error:#}");
                        Vec::new()
                    });
                self.defaults = Some(d);
            }
            Err(error) => {
                log::warn!("[client910] graphics defaults unavailable for the minimap: {error:#}")
            }
        }
    }
    /// Apply one `HINT_ARROW` packet. The
    /// packet owns nine slots; type 0 clears a slot, types 1/10 target an NPC
    /// or player index, and types 2..6 normalize to a tile target.
    /// A rebuild at a new base moves every retained
    /// hint's tile offset by the base delta.
    pub fn rebase_hint_arrows(&mut self, base: [i32; 2]) {
        let Some(old) = self.hint_arrow_base.replace(base) else {
            return;
        };
        let delta = [base[0] - old[0], base[1] - old[1]];
        if delta == [0, 0] {
            return;
        }
        for arrow in self.hint_arrow_state.iter_mut().flatten() {
            if let Some(fine) = arrow.fine.as_mut() {
                fine[0] -= delta[0] * 512;
                fine[1] -= delta[1] * 512;
            }
        }
    }
    pub fn apply_hint_arrow(&mut self, bytes: &[u8], base: [i32; 2]) -> Result<()> {
        anyhow::ensure!(bytes.len() == 14, "HINT_ARROW length");
        self.rebase_hint_arrows(base);
        let mut c = Cursor::new(bytes);
        let header = c.g1()?;
        let slot = usize::from(header >> 5);
        anyhow::ensure!(slot < self.hint_arrow_state.len(), "HINT_ARROW slot");
        let mut hint_type = header & 0x1f;
        if hint_type == 0 {
            self.hint_arrow_state[slot] = None;
            return Ok(());
        }
        let sprite = c.g1()?;
        // An out-of-range sprite leaves the slot untouched.
        if usize::from(sprite) >= self.hintarrows.len() {
            return Ok(());
        }
        let mut arrow = HintArrow {
            hint_type,
            sprite,
            npc_index: None,
            player_index: None,
            fine: None,
            level: None,
            distance_tiles: 0,
            blink: 0,
            height: 0,
            model: -1,
        };
        if hint_type == 1 || hint_type == 10 {
            let target = usize::from(c.g2()?);
            if hint_type == 1 {
                arrow.npc_index = Some(target);
            } else {
                arrow.player_index = Some(target);
            }
            arrow.blink = i32::from(c.g2()?);
            let _ = c.g4s()?;
            arrow.model = c.g4s()?;
        } else if (2..=6).contains(&hint_type) {
            let [offset_x, offset_z] = match hint_type {
                2 => [256, 256],
                3 => [0, 256],
                4 => [512, 256],
                5 => [256, 0],
                _ => [256, 512],
            };
            hint_type = 2;
            let level = c.g1()?;
            let x = i32::from(c.g2()?) - base[0];
            let z = i32::from(c.g2()?) - base[1];
            arrow.height = i32::from(c.g1()?) << 2;
            let distance_tiles = i32::from(c.g2()?);
            arrow.model = c.g4s()?;
            arrow.hint_type = hint_type;
            arrow.fine = Some([x * 512 + offset_x, z * 512 + offset_z]);
            arrow.distance_tiles = distance_tiles;
            // The minimap has one level owner; retain the decoded level for
            // the overlay filter without widening the public dot structs.
            arrow.level = Some(i32::from(level));
        } else {
            // Other types read only the model id; the fixed-size
            // packet's remaining bytes are skipped. The marker is stored
            // but no consumer draws these types.
            arrow.model = c.g4s()?;
            let _ = c.g4s()?;
            let _ = c.g4s()?;
        }
        anyhow::ensure!(c.remaining() == 0, "HINT_ARROW trailing bytes");
        self.hint_arrow_state[slot] = Some(arrow);
        Ok(())
    }
    /// A world rebuild drops the base sprite. The old scene's map elements go with it
    /// (they are reloaded for the new base).
    pub fn rebuild(&mut self) {
        self.base = None;
        self.cached_level = -1;
        self.world_map = None;
        self.queued_elements.clear();
        self.queued_locs.clear();
    }
    /// Loads the map elements of the world-map area containing
    /// `(size_x / 2 + base x, size_x / 2 + base z)` (note both use `size_x`), once per scene.
    pub fn ensure_world_map(&mut self, base: [i32; 2], size_x: i32) {
        if self.world_map.as_ref().is_some_and(|(b, _)| *b == base) {
            return;
        }
        let Some(pack) = self.pack.clone() else {
            return;
        };
        let elements = match area_for(&self.areas, size_x / 2 + base[0], size_x / 2 + base[1]) {
            Some(area) => WorldMapElements::load(&pack, &area.name, self.logged_in_members)
                .unwrap_or_else(|error| {
                    log::warn!(
                        "[client910] world map elements for {:?} unavailable: {error:#}",
                        area.name
                    );
                    WorldMapElements::default()
                }),
            None => WorldMapElements::default(),
        };
        self.world_map = Some((base, elements));
    }
    /// Queues, after the loc refresh, the map elements
    /// of the player's level inside the scene (or whose polygon reaches it).
    pub fn queue_map_elements(&mut self, base: [i32; 2], size_x: i32, size_z: i32, level: i32) {
        self.queued_elements.clear();
        let Some((_, world_map)) = self.world_map.as_ref() else {
            return;
        };
        for (i, &coord) in world_map.coords.iter().enumerate() {
            if coord >> 28 != level {
                continue;
            }
            let x = (coord >> 14 & 0x3FFF) - base[0];
            let z = (coord & 0x3FFF) - base[1];
            if x >= 0 && x < size_x && z >= 0 && z < size_z {
                self.queued_elements.push(i);
            } else if let Some(e) = self.elements.get(world_map.elements[i]) {
                // An absent element decodes to the defaults, which have no polygon;
                // it is skipped either way.
                if !e.polygon.is_empty()
                    && e.bounds[2] + x >= 0
                    && e.bounds[0] + x < size_x
                    && e.bounds[3] + z >= 0
                    && e.bounds[1] + z < size_z
                {
                    self.queued_elements.push(i);
                }
            }
        }
    }
    /// The base-sprite rebuild waits until every visible wall, primary entity and ground
    /// decoration map-scene icon can be resolved before replacing the base
    /// sprite; keep that wait on the same MSI/scene owners used by the marks.
    pub fn are_loc_icons_ready(
        &mut self,
        scene: &Scene,
        terrain: &crate::protocol910::terrain::Terrain,
        level: i32,
        player_tile: Option<[i32; 2]>,
    ) -> bool {
        let Some(pack) = self.pack.clone() else {
            return true;
        };
        let size_x = terrain.width as i32;
        let size_z = terrain.height as i32;
        let first = self.first_level(level, player_tile);
        for l in first..=3 {
            for x in 0..size_x {
                for z in 0..size_z {
                    if !(l < level || tile_visible(terrain, level, l, x, z)) {
                        continue;
                    }
                    let loc_level = if link_below(terrain, x, z) { l - 1 } else { l };
                    if loc_level < 0 {
                        continue;
                    }
                    let Some(tile) = scene.tile(loc_level as usize, x as usize, z as usize) else {
                        continue;
                    };
                    let ids = [
                        tile.wall
                            .and_then(|i| scene.walls.get(i))
                            .map(|loc| loc.loc_id),
                        primary_loc(scene, tile, x, z).map(|loc| loc.loc_id),
                        tile.ground_decoration
                            .and_then(|i| scene.ground_decors.get(i))
                            .map(|loc| loc.loc_id),
                    ];
                    if ids
                        .into_iter()
                        .flatten()
                        .any(|id| !self.map_icon_ready(&pack, id))
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
    fn map_icon_ready(&mut self, pack: &Pack, id: u32) -> bool {
        let Some(loc) = self.locs.as_ref().and_then(|locs| locs.get(id)) else {
            return true;
        };
        if loc.mapsceneicon == -1 {
            return true;
        }
        let msi = self.msi.get(loc.mapsceneicon);
        if msi.sprite == -1 {
            return true;
        }
        self.msi.sprite(pack, loc.mapsceneicon, 0, false).is_some()
    }
    /// Queues every loc with a map element, one per tile and level in the order
    /// ground decoration, entity, wall, wall decoration (the `&&` chain stops
    /// at the first queued loc). `replaced` holds the scene's runtime-replaced
    /// occupants that the graph does not hold.
    pub fn refresh(
        &mut self,
        scene: &Scene,
        terrain: &crate::protocol910::terrain::Terrain,
        level: i32,
        player_tile: Option<[i32; 2]>,
        replaced: &LocOverrides,
    ) {
        self.queued_locs.clear();
        let size_x = terrain.width as i32;
        let size_z = terrain.height as i32;
        let first = self.first_level(level, player_tile);
        for x in 0..size_x {
            for z in 0..size_z {
                let mut l = first;
                while l <= level + 1 && l <= 3 {
                    if l < level || tile_visible(terrain, level, l, x, z) {
                        let tile = scene.tile(l as usize, x as usize, z as usize);
                        let ground = tile
                            .and_then(|t| t.ground_decoration)
                            .and_then(|i| scene.ground_decors.get(i))
                            .map(|g| g.loc_id);
                        let entity = tile
                            .and_then(|t| primary_loc(scene, t, x, z))
                            .map(|s| s.loc_id);
                        let wall = tile
                            .and_then(|t| t.wall)
                            .and_then(|i| scene.walls.get(i))
                            .map(|w| w.loc_id);
                        let decor = tile
                            .and_then(|t| t.wall_decoration)
                            .and_then(|i| scene.wall_decors.get(i))
                            .map(|d| d.loc_id);
                        // Layers 3, 2, 0, 1 (ground decoration, entity, wall, wall
                        // decoration);
                        // a replacement may sit on a tile the graph left empty.
                        let occupant =
                            |layer: i32, own: Option<u32>| match replaced.get(&(l, layer, x, z)) {
                                Some(&Some((id, shape))) => {
                                    (layer != 2 || self.primary_layer(id, shape)).then_some(id)
                                }
                                Some(None) => None,
                                None => own,
                            };
                        let ids = [
                            occupant(3, ground),
                            occupant(2, entity),
                            occupant(0, wall),
                            occupant(1, decor),
                        ];
                        for id in ids {
                            if self.queue_loc_if_mappable(id, x, z) {
                                break;
                            }
                        }
                    }
                    l += 1;
                }
            }
        }
    }
    /// Whether a replaced occupant counts as a primary-layer entity: a centrepiece
    /// (shape 10/11) is primary unless its config layer is 2; every other shape
    /// (roofs, diagonal walls) always is.
    fn primary_layer(&self, id: u32, shape: i32) -> bool {
        !(shape == 10 || shape == 11)
            || self
                .locs
                .as_ref()
                .and_then(|locs| locs.get(id))
                .is_none_or(|loc| loc.layer != 2)
    }
    /// Queues the loc if it (or one of its multiloc targets) has a map element.
    fn queue_loc_if_mappable(&mut self, id: Option<u32>, x: i32, z: i32) -> bool {
        let Some(id) = id else { return false };
        let Some(locs) = self.locs.as_ref() else {
            return false;
        };
        let Some(loc) = locs.get(id) else {
            return false;
        };
        if loc.members && !self.logged_in_members {
            return false;
        }
        let mut element = loc.mapelement;
        for &m in &loc.multiloc {
            if m != -1 {
                if let Some(other) = locs.get(m as u32) {
                    if other.mapelement >= 0 {
                        element = other.mapelement;
                    }
                }
            }
        }
        if element < 0 {
            return false;
        }
        self.queued_locs.push((id, x, z));
        true
    }
    /// Multiloc/multinpc index selection over the local player's vars: `None` = no target.
    fn multi_index(
        multivarbit: i32,
        multivarp: i32,
        list: &[i32],
        read: &dyn Fn(bool, i32) -> Result<i32>,
    ) -> Result<Option<i32>> {
        let value = if multivarbit != -1 {
            read(true, multivarbit)?
        } else if multivarp != -1 {
            read(false, multivarp)?
        } else {
            -1
        };
        let last = list.len() - 1;
        let id = if value >= 0 && (value as usize) < last {
            list[value as usize]
        } else {
            list[last]
        };
        Ok((id != -1).then_some(id))
    }
    /// Overlay resolution for the per-frame draw (everything but the draw itself):
    /// map elements, loc icons, obj stacks, NPC and player dots, the flag and
    /// hint arrows, including their edge/rotated presentation metadata.
    pub fn overlays(&mut self, input: &OverlayInput<'_>) -> Vec<Overlay> {
        let mut out = Vec::new();
        let read = input.read;
        let px = input.player[0] / 128;
        let pz = input.player[1] / 128;
        let Some(pack) = self.pack.clone() else {
            return out;
        };
        // Map elements.
        let queued = self.queued_elements.clone();
        if let Some((_, world_map)) = self.world_map.clone() {
            for i in queued {
                let coord = world_map.coords[i];
                let x = (coord >> 14 & 0x3FFF) - input.base[0];
                let z = (coord & 0x3FFF) - input.base[1];
                self.map_element(
                    &pack,
                    [x * 4 + 2 - px, z * 4 + 2 - pz],
                    world_map.elements[i],
                    read,
                    &mut out,
                );
            }
        }
        // Loc icons (multilocs resolved against the player's vars).
        let locs_queued = self.queued_locs.clone();
        for (id, x, z) in locs_queued {
            let Some(loc) = self.locs.as_ref().and_then(|l| l.get(id)) else {
                continue;
            };
            let element = if loc.has_multiloc {
                match Self::multi_index(loc.multivarbit, loc.multivarp, &loc.multiloc, read) {
                    Ok(Some(m)) => match self.locs.as_ref().and_then(|l| l.get(m as u32)) {
                        Some(other) if other.mapelement != -1 => other.mapelement,
                        _ => continue,
                    },
                    Ok(None) => continue,
                    Err(error) => {
                        Self::note(&mut self.skipped_overlays, "multiloc vars", &error);
                        continue;
                    }
                }
            } else {
                loc.mapelement
            };
            self.map_element(
                &pack,
                [x * 4 + 2 - px, z * 4 + 2 - pz],
                element,
                read,
                &mut out,
            );
        }
        // Obj stacks on the cached level (mapdots frame 0).
        if let Some(dot) = self.mapdots.first().cloned() {
            for &key in input.stacks {
                if (key >> 28 & 0x3) as i32 != self.cached_level {
                    continue;
                }
                let x = (key & 0x3FFF) as i32 - input.base[0];
                let z = (key >> 14 & 0x3FFF) as i32 - input.base[1];
                out.push(Overlay::sprite(
                    [x * 4 + 2 - px, z * 4 + 2 - pz],
                    dot.clone(),
                ));
            }
        }
        // NPCs.
        for npc in input.npcs {
            if npc.level != input.level {
                continue;
            }
            let Some(store) = self.npcs.as_ref() else {
                break;
            };
            let mut t = store.get(npc.type_id as u32);
            if let Some(ty) = t {
                if !ty.multinpc.is_empty() {
                    t = match Self::multi_index(ty.multivarbit, ty.multivarp, &ty.multinpc, read) {
                        Ok(Some(m)) => store.get(m as u32),
                        Ok(None) => None,
                        Err(error) => {
                            Self::note(&mut self.skipped_overlays, "multinpc vars", &error);
                            None
                        }
                    };
                }
            }
            let Some(ty) = t else { continue };
            if !(ty.minimap && ty.active) {
                continue;
            }
            let d = [npc.fine[0] / 128 - px, npc.fine[1] / 128 - pz];
            if ty.mapelement == -1 {
                if let Some(dot) = self.mapdots.get(1).cloned() {
                    out.push(Overlay::sprite(d, dot));
                }
            } else {
                let element = ty.mapelement;
                self.map_element(&pack, d, element, read, &mut out);
            }
        }
        // Other players.
        for p in input.players {
            if !p.has_model || p.hidden || p.level != input.level {
                continue;
            }
            let d = [p.fine[0] / 128 - px, p.fine[1] / 128 - pz];
            let same_team = input.local_team != 0 && p.team != 0 && input.local_team == p.team;
            let fake_npc = p.transmog_npc != -1
                && self
                    .npcs
                    .as_ref()
                    .and_then(|n| n.get(p.transmog_npc as u32))
                    .is_some_and(|t| t.transmogfakenpc);
            // Friends and clan chat are resolved by the
            // retained social owner before this overlay list is built.
            let (friend, clan) = (p.friend, p.clan);
            let frame = if fake_npc {
                1
            } else if p.partner == 1 {
                8
            } else if p.partner == 2 {
                6
            } else if same_team {
                4
            } else if friend {
                3
            } else if p.suppress_partner {
                7
            } else if clan {
                5
            } else {
                2
            };
            if let Some(dot) = self.mapdots.get(frame).cloned() {
                out.push(Overlay::sprite(d, dot));
            }
        }
        // Hint arrows blink for ten of every twenty
        // logic cycles and resolves NPC/player targets through live slots.
        if input.cycle % 20 < 10 {
            for arrow in self.hint_arrow_state.iter().flatten() {
                let target = if arrow.hint_type == 1 {
                    arrow.npc_index.and_then(|id| {
                        input
                            .npcs
                            .iter()
                            .find(|npc| npc.index == id)
                            .map(|npc| (npc.level, npc.fine))
                    })
                } else if arrow.hint_type == 10 {
                    arrow.player_index.and_then(|id| {
                        input
                            .players
                            .iter()
                            .find(|player| player.index == id)
                            .map(|player| (player.level, player.fine))
                    })
                } else {
                    arrow
                        .fine
                        .map(|fine| (arrow.level.unwrap_or(input.level), fine))
                };
                let Some((level, fine)) = target else {
                    continue;
                };
                if level != input.level {
                    continue;
                }
                let d = [fine[0] / 128 - px, fine[1] / 128 - pz];
                if arrow.distance_tiles > 0 {
                    let max = arrow.distance_tiles.saturating_mul(128);
                    if d[0]
                        .saturating_mul(d[0])
                        .saturating_add(d[1].saturating_mul(d[1]))
                        > max.saturating_mul(max)
                    {
                        continue;
                    }
                }
                if let Some(sprite) = self
                    .hintarrow_minimap
                    .get(usize::from(arrow.sprite))
                    .cloned()
                {
                    let mut overlay = Overlay::sprite(d, sprite);
                    overlay.edge_arrow = true;
                    overlay.edge_sprite =
                        self.hintarrow_edges.get(usize::from(arrow.sprite)).cloned();
                    out.push(overlay);
                }
            }
        }
        // The map flag (offset by the graphics-defaults draw offsets).
        let flag = input.flag;
        if flag.flag[0] != -1 {
            if let Some(sprite) = self.mapflag.get(usize::from(flag.map_flag)).cloned() {
                let offset = self
                    .defaults
                    .as_ref()
                    .map(|d| d.mapflag_offset)
                    .unwrap_or([0, 0]);
                let grow = (input.local_size - 1) * 2;
                out.push(Overlay {
                    d: [
                        flag.flag[0] * 4 + 2 - px + grow,
                        flag.flag[1] * 4 + 2 - pz + grow,
                    ],
                    sprite,
                    polygon: None,
                    draw_sprite: true,
                    label: None,
                    scale: 100.0,
                    align: [1, 1],
                    offset: [-offset[0], -offset[1]],
                    edge_arrow: false,
                    edge_sprite: None,
                });
            }
        }
        out
    }
    /// One map element minus the draw: multi/variable resolution and the sprite,
    /// polygon and label overlays are retained for the backend consumer.
    fn map_element(
        &mut self,
        pack: &Pack,
        d: [i32; 2],
        element: i32,
        read: &dyn Fn(bool, i32) -> Result<i32>,
        out: &mut Vec<Overlay>,
    ) {
        let Some(mut e) = self.elements.get(element).cloned() else {
            return;
        };
        if !e.multime.is_empty() {
            match e.variable_test(read) {
                Ok(true) => match e.multi(read) {
                    Ok(Some(id)) => match self.elements.get(id).cloned() {
                        Some(m) => e = m,
                        None => return,
                    },
                    Ok(None) => return,
                    Err(error) => {
                        Self::note(&mut self.skipped_overlays, "multime vars", &error);
                        return;
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    Self::note(&mut self.skipped_overlays, "element vars", &error);
                    return;
                }
            }
        }
        if !e.show_on_minimap {
            return;
        }
        match e.variable_test(read) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                Self::note(&mut self.skipped_overlays, "element vars", &error);
                return;
            }
        }
        let polygon = (e.polygon.len() >= 6).then(|| {
            let points: Vec<[i32; 2]> = e
                .polygon
                .chunks_exact(2)
                .map(|pair| [d[0] + pair[0] * 4, d[1] + pair[1] * 4])
                .collect();
            let lines = points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .take(points.len())
                .enumerate()
                .map(|(edge, (from, to))| {
                    let colour = e
                        .line_index
                        .get(edge)
                        .map(|&index| usize::from(index as u8))
                        .and_then(|index| e.line_colours.get(index))
                        .copied()
                        .or_else(|| e.line_colours.first().copied())
                        .unwrap_or(0);
                    WorldMapLine {
                        from: *from,
                        to: *to,
                        colour,
                        dash: e.line_style[0],
                        gap: e.line_style[1],
                        phase: e.line_style[2],
                    }
                })
                .collect();
            WorldMapPolygon {
                points,
                fill: e.polygon_fill,
                lines,
            }
        });
        let sprite = (e.sprite != -1)
            .then(|| self.elements.sprite(pack, e.sprite))
            .flatten();
        let label = e.text.as_ref().map(|text| WorldMapLabel {
            text: text.clone(),
            colour: e.text_colour,
            size: e.text_size,
        });
        if polygon.is_some() || sprite.is_some() || label.is_some() {
            let draw_sprite = sprite.is_some();
            let sprite = sprite.unwrap_or_else(|| {
                Rc::new(Sprite {
                    paletted: None,
                    size: [1, 1],
                    padding: [0; 4],
                    argb: vec![0],
                })
            });
            let scale = if e.scale > 0 {
                e.scale
            } else {
                self.defaults
                    .as_ref()
                    .map(|d| d.map_element_scale)
                    .unwrap_or(100)
            };
            out.push(Overlay {
                d,
                sprite,
                polygon,
                draw_sprite,
                label,
                scale: f64::from(scale),
                align: e.align,
                offset: [0, 0],
                edge_arrow: false,
                edge_sprite: None,
            });
        }
    }
    fn note(
        skipped: &mut BTreeMap<&'static str, usize>,
        what: &'static str,
        error: &anyhow::Error,
    ) {
        let n = skipped.entry(what).or_default();
        if *n == 0 {
            log::warn!("[client910] minimap {what}: {error:#}");
        }
        *n += 1;
    }
    /// Resets the per-block forced-ground table for a scene of this size.
    pub fn init_force_ground_level(&mut self, size_x: usize, size_z: usize) {
        self.force_ground_level = vec![vec![false; size_z >> 3]; size_x >> 3];
    }
    /// The lowest level to draw, pulled to 0 when the player's 8x8 block forces ground.
    fn first_level(&self, level: i32, player_tile: Option<[i32; 2]>) -> i32 {
        if let Some([tx, tz]) = player_tile {
            let (bx, bz) = (tx >> 3, tz >> 3);
            if bx >= 0
                && (bx as usize) < self.force_ground_level.len()
                && bz >= 0
                && (bz as usize) < self.force_ground_level[bx as usize].len()
                && self.force_ground_level[bx as usize][bz as usize]
            {
                return 0;
            }
        }
        level
    }
    /// The base-sprite rebuild minus the GPU work: which floor tiles each level
    /// contributes and the wall/loc marks. `size` is `size_x * 4 + 96`.
    /// The random noise of the original is replaced by its expected value, as for the
    /// wall colours; the chunk noise is the last mark. Every chunk adds the same noise
    /// value over exactly the rectangle it copies into the base sprite, so the
    /// whole-map form is one additive fill over the tile area.
    pub fn base_plan(
        &mut self,
        scene: &Scene,
        terrain: &crate::protocol910::terrain::Terrain,
        level: i32,
        player_tile: Option<[i32; 2]>,
    ) -> BasePlan {
        let size_x = terrain.width as i32;
        let size_z = terrain.height as i32;
        let size = (size_x * 4 + 48 + 48) as u32;
        let first = self.first_level(level, player_tile);
        let mut level_tiles = Vec::new();
        let mut marks = Vec::new();
        // Wall colour with the random channel value at its expectation (0.5): 238 per
        // channel; active walls use the red variant.
        let colours = MarkColours {
            wall: 0xFF000000u32 as i32 | (238 << 16) | (238 << 8) | 238,
            active: (238 | 0xFF00) << 16,
        };
        for l in first..=3 {
            let mut tiles = Vec::new();
            for x in 0..size_x {
                for z in 0..size_z {
                    let visible = l < level || tile_visible(terrain, level, l, x, z);
                    if !visible {
                        continue;
                    }
                    tiles.push((z as usize) * (size_x as usize) + x as usize);
                    // The loc level steps down under a link-below tile.
                    let loc_level = if link_below(terrain, x, z) { l - 1 } else { l };
                    if loc_level >= 0 {
                        // Pixel origin of this tile (whole-map pass: chunk offsets fold away).
                        let px = x * 4 + 48;
                        let py = (size_z - 1 - z) * 4 + 48;
                        self.walls_and_locs(
                            scene,
                            [loc_level, x, z],
                            [px, py],
                            colours,
                            &mut marks,
                        );
                    }
                }
            }
            level_tiles.push((l as usize, tiles));
        }
        // Noise channel with the random value at its expectation (0.5): (int) (0.5 * 8.0) = 4
        // per channel.
        let noise = (4 << 16) | (4 << 8) | 4;
        marks.push(Mark::Add {
            rect: [48, 48, size_x * 4, size_z * 4],
            colour: noise,
        });
        BasePlan {
            size,
            first_level: first,
            level_tiles,
            marks,
        }
    }
    /// The wall and loc marks of one tile, at pixel origin `pixel`.
    fn walls_and_locs(
        &mut self,
        scene: &Scene,
        tile: [i32; 3],
        pixel: [i32; 2],
        colours: MarkColours,
        marks: &mut Vec<Mark>,
    ) {
        let [level, x, z] = tile;
        let [px, py] = pixel;
        let MarkColours {
            wall: wall_colour,
            active: active_colour,
        } = colours;
        let Some(tile) = scene.tile(level as usize, x as usize, z as usize) else {
            return;
        };
        if self.locs.is_none() {
            return;
        }
        // Wall.
        if let Some(w) = tile.wall.and_then(|i| scene.walls.get(i)) {
            let Some(loc) = self.icon_loc(w.loc_id) else {
                return;
            };
            let angle = w.angle & 3;
            let shape = w.shape;
            if loc.mapsceneicon == -1 {
                let colour = if loc.active > 0 {
                    active_colour
                } else {
                    wall_colour
                };
                // WALL_STRAIGHT (0) or WALL_L (2): the outer edge line.
                if shape == 0 || shape == 2 {
                    match angle {
                        0 => marks.push(Mark::Fill {
                            rect: [px, py, 1, 4],
                            colour,
                        }),
                        1 => marks.push(Mark::Fill {
                            rect: [px, py, 4, 1],
                            colour,
                        }),
                        2 => marks.push(Mark::Fill {
                            rect: [px + 3, py, 1, 4],
                            colour,
                        }),
                        _ => marks.push(Mark::Fill {
                            rect: [px, py + 3, 4, 1],
                            colour,
                        }),
                    }
                }
                // WALL_SQUARE_CORNER (3): one pixel.
                if shape == 3 {
                    match angle {
                        0 => marks.push(Mark::Fill {
                            rect: [px, py, 1, 1],
                            colour,
                        }),
                        1 => marks.push(Mark::Fill {
                            rect: [px + 3, py, 1, 1],
                            colour,
                        }),
                        2 => marks.push(Mark::Fill {
                            rect: [px + 3, py + 3, 1, 1],
                            colour,
                        }),
                        _ => marks.push(Mark::Fill {
                            rect: [px, py + 3, 1, 1],
                            colour,
                        }),
                    }
                }
                // WALL_L (2): the second edge.
                if shape == 2 {
                    match angle {
                        0 => marks.push(Mark::Fill {
                            rect: [px, py, 4, 1],
                            colour,
                        }),
                        1 => marks.push(Mark::Fill {
                            rect: [px + 3, py, 1, 4],
                            colour,
                        }),
                        2 => marks.push(Mark::Fill {
                            rect: [px, py + 3, 4, 1],
                            colour,
                        }),
                        _ => marks.push(Mark::Fill {
                            rect: [px, py, 1, 4],
                            colour,
                        }),
                    }
                }
            } else {
                self.map_scene_icon(loc, angle, px, py, marks);
            }
        }
        // Primary entity.
        let entity = primary_loc(scene, tile, x, z);
        if let Some(e) = entity {
            if let Some(loc) = self.icon_loc(e.loc_id) {
                let angle = e.angle & 3;
                if loc.mapsceneicon != -1 {
                    self.map_scene_icon(loc, angle, px, py, marks);
                } else if e.shape == 9 {
                    // WALL_DIAGONAL: -1118482 grey, -1179648 red when active.
                    let colour = if loc.active > 0 { -1179648 } else { -1118482 };
                    if angle == 0 || angle == 2 {
                        marks.push(Mark::Line {
                            from: [px, py + 3],
                            to: [px + 3, py],
                            colour,
                        });
                    } else {
                        marks.push(Mark::Line {
                            from: [px, py],
                            to: [px + 3, py + 3],
                            colour,
                        });
                    }
                }
            }
        }
        // Ground decoration.
        if let Some(g) = tile
            .ground_decoration
            .and_then(|i| scene.ground_decors.get(i))
        {
            if let Some(loc) = self.icon_loc(g.loc_id) {
                if loc.mapsceneicon != -1 {
                    self.map_scene_icon(loc, g.angle & 3, px, py, marks);
                }
            }
        }
    }
    /// The loc config fields the wall/loc marks and scene icons read.
    fn icon_loc(&self, id: u32) -> Option<IconLoc> {
        let store = self.locs.as_ref()?;
        let loc = store.get(id)?;
        Some(IconLoc {
            active: loc.active_for(store.allow_members.get()),
            mapsceneicon: loc.mapsceneicon,
            rotate: loc.mapsceneiconrotate,
            rotation: loc.map_icon_rotation,
            mirror: loc.mapsceneiconmirror,
            width: i32::from(loc.width),
            length: i32::from(loc.length),
        })
    }
    /// The map scene icon mark of a loc at pixel origin (`px`, `py`).
    fn map_scene_icon(
        &mut self,
        loc: IconLoc,
        angle: i32,
        px: i32,
        py: i32,
        marks: &mut Vec<Mark>,
    ) {
        let msi = self.msi.get(loc.mapsceneicon);
        if msi.sprite == -1 {
            return;
        }
        let rotation = if loc.rotate {
            (loc.rotation + angle) & 3
        } else {
            0
        };
        let Some(pack) = self.pack.clone() else {
            return;
        };
        let Some(sprite) = self
            .msi
            .sprite(&pack, loc.mapsceneicon, rotation, loc.mirror)
        else {
            return;
        };
        let (mut w, mut l) = (loc.width, loc.length);
        if rotation & 1 == 1 {
            std::mem::swap(&mut w, &mut l);
        }
        let [mut sw, mut sh] = sprite.size;
        if msi.scale_to_loc {
            sw = w * 4;
            sh = l * 4;
        }
        // The tint is already applied to the pixels.
        marks.push(Mark::Icon {
            sprite,
            rect: [px, py - (l - 1) * 4, sw, sh],
        });
    }
}

/// The first entity of the tile on the primary layer (a scenery entity whose
/// primary-layer flag holds) whose origin tile is `(x, z)`; a larger loc overlapping
/// the tile is skipped.
fn primary_loc<'a>(
    scene: &'a Scene,
    tile: &crate::scene::Tile,
    x: i32,
    z: i32,
) -> Option<&'a crate::scene::SceneryEntity> {
    tile.entities.iter().find_map(|e| match e {
        crate::scene::PrimaryRef::Scenery(i) => scene
            .scenery
            .get(*i)
            .filter(|s| s.primary_layer && s.min_tx == x && s.min_tz == z),
        crate::scene::PrimaryRef::Temporary(_) => None,
    })
}

/// The loc config fields the minimap marks depend on.
#[derive(Clone, Copy, Debug)]
struct IconLoc {
    active: i32,
    mapsceneicon: i32,
    rotate: bool,
    rotation: i32,
    mirror: bool,
    width: i32,
    length: i32,
}

/// Inputs of the per-frame minimap draw, resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The base sprite (None: the `toggle == 2 || 5 || no sprite` path).
    pub base: Option<Base>,
    /// The minimap toggle.
    pub toggle: i32,
    /// The local player's x/z (scene-local fine units).
    pub player: [i32; 2],
    /// The scene's z size.
    pub size_z: i32,
    /// The rotation in 14-bit units.
    pub angle: i32,
    /// `4096 - zoom * 16`.
    pub scale: i32,
    /// Whether the local player is hidden (draws the white square).
    pub player_hidden: bool,
    /// The overlay rotation (the un-negated yaw, unlike `angle`).
    pub overlay_angle: i32,
    /// The minimap zoom.
    pub zoom: i32,
    /// Camera state 4 (no zoom scaling of the overlay trig).
    pub camera_state_4: bool,
    pub overlays: Vec<Overlay>,
}

/// One overlay drawn on the minimap: `d` is the tile-space offset from the player,
/// `offset` shifts the component origin (used by the map flag).
#[derive(Clone, Debug)]
pub struct Overlay {
    pub d: [i32; 2],
    pub sprite: Rc<Sprite>,
    /// Optional map-element polygon in minimap-local units.
    pub polygon: Option<WorldMapPolygon>,
    /// Polygon-only overlays use a one-pixel placeholder sprite that must not
    /// reach the sprite compositor.
    pub draw_sprite: bool,
    /// Text label attached to a map-element overlay.
    pub label: Option<WorldMapLabel>,
    /// Sprite scale in percent.
    pub scale: f64,
    /// The x/y alignment index.
    pub align: [i32; 2],
    pub offset: [i32; 2],
    /// Hint arrows may be placed on the minimap edge and rotated toward their
    /// target by the backend renderer.
    pub edge_arrow: bool,
    pub edge_sprite: Option<Rc<Sprite>>,
}
impl Overlay {
    /// A sprite overlay at the default scale (100) and alignment (index 2).
    pub fn sprite(d: [i32; 2], sprite: Rc<Sprite>) -> Self {
        Self {
            d,
            sprite,
            polygon: None,
            draw_sprite: true,
            label: None,
            scale: 100.0,
            align: [2, 2],
            offset: [0, 0],
            edge_arrow: false,
            edge_sprite: None,
        }
    }
}
impl PartialEq for Overlay {
    fn eq(&self, o: &Self) -> bool {
        let edge_sprite_eq = match (self.edge_sprite.as_ref(), o.edge_sprite.as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        };
        self.d == o.d
            && Rc::ptr_eq(&self.sprite, &o.sprite)
            && self.polygon == o.polygon
            && self.draw_sprite == o.draw_sprite
            && self.label.as_ref().map(|v| (&v.text, v.colour, v.size))
                == o.label.as_ref().map(|v| (&v.text, v.colour, v.size))
            && self.scale == o.scale
            && self.align == o.align
            && self.offset == o.offset
            && self.edge_arrow == o.edge_arrow
            && edge_sprite_eq
    }
}

/// The entity state the overlay list reads, in scene-local units.
pub struct OverlayInput<'a> {
    /// The scene base.
    pub base: [i32; 2],
    /// The local player's x/z.
    pub player: [i32; 2],
    /// The local player's level, size and team.
    pub level: i32,
    /// The logic cycle counter, used for the ten-cycle hint-arrow blink.
    pub cycle: i32,
    pub local_size: i32,
    pub local_team: i32,
    /// Obj stack keys (`level << 28 | z << 14 | x`, absolute tiles).
    pub stacks: &'a [i64],
    pub npcs: &'a [NpcDot],
    pub players: &'a [PlayerDot],
    /// Local player variable reads: `(is_varbit, id)`.
    pub read: &'a dyn Fn(bool, i32) -> Result<i32>,
    /// The flag tile and map-flag switch (the client core's).
    pub flag: MinimapFlag,
}
/// The NPC fields the overlay reads (only NPCs that exist).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpcDot {
    pub index: usize,
    pub type_id: i32,
    pub level: i32,
    pub fine: [i32; 2],
}
/// The player fields the overlay reads (the local player excluded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerDot {
    pub index: usize,
    pub has_model: bool,
    /// Whether the player is hidden.
    pub hidden: bool,
    pub level: i32,
    pub fine: [i32; 2],
    /// The transmogrified NPC id (-1 none).
    pub transmog_npc: i32,
    /// Community partner type serial (1 and 2 select distinct dots).
    pub partner: i32,
    pub suppress_partner: bool,
    pub team: i32,
    /// Retained social membership used by the player dots.
    pub friend: bool,
    pub clan: bool,
}

/// Inputs of the compass draw.
#[derive(Clone, Debug)]
pub struct CompassFrame {
    pub sprite: Rc<Sprite>,
    pub toggle: i32,
    /// The compass angle: `anticheat_angle * 2 + yaw & 0x3FFF`.
    pub angle: i32,
}

#[cfg(test)]
mod tests;
