//! CPU-side animation sequence configs: frame lists, priorities, blend group
//! and the sound tables. Sound bytes are decoded here; no audio or renderer
//! side effects occur. An opcode outside the table is an error naming the id
//! and the opcode instead of skipping bytes.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};
use std::collections::BTreeMap;

/// A blend group: which skeleton labels a blended sequence affects.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Group {
    pub mask: Option<Vec<bool>>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

/// Blend group opcodes of this revision.
static GROUP_OPCODES: Table<Group, Error> = Table::new(
    &[
        Entry::new(at(2), Rule::Custom(read_mask)),
        Entry::new(at(3), Rule::Custom(skip_layers)),
    ],
    Unknown::Reject,
);

/// A count, then that many mask indices into a 400-entry mask.
fn read_mask(source: Input<Error>, group: &mut Group, _: Slot) -> Result<()> {
    let mut mask = vec![false; 400];
    for _ in 0..source.smart()? {
        let index = source.smart()? as usize;
        *mask
            .get_mut(index)
            .ok_or(Error::Invalid("sequence group mask index"))? = true;
    }
    group.mask = Some(mask);
    Ok(())
}

/// A byte, then a count of `(smart, byte)` entries; read and dropped.
fn skip_layers(source: Input<Error>, _: &mut Group, _: Slot) -> Result<()> {
    source.byte()?;
    for _ in 0..source.smart()? {
        source.smart()?;
        source.byte()?;
    }
    Ok(())
}

impl Group {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut group = Self {
            raw: bytes.into(),
            ..Self::default()
        };
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "sequence group",
            id: -1,
        };
        GROUP_OPCODES.run(&mut packet, &mut group, record)?;
        group.consumed = packet.pos;
        Ok(group)
    }
}

/// One animation sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub id: i32,
    pub frame_ids: Option<Vec<i32>>,
    pub secondary: Option<Vec<i32>>,
    pub frames: Option<Vec<i32>>,
    pub length: i32,
    pub skeletal: i32,
    pub blend: i32,
    pub replayoff: i32,
    pub priority: i32,
    pub mainhand: i32,
    pub offhand: i32,
    pub replaycount: i32,
    pub moving: i32,
    pub stationary: i32,
    pub restart: i32,
    pub extra: bool,
    pub tween: bool,
    /// Percentage the sequence's sounds are scaled to when played by an
    /// entity that is neither the local player nor the active target; `-1`
    /// leaves them at full volume.
    pub remote_volume_percent: i32,
    pub sound: Option<Vec<Option<Vec<i32>>>>,
    pub volume: Option<Vec<i32>>,
    pub minrate: Option<Vec<i32>>,
    pub maxrate: Option<Vec<i32>>,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Sequence {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            frame_ids: None,
            secondary: None,
            frames: None,
            length: 0,
            skeletal: -1,
            blend: -1,
            replayoff: -1,
            priority: 5,
            mainhand: -1,
            offhand: -1,
            replaycount: 99,
            moving: -1,
            stationary: -1,
            restart: 2,
            extra: false,
            tween: false,
            remote_volume_percent: -1,
            sound: None,
            volume: None,
            minrate: None,
            maxrate: None,
            consumed: 0,
            raw: vec![],
        }
    }

    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut sequence = Self::empty(id);
        sequence.raw = bytes.into();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "sequence",
            id: i64::from(id),
        };
        SEQUENCE_OPCODES.run(&mut packet, &mut sequence, record)?;
        sequence.consumed = packet.pos;
        Ok(sequence)
    }

    /// Fill the values that depend on the resolved blend group: the moving
    /// and stationary behaviour default from whether the group has a mask,
    /// and the length is the sum of the frame lengths.
    pub fn post_decode(&mut self, groups: &BTreeMap<i32, Group>) {
        let mask = groups.get(&self.blend).is_some_and(|g| g.mask.is_some());
        if self.moving == -1 {
            self.moving = if mask { 2 } else { 0 }
        }
        if self.stationary == -1 {
            self.stationary = if mask { 2 } else { 0 }
        }
        if let Some(f) = &self.frames {
            self.length = f.iter().fold(0i32, |sum, &v| sum.wrapping_add(v));
        }
    }

    pub fn selection(&self) -> crate::types910::animation_state::Sequence {
        crate::types910::animation_state::Sequence {
            id: self.id,
            frames: self.frame_ids.as_ref().map(Vec::len),
            skeletal: self.skeletal != -1,
            restart: self.restart,
            priority: self.priority,
            stationary: self.stationary,
            moving: self.moving,
        }
    }
}

