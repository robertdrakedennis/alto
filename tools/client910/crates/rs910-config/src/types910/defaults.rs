//! Entity defaults: the graphics and wear-position files of the defaults
//! archive (flat files 3 and 6). Every known field is retained, including
//! ones the palette loader alone would drop. No GPU or model types. An
//! opcode outside the table is an error: newer caches may add opcodes whose
//! meaning is not known here, and guessing would corrupt the rest of the
//! file. The input remains owned by the caller.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, Entry, Input, Record, Rule, Slot, Table, Unknown};

/// A decoded value and the number of bytes it consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded<T> {
    pub value: T,
    pub consumed: usize,
}

/// Wear-position defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wear {
    pub positions: Vec<i32>,
    pub mainhand: i32,
    pub offhand: i32,
    /// Wear slots a main-hand sequence overrides.
    pub mainhand_override_slots: Option<Vec<i32>>,
    /// Wear slots an off-hand sequence overrides.
    pub offhand_override_slots: Option<Vec<i32>>,
}

/// Wear defaults while their opcodes are being read.
struct WearDraft {
    wear: Wear,
    /// The position list is mandatory; it is checked after the last opcode.
    positions: Option<Vec<i32>>,
}

/// Wear-position opcodes of this revision.
static WEAR_OPCODES: Table<WearDraft, Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Custom(|s, d, _| {
                d.positions = Some(byte_array(s)?);
                Ok(())
            }),
        ),
        Entry::new(at(3), Rule::Byte(|d, _, v| d.wear.mainhand = i32::from(v))),
        Entry::new(at(4), Rule::Byte(|d, _, v| d.wear.offhand = i32::from(v))),
        Entry::new(
            at(5),
            Rule::Custom(|s, d, _| {
                d.wear.mainhand_override_slots = Some(byte_array(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(6),
            Rule::Custom(|s, d, _| {
                d.wear.offhand_override_slots = Some(byte_array(s)?);
                Ok(())
            }),
        ),
    ],
    Unknown::Fail(|_, _| Error::UnsupportedContext("unknown wear defaults opcode")),
);

impl Wear {
    pub fn decode(bytes: &[u8]) -> Result<Decoded<Self>> {
        let mut packet = Packet::new(bytes);
        let mut draft = WearDraft {
            wear: Self {
                positions: vec![],
                mainhand: -1,
                offhand: -1,
                mainhand_override_slots: None,
                offhand_override_slots: None,
            },
            positions: None,
        };
        let record = Record {
            kind: "wear defaults",
            id: -1,
        };
        WEAR_OPCODES.run(&mut packet, &mut draft, record)?;
        draft.wear.positions = draft
            .positions
            .ok_or(Error::Invalid("missing wear-position defaults"))?;
        Ok(Decoded {
            value: draft.wear,
            consumed: packet.pos,
        })
    }
}

/// A count byte, then that many bytes.
fn byte_array(source: Input<Error>) -> Result<Vec<i32>> {
    let count = source.byte()?;
    (0..count).map(|_| source.byte().map(i32::from)).collect()
}

/// Recolour or retexture palettes: for each of ten slots and four entries a
/// source colour and a list of destinations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub source: [[i16; 4]; 10],
    pub destinations: [[Vec<i16>; 4]; 10],
}

impl Palette {
    fn read(source: Input<Error>) -> Result<Self> {
        let mut out = Self {
            source: [[0; 4]; 10],
            destinations: std::array::from_fn(|_| std::array::from_fn(|_| vec![])),
        };
        for slot in 0..10 {
            for entry in 0..4 {
                out.source[slot][entry] = source.short()? as i16;
                let count = source.short()?;
                out.destinations[slot][entry] = (0..count)
                    .map(|_| source.short().map(|v| v as i16))
                    .collect::<Result<_>>()?;
            }
        }
        Ok(out)
    }

    pub fn appearance_lengths(&self) -> [usize; 10] {
        std::array::from_fn(|slot| self.destinations[slot][0].len())
    }
}

/// The scalar settings of the graphics defaults. Fields marked `unknown` are
/// decoded and kept, but nothing reads them yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scalars {
    pub maxhitmarks: i32,
    /// Most headbar updates an entity keeps.
    pub maxheadbar_updates: i32,
    /// Most headbars an entity shows.
    pub maxheadbars: i32,
    /// Gap left where no headbar drew.
    pub headbar_gap: i32,
    pub performancemetricsmodel: i32,
    pub login_interface: i32,
    pub lobby_interface: i32,
    pub show_single_option_menu: i32,
    pub spotshadowtexture: i32,
    pub spotshadowtexture_alpha: i32,
    pub unknown_21: i32,
    pub npc_should_display_chat: i32,
    pub npc_chat_timeout: i32,
    pub player_should_display_chat: i32,
    pub player_chat_timeout: i32,
    /// Client frame size; both `-1` leaves the frame as it is.
    pub client_frame_width: i32,
    pub client_frame_height: i32,
    /// Non-zero selects camera state 3 as the default instead of 2.
    pub default_camera_alt: i32,
    pub inv_hundred_color: i32,
    pub inv_thousand_color: i32,
    pub inv_million_color: i32,
    pub p11_full: i32,
    pub p12_full: i32,
    pub b12_full: i32,
    pub hintarrows: i32,
    pub unknown_sprite_1: i32,
    pub mapflag: i32,
    pub unknown_signed_1: i32,
    pub unknown_signed_2: i32,
    pub cross: i32,
    pub mapdots: i32,
    /// Sprite group of the inline font icons.
    pub font_icons: i32,
    pub unknown_sprite_2: i32,
    pub compass: i32,
    /// Sprite group of the menu submenu arrow.
    pub submenu_arrow: i32,
    pub unknown_sprite_3: i32,
}

