//! js5 container primitives: decompression, archive-index decode, group unpack.
//!
//! These mirror the 910 client's loader semantics: container header
//! (`compression u8`, `compressed_len i32`, optional `uncompressed_len i32`),
//! bzip2 payloads without their `BZh1` framing (re-attached here), gzip, and
//! LZMA-alone payloads in the client's 14-byte property layout.

use crate::error::{NativeError, Result};
use crate::packet::Packet;
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

/// A decoded archive index: group ids, per-group file counts, and the sparse
/// file-id tables for groups whose file ids are not dense.
#[derive(Clone, Debug)]
pub struct ArchiveIndex {
    /// Index version.
    pub version: i32,
    /// Number of groups listed.
    pub group_count: usize,
    /// Group ids in index order.
    pub group_id: Vec<u32>,
    /// Highest group id + 1, or 0 for an empty index.
    pub capacity: usize,
    /// Group checksums by group id.
    pub group_checksums: Vec<i32>,
    /// Group digests by group id when the index carries whirlpool digests
    /// (flag 2).
    pub group_digests: Option<Vec<Option<[u8; 64]>>>,
    /// Group versions by group id.
    pub group_versions: Vec<i32>,
    /// Group sizes (file counts) by group id.
    pub group_sizes: Vec<u32>,
    /// Group capacities (highest file id + 1) by group id.
    pub group_capacities: Vec<u32>,
    /// Group name hashes by group id (`-1` unnamed), when named.
    pub group_name_hashes: Option<Vec<i32>>,
    /// File name hashes by group id then file id (`-1` unnamed), when named.
    pub file_name_hashes: Option<Vec<Vec<i32>>>,
    per_group: Vec<PerGroup>,
}

/// Loader hardening bounds. 910 runtime packs are small (the whole scripts
/// pack is ~3 MB); anything past these bounds is corrupt input, rejected
/// instead of aborting the process on a giant allocation. The corpus gate
/// proves real data fits comfortably underneath.
const MAX_GROUPS: usize = 1 << 20;
const MAX_FILES_PER_GROUP: usize = 1 << 20;
const MAX_DECOMPRESSED_BYTES: usize = 64 << 20;

/// Per-group layout: file count plus the sparse file-id table (`None` when the
/// group's file ids are dense `0..count`).
#[derive(Clone, Debug)]
struct PerGroup {
    file_count: u32,
    file_ids: Option<Vec<u32>>,
}

impl ArchiveIndex {
    /// Decode an archive-index payload (already decompressed).
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut packet = Packet::new(data);
        let protocol = packet.g1()?;
        if !(5..=7).contains(&protocol) {
            return Err(NativeError::Invalid(format!(
                "unsupported archive index protocol: {protocol}"
            )));
        }

        let version = if protocol >= 6 { packet.g4s()? } else { 0 };
        let flags = packet.g1()?;
        let has_names = (flags & 1) != 0;
        let has_digests = (flags & 2) != 0;
        let has_lengths = (flags & 4) != 0;
        let has_uncompressed_checksums = (flags & 8) != 0;

        let group_count = if protocol >= 7 {
            usize::try_from(packet.gsmart2or4null()?).map_err(|_| invalid("group count"))?
        } else {
            usize::from(packet.g2()?)
        };
        if group_count > MAX_GROUPS {
            return Err(invalid("group count exceeds loader bound"));
        }

        let mut group_id = Vec::with_capacity(group_count);
        let mut last_group = 0_u32;
        let mut max_group = 0_u32;
        for _ in 0..group_count {
            let delta = if protocol >= 7 {
                u32::try_from(packet.gsmart2or4null()?).map_err(|_| invalid("group delta"))?
            } else {
                u32::from(packet.g2()?)
            };
            last_group = last_group
                .checked_add(delta)
                .ok_or_else(|| invalid("group id overflow"))?;
            max_group = max_group.max(last_group);
            group_id.push(last_group);
        }

        let array_size = usize::try_from(max_group)
            .map_err(|_| invalid("group id too large"))?
            .saturating_add(1);
        if array_size > MAX_GROUPS {
            return Err(invalid("group id exceeds loader bound"));
        }

