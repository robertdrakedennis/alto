//! M3 corpus gates: every 910 enum/struct/param entry decodes with its pack's
//! group math and re-encodes byte-identical.
//!
//! Mirrors `tests/interface_corpus.rs`: each gate walks its runtime pack,
//! round-trips every entry, and reports counts; a missing pack fails the gate
//! (`--features no-pack` reports it ignored). Byte-identity alone cannot catch semantic misreads (a shifted
//! walk over zero-filled regions still round-trips), so the meaning gate pins
//! decoded CONTENT on representative entries alongside the byte gates.

mod common;

use native910::config::{
    ConfigValue, EnumRow, EnumSlot, EnumValues, StructRow, VarBitBase, VarBitConfig, VarBitRange,
    VarConfig, VarValueKind, decode_enum, decode_param, decode_struct, decode_var, decode_varbit,
    encode_enum, encode_param, encode_struct, encode_var, encode_varbit, var_domain_from_group,
    var_group_id, var_value_kind,
};
use native910::pack::PackArchive;
use native910::vars::VarScope;
use std::path::PathBuf;

fn pack_path(name: &str) -> PathBuf {
    common::pack_root().join(name)
}

fn open_pack(name: &str) -> PackArchive {
    let path = pack_path(name);
    common::require_present(&path);
    PackArchive::open(&path).expect("open pack")
}

/// Split an enum entry id into its pack location (256 files per group).
fn enum_group_file(id: u32) -> (u32, u32) {
    (id / 256, id % 256)
}

