//! Body animation set (BAS): which animation an entity plays for each
//! movement state, plus turn, tilt and per-wear-slot tuning. Matrix
//! construction and weighted idle selection stay with the caller. An opcode
//! outside the table is an error naming the id and the opcode.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};

/// One body animation set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bas {
    pub readyanim: i32,
    pub readyanim_l: i32,
    pub readyanim_r: i32,
    pub walkanim: i32,
    pub walkanim_b: i32,
    pub walkanim_l: i32,
    pub walkanim_r: i32,
    pub runanim: i32,
    pub runanim_b: i32,
    pub runanim_l: i32,
    pub runanim_r: i32,
    pub crawlanim: i32,
    pub crawlanim_b: i32,
    pub crawlanim_l: i32,
    pub crawlanim_r: i32,
    pub crawl_turn_left: i32,
    pub crawl_turn_right: i32,
    pub run_turn_left: i32,
    pub run_turn_right: i32,
    pub walk_turn_left: i32,
    pub walk_turn_right: i32,
    /// Sum of the idle weights.
    pub idle_weight_total: i32,
    pub tilt_x: i32,
    pub tilt_z: i32,
    pub tilt_scale_x: i32,
    pub tilt_scale_z: i32,
    /// Turn acceleration, maximum speed and target.
    pub turn_accel: i32,
    pub turn_max: i32,
    /// Roll acceleration, maximum speed and target.
    pub roll_accel: i32,
    pub roll_max: i32,
    pub roll_target: i32,
    /// Pitch acceleration, maximum speed and target.
    pub pitch_accel: i32,
    pub pitch_max: i32,
    pub pitch_target: i32,
    pub walkspeed: i32,
    /// Entity height override; `-1` derives it from the drawn model.
    pub height: i32,
    /// Whether the entity casts a shadow (cleared by one opcode).
    pub casts_shadow: bool,
    /// Extra idle sequences and their weights.
    pub extra_seq_ids: Option<Vec<i32>>,
    pub idle_weights: Option<Vec<i32>>,
    /// Remap of worn model slots for interface models (`-1` for none).
    pub worn_slot_remap: Option<Vec<i32>>,
    /// Turn speed per wear slot.
    pub wear_turn_speeds: Option<Vec<i32>>,
    /// Per-slot transforms, six values each, applied to spot animations and
    /// projectiles launched from a slot.
    pub slot_transforms: Option<Vec<Option<Vec<i32>>>>,
    /// Per-slot offsets, three values each, added to `slot_transforms`.
    pub slot_offsets: Option<Vec<Option<Vec<i32>>>>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Default for Bas {
    fn default() -> Self {
        Self {
            readyanim: -1,
            readyanim_l: -1,
            readyanim_r: -1,
            walkanim: -1,
            walkanim_b: -1,
            walkanim_l: -1,
            walkanim_r: -1,
            runanim: -1,
            runanim_b: -1,
            runanim_l: -1,
            runanim_r: -1,
            crawlanim: -1,
            crawlanim_b: -1,
            crawlanim_l: -1,
            crawlanim_r: -1,
            crawl_turn_left: -1,
            crawl_turn_right: -1,
            run_turn_left: -1,
            run_turn_right: -1,
            walk_turn_left: -1,
            walk_turn_right: -1,
            idle_weight_total: 0,
            tilt_x: 0,
            tilt_z: 0,
            tilt_scale_x: 0,
            tilt_scale_z: 0,
            turn_accel: 0,
            turn_max: 0,
            roll_accel: 0,
            roll_max: 0,
            roll_target: 0,
            pitch_accel: 0,
            pitch_max: 0,
            pitch_target: 0,
            walkspeed: -1,
            height: -1,
            casts_shadow: true,
            extra_seq_ids: None,
            idle_weights: None,
            worn_slot_remap: None,
            wear_turn_speeds: None,
            slot_transforms: None,
            slot_offsets: None,
            consumed: 0,
            raw: vec![],
        }
    }
}

