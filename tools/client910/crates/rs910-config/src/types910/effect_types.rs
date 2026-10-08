//! Spot animation configs: the model, sequence and placement inputs the CPU
//! side needs. Model creation, hill skew and drawing stay with the consumers.
//! An opcode outside the table is an error naming the id and the opcode.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, Entry, Input, Record, Rule, Slot, Table, Unknown};

/// One spot animation type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    pub model: i32,
    pub sequence: i32,
    pub resizeh: i32,
    pub resizev: i32,
    pub orientation: i32,
    pub ambient: i32,
    pub contrast: i32,
    pub looping: bool,
    pub hillskew_mode: i32,
    pub hillskew_value: i32,
    pub colours: Option<Vec<[i16; 2]>>,
    pub textures: Option<Vec<[i16; 2]>>,
    pub colour_indices: Option<Vec<i8>>,
    pub texture_indices: Option<Vec<i8>>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Default for Effect {
    fn default() -> Self {
        Self {
            model: 0,
            sequence: -1,
            resizeh: 128,
            resizev: 128,
            orientation: 0,
            ambient: 0,
            contrast: 0,
            looping: false,
            hillskew_mode: 0,
            hillskew_value: -1,
            colours: None,
            textures: None,
            colour_indices: None,
            texture_indices: None,
            consumed: 0,
            raw: vec![],
        }
    }
}

/// Spot animation opcodes of this revision.
static EFFECT_OPCODES: Table<Effect, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::SmartId(|e, _, v| e.model = v)),
        Entry::new(at(2), Rule::SmartId(|e, _, v| e.sequence = v)),
        Entry::new(at(4), Rule::Short(|e, _, v| e.resizeh = i32::from(v))),
        Entry::new(at(5), Rule::Short(|e, _, v| e.resizev = i32::from(v))),
        Entry::new(at(6), Rule::Short(|e, _, v| e.orientation = i32::from(v))),
        Entry::new(at(7), Rule::Byte(|e, _, v| e.ambient = i32::from(v))),
        Entry::new(at(8), Rule::Byte(|e, _, v| e.contrast = i32::from(v))),
        Entry::new(
            at(9),
            Rule::Flag(|e, _| {
                e.hillskew_mode = 3;
                e.hillskew_value = 8224;
            }),
        ),
        Entry::new(at(10), Rule::Flag(|e, _| e.looping = true)),
        Entry::new(
            at(15),
            Rule::Short(|e, _, v| {
                e.hillskew_mode = 3;
                e.hillskew_value = i32::from(v);
            }),
        ),
        Entry::new(at(16), Rule::Custom(read_wide_hillskew)),
        Entry::new(
            at(40),
            Rule::Custom(|s, e, _| {
                e.colours = Some(read_pairs(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, e, _| {
                e.textures = Some(read_pairs(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(44),
            Rule::Short(|e, _, v| e.colour_indices = Some(indices(i32::from(v)))),
        ),
        Entry::new(
            at(45),
            Rule::Short(|e, _, v| e.texture_indices = Some(indices(i32::from(v)))),
        ),
        Entry::new(at(46), Rule::Skip(&[])),
    ],
    Unknown::Reject,
);

/// A hill skew value stored as two 16-bit halves.
fn read_wide_hillskew(source: Input<Error>, effect: &mut Effect, _: Slot) -> Result<()> {
    effect.hillskew_mode = 3;
    effect.hillskew_value =
        i32::from(source.short()?).wrapping_shl(16) | i32::from(source.short()?);
    Ok(())
}

impl Effect {
    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut effect = Self {
            raw: bytes.into(),
            ..Self::default()
        };
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "effect",
            id: i64::from(id),
        };
        EFFECT_OPCODES.run(&mut packet, &mut effect, record)?;
        effect.consumed = packet.pos;
        Ok(effect)
    }

    pub fn selection(&self) -> crate::types910::animation_state::Effect {
        crate::types910::animation_state::Effect {
            sequence: self.sequence,
            looping: self.looping,
        }
    }
}

/// A count byte, then that many `[source, destination]` pairs of signed shorts.
fn read_pairs(source: Input<Error>) -> Result<Vec<[i16; 2]>> {
    let mut pairs = vec![];
    for _ in 0..source.byte()? {
        pairs.push([source.short()? as i16, source.short()? as i16]);
    }
    Ok(pairs)
}

/// One running index per set bit of `mask`, `-1` per clear bit, up to the
/// highest set bit.
fn indices(mask: i32) -> Vec<i8> {
    let n = 32 - mask.leading_zeros();
    let mut next = 0;
    let mut a = vec![];
    for i in 0..n {
        a.push(if mask & (1 << i) != 0 {
            let v = next;
            next += 1;
            v
        } else {
            -1
        });
    }
    a
}
