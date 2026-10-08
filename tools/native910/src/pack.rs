//! Single-file `.js5` runtime pack reader.
//!
//! A pack (as written by the server's `Js5.packArchive`) is the raw
//! archive-index container at offset 0, then every group's container
//! concatenated in index order, then an `index.group_count` big-endian `u32`
//! trailer of per-group stored lengths (`0` = absent group).
//!
//! v1 reads one pack file exactly as laid out. There is deliberately no patch
//! overlay: 910 is the only revision and the runtime pack is the truth. Patch
//! semantics (if repacking ever needs them) will be a milestone-4 design
//! decision, not an implicit merge here.

use crate::error::{NativeError, Result};
use crate::js5::{ArchiveIndex, decompress, unpack_group};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// A decoded `.js5` pack: the archive index plus each present group's byte
/// range within the retained file image.
#[derive(Debug)]
pub struct PackArchive {
    file: Vec<u8>,
    index: ArchiveIndex,
    ranges: BTreeMap<u32, Option<(usize, usize)>>,
}

/// Container header sizes (without / with the optional 2-byte version trailer)
/// for the index container at offset 0.
fn index_container_sizes(file: &[u8]) -> Result<(usize, usize)> {
    if file.len() < 5 {
        return Err(NativeError::Invalid(
            "pack file too short for an index container header".to_string(),
        ));
    }
    let compression = file[0];
    let declared = u32::from_be_bytes([file[1], file[2], file[3], file[4]]);
    let declared = usize::try_from(declared)
        .map_err(|_| NativeError::Invalid("index container length overflow".to_string()))?;
    let header: usize = match compression {
        0 => 1 + 4,
        1..=3 => 1 + 4 + 4,
        other => {
            return Err(NativeError::Invalid(format!(
                "unsupported index container compression: {other}"
            )));
        }
    };
    let base = header
        .checked_add(declared)
        .ok_or_else(|| NativeError::Invalid("index container size overflow".to_string()))?;
    let with_trailer = base
        .checked_add(2)
        .ok_or_else(|| NativeError::Invalid("index container size overflow".to_string()))?;
    Ok((base, with_trailer))
}

impl PackArchive {
    /// Read and decode a `.js5` pack from disk.
    pub fn open(path: &Path) -> Result<Self> {
        let file = fs::read(path).map_err(NativeError::Io)?;
        Self::from_bytes(file)
            .map_err(|error| NativeError::Invalid(format!("{}: {error}", path.display())))
    }

    /// Decode an in-memory pack image.
    pub fn from_bytes(file: Vec<u8>) -> Result<Self> {
        let index_bytes = decompress(&file)?;
        let index = ArchiveIndex::decode(&index_bytes)?;
        let count = index.group_count;

        let trailer_len = count
            .checked_mul(4)
            .ok_or_else(|| NativeError::Invalid("trailer length overflow".to_string()))?;
        let trailer_off = file.len().checked_sub(trailer_len).ok_or_else(|| {
            NativeError::Invalid("pack file too short for its length trailer".to_string())
        })?;

        let mut lengths = Vec::with_capacity(count);
        let mut total = 0_usize;
        for position in 0..count {
            let off = trailer_off + position * 4;
            let len = u32::from_be_bytes([file[off], file[off + 1], file[off + 2], file[off + 3]]);
            let len = usize::try_from(len)
                .map_err(|_| NativeError::Invalid("group length overflow".to_string()))?;
            total = total
                .checked_add(len)
                .ok_or_else(|| NativeError::Invalid("group length sum overflow".to_string()))?;
            lengths.push(len);
        }

        let data_start = trailer_off.checked_sub(total).ok_or_else(|| {
            NativeError::Invalid("pack group lengths exceed available bytes".to_string())
        })?;
        if data_start == 0 {
            return Err(NativeError::Invalid(
                "pack data start computed as zero".to_string(),
            ));
        }
        let (size_plain, size_trailer) = index_container_sizes(&file)?;
        if data_start != size_plain && data_start != size_trailer {
            return Err(NativeError::Invalid(format!(
                "pack sanity identity failed: data starts at {data_start}, index container is \
                 {size_plain} or {size_trailer} bytes"
            )));
        }

        if index.group_id.len() != count {
            return Err(NativeError::Invalid(format!(
                "index lists {} groups but declares {count}",
                index.group_id.len()
            )));
        }

        let mut ranges = BTreeMap::new();
        let mut cursor = data_start;
        for (position, group) in index.group_id.iter().enumerate() {
            let len = lengths[position];
            if len == 0 {
                ranges.insert(*group, None);
                continue;
            }
            let end = cursor
                .checked_add(len)
                .ok_or_else(|| NativeError::Invalid("group slice overflow".to_string()))?;
            if end > trailer_off {
                return Err(NativeError::Invalid(format!(
                    "group {group} container exceeds the trailer offset"
                )));
            }
            ranges.insert(*group, Some((cursor, end)));
            cursor = end;
        }
        if cursor != trailer_off {
            return Err(NativeError::Invalid(format!(
                "pack group bytes stop at {cursor}, trailer starts at {trailer_off}"
            )));
        }

        Ok(Self {
            file,
            index,
            ranges,
        })
    }