/// BAS opcodes of this revision.
static BAS_OPCODES: Table<Bas, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_ready_and_walk)),
        Entry::new(at(2), Rule::SmartId(|b, _, v| b.crawlanim = v)),
        Entry::new(at(3), Rule::SmartId(|b, _, v| b.crawlanim_b = v)),
        Entry::new(at(4), Rule::SmartId(|b, _, v| b.crawlanim_l = v)),
        Entry::new(at(5), Rule::SmartId(|b, _, v| b.crawlanim_r = v)),
        Entry::new(at(6), Rule::SmartId(|b, _, v| b.runanim = v)),
        Entry::new(at(7), Rule::SmartId(|b, _, v| b.runanim_b = v)),
        Entry::new(at(8), Rule::SmartId(|b, _, v| b.runanim_l = v)),
        Entry::new(at(9), Rule::SmartId(|b, _, v| b.runanim_r = v)),
        Entry::new(at(26), Rule::Custom(read_tilt)),
        Entry::new(
            at(27),
            Rule::Custom(|s, b, _| read_row(s, &mut b.slot_transforms, 6)),
        ),
        Entry::new(at(28), Rule::Custom(read_worn_slot_remap)),
        Entry::new(at(29), Rule::Byte(|b, _, v| b.turn_accel = i32::from(v))),
        Entry::new(at(30), Rule::Short(|b, _, v| b.turn_max = i32::from(v))),
        Entry::new(at(31), Rule::Byte(|b, _, v| b.roll_accel = i32::from(v))),
        Entry::new(at(32), Rule::Short(|b, _, v| b.roll_max = i32::from(v))),
        Entry::new(
            at(33),
            Rule::SignedShort(|b, _, v| b.roll_target = i32::from(v)),
        ),
        Entry::new(at(34), Rule::Byte(|b, _, v| b.pitch_accel = i32::from(v))),
        Entry::new(at(35), Rule::Short(|b, _, v| b.pitch_max = i32::from(v))),
        Entry::new(
            at(36),
            Rule::SignedShort(|b, _, v| b.pitch_target = i32::from(v)),
        ),
        Entry::new(at(37), Rule::Byte(|b, _, v| b.walkspeed = i32::from(v))),
        Entry::new(at(38), Rule::SmartId(|b, _, v| b.readyanim_l = v)),
        Entry::new(at(39), Rule::SmartId(|b, _, v| b.readyanim_r = v)),
        Entry::new(at(40), Rule::SmartId(|b, _, v| b.walkanim_b = v)),
        Entry::new(at(41), Rule::SmartId(|b, _, v| b.walkanim_l = v)),
        Entry::new(at(42), Rule::SmartId(|b, _, v| b.walkanim_r = v)),
        // Stored but not used.
        Entry::new(at(43), Rule::Skip(&[Field::Short])),
        Entry::new(at(44), Rule::Skip(&[Field::Short])),
        Entry::new(at(45), Rule::Short(|b, _, v| b.height = i32::from(v))),
        Entry::new(at(46), Rule::SmartId(|b, _, v| b.crawl_turn_left = v)),
        Entry::new(at(47), Rule::SmartId(|b, _, v| b.crawl_turn_right = v)),
        Entry::new(at(48), Rule::SmartId(|b, _, v| b.run_turn_left = v)),
        Entry::new(at(49), Rule::SmartId(|b, _, v| b.run_turn_right = v)),
        Entry::new(at(50), Rule::SmartId(|b, _, v| b.walk_turn_left = v)),
        Entry::new(at(51), Rule::SmartId(|b, _, v| b.walk_turn_right = v)),
        Entry::new(at(52), Rule::Custom(read_extra_idles)),
        Entry::new(at(53), Rule::Flag(|b, _| b.casts_shadow = false)),
        Entry::new(at(54), Rule::Custom(read_tilt_scale)),
        Entry::new(at(55), Rule::Custom(read_wear_turn_speed)),
        Entry::new(
            at(56),
            Rule::Custom(|s, b, _| read_row(s, &mut b.slot_offsets, 3)),
        ),
    ],
    Unknown::Reject,
);

fn read_ready_and_walk(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    bas.readyanim = source.smart_id()?;
    bas.walkanim = source.smart_id()?;
    Ok(())
}

fn read_tilt(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    bas.tilt_x = i32::from(source.byte()?) * 4;
    bas.tilt_z = i32::from(source.byte()?) * 4;
    Ok(())
}

fn read_tilt_scale(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    bas.tilt_scale_x = i32::from(source.byte()?) << 6;
    bas.tilt_scale_z = i32::from(source.byte()?) << 6;
    Ok(())
}

/// Worn-slot remap: a count, then one byte per slot (255 is "none").
fn read_worn_slot_remap(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    let mut slots = vec![];
    for _ in 0..source.byte()? {
        let value = i32::from(source.byte()?);
        slots.push(if value == 255 { -1 } else { value });
    }
    bas.worn_slot_remap = Some(slots);
    Ok(())
}

/// Extra idle sequences: a count, then per entry a sequence id and a weight.
fn read_extra_idles(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    let mut ids = vec![];
    let mut weights = vec![];
    for _ in 0..source.byte()? {
        ids.push(source.smart_id()?);
        let weight = i32::from(source.byte()?);
        weights.push(weight);
        bas.idle_weight_total = bas.idle_weight_total.wrapping_add(weight);
    }
    bas.extra_seq_ids = Some(ids);
    bas.idle_weights = Some(weights);
    Ok(())
}

/// One entry of the turn-speed table: a slot index, then its speed.
fn read_wear_turn_speed(source: Input<Error>, bas: &mut Bas, _: Slot) -> Result<()> {
    let index = usize::from(source.byte()?);
    let speeds = bas.wear_turn_speeds.get_or_insert_with(Vec::new);
    if speeds.len() <= index {
        speeds.resize(index + 1, 0);
    }
    speeds[index] = i32::from(source.short()?);
    Ok(())
}

/// One row of a per-slot table: a slot index, then `width` signed shorts.
fn read_row(
    source: Input<Error>,
    table: &mut Option<Vec<Option<Vec<i32>>>>,
    width: usize,
) -> Result<()> {
    let index = usize::from(source.byte()?);
    let table = table.get_or_insert_with(Vec::new);
    if table.len() <= index {
        table.resize(index + 1, None);
    }
    let mut row = vec![];
    for _ in 0..width {
        row.push(i32::from(source.signed_short()?));
    }
    table[index] = Some(row);
    Ok(())
}

impl Bas {
    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut bas = Self {
            raw: bytes.into(),
            ..Self::default()
        };
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "BAS",
            id: i64::from(id),
        };
        BAS_OPCODES.run(&mut packet, &mut bas, record)?;
        bas.consumed = packet.pos;
        Ok(bas)
    }

    /// The movement tuning as the entity movement code consumes it.
    pub fn movement(&self) -> crate::types910::movement::Bas {
        crate::types910::movement::Bas {
            walk_speed: self.walkspeed,
            turn_accel: self.turn_accel,
            turn_max: self.turn_max,
            roll_accel: self.roll_accel,
            roll_max: self.roll_max,
            roll_target: self.roll_target,
            pitch_accel: self.pitch_accel,
            pitch_max: self.pitch_max,
            pitch_target: self.pitch_target,
        }
    }
}
