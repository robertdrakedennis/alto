//! Hitmark and headbar configs: decoded state only. Draw-time morphing,
//! damage formatting and sprite loading stay with their consumers; hit
//! replacement uses the original type. An opcode outside the table is an
//! error naming the id and the opcode.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};

/// One hitmark type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub damagefont: i32,
    pub damagecolour: i32,
    pub sticktime: i32,
    pub classgraphic: i32,
    pub middlegraphic: i32,
    pub leftgraphic: i32,
    pub rightgraphic: i32,
    pub scrolltooffsetx: i32,
    pub scrolltooffsety: i32,
    pub fadeat: i32,
    pub replacemode: i32,
    pub damageyof: i32,
    pub graphicxof: i32,
    pub graphicyof: i32,
    pub multivarbit: i32,
    pub multivarp: i32,
    pub damagescaleto: i32,
    pub damagescalefrom: i32,
    pub damagecolour_set: bool,
    pub damageformat: String,
    pub multimark: Option<Vec<i32>>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Default for Hit {
    fn default() -> Self {
        Self {
            damagefont: -1,
            damagecolour: 16777215,
            sticktime: 70,
            classgraphic: -1,
            middlegraphic: -1,
            leftgraphic: -1,
            rightgraphic: -1,
            scrolltooffsetx: 0,
            scrolltooffsety: 0,
            fadeat: -1,
            replacemode: -1,
            damageyof: 0,
            graphicxof: 0,
            graphicyof: 0,
            multivarbit: -1,
            multivarp: -1,
            damagescaleto: 1,
            damagescalefrom: 1,
            damagecolour_set: false,
            damageformat: String::new(),
            multimark: None,
            consumed: 0,
            raw: vec![],
        }
    }
}

/// One headbar type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub showpriority: i32,
    pub hidepriority: i32,
    pub fadeout: i32,
    /// Step size of the fill animation: the elapsed time is rounded down to a
    /// multiple of it, and 0 freezes the fill. No opcode sets it.
    pub fill_step: i32,
    pub sticktime: i32,
    pub full: i32,
    pub empty: i32,
    pub fulllocalpartner: i32,
    pub emptylocalpartner: i32,
    pub fullglobalpartner: i32,
    pub emptyglobalpartner: i32,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            showpriority: 255,
            hidepriority: 255,
            fadeout: -1,
            fill_step: 1,
            sticktime: 70,
            full: -1,
            empty: -1,
            fulllocalpartner: -1,
            emptylocalpartner: -1,
            fullglobalpartner: -1,
            emptyglobalpartner: -1,
            consumed: 0,
            raw: vec![],
        }
    }
}

/// Hitmark opcodes of this revision.
static HIT_OPCODES: Table<Hit, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::SmartId(|h, _, v| h.damagefont = v)),
        Entry::new(at(2), Rule::Custom(read_damage_colour)),
        Entry::new(at(3), Rule::SmartId(|h, _, v| h.classgraphic = v)),
        Entry::new(at(4), Rule::SmartId(|h, _, v| h.leftgraphic = v)),
        Entry::new(at(5), Rule::SmartId(|h, _, v| h.middlegraphic = v)),
        Entry::new(at(6), Rule::SmartId(|h, _, v| h.rightgraphic = v)),
        Entry::new(
            at(7),
            Rule::SignedShort(|h, _, v| h.scrolltooffsetx = i32::from(v)),
        ),
        Entry::new(at(8), Rule::Custom(read_damage_format)),
        Entry::new(at(9), Rule::Short(|h, _, v| h.sticktime = i32::from(v))),
        Entry::new(
            at(10),
            Rule::SignedShort(|h, _, v| h.scrolltooffsety = i32::from(v)),
        ),
        Entry::new(at(11), Rule::Flag(|h, _| h.fadeat = 0)),
        Entry::new(at(12), Rule::Byte(|h, _, v| h.replacemode = i32::from(v))),
        Entry::new(
            at(13),
            Rule::SignedShort(|h, _, v| h.damageyof = i32::from(v)),
        ),
        Entry::new(at(14), Rule::Short(|h, _, v| h.fadeat = i32::from(v))),
        Entry::new(at(16), Rule::Custom(read_graphic_offsets)),
        Entry::new(at(17), Rule::Custom(|s, h, _| read_morphs(s, h, false))),
        Entry::new(at(18), Rule::Custom(|s, h, _| read_morphs(s, h, true))),
        Entry::new(
            at(19),
            Rule::Short(|h, _, v| h.damagescaleto = i32::from(v)),
        ),
        Entry::new(
            at(20),
            Rule::Short(|h, _, v| h.damagescalefrom = i32::from(v)),
        ),
    ],
    Unknown::Reject,
);

