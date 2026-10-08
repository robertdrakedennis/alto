//! Master and archive index decoding and integrity checks.

use anyhow::Context;

use rs910_core::whirlpool;

use super::getcrc;

// ---------------------------------------------------------------------------
// Js5MasterIndex / Js5Index
// ---------------------------------------------------------------------------

/// One archive's entry in the master index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterIndexArchive {
    pub crc: i32,
    pub group_count: i32,
    pub version: i32,
    pub whirlpool: [u8; 64],
}

/// The master index: one entry per archive.
#[derive(Clone, Debug)]
pub struct MasterIndex {
    pub archives: Vec<MasterIndexArchive>,
}

impl MasterIndex {
    /// Decode the `255/255` container. RSA signing is off, so the 65-byte
    /// trailer is `0` + the Whirlpool of the entry table, which is checked.
    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        let count = usize::from(*data.get(5).context("js5 master index: short")?);
        let table_end = 6 + count * 80;
        anyhow::ensure!(data.len() >= table_end, "js5 master index: short table");
        let rsa = &data[table_end..];
        anyhow::ensure!(
            rsa.len() == 65,
            "js5 master index: trailer {} != 65",
            rsa.len()
        );
        let digest = whirlpool::compute(&data[5..table_end]);
        anyhow::ensure!(
            rsa[1..65] == digest[..],
            "js5 master index: whirlpool mismatch"
        );
        let mut archives = Vec::with_capacity(count);
        for i in 0..count {
            let p = &data[i * 80 + 6..i * 80 + 86];
            let g4 = |o: usize| i32::from_be_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
            let mut whirlpool = [0_u8; 64];
            whirlpool.copy_from_slice(&p[16..80]);
            archives.push(MasterIndexArchive {
                crc: g4(0),
                version: g4(4),
                group_count: g4(8),
                whirlpool,
            });
        }
        Ok(Self { archives })
    }
}

/// `Js5Index`: the archive index checked against the
/// master index's CRC and Whirlpool, decoded by
/// `native910::js5::ArchiveIndex`.
#[derive(Clone, Debug)]
pub struct Js5Index {
    pub crc: i32,
    pub whirlpool: [u8; 64],
    pub index: native910::js5::ArchiveIndex,
}

impl Js5Index {
    pub fn new(bytes: &[u8], crc: i32, whirlpool: Option<&[u8; 64]>) -> anyhow::Result<Self> {
        let actual = getcrc(bytes);
        anyhow::ensure!(actual == crc, "js5 index crc {actual} != {crc}");
        let digest = whirlpool::compute(bytes);
        if let Some(expected) = whirlpool {
            anyhow::ensure!(digest == *expected, "js5 index whirlpool mismatch");
        }
        let raw = native910::js5::decompress(bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
        let index =
            native910::js5::ArchiveIndex::decode(&raw).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            crc: actual,
            whirlpool: digest,
            index,
        })
    }
}
