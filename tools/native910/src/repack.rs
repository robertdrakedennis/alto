//! Lossless replacement/addition of script and interface groups. Unchanged builds
//! return the original pack image. Changed containers are stored uncompressed.
//! Index metadata is retained; digest-bearing indexes require a digest writer
//! and are explicitly rejected rather than published with stale hashes.
use crate::error::{NativeError, Result};
use crate::js5::decompress;
use crate::pack::PackArchive;
use crate::packet::{ByteWriter, Packet};
use std::collections::BTreeMap;

#[derive(Default)]
struct Group {
    name: i32,
    crc: i32,
    uncompressed_crc: i32,
    length: i32,
    uncompressed_length: i32,
    version: i32,
    file_names: BTreeMap<u32, i32>,
    container: Vec<u8>,
}

fn invalid(message: &str) -> NativeError {
    NativeError::Invalid(message.into())
}

fn number(packet: &mut Packet<'_>, protocol: u8) -> Result<u32> {
    if protocol >= 7 {
        u32::try_from(packet.gsmart2or4null()?).map_err(|_| invalid("negative index number"))
    } else {
        Ok(u32::from(packet.g2()?))
    }
}

fn put_number(writer: &mut ByteWriter, value: u32, protocol: u8) -> Result<()> {
    if protocol >= 7 && value >= 32768 {
        if value >= 0x7fff_ffff {
            return Err(invalid("index number exceeds smart range"));
        }
        writer.p4s((value | 0x8000_0000) as i32);
    } else {
        writer.p2(u16::try_from(value).map_err(|_| invalid("index number exceeds u16"))?);
    }
    Ok(())
}

/// CRC-32 used by JS5 containers (IEEE polynomial).
pub fn crc32(data: &[u8]) -> i32 {
    let mut crc = !0_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    (!crc) as i32
}

fn container(data: &[u8]) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    writer.p1(0);
    writer.p4s(i32::try_from(data.len()).map_err(|_| invalid("container too large"))?);
    writer.pdata(data);
    Ok(writer.data)
}

/// Build a script archive, preserving untouched group containers exactly.
/// New numeric IDs use unnamed index entries; existing name hashes are retained.
pub fn scripts(original: &[u8], replacements: &BTreeMap<u32, Vec<u8>>) -> Result<Vec<u8>> {
    scripts_with_metadata(original, replacements, &BTreeMap::new())
}

/// Compiler execution metadata travels in the same JS5 group as its script.
/// Existing metadata is preserved and checked when only file zero changes.
pub fn scripts_with_metadata(
    original: &[u8],
    replacements: &BTreeMap<u32, Vec<u8>>,
    metadata: &BTreeMap<u32, Vec<u8>>,
) -> Result<Vec<u8>> {
    use crate::execution::{METADATA_FILE, SCRIPT_FILE, decode_accounting};
    let book = crate::opcode::OpcodeBook::embedded()?;
    let archive = PackArchive::from_bytes(original.to_vec())?;
    let ids: std::collections::BTreeSet<_> = replacements
        .keys()
        .chain(metadata.keys())
        .copied()
        .collect();
    let mut groups = BTreeMap::new();
    for id in ids {
        let mut files = archive.group_files(id)?.unwrap_or_default();
        if let Some(bytes) = replacements.get(&id) {
            files.insert(SCRIPT_FILE, bytes.clone());
        }
        if let Some(bytes) = metadata.get(&id) {
            files.insert(METADATA_FILE, bytes.clone());
        }
        let bytes = files
            .get(&SCRIPT_FILE)
            .ok_or_else(|| invalid("execution metadata lacks script file"))?;
        let script = crate::script::decode_script(bytes, &book)?;
        decode_accounting(
            i32::try_from(id).map_err(|_| invalid("script ID out of range"))?,
            bytes,
            files.get(&METADATA_FILE).map(Vec::as_slice),
            &script,
        )?;
        groups.insert(id, files);
    }
    rebuild(original, &groups, false)
}