        let capacity = if group_count == 0 { 0 } else { array_size };
        let group_name_hashes = if has_names {
            let mut hashes = vec![-1_i32; capacity];
            for group in &group_id {
                hashes[*group as usize] = packet.g4s()?;
            }
            Some(hashes)
        } else {
            None
        };
        let mut group_checksums = vec![0_i32; capacity];
        for group in &group_id {
            group_checksums[*group as usize] = packet.g4s()?;
        }
        if has_uncompressed_checksums {
            for _ in 0..group_count {
                let _ = packet.g4s()?;
            }
        }
        let group_digests = if has_digests {
            let mut digests = vec![None; capacity];
            for group in &group_id {
                let bytes = packet.gdata(64, "digest")?;
                let mut digest = [0_u8; 64];
                digest.copy_from_slice(&bytes);
                digests[*group as usize] = Some(digest);
            }
            Some(digests)
        } else {
            None
        };
        if has_lengths {
            for _ in &group_id {
                let _ = packet.g4s()?;
                let _ = packet.g4s()?;
            }
        }
        let mut group_versions = vec![0_i32; capacity];
        for group in &group_id {
            group_versions[*group as usize] = packet.g4s()?;
        }

        let mut group_size = vec![0_u32; array_size];
        for group in &group_id {
            let index = usize::try_from(*group).map_err(|_| invalid("group index"))?;
            group_size[index] = if protocol >= 7 {
                u32::try_from(packet.gsmart2or4null()?).map_err(|_| invalid("group size"))?
            } else {
                u32::from(packet.g2()?)
            };
        }

        let mut per_group = vec![
            PerGroup {
                file_count: 0,
                file_ids: None,
            };
            array_size
        ];
        for group in &group_id {
            let index = usize::try_from(*group).map_err(|_| invalid("group index"))?;
            let size = group_size[index];
            if usize::try_from(size).map_err(|_| invalid("file count too large"))?
                > MAX_FILES_PER_GROUP
            {
                return Err(invalid("file count exceeds loader bound"));
            }
            let mut ids = Vec::with_capacity(size as usize);
            let mut last = 0_u32;
            let mut max_id = 0_u32;
            for _ in 0..size {
                let delta = if protocol >= 7 {
                    u32::try_from(packet.gsmart2or4null()?).map_err(|_| invalid("file delta"))?
                } else {
                    u32::from(packet.g2()?)
                };
                last = last
                    .checked_add(delta)
                    .ok_or_else(|| invalid("file id overflow"))?;
                max_id = max_id.max(last);
                ids.push(last);
            }
            let dense = usize::try_from(max_id)
                .map_err(|_| invalid("file id too large"))?
                .saturating_add(1)
                == size as usize;
            per_group[index] = PerGroup {
                file_count: size,
                file_ids: if dense { None } else { Some(ids) },
            };
        }

        let mut group_sizes = vec![0_u32; capacity];
        let mut group_capacities = vec![0_u32; capacity];
        for group in &group_id {
            let index = *group as usize;
            let entry = &per_group[index];
            group_sizes[index] = entry.file_count;
            group_capacities[index] = match &entry.file_ids {
                Some(ids) => ids.iter().max().map_or(0, |max| max + 1),
                None => entry.file_count,
            };
        }

        let file_name_hashes = if has_names {
            let mut tables = vec![Vec::new(); capacity];
            for group in &group_id {
                let index = usize::try_from(*group).map_err(|_| invalid("group index"))?;
                let count = usize::try_from(per_group[index].file_count)
                    .map_err(|_| invalid("file count too large"))?;
                let mut table = vec![-1_i32; group_capacities[index] as usize];
                for file_index in 0..count {
                    let file = match &per_group[index].file_ids {
                        Some(ids) => ids[file_index] as usize,
                        None => file_index,
                    };
                    table[file] = packet.g4s()?;
                }
                tables[index] = table;
            }
            Some(tables)
        } else {
            None
        };