impl Default for Scalars {
    fn default() -> Self {
        Self {
            maxhitmarks: 4,
            maxheadbar_updates: 4,
            maxheadbars: 4,
            headbar_gap: 7,
            performancemetricsmodel: -1,
            login_interface: -1,
            lobby_interface: -1,
            show_single_option_menu: 1,
            spotshadowtexture: -1,
            spotshadowtexture_alpha: 0,
            unknown_21: 100,
            npc_should_display_chat: 1,
            npc_chat_timeout: 2,
            player_should_display_chat: 1,
            player_chat_timeout: 3,
            client_frame_width: -1,
            client_frame_height: -1,
            default_camera_alt: 0,
            inv_hundred_color: 16776960,
            inv_thousand_color: 16777215,
            inv_million_color: 65408,
            p11_full: -1,
            p12_full: -1,
            b12_full: -1,
            hintarrows: -1,
            unknown_sprite_1: -1,
            mapflag: -1,
            unknown_signed_1: 0,
            unknown_signed_2: 0,
            cross: -1,
            mapdots: -1,
            font_icons: -1,
            unknown_sprite_2: -1,
            compass: -1,
            submenu_arrow: -1,
            unknown_sprite_3: -1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graphics {
    pub scalars: Scalars,
    pub hitmark_positions: Vec<[i32; 2]>,
    pub recolour: Option<Palette>,
    pub retexture: Option<Palette>,
}

/// Graphics defaults while their opcodes are being read.
struct GraphicsDraft {
    graphics: Graphics,
    /// The hitmark position list has been allocated.
    allocated: bool,
    /// An opcode supplied the hitmark positions.
    explicit_positions: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityLimits {
    pub hitmarks: usize,
    pub headbars: usize,
    pub headbar_updates: usize,
    pub player_chat_ticks: i32,
    pub npc_chat_ticks: i32,
    pub player_chat_visible: bool,
    pub npc_chat_visible: bool,
}

impl Graphics {
    /// The per-entity limits; chat timeouts multiply the caller's logic rate.
    pub fn entity_limits(&self, logic_rate: i32) -> EntityLimits {
        let s = &self.scalars;
        EntityLimits {
            hitmarks: s.maxhitmarks as usize,
            headbars: s.maxheadbars as usize,
            headbar_updates: s.maxheadbar_updates as usize,
            player_chat_ticks: logic_rate.wrapping_mul(s.player_chat_timeout),
            npc_chat_ticks: logic_rate.wrapping_mul(s.npc_chat_timeout),
            player_chat_visible: s.player_should_display_chat != 0,
            npc_chat_visible: s.npc_should_display_chat != 0,
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Decoded<Self>> {
        let mut packet = Packet::new(bytes);
        let mut draft = GraphicsDraft {
            graphics: Self {
                scalars: Default::default(),
                hitmark_positions: vec![],
                recolour: None,
                retexture: None,
            },
            allocated: false,
            explicit_positions: false,
        };
        let record = Record {
            kind: "graphics defaults",
            id: -1,
        };
        GRAPHICS_OPCODES.run(&mut packet, &mut draft, record)?;
        // Without explicit positions the hitmarks stack downwards, 20 apart.
        if !draft.explicit_positions {
            let graphics = &mut draft.graphics;
            if !draft.allocated {
                graphics.scalars.maxhitmarks = 4;
                graphics.hitmark_positions = vec![[0; 2]; 4];
            }
            for (i, at) in graphics.hitmark_positions.iter_mut().enumerate() {
                *at = [0, i as i32 * 20];
            }
        }
        Ok(Decoded {
            value: draft.graphics,
            consumed: packet.pos,
        })
    }
}

/// Graphics defaults opcodes of this revision.
static GRAPHICS_OPCODES: Table<GraphicsDraft, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_hitmark_positions)),
        Entry::new(
            at(2),
            Rule::SmartId(|d, _, v| d.graphics.scalars.performancemetricsmodel = v),
        ),
        Entry::new(
            at(3),
            Rule::Byte(|d, _, v| {
                d.graphics.scalars.maxhitmarks = i32::from(v);
                d.graphics.hitmark_positions = vec![[0; 2]; usize::from(v)];
                d.allocated = true;
            }),
        ),
        Entry::new(
            at(4),
            Rule::Flag(|d, _| d.graphics.scalars.show_single_option_menu = 0),
        ),
        Entry::new(
            at(5),
            Rule::Medium(|d, _, v| d.graphics.scalars.login_interface = v as i32),
        ),
        Entry::new(
            at(6),
            Rule::Medium(|d, _, v| d.graphics.scalars.lobby_interface = v as i32),
        ),
        Entry::new(
            at(7),
            Rule::Custom(|s, d, _| {
                d.graphics.recolour = Some(Palette::read(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(8),
            Rule::Flag(|d, _| d.graphics.scalars.npc_should_display_chat = 0),
        ),
        Entry::new(
            at(9),
            Rule::Byte(|d, _, v| d.graphics.scalars.npc_chat_timeout = i32::from(v)),
        ),
        Entry::new(
            at(10),
            Rule::Flag(|d, _| d.graphics.scalars.player_should_display_chat = 0),
        ),
        Entry::new(
            at(11),
            Rule::Byte(|d, _, v| {
                d.graphics.scalars.player_chat_timeout = i32::from(v);
            }),
        ),
        Entry::new(
            at(12),
            Rule::Custom(|s, d, _| {
                d.graphics.scalars.client_frame_width = i32::from(s.short()?);
                d.graphics.scalars.client_frame_height = i32::from(s.short()?);
                Ok(())
            }),
        ),
        Entry::new(
            at(13),
            Rule::Byte(|d, _, v| d.graphics.scalars.maxheadbars = i32::from(v)),
        ),
        Entry::new(
            at(14),
            Rule::Byte(|d, _, v| {
                d.graphics.scalars.maxheadbar_updates = i32::from(v);
            }),
        ),
        Entry::new(
            at(15),
            Rule::Byte(|d, _, v| d.graphics.scalars.headbar_gap = i32::from(v)),
        ),
        Entry::new(
            at(16),
            Rule::Flag(|d, _| d.graphics.scalars.default_camera_alt = 1),
        ),
        Entry::new(
            at(17),
            Rule::Int(|d, _, v| d.graphics.scalars.inv_hundred_color = v),
        ),
        Entry::new(
            at(18),
            Rule::Int(|d, _, v| d.graphics.scalars.inv_thousand_color = v),
        ),
        Entry::new(
            at(19),
            Rule::Int(|d, _, v| d.graphics.scalars.inv_million_color = v),
        ),
        Entry::new(at(20), Rule::Custom(read_spot_shadow)),
        Entry::new(
            at(21),
            Rule::Byte(|d, _, v| d.graphics.scalars.unknown_21 = i32::from(v)),
        ),
        Entry::new(at(22), Rule::Custom(read_interface_sprites)),
        Entry::new(
            at(23),
            Rule::Custom(|s, d, _| {
                d.graphics.retexture = Some(Palette::read(s)?);
                Ok(())
            }),
        ),
    ],
    Unknown::Fail(|_, _| Error::UnsupportedContext("unknown graphics defaults opcode")),
);

/// Explicit hitmark positions: one signed `(x, y)` pair per hitmark slot.
fn read_hitmark_positions(source: Input<Error>, d: &mut GraphicsDraft, _: Slot) -> Result<()> {
    if !d.allocated {
        d.graphics.scalars.maxhitmarks = 4;
        d.graphics.hitmark_positions = vec![[0; 2]; 4];
        d.allocated = true;
    }
    for at in &mut d.graphics.hitmark_positions {
        *at = [
            i32::from(source.short()? as i16),
            i32::from(source.short()? as i16),
        ];
    }
    d.explicit_positions = true;
    Ok(())
}

/// Spot shadow texture (signed) and alpha (signed byte).
fn read_spot_shadow(source: Input<Error>, d: &mut GraphicsDraft, _: Slot) -> Result<()> {
    d.graphics.scalars.spotshadowtexture = i32::from(source.short()? as i16);
    d.graphics.scalars.spotshadowtexture_alpha = i32::from(source.byte()? as i8);
    Ok(())
}

/// The run of sprite and font ids the interface uses, in stored order.
fn read_interface_sprites(source: Input<Error>, d: &mut GraphicsDraft, _: Slot) -> Result<()> {
    let s = &mut d.graphics.scalars;
    s.p11_full = source.smart_id()?;
    s.p12_full = source.smart_id()?;
    s.b12_full = source.smart_id()?;
    s.hintarrows = source.smart_id()?;
    s.unknown_sprite_1 = source.smart_id()?;
    s.mapflag = source.smart_id()?;
    s.unknown_signed_1 = i32::from(source.byte()? as i8);
    s.unknown_signed_2 = i32::from(source.byte()? as i8);
    s.cross = source.smart_id()?;
    s.mapdots = source.smart_id()?;
    s.font_icons = source.smart_id()?;
    s.unknown_sprite_2 = source.smart_id()?;
    s.compass = source.smart_id()?;
    s.submenu_arrow = source.smart_id()?;
    s.unknown_sprite_3 = source.smart_id()?;
    Ok(())
}

/// Complete defaults data. The two files are the defaults archive's flat
/// files 3 (graphics) and 6 (wear); a group is never read as "its first file".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityDefaults {
    pub graphics: Graphics,
    pub wear: Wear,
    pub graphics_bytes: Vec<u8>,
    pub wear_bytes: Vec<u8>,
}
impl EntityDefaults {
    pub fn apply_appearance(&self, c: &mut super::appearance::Config) -> Result<()> {
        let colour = self
            .graphics
            .recolour
            .as_ref()
            .ok_or(Error::UnsupportedContext(
                "missing appearance recolour palette",
            ))?
            .appearance_lengths();
        let texture = self
            .graphics
            .retexture
            .as_ref()
            .ok_or(Error::UnsupportedContext(
                "missing appearance retexture palette",
            ))?
            .appearance_lengths();
        c.wear = self.wear.positions.clone();
        c.colour_lengths = colour;
        c.texture_lengths = texture;
        Ok(())
    }
}