    /// The decoded archive index.
    #[must_use]
    pub fn index(&self) -> &ArchiveIndex {
        &self.index
    }

    /// All group ids in index order.
    pub fn group_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranges
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
    }

    /// Whether the group id is listed in the index (even with zero length).
    #[must_use]
    pub fn has_group(&self, group: u32) -> bool {
        self.ranges.contains_key(&group)
    }

    /// Raw container bytes for a present group; `None` when absent or zero-length.
    #[must_use]
    pub fn group_container(&self, group: u32) -> Option<&[u8]> {
        let (start, end) = (*self.ranges.get(&group)?)?;
        Some(&self.file[start..end])
    }

    /// Decompress and split a present group into its file map. `Ok(None)` when
    /// the group is absent, so callers distinguish "no such group" from corrupt.
    pub fn group_files(&self, group: u32) -> Result<Option<BTreeMap<u32, Vec<u8>>>> {
        match self.group_container(group) {
            Some(container) => Ok(Some(unpack_group(&self.index, group, container)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::ByteWriter;

    fn container0(payload: &[u8]) -> Vec<u8> {
        let mut writer = ByteWriter::default();
        writer.p1(0);
        writer.p4s(payload.len() as i32);
        writer.pdata(payload);
        writer.data
    }

    /// Protocol-6 index: one dense file per group, file id = group parity stub.
    fn index_container(groups: &[u32]) -> Vec<u8> {
        let mut writer = ByteWriter::default();
        writer.p1(6);
        writer.p4s(0);
        writer.p1(0);
        writer.p2(groups.len() as u16);
        let mut last = 0_u32;
        for group in groups {
            writer.p2((group - last) as u16);
            last = *group;
        }
        for _ in groups {
            writer.p4s(0);
        }
        for _ in groups {
            writer.p4s(0);
        }
        for _ in groups {
            writer.p2(1);
        }
        for _ in groups {
            writer.p2(0);
        }
        container0(&writer.data)
    }

    fn build_pack(index: &[u8], groups: &[Option<Vec<u8>>]) -> Vec<u8> {
        let mut out = Vec::from(index);
        let mut lengths = Vec::new();
        for group in groups {
            match group {
                Some(bytes) => {
                    out.extend_from_slice(bytes);
                    lengths.push(bytes.len() as u32);
                }
                None => lengths.push(0),
            }
        }
        for len in lengths {
            out.extend_from_slice(&len.to_be_bytes());
        }
        out
    }

    #[test]
    fn pack_layout_roundtrips_with_absent_groups() {
        let g0 = container0(b"AAAA");
        let pack = build_pack(
            &index_container(&[0, 1, 2]),
            &[Some(g0.clone()), None, Some(container0(b"CC"))],
        );
        let archive = PackArchive::from_bytes(pack).unwrap();
        assert_eq!(archive.group_ids().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(archive.group_container(0), Some(g0.as_slice()));
        assert_eq!(archive.group_container(1), None);
        assert!(archive.has_group(1));
        assert!(!archive.has_group(9));
        let files = archive.group_files(0).unwrap().unwrap();
        assert_eq!(files[&0], b"AAAA");
        assert!(archive.group_files(1).unwrap().is_none());
    }

    #[test]
    fn pack_rejects_broken_sanity_identity() {
        let mut pack = Vec::from(index_container(&[0]).as_slice());
        pack.push(0xFF);
        let g0 = container0(b"AAAA");
        pack.extend_from_slice(&g0);
        pack.extend_from_slice(&(g0.len() as u32).to_be_bytes());
        let error = PackArchive::from_bytes(pack).unwrap_err();
        assert!(
            format!("{error}").contains("sanity identity"),
            "unexpected error: {error}"
        );
    }
}
