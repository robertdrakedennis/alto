//! Animation sequence configs as the interface scripts read them: the frame
//! list, its per-frame lengths and the parameters, taken from the full
//! [`Sequence`] decode.

use std::collections::BTreeMap;

use anyhow::Result;

use super::{decode_records, load_all, ParamValue, SEQ_ARCHIVE, SEQ_GROUP_BITS};
use crate::cache::Pack;
use crate::types910::sequence_types::Sequence;

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

/// Decode one seq entry. An unknown opcode is an error naming the id and the
/// opcode.
pub fn decode_seq(id: u32, data: &[u8]) -> Result<Seq> {
    let sequence =
        Sequence::decode(id as i32, data).map_err(|e| anyhow::anyhow!("seq {id}: {e:?}"))?;
    Ok(Seq {
        id,
        frame_ids: sequence
            .frame_ids
            .unwrap_or_default()
            .into_iter()
            .map(|v| v as u32)
            .collect(),
        frame_lengths: sequence
            .frames
            .unwrap_or_default()
            .into_iter()
            .map(|v| v as u16)
            .collect(),
        params: sequence.params,
    })
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