/// Damage colour: a byte and a short forming a 24-bit colour.
fn read_damage_colour(source: Input<Error>, hit: &mut Hit, _: Slot) -> Result<()> {
    hit.damagecolour = (i32::from(source.byte()?) << 16) | i32::from(source.short()?);
    hit.damagecolour_set = true;
    Ok(())
}

/// Damage text format: a version byte that must be 0, then the text.
fn read_damage_format(source: Input<Error>, hit: &mut Hit, _: Slot) -> Result<()> {
    if source.byte()? != 0 {
        return Err(Error::Invalid("hitmark string version"));
    }
    hit.damageformat = source.text()?;
    Ok(())
}

fn read_graphic_offsets(source: Input<Error>, hit: &mut Hit, _: Slot) -> Result<()> {
    hit.graphicxof = i32::from(source.signed_short()?);
    hit.graphicyof = i32::from(source.signed_short()?);
    Ok(())
}

/// Replacement hitmarks selected by a variable: varbit, varp, an optional
/// explicit fallback, a count, then the targets; the fallback goes last.
fn read_morphs(source: Input<Error>, hit: &mut Hit, has_fallback: bool) -> Result<()> {
    hit.multivarbit = nullable(source)?;
    hit.multivarp = nullable(source)?;
    let end = if has_fallback { nullable(source)? } else { -1 };
    let count = source.byte()?;
    let mut list = vec![];
    for _ in 0..=count {
        list.push(nullable(source)?);
    }
    list.push(end);
    hit.multimark = Some(list);
    Ok(())
}

/// Headbar opcodes of this revision.
static BAR_OPCODES: Table<Bar, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Skip(&[Field::Short])),
        Entry::new(at(2), Rule::Byte(|b, _, v| b.showpriority = i32::from(v))),
        Entry::new(at(3), Rule::Byte(|b, _, v| b.hidepriority = i32::from(v))),
        Entry::new(at(4), Rule::Flag(|b, _| b.fadeout = 0)),
        Entry::new(at(5), Rule::Short(|b, _, v| b.sticktime = i32::from(v))),
        Entry::new(at(6), Rule::Skip(&[Field::Byte])),
        Entry::new(at(7), Rule::SmartId(|b, _, v| b.full = v)),
        Entry::new(at(8), Rule::SmartId(|b, _, v| b.empty = v)),
        Entry::new(at(9), Rule::SmartId(|b, _, v| b.fulllocalpartner = v)),
        Entry::new(at(10), Rule::SmartId(|b, _, v| b.emptylocalpartner = v)),
        Entry::new(at(11), Rule::Short(|b, _, v| b.fadeout = i32::from(v))),
        Entry::new(at(12), Rule::SmartId(|b, _, v| b.fullglobalpartner = v)),
        Entry::new(at(13), Rule::SmartId(|b, _, v| b.emptyglobalpartner = v)),
    ],
    Unknown::Reject,
);

impl Hit {
    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut hit = Self {
            raw: bytes.into(),
            ..Self::default()
        };
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "hitmark",
            id: i64::from(id),
        };
        HIT_OPCODES.run(&mut packet, &mut hit, record)?;
        hit.consumed = packet.pos;
        Ok(hit)
    }

    pub fn combat(&self) -> super::combat::HitType {
        super::combat::HitType {
            replace: self.replacemode,
            duration: self.sticktime,
        }
    }
}

impl Bar {
    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut bar = Self {
            raw: bytes.into(),
            ..Self::default()
        };
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "headbar",
            id: i64::from(id),
        };
        BAR_OPCODES.run(&mut packet, &mut bar, record)?;
        bar.consumed = packet.pos;
        Ok(bar)
    }

    pub fn combat(&self) -> super::combat::BarType {
        super::combat::BarType {
            show: self.showpriority,
            hide: self.hidepriority,
            duration: self.sticktime,
        }
    }
}

fn nullable(source: Input<Error>) -> Result<i32> {
    crate::opcode_table::payload::nullable_short(source)
}