        Ok(Self {
            version,
            group_count,
            group_id,
            capacity,
            group_checksums,
            group_digests,
            group_versions,
            group_sizes,
            group_capacities,
            group_name_hashes,
            file_name_hashes,
            per_group,
        })
    }

    /// File count for a group.
    pub fn file_count_for_group(&self, group: u32) -> Result<usize> {
        let index = usize::try_from(group).map_err(|_| invalid("group id"))?;
        let count = self
            .per_group
            .get(index)
            .ok_or_else(|| NativeError::Invalid(format!("group {group} out of range")))?
            .file_count;
        usize::try_from(count).map_err(|_| invalid("file count too large"))
    }

    /// File id for a group's positional file index.
    pub fn file_id_for_group_index(&self, group: u32, file_index: usize) -> Result<u32> {
        let index = usize::try_from(group).map_err(|_| invalid("group id"))?;
        let entry = self
            .per_group
            .get(index)
            .ok_or_else(|| NativeError::Invalid(format!("group {group} out of range")))?;
        if file_index >= entry.file_count as usize {
            return Err(NativeError::Invalid(format!(
                "file index {file_index} out of range for group {group}"
            )));
        }
        match &entry.file_ids {
            Some(ids) => Ok(ids[file_index]),
            None => u32::try_from(file_index).map_err(|_| invalid("file id too large")),
        }
    }

    fn file_ids_for_group(&self, group: u32) -> Result<Vec<u32>> {
        let count = self.file_count_for_group(group)?;
        let mut ids = Vec::with_capacity(count);
        for file_index in 0..count {
            ids.push(self.file_id_for_group_index(group, file_index)?);
        }
        Ok(ids)
    }
}

fn invalid(what: &str) -> NativeError {
    NativeError::Invalid(format!("malformed archive index ({what})"))
}

/// Reject a declared output size past the loader bound before decoding into
/// it: a corrupt header claiming gigabytes would otherwise OOM the process.
fn check_decompressed_size(expected: usize, kind: &str) -> Result<()> {
    if expected > MAX_DECOMPRESSED_BYTES {
        return Err(NativeError::Invalid(format!(
            "{kind} declares {expected} output bytes past the loader bound"
        )));
    }
    Ok(())
}

/// Decompress one js5 container (header + payload) to its raw bytes.
pub fn decompress(container: &[u8]) -> Result<Vec<u8>> {
    let mut packet = Packet::new(container);
    let compression = packet.g1()?;
    let compressed_size = usize::try_from(packet.g4s()?)
        .map_err(|_| NativeError::Invalid("negative compressed size".to_string()))?;
    if container.len() < 5 + compressed_size {
        return Err(NativeError::Invalid(format!(
            "container shorter than its header declares: {} < {}",
            container.len(),
            5 + compressed_size
        )));
    }

    match compression {
        0 => Ok(container[5..5 + compressed_size].to_vec()),
        1 => {
            let expected = usize::try_from(packet.g4s()?).map_err(|_| {
                NativeError::Invalid("negative bzip2 uncompressed size".to_string())
            })?;
            check_decompressed_size(expected, "bzip2")?;
            // The payload omits the 4-byte `BZh1` framing the decoder needs.
            let mut framed = Vec::with_capacity(4 + container.len());
            framed.extend_from_slice(b"BZh1");
            framed.extend_from_slice(&container[9..]);
            let mut output = Vec::new();
            BzDecoder::new(Cursor::new(framed))
                .read_to_end(&mut output)
                .map_err(|error| {
                    NativeError::Invalid(format!("bzip2 payload failed to decode: {error}"))
                })?;
            if output.len() != expected {
                return Err(NativeError::Invalid(format!(
                    "bzip2 size mismatch: decoded {} expected {expected}",
                    output.len()
                )));
            }
            Ok(output)
        }
        2 => {
            let expected = usize::try_from(packet.g4s()?)
                .map_err(|_| NativeError::Invalid("negative gzip uncompressed size".to_string()))?;
            check_decompressed_size(expected, "gzip")?;
            let mut output = Vec::new();
            GzDecoder::new(Cursor::new(&container[9..]))
                .read_to_end(&mut output)
                .map_err(|error| {
                    NativeError::Invalid(format!("gzip payload failed to decode: {error}"))
                })?;
            if output.len() != expected {
                return Err(NativeError::Invalid(format!(
                    "gzip size mismatch: decoded {} expected {expected}",
                    output.len()
                )));
            }
            Ok(output)
        }
        3 => {
            let expected = usize::try_from(packet.g4s()?)
                .map_err(|_| NativeError::Invalid("negative lzma uncompressed size".to_string()))?;
            check_decompressed_size(expected, "lzma")?;
            let properties = packet.g1()?;
            let dictionary = packet.gdata(4, "lzma dictionary")?;
            // lzma-rs takes an LZMA-alone stream: 13-byte header + payload.
            let mut alone = Vec::with_capacity(13 + container.len());
            alone.push(properties);
            alone.extend_from_slice(&dictionary);
            alone.extend_from_slice(&(expected as u64).to_le_bytes());
            alone.extend_from_slice(&container[14..]);
            let mut output = Vec::new();
            lzma_rs::lzma_decompress(&mut Cursor::new(alone), &mut output).map_err(|error| {
                NativeError::Invalid(format!("lzma payload failed to decode: {error:?}"))
            })?;
            if output.len() != expected {
                return Err(NativeError::Invalid(format!(
                    "lzma size mismatch: decoded {} expected {expected}",
                    output.len()
                )));
            }
            Ok(output)
        }
        other => Err(NativeError::Invalid(format!(
            "unsupported js5 compression type: {other}"
        ))),
    }
}