/// Replace or add interface components, retaining all unedited siblings.
/// Component identities must fit the positive packed component namespace.
pub fn interfaces(
    original: &[u8],
    replacements: &BTreeMap<(u32, u32), Vec<u8>>,
) -> Result<Vec<u8>> {
    let archive = PackArchive::from_bytes(original.to_vec())?;
    let mut groups = BTreeMap::new();
    for ((group, file), bytes) in replacements {
        let reference = crate::xref::ComponentRef {
            iface: (*group)
                .try_into()
                .map_err(|_| invalid("interface ID exceeds i32"))?,
            child: (*file)
                .try_into()
                .map_err(|_| invalid("component ID exceeds i32"))?,
        };
        let packed = crate::xref::pack_component(reference)
            .filter(|packed| *packed >= i32::default())
            .ok_or_else(|| invalid("component identity exceeds packed address range"))?;
        crate::interface::decode_component(bytes, packed)?;
        if !groups.contains_key(group) {
            groups.insert(*group, archive.group_files(*group)?.unwrap_or_default());
        }
        groups
            .get_mut(group)
            .ok_or_else(|| invalid("missing component group"))?
            .insert(*file, bytes.clone());
    }
    rebuild(original, &groups, false)
}

/// One stripe is sufficient to encode any ordered file map. Footer deltas
/// are signed: a shorter file after a longer one must retain its negative delta.
fn group_payload(files: &BTreeMap<u32, Vec<u8>>) -> Result<Vec<u8>> {
    const SINGLE_FILE_COUNT: usize = 1;
    const SINGLE_STRIPE_COUNT: u8 = 1;
    if files.is_empty() {
        return Err(invalid("cannot publish an empty group"));
    }
    let mut writer = ByteWriter::default();
    for bytes in files.values() {
        writer.pdata(bytes);
    }
    if files.len() > SINGLE_FILE_COUNT {
        let mut previous_length = i32::default();
        for bytes in files.values() {
            let length =
                i32::try_from(bytes.len()).map_err(|_| invalid("file length exceeds i32"))?;
            writer.p4s(
                length
                    .checked_sub(previous_length)
                    .ok_or_else(|| invalid("file delta overflow"))?,
            );
            previous_length = length;
        }
        writer.p1(SINGLE_STRIPE_COUNT);
    }
    Ok(writer.data)
}