/// Fetch one enum entry's raw bytes by entry id.
fn enum_bytes(archive: &PackArchive, id: u32) -> Vec<u8> {
    let (group, file) = enum_group_file(id);
    archive
        .group_files(group)
        .expect("unpack enum group")
        .expect("enum group present")[&file]
        .clone()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_enums_are_byte_exact() {
    let archive = open_pack("client.enum.config.js5");
    let mut entries = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            entries += 1;
            let id = group * 256 + file;
            let decoded = match decode_enum(bytes) {
                Ok(decoded) => decoded,
                Err(error) => {
                    failures.push(format!("enum {id}: decode: {error}"));
                    continue;
                }
            };
            match encode_enum(&decoded) {
                Ok(out) if out == *bytes => {}
                Ok(out) => failures.push(format!(
                    "enum {id}: re-encoded {} bytes, original {}",
                    out.len(),
                    bytes.len()
                )),
                Err(error) => failures.push(format!("enum {id}: re-encode: {error}")),
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} enum failure(s) over {entries} entries",
        failures.len()
    );
    eprintln!("enum corpus: {entries} entries byte-exact, 0 failures");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_structs_are_byte_exact() {
    let archive = open_pack("client.struct.config.js5");
    let mut entries = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            entries += 1;
            let id = group * 32 + file;
            let decoded = match decode_struct(bytes) {
                Ok(decoded) => decoded,
                Err(error) => {
                    failures.push(format!("struct {id}: decode: {error}"));
                    continue;
                }
            };
            match encode_struct(&decoded) {
                Ok(out) if out == *bytes => {}
                Ok(out) => failures.push(format!(
                    "struct {id}: re-encoded {} bytes, original {}",
                    out.len(),
                    bytes.len()
                )),
                Err(error) => failures.push(format!("struct {id}: re-encode: {error}")),
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} struct failure(s) over {entries} entries",
        failures.len()
    );
    eprintln!("struct corpus: {entries} entries byte-exact, 0 failures");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_params_are_byte_exact() {
    let archive = open_pack("client.config.js5");
    // Params live in group 11 only (`Js5ConfigGroup(11)`); other groups in this
    // pack are other config families, not params.
    let files = archive
        .group_files(11)
        .expect("unpack group 11")
        .expect("param group present");
    let mut entries = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for (file, bytes) in &files {
        entries += 1;
        let decoded = match decode_param(bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                failures.push(format!("param {file}: decode: {error}"));
                continue;
            }
        };
        match encode_param(&decoded) {
            Ok(out) if out == *bytes => {}
            Ok(out) => failures.push(format!(
                "param {file}: re-encoded {} bytes, original {}",
                out.len(),
                bytes.len()
            )),
            Err(error) => failures.push(format!("param {file}: re-encode: {error}")),
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} param failure(s) over {entries} entries",
        failures.len()
    );
    eprintln!("param corpus: {entries} entries byte-exact, 0 failures");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_vars_are_byte_exact() {
    let archive = open_pack("client.config.js5");
    // One gate per var domain; entry id = file id within the domain's group.
    let domains = [
        VarScope::Player,
        VarScope::Npc,
        VarScope::Client,
        VarScope::World,
        VarScope::Region,
        VarScope::Object,
        VarScope::Clan,
        VarScope::ClanSetting,
        VarScope::Controller,
        VarScope::Global,
        VarScope::Group,
    ];
    let mut entries = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for domain in domains {
        let group = var_group_id(domain);
        // The gate walks the pack's real group, not the mapper's claim: a
        // mapper/pack disagreement must fail loudly, not skip silently.
        assert_eq!(
            var_domain_from_group(group).expect("mapper round-trips its own group"),
            domain
        );
        let files = archive
            .group_files(group)
            .expect("unpack var group")
            .unwrap_or_default();
        assert!(!files.is_empty(), "var group {group} unexpectedly empty");
        for (file, bytes) in &files {
            entries += 1;
            let decoded = match decode_var(bytes, domain) {
                Ok(decoded) => decoded,
                Err(error) => {
                    failures.push(format!("var {domain:?}/{file}: decode: {error}"));
                    continue;
                }
            };
            match encode_var(&decoded, domain) {
                Ok(out) if out == *bytes => {}
                Ok(out) => failures.push(format!(
                    "var {domain:?}/{file}: re-encoded {} bytes, original {}",
                    out.len(),
                    bytes.len()
                )),
                Err(error) => {
                    failures.push(format!("var {domain:?}/{file}: re-encode: {error}"));
                }
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} var failure(s) over {entries} entries",
        failures.len()
    );
    eprintln!(
        "var corpus: {entries} entries over {} domains byte-exact, 0 failures",
        domains.len()
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_varbits_are_byte_exact() {
    let archive = open_pack("client.config.js5");
    let files = archive
        .group_files(69)
        .expect("unpack varbit group 69")
        .expect("varbit group present");
    let mut entries = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for (file, bytes) in &files {
        entries += 1;
        let decoded = match decode_varbit(bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                failures.push(format!("varbit {file}: decode: {error}"));
                continue;
            }
        };
        match encode_varbit(&decoded) {
            Ok(out) if out == *bytes => {}
            Ok(out) => failures.push(format!(
                "varbit {file}: re-encoded {} bytes, original {}",
                out.len(),
                bytes.len()
            )),
            Err(error) => failures.push(format!("varbit {file}: re-encode: {error}")),
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} varbit failure(s) over {entries} entries",
        failures.len()
    );
    eprintln!("varbit corpus: {entries} entries byte-exact, 0 failures");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_var_meaning_spot_checks() {
    let archive = open_pack("client.config.js5");

    // Player var 0: int type, lifetime 1, client code 0.
    let files = archive
        .group_files(var_group_id(VarScope::Player))
        .expect("unpack player vars")
        .expect("present");
    let entry = decode_var(&files[&0], VarScope::Player).expect("decode player var 0");
    assert_eq!(entry.data_type, Some(0));
    assert_eq!(var_value_kind(0), Some(VarValueKind::Int));
    assert_eq!(entry.lifetime, Some(1));
    assert_eq!(entry.client_code, Some(0));
    assert!(entry.legacy_default_value);
    assert_eq!(entry.debug_name, None);
    assert_eq!(entry.transmit_level, None);
    assert!(!entry.varname_hash);

    // Client var 178: lifetime but no client code (basic grammar).
    let files = archive
        .group_files(var_group_id(VarScope::Client))
        .expect("unpack client vars")
        .expect("present");
    let entry = decode_var(&files[&178], VarScope::Client).expect("decode client var 178");
    assert_eq!(entry.data_type, Some(0));
    assert_eq!(entry.lifetime, Some(1));
    assert_eq!(entry.client_code, None);

    // Client var 0: bare type-only entry.
    let entry = decode_var(&files[&0], VarScope::Client).expect("decode client var 0");
    assert_eq!(
        entry,
        VarConfig {
            data_type: Some(0),
            ..Default::default()
        }
    );

    // Varbit 0: base domain 0 var 4, bits 0-1. Varbit 1: same base, bits 2-5.
    let files = archive
        .group_files(69)
        .expect("unpack varbits")
        .expect("present");
    assert_eq!(
        decode_varbit(&files[&0]).expect("decode varbit 0"),
        VarBitConfig {
            base: Some(VarBitBase { domain: 0, var: 4 }),
            bits: Some(VarBitRange { start: 0, end: 1 }),
        }
    );
    assert_eq!(
        decode_varbit(&files[&1]).expect("decode varbit 1"),
        VarBitConfig {
            base: Some(VarBitBase { domain: 0, var: 4 }),
            bits: Some(VarBitRange { start: 2, end: 5 }),
        }
    );

    // Group/scope mapper pins (mirror of the client's config group table).
    assert_eq!(var_group_id(VarScope::Player), 60);
    assert_eq!(var_group_id(VarScope::Global), 75);
    assert_eq!(var_group_id(VarScope::Group), 80);
    assert_eq!(
        var_domain_from_group(62).expect("group 62"),
        VarScope::Client
    );
    assert!(var_domain_from_group(11).is_err());
    assert!(var_domain_from_group(69).is_err());
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_meaning_spot_checks() {
    {
        let archive = open_pack("client.enum.config.js5");
        // 3907: aura table. Dense string block whose capacity (73) is one less
        // than its slot count (74): index 27 repeats, so rows stay a list.
        let entry = decode_enum(&enum_bytes(&archive, 3907)).expect("decode enum 3907");
        assert_eq!(entry.input_type, Some(0));
        assert_eq!(entry.output_type, Some(36));
        assert_eq!(entry.default_string.as_deref(), Some(""));
        assert_eq!(entry.default_int, None);
        let Some(EnumValues::DenseString { capacity, slots }) = entry.values else {
            panic!("enum 3907 must be a dense string table");
        };
        assert_eq!(capacity, 73);
        assert_eq!(slots.len(), 74);
        assert_eq!(
            slots[0],
            EnumSlot {
                index: 0,
                value: ConfigValue::Str("Oddball".to_string()),
            }
        );
        // The repeated index, in file order: Wisdom then Aegis.
        assert_eq!(slots[27].index, 27);
        assert_eq!(slots[27].value, ConfigValue::Str("Wisdom".to_string()));
        assert_eq!(slots[28].index, 27);
        assert_eq!(slots[28].value, ConfigValue::Str("Aegis".to_string()));

        // 688: dense int block, capacity 3 with 4 slots (index 2 repeats).
        let entry = decode_enum(&enum_bytes(&archive, 688)).expect("decode enum 688");
        assert_eq!(entry.input_type, Some(0));
        assert_eq!(entry.output_type, Some(57));
        assert_eq!(entry.default_int, Some(45_348));
        let Some(EnumValues::DenseInt { capacity, slots }) = entry.values else {
            panic!("enum 688 must be a dense int table");
        };
        assert_eq!(capacity, 3);
        assert_eq!(
            slots,
            vec![
                EnumSlot {
                    index: 0,
                    value: ConfigValue::Int(45_348)
                },
                EnumSlot {
                    index: 1,
                    value: ConfigValue::Int(45_347)
                },
                EnumSlot {
                    index: 2,
                    value: ConfigValue::Int(45_350)
                },
                EnumSlot {
                    index: 2,
                    value: ConfigValue::Int(45_349)
                },
            ]
        );

        // 1547: sparse int block with repeated keys (key -1 twice, and a
        // repeated key range further on) — all preserved in file order.
        let entry = decode_enum(&enum_bytes(&archive, 1547)).expect("decode enum 1547");
        assert_eq!(entry.input_type, Some(33));
        assert_eq!(entry.output_type, Some(0));
        assert_eq!(entry.default_int, Some(0));
        let Some(EnumValues::SparseInt(rows)) = entry.values else {
            panic!("enum 1547 must be a sparse int table");
        };
        assert_eq!(rows.len(), 280);
        assert_eq!(
            rows[0],
            EnumRow {
                key: 5733,
                value: ConfigValue::Int(1)
            }
        );
        assert_eq!(rows.iter().filter(|row| row.key == -1).count(), 2);

        // 953: one of two entries still using the legacy char type forms.
        let entry = decode_enum(&enum_bytes(&archive, 953)).expect("decode enum 953");
        assert_eq!(entry.input_legacy, Some(0x69));
        assert_eq!(entry.output_legacy, Some(0x73));
        assert_eq!(entry.input_type, None);
        assert_eq!(entry.output_type, None);
        let Some(EnumValues::DenseString { capacity, slots }) = entry.values else {
            panic!("enum 953 must be a dense string table");
        };
        assert_eq!(capacity, 241);
        assert_eq!(slots.len(), 241);
        assert_eq!(
            slots[0],
            EnumSlot {
                index: 0,
                value: ConfigValue::Str("Afghanistan".to_string()),
            }
        );
    }

    {
        let archive = open_pack("client.struct.config.js5");
        // Struct 1: mixed string + int rows.
        let files = archive
            .group_files(0)
            .expect("unpack struct group 0")
            .expect("present");
        let entry = decode_struct(&files[&1]).expect("decode struct 1");
        assert_eq!(
            entry.params,
            vec![
                StructRow {
                    param: 0x1CF,
                    value: ConfigValue::Str("harpie bug swarms".to_string()),
                },
                StructRow {
                    param: 0x1CC,
                    value: ConfigValue::Int(0x46)
                },
                StructRow {
                    param: 0x293,
                    value: ConfigValue::Int(0xC51)
                },
            ]
        );

        // Struct 0: int-only rows.
        let entry = decode_struct(&files[&0]).expect("decode struct 0");
        assert_eq!(entry.params.len(), 4);
        assert_eq!(
            entry.params[0],
            StructRow {
                param: 0x82,
                value: ConfigValue::Int(22_452)
            }
        );
    }

    {
        let archive = open_pack("client.config.js5");
        let files = archive
            .group_files(11)
            .expect("unpack param group 11")
            .expect("present");
        // Param 0: smart kind, int default, autodisable on.
        let entry = decode_param(&files[&0]).expect("decode param 0");
        assert_eq!(entry.kind, Some(0));
        assert_eq!(entry.kind_legacy, None);
        assert_eq!(entry.default_int, Some(0));
        assert_eq!(entry.default_string, None);
        assert!(entry.autodisable);

        // Param 3: the autodisable-clear marker first, then kind + default.
        let entry = decode_param(&files[&3]).expect("decode param 3");
        assert!(!entry.autodisable);
        assert_eq!(entry.kind, Some(0));
        assert_eq!(entry.default_int, Some(0));

        // Param 65: string default (empty string, not absent).
        let entry = decode_param(&files[&65]).expect("decode param 65");
        assert_eq!(entry.kind, Some(36));
        assert_eq!(entry.default_string.as_deref(), Some(""));
        assert_eq!(entry.default_int, None);
        assert!(entry.autodisable);
    }
}
