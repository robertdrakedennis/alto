//! Animation sequence configs: the frame list and its per-frame lengths.
//! Everything else a sequence stores (sounds, chat frames, hand items) is
//! read past and dropped.

use std::collections::BTreeMap;

use anyhow::Result;

use super::{decode_records, load_all, ParamValue, SEQ_ARCHIVE, SEQ_GROUP_BITS};
use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// One animation sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seq {
    /// Config id.
    pub id: u32,
    /// Frame ids: a low word, then a high word shifted into bits 16 and up.
    pub frame_ids: Vec<u32>,
    /// Length of each frame in ticks.
    pub frame_lengths: Vec<u16>,
    /// Parameter overrides.
    pub params: Vec<(i32, ParamValue)>,
}

/// Sequence opcodes of this revision.
static SEQ_OPCODES: Table<Seq, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_frames)),
        // Replay offset.
        Entry::new(at(2), Rule::Skip(&[Field::Short])),
        // Priority.
        Entry::new(at(5), Rule::Skip(&[Field::Byte])),
        // Main hand and off hand items.
        Entry::new(span(6, 7), Rule::Skip(&[Field::Short])),
        // Replay count, moving and stationary behaviour, restart mode.
        Entry::new(span(8, 11), Rule::Skip(&[Field::Byte])),
        Entry::new(at(12), Rule::Custom(skip_frame_pairs_byte_count)),
        Entry::new(at(13), Rule::Custom(skip_frame_sounds)),
        // Flags without payload.
        Entry::new(span(14, 15), Rule::Skip(&[])),
        Entry::new(at(16), Rule::Skip(&[])),
        Entry::new(at(18), Rule::Skip(&[])),
        Entry::new(at(19), Rule::Skip(&[Field::Byte, Field::Byte])),
        Entry::new(
            at(20),
            Rule::Skip(&[Field::Byte, Field::Short, Field::Short]),
        ),
        Entry::new(at(22), Rule::Skip(&[Field::Byte])),
        Entry::new(span(23, 25), Rule::Skip(&[Field::Short])),
        Entry::new(at(112), Rule::Custom(skip_frame_pairs_short_count)),
        Entry::new(at(119), Rule::Skip(&[Field::Short, Field::Byte])),
        Entry::new(
            at(120),
            Rule::Skip(&[Field::Short, Field::Short, Field::Short]),
        ),
        Entry::new(
            at(249),
            Rule::Custom(|s, q, _| super::read_params(s, &mut q.params)),
        ),
    ],
    Unknown::Reject,
);

/// A frame count, then that many lengths, low words and high words.
fn read_frames(source: Input<anyhow::Error>, seq: &mut Seq, _: Slot) -> Result<()> {
    let count = source.short()?;
    let mut lengths = Vec::new();
    for _ in 0..count {
        lengths.push(source.short()?);
    }
    let mut ids = Vec::new();
    for _ in 0..count {
        ids.push(u32::from(source.short()?));
    }
    for frame in ids.iter_mut() {
        let high = u32::from(source.short()?);
        *frame |= high << 16;
    }
    seq.frame_lengths = lengths;
    seq.frame_ids = ids;
    Ok(())
}

fn skip_shorts(source: Input<anyhow::Error>, count: u32) -> Result<()> {
    for _ in 0..count {
        source.short()?;
    }
    Ok(())
}

/// A table of `count` low words followed by `count` high words; the count is
/// one byte.
fn skip_frame_pairs_byte_count(source: Input<anyhow::Error>, _: &mut Seq, _: Slot) -> Result<()> {
    let count = u32::from(source.byte()?);
    skip_shorts(source, count * 2)
}

/// [`skip_frame_pairs_byte_count`] with a two-byte count.
fn skip_frame_pairs_short_count(source: Input<anyhow::Error>, _: &mut Seq, _: Slot) -> Result<()> {
    let count = u32::from(source.short()?);
    skip_shorts(source, count * 2)
}

/// Per-frame sound table: a frame count, then per frame a sound count, and
/// for a non-empty frame a 24-bit first sound followed by 16-bit ones.
fn skip_frame_sounds(source: Input<anyhow::Error>, _: &mut Seq, _: Slot) -> Result<()> {
    let frames = source.short()?;
    for _ in 0..frames {
        let sounds = source.byte()?;
        if sounds > 0 {
            source.medium()?;
            for _ in 1..sounds {
                source.short()?;
            }
        }
    }
    Ok(())
}

/// Decode one seq entry. An unknown opcode is an error naming the id and the
/// opcode.
pub fn decode_seq(id: u32, data: &[u8]) -> Result<Seq> {
    let record = Record {
        kind: "seq",
        id: i64::from(id),
    };
    let blank = Seq {
        id,
        frame_ids: Vec::new(),
        frame_lengths: Vec::new(),
        params: Vec::new(),
    };
    decode_record(&SEQ_OPCODES, "config", record, data, blank)
}

/// All seq configs, keyed by id.
#[derive(Clone, Debug, Default)]
pub struct SeqStore {
    entries: BTreeMap<u32, Seq>,
}

impl SeqStore {
    /// A store over decoded sequences, so tests can inject synthetic ones.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_map(entries: BTreeMap<u32, Seq>) -> Self {
        Self { entries }
    }

    /// Decode every file of the seq archive (`id = group << 7 | file`).
    pub fn load(pack: &Pack) -> Result<Self> {
        Ok(Self {
            entries: load_all(pack, SEQ_ARCHIVE, SEQ_GROUP_BITS, decode_seq)?,
        })
    }

    /// [`Self::load`] over the seq archive already read (ids as keys).
    pub fn from_records(files: &BTreeMap<i32, Vec<u8>>) -> Result<Self> {
        Ok(Self {
            entries: decode_records(SEQ_ARCHIVE, files, decode_seq)?,
        })
    }

    /// Look up one seq by id.
    pub fn get(&self, id: u32) -> Option<&Seq> {
        self.entries.get(&id)
    }

    /// Number of decoded seqs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no seqs.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(id, seq)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Seq)> {
        self.entries.iter()
    }
}