fn rebuild(
    original: &[u8],
    replacements: &BTreeMap<u32, BTreeMap<u32, Vec<u8>>>,
    scripts_only: bool,
) -> Result<Vec<u8>> {
    let archive = PackArchive::from_bytes(original.to_vec())?;
    let mut changed = BTreeMap::new();
    for (id, files) in replacements {
        if archive.group_files(*id)?.as_ref() != Some(files) {
            changed.insert(*id, files);
        }
    }
    if changed.is_empty() {
        return Ok(original.to_vec());
    }
    let index = decompress(original)?;
    let mut p = Packet::new(&index);
    let protocol = p.g1()?;
    let version = if protocol >= 6 { p.g4s()? } else { 0 };
    let flags = p.g1()?;
    if flags & !0x0d != 0 {
        return Err(invalid(
            "pack writer supports name/length/checksum flags; digest or unknown flags require another writer",
        ));
    }
    let count = number(&mut p, protocol)?;
    let mut groups = BTreeMap::new();
    let mut id = 0_u32;
    for _ in 0..count {
        id = id
            .checked_add(number(&mut p, protocol)?)
            .ok_or_else(|| invalid("group ID overflow"))?;
        if groups
            .insert(
                id,
                Group {
                    container: archive.group_container(id).unwrap_or_default().to_vec(),
                    ..Group::default()
                },
            )
            .is_some()
        {
            return Err(invalid("duplicate group ID"));
        }
    }
    if flags & 1 != 0 {
        for g in groups.values_mut() {
            g.name = p.g4s()?;
        }
    }
    for g in groups.values_mut() {
        g.crc = p.g4s()?;
    }
    if flags & 8 != 0 {
        for g in groups.values_mut() {
            g.uncompressed_crc = p.g4s()?;
        }
    }
    if flags & 4 != 0 {
        for g in groups.values_mut() {
            g.length = p.g4s()?;
            g.uncompressed_length = p.g4s()?;
        }
    }
    for g in groups.values_mut() {
        g.version = p.g4s()?;
    }
    let mut file_counts = Vec::new();
    for _ in 0..count {
        let file_count = number(&mut p, protocol)?;
        if scripts_only && file_count != 1 {
            return Err(invalid("script writer requires one file per group"));
        }
        file_counts.push(file_count);
    }
    for (group, file_count) in groups.values_mut().zip(file_counts) {
        let mut file = u32::default();
        for _ in 0..file_count {
            file = file
                .checked_add(number(&mut p, protocol)?)
                .ok_or_else(|| invalid("file ID overflow"))?;
            if scripts_only && file != 0 {
                return Err(invalid("script writer requires file zero"));
            }
            if group.file_names.insert(file, i32::default()).is_some() {
                return Err(invalid("duplicate file ID"));
            }
        }
    }
    if flags & 1 != 0 {
        for g in groups.values_mut() {
            for name in g.file_names.values_mut() {
                *name = p.g4s()?;
            }
        }
    }
    if !p.is_empty() {
        return Err(invalid("trailing index metadata"));
    }
    for (id, files) in &changed {
        let g = groups.entry(*id).or_insert_with(|| Group {
            name: -1,
            ..Group::default()
        });
        let bytes = group_payload(files)?;
        g.file_names.retain(|file, _| files.contains_key(file));
        for file in files.keys() {
            g.file_names.entry(*file).or_insert(-1);
        }
        g.container = container(&bytes)?;
        g.crc = crc32(&g.container);
        g.uncompressed_crc = crc32(&bytes);
        g.length = i32::try_from(g.container.len()).map_err(|_| invalid("container length"))?;
        g.uncompressed_length =
            i32::try_from(bytes.len()).map_err(|_| invalid("group payload length"))?;
        g.version = g
            .version
            .checked_add(1)
            .ok_or_else(|| invalid("group version exhausted"))?;
    }
    let mut w = ByteWriter::default();
    w.p1(protocol);
    if protocol >= 6 {
        w.p4s(
            version
                .checked_add(1)
                .ok_or_else(|| invalid("index version exhausted"))?,
        );
    }
    w.p1(flags);
    put_number(
        &mut w,
        u32::try_from(groups.len()).map_err(|_| invalid("group count"))?,
        protocol,
    )?;
    let mut previous = 0;
    for id in groups.keys() {
        put_number(&mut w, id - previous, protocol)?;
        previous = *id;
    }
    if flags & 1 != 0 {
        for g in groups.values() {
            w.p4s(g.name);
        }
    }
    for g in groups.values() {
        w.p4s(g.crc);
    }
    if flags & 8 != 0 {
        for g in groups.values() {
            w.p4s(g.uncompressed_crc);
        }
    }
    if flags & 4 != 0 {
        for g in groups.values() {
            w.p4s(g.length);
            w.p4s(g.uncompressed_length);
        }
    }
    for g in groups.values() {
        w.p4s(g.version);
    }
    for g in groups.values() {
        put_number(
            &mut w,
            g.file_names
                .len()
                .try_into()
                .map_err(|_| invalid("file count"))?,
            protocol,
        )?;
    }
    for g in groups.values() {
        let mut previous = u32::default();
        for file in g.file_names.keys() {
            put_number(&mut w, file - previous, protocol)?;
            previous = *file;
        }
    }
    if flags & 1 != 0 {
        for g in groups.values() {
            for name in g.file_names.values() {
                w.p4s(*name);
            }
        }
    }
    let mut output = container(&w.data)?;
    for g in groups.values() {
        output.extend_from_slice(&g.container);
    }
    for g in groups.values() {
        output.extend_from_slice(
            &u32::try_from(g.container.len())
                .map_err(|_| invalid("group size"))?
                .to_be_bytes(),
        );
    }
    let rebuilt = PackArchive::from_bytes(output.clone())?;
    for (id, expected) in changed {
        if rebuilt.group_files(id)?.as_ref() != Some(expected) {
            return Err(invalid("rebuilt group verification failed"));
        }
    }
    Ok(output)
}