/// Sequence opcodes of this revision.
static SEQUENCE_OPCODES: Table<Sequence, Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_frames)),
        Entry::new(at(2), Rule::Short(|s, _, v| s.replayoff = i32::from(v))),
        Entry::new(at(5), Rule::Byte(|s, _, v| s.priority = i32::from(v))),
        Entry::new(at(6), Rule::Short(|s, _, v| s.mainhand = i32::from(v))),
        Entry::new(at(7), Rule::Short(|s, _, v| s.offhand = i32::from(v))),
        Entry::new(at(8), Rule::Byte(|s, _, v| s.replaycount = i32::from(v))),
        Entry::new(at(9), Rule::Byte(|s, _, v| s.moving = i32::from(v))),
        Entry::new(at(10), Rule::Byte(|s, _, v| s.stationary = i32::from(v))),
        Entry::new(at(11), Rule::Byte(|s, _, v| s.restart = i32::from(v))),
        Entry::new(
            at(12),
            Rule::Custom(|src, s, _| read_secondary(src, s, false)),
        ),
        Entry::new(at(13), Rule::Custom(read_sounds)),
        Entry::new(at(14), Rule::Flag(|s, _| s.extra = true)),
        Entry::new(at(15), Rule::Flag(|s, _| s.tween = true)),
        Entry::new(at(16), Rule::Skip(&[])),
        Entry::new(at(18), Rule::Skip(&[])),
        Entry::new(at(19), Rule::Custom(|src, s, _| read_volume(src, s, false))),
        Entry::new(at(20), Rule::Custom(|src, s, _| read_rates(src, s, false))),
        Entry::new(
            at(22),
            Rule::Byte(|s, _, v| s.remote_volume_percent = i32::from(v)),
        ),
        Entry::new(at(23), Rule::Skip(&[Field::Short])),
        Entry::new(at(24), Rule::Short(|s, _, v| s.blend = i32::from(v))),
        Entry::new(at(25), Rule::Short(|s, _, v| s.skeletal = i32::from(v))),
        Entry::new(
            at(112),
            Rule::Custom(|src, s, _| read_secondary(src, s, true)),
        ),
        Entry::new(at(119), Rule::Custom(|src, s, _| read_volume(src, s, true))),
        Entry::new(at(120), Rule::Custom(|src, s, _| read_rates(src, s, true))),
        Entry::new(at(249), Rule::Custom(skip_params)),
    ],
    Unknown::Reject,
);

/// A frame count, then that many frame lengths, low words and high words.
fn read_frames(source: Input<Error>, sequence: &mut Sequence, _: Slot) -> Result<()> {
    let count = usize::from(source.short()?);
    sequence.frames = Some(read_words(source, count)?);
    sequence.frame_ids = Some(read_ids(source, count)?);
    Ok(())
}

/// The secondary frame list; its count is one byte, or two with `wide`.
fn read_secondary(source: Input<Error>, sequence: &mut Sequence, wide: bool) -> Result<()> {
    let count = if wide {
        usize::from(source.short()?)
    } else {
        usize::from(source.byte()?)
    };
    sequence.secondary = Some(read_ids(source, count)?);
    Ok(())
}

/// Per-frame sound table: a frame count, then per frame a sound count, and
/// for a non-empty frame a 24-bit first sound followed by 16-bit ones.
fn read_sounds(source: Input<Error>, sequence: &mut Sequence, _: Slot) -> Result<()> {
    let mut frames = vec![];
    for _ in 0..source.short()? {
        let count = usize::from(source.byte()?);
        frames.push(if count == 0 {
            None
        } else {
            let mut row = vec![source.medium()? as i32];
            row.extend(read_words(source, count - 1)?);
            Some(row)
        });
    }
    sequence.sound = Some(frames);
    Ok(())
}

/// One entry of the volume table: a frame index (a byte, or two with
/// `wide`) and its volume. The table starts at full volume.
fn read_volume(source: Input<Error>, sequence: &mut Sequence, wide: bool) -> Result<()> {
    if sequence.volume.is_none() {
        sequence.volume = Some(vec![
            255;
            sequence
                .sound
                .as_ref()
                .ok_or(Error::Invalid("sequence sound not allocated"))?
                .len()
        ]);
    }
    let index = if wide {
        usize::from(source.short()?)
    } else {
        usize::from(source.byte()?)
    };
    let volume = i32::from(source.byte()?);
    *sequence
        .volume
        .as_mut()
        .expect("allocated above")
        .get_mut(index)
        .ok_or(Error::Invalid("sequence sound index"))? = volume;
    Ok(())
}

/// One entry of the playback-rate tables: a frame index (a byte, or two with
/// `wide`), then the slowest and fastest rate. The tables start at 256.
fn read_rates(source: Input<Error>, sequence: &mut Sequence, wide: bool) -> Result<()> {
    if sequence.minrate.is_none() || sequence.maxrate.is_none() {
        let frames = sequence
            .sound
            .as_ref()
            .ok_or(Error::Invalid("sequence sound not allocated"))?
            .len();
        sequence.minrate = Some(vec![256; frames]);
        sequence.maxrate = Some(vec![256; frames]);
    }
    let index = if wide {
        usize::from(source.short()?)
    } else {
        usize::from(source.byte()?)
    };
    let low = i32::from(source.short()?);
    let high = i32::from(source.short()?);
    *sequence
        .minrate
        .as_mut()
        .expect("allocated above")
        .get_mut(index)
        .ok_or(Error::Invalid("sequence rate index"))? = low;
    *sequence
        .maxrate
        .as_mut()
        .expect("allocated above")
        .get_mut(index)
        .ok_or(Error::Invalid("sequence rate index"))? = high;
    Ok(())
}

/// A parameter table, read and dropped.
fn skip_params(source: Input<Error>, _: &mut Sequence, _: Slot) -> Result<()> {
    crate::config::read_params(source, &mut Vec::new())
}

fn read_words(source: Input<Error>, count: usize) -> Result<Vec<i32>> {
    (0..count).map(|_| Ok(i32::from(source.short()?))).collect()
}

/// `count` low words followed by `count` high words, combined into ids.
fn read_ids(source: Input<Error>, count: usize) -> Result<Vec<i32>> {
    let mut ids = read_words(source, count)?;
    for id in &mut ids {
        *id = id.wrapping_add(i32::from(source.short()?).wrapping_shl(16));
    }
    Ok(ids)
}