/// Decompress a group container and split its payload into the file map. A
/// single-file group is the whole payload; multi-file groups use the client's
/// striped chunk footer (trailing marker byte = chunk count).
pub fn unpack_group(
    index: &ArchiveIndex,
    group: u32,
    container: &[u8],
) -> Result<BTreeMap<u32, Vec<u8>>> {
    let payload = decompress(container)?;
    let file_count = index.file_count_for_group(group)?;
    if file_count == 1 {
        let mut files = BTreeMap::new();
        files.insert(index.file_id_for_group_index(group, 0)?, payload);
        return Ok(files);
    }

    let chunks =
        usize::from(*payload.last().ok_or_else(|| {
            NativeError::Invalid("group payload missing chunk marker".to_string())
        })?);
    let footer_len = chunks
        .checked_mul(file_count)
        .and_then(|size| size.checked_mul(4))
        .ok_or_else(|| NativeError::Invalid("group footer size overflow".to_string()))?;
    if payload.len() < 1 + footer_len {
        return Err(NativeError::Invalid(
            "group payload too short for its chunk footer".to_string(),
        ));
    }
    let body_len = payload.len() - 1 - footer_len;

    // Pass 1: recover each file's total size from the footer deltas.
    let mut footer = Packet::with_pos(&payload, body_len)?;
    let mut file_sizes = vec![0_i64; file_count];
    for _ in 0..chunks {
        let mut chunk_len = 0_i64;
        for size in &mut file_sizes {
            chunk_len = chunk_len
                .checked_add(i64::from(footer.g4s()?))
                .ok_or_else(|| NativeError::Invalid("file chunk size overflow".to_string()))?;
            if chunk_len < 0 {
                return Err(NativeError::Invalid(
                    "negative cumulative file chunk size".to_string(),
                ));
            }
            *size = size
                .checked_add(chunk_len)
                .ok_or_else(|| NativeError::Invalid("file size overflow".to_string()))?;
        }
    }
    let mut files = Vec::with_capacity(file_count);
    for size in &file_sizes {
        files.push(vec![
            0_u8;
            usize::try_from(*size).map_err(|_| {
                NativeError::Invalid("negative expanded file size".to_string())
            })?
        ]);
    }
    // The body stripes fully into the files: claimed sizes must sum to it
    // exactly. Anything else is a corrupt footer, caught here instead of
    // panicking on a short slice below.
    if file_sizes.iter().sum::<i64>() != body_len as i64 {
        return Err(NativeError::Invalid(
            "group file sizes do not sum to the body length".to_string(),
        ));
    }

    // Pass 2: stripe the body bytes into the files.
    let mut footer = Packet::with_pos(&payload, body_len)?;
    let mut written = vec![0_i64; file_count];
    let mut body_pos = 0_usize;
    for _ in 0..chunks {
        let mut chunk_len = 0_i64;
        for (file, out) in files.iter_mut().enumerate() {
            chunk_len = chunk_len
                .checked_add(i64::from(footer.g4s()?))
                .ok_or_else(|| NativeError::Invalid("chunk length overflow".to_string()))?;
            if chunk_len < 0 {
                return Err(NativeError::Invalid(
                    "negative cumulative chunk length".to_string(),
                ));
            }
            let take = usize::try_from(chunk_len)
                .map_err(|_| NativeError::Invalid("negative chunk length".to_string()))?;
            let end = body_pos
                .checked_add(take)
                .ok_or_else(|| NativeError::Invalid("payload offset overflow".to_string()))?;
            if end > body_len {
                return Err(NativeError::Invalid(
                    "group chunk exceeds body length".to_string(),
                ));
            }
            let dst = usize::try_from(written[file])
                .map_err(|_| NativeError::Invalid("negative destination".to_string()))?;
            out[dst..dst + take].copy_from_slice(&payload[body_pos..end]);
            written[file] += chunk_len;
            body_pos = end;
        }
    }

    let file_ids = index.file_ids_for_group(group)?;
    let mut map = BTreeMap::new();
    for (position, bytes) in files.into_iter().enumerate() {
        map.insert(file_ids[position], bytes);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::ByteWriter;

    /// Wrap a payload in a compression-0 container (no version trailer).
    fn container0(payload: &[u8]) -> Vec<u8> {
        let mut writer = ByteWriter::default();
        writer.p1(0);
        writer.p4s(payload.len() as i32);
        writer.pdata(payload);
        writer.data
    }

    /// Protocol-6 index container: `groups` ids, one dense file each.
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
        // File id deltas accumulate from zero: delta 0 -> file id 0.
        for _ in groups {
            writer.p2(0);
        }
        container0(&writer.data)
    }

    #[test]
    fn containers_roundtrip_uncompressed() {
        let payload = b"hello 910";
        let bytes = decompress(&container0(payload)).unwrap();
        assert_eq!(bytes, payload);
        assert!(decompress(&[]).is_err());
        assert!(decompress(&[9, 0, 0, 0, 1, 0]).is_err());
        assert!(decompress(&[0, 0, 0, 0, 99]).is_err());
    }

    #[test]
    fn index_and_single_file_group_unpack() {
        let index = ArchiveIndex::decode(&decompress(&index_container(&[3, 7])).unwrap()).unwrap();
        assert_eq!(index.group_count, 2);
        assert_eq!(index.group_id, vec![3, 7]);
        assert_eq!(index.file_count_for_group(7).unwrap(), 1);
        assert_eq!(index.file_id_for_group_index(7, 0).unwrap(), 0);
        assert!(index.file_count_for_group(9).is_err());

        let files = unpack_group(&index, 7, &container0(b"script-bytes")).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[&0], b"script-bytes");
    }

    #[test]
    fn multi_file_group_restripes_chunks() {
        // Two files, one chunk: body = file0 ++ file1. Footer ints accumulate
        // across files within a chunk, so sizes [4, 6] encode as [4, 2].
        let body = b"AAAABBBBBB";
        let mut writer = ByteWriter::default();
        writer.pdata(body);
        writer.p4s(4);
        writer.p4s(2);
        writer.p1(1);
        let payload = writer.data;

        let mut index_writer = ByteWriter::default();
        index_writer.p1(6);
        index_writer.p4s(0);
        index_writer.p1(0);
        index_writer.p2(1);
        index_writer.p2(5);
        index_writer.p4s(0);
        index_writer.p4s(0);
        index_writer.p2(2);
        // Deltas accumulate from zero: 0 -> id 0, then +1 -> id 1.
        index_writer.p2(0);
        index_writer.p2(1);
        let index =
            ArchiveIndex::decode(&decompress(&container0(&index_writer.data)).unwrap()).unwrap();
        assert_eq!(index.file_count_for_group(5).unwrap(), 2);

        let files = unpack_group(&index, 5, &container0(&payload)).unwrap();
        assert_eq!(files[&0], b"AAAA");
        assert_eq!(files[&1], b"BBBBBB");
    }
}
