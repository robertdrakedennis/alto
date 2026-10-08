//! dbtable/dbrow corpus gates: every 910 table schema and row tuple decodes
//! with its pack's group math and re-encodes byte-identical.
//!
//! Mirrors `tests/config_corpus.rs`: each gate walks its runtime pack,
//! round-trips every entry, and reports counts; a missing pack fails the gate
//! (`--features no-pack` reports it ignored). Byte-identity alone cannot catch semantic misreads (a shifted
//! walk over zero-filled regions still round-trips), so the meaning gates pin
//! decoded CONTENT on representative entries alongside the byte gates, plus a
//! full cross-check that every row column's types match its table's schema.

mod common;

use native910::dbtable::{
    DbCellBase, DbCellValue, DbRowColumn, DbRowType, DbTableColumn, DbTableType, db_cell_base,
    decode_dbrow, decode_dbtable, encode_dbrow, encode_dbtable,
};
use native910::pack::PackArchive;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn pack_path(name: &str) -> PathBuf {
    common::pack_root().join(name)
}

fn open_pack(name: &str) -> PackArchive {
    let path = pack_path(name);
    common::require_present(&path);
    PackArchive::open(&path).expect("open pack")
}

/// Fetch every entry of a `client.config.js5` group keyed by file id (= entry
/// id for the bit-less groups 40/41).
fn config_group_entries(archive: &PackArchive, group: u32) -> BTreeMap<u32, Vec<u8>> {
    archive
        .group_files(group)
        .expect("unpack config group")
        .unwrap_or_default()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dbtables_are_byte_exact() {
    let archive = open_pack("client.config.js5");
    // Dbtables live in group 40 only (`Js5ConfigGroup(40)`); entry id = file id.
    let files = config_group_entries(&archive, 40);
    let mut entries = 0_usize;
    let mut bare = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for (file, bytes) in &files {
        entries += 1;
        if *bytes == vec![0] {
            bare += 1;
        }
        let decoded = match decode_dbtable(bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                failures.push(format!("dbtable {file}: decode: {error}"));
                continue;
            }
        };
        match encode_dbtable(&decoded) {
            Ok(out) if out == *bytes => {}
            Ok(out) => failures.push(format!(
                "dbtable {file}: re-encoded {} bytes, original {}",
                out.len(),
                bytes.len()
            )),
            Err(error) => failures.push(format!("dbtable {file}: re-encode: {error}")),
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} dbtable failure(s) over {entries} entries",
        failures.len()
    );
    assert_eq!(entries, 75, "unexpected dbtable corpus size");
    assert_eq!(bare, 33, "unexpected bare-table count");
    eprintln!("dbtable corpus: {entries} entries byte-exact, 0 failures ({bare} bare)");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dbrows_are_byte_exact() {
    let archive = open_pack("client.config.js5");
    // Dbrows live in group 41 only (`Js5ConfigGroup(41)`); entry id = file id.
    let files = config_group_entries(&archive, 41);
    let mut entries = 0_usize;
    let mut table_only = 0_usize;
    let mut empty_block = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for (file, bytes) in &files {
        entries += 1;
        let decoded = match decode_dbrow(bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                failures.push(format!("dbrow {file}: decode: {error}"));
                continue;
            }
        };
        if decoded.columns.is_empty() {
            if decoded.column_count.is_none() {
                table_only += 1;
            } else {
                // Rows 1578, 1621 and 1669 carry an empty columns block
                // (`03 01 ff`: size byte, no columns) after their link.
                empty_block += 1;
            }
        }
        match encode_dbrow(&decoded) {
            Ok(out) if out == *bytes => {}
            Ok(out) => failures.push(format!(
                "dbrow {file}: re-encoded {} bytes, original {}",
                out.len(),
                bytes.len()
            )),
            Err(error) => failures.push(format!("dbrow {file}: re-encode: {error}")),
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} dbrow failure(s) over {entries} entries",
        failures.len()
    );
    assert_eq!(entries, 2399, "unexpected dbrow corpus size");
    assert_eq!(table_only, 1106, "unexpected table-only row count");
    assert_eq!(empty_block, 3, "unexpected empty-block row count");
    eprintln!(
        "dbrow corpus: {entries} entries byte-exact, 0 failures ({table_only} table-only, {empty_block} empty-block)"
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dbtable_meaning_spot_checks() {
    let archive = open_pack("client.config.js5");
    let files = config_group_entries(&archive, 40);

    // Table 0: bare terminator — no schema block at all.
    assert_eq!(
        decode_dbtable(&files[&0]).expect("decode table 0"),
        DbTableType::default()
    );
    assert_eq!(files[&0], vec![0]);

    // Table 74: nine columns over seven distinct type serials, with and
    // without defaults, including an 8-tuple mixing ints and a string.
    let table = decode_dbtable(&files[&74]).expect("decode table 74");
    assert_eq!(table.column_count, Some(9));
    assert_eq!(table.columns.len(), 9);
    let columns: BTreeMap<u8, &DbTableColumn> = table
        .columns
        .iter()
        .map(|column| (column.column, column))
        .collect();
    assert_eq!(columns[&0].types, vec![36]);
    assert_eq!(columns[&0].defaults, None);
    assert_eq!(columns[&1].types, vec![36]);
    assert_eq!(columns[&2].types, vec![17]);
    assert_eq!(columns[&3].defaults, Some(vec![vec![DbCellValue::Int(1)]]));
    assert_eq!(
        columns[&4].defaults,
        Some(vec![vec![DbCellValue::Int(100)]])
    );
    assert_eq!(columns[&5].types, vec![0, 0]);
    assert_eq!(
        columns[&5].defaults,
        Some(vec![vec![DbCellValue::Int(1000), DbCellValue::Int(1000)]])
    );
    assert_eq!(columns[&6].defaults, Some(vec![vec![DbCellValue::Int(1)]]));
    assert_eq!(columns[&7].types, vec![0, 0, 0, 33, 32, 30, 1, 36]);
    assert_eq!(columns[&7].defaults, None);
    assert_eq!(columns[&8].defaults, Some(vec![vec![DbCellValue::Int(0)]]));

    // Table 5: column markers arrive out of order in the file (2 before 3 is
    // fine, but 8's marker 136 sorts after 6 yet 13-15 follow 12) — file
    // order is preserved, never sorted.
    let table = decode_dbtable(&files[&5]).expect("decode table 5");
    assert_eq!(table.column_count, Some(16));
    let order: Vec<u8> = table.columns.iter().map(|column| column.column).collect();
    assert_eq!(
        order,
        vec![0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    assert_eq!(table.columns[2].column, 2);
    assert_eq!(table.columns[2].types, vec![36]);
    assert_eq!(
        table.columns[2].defaults,
        Some(vec![vec![DbCellValue::Str(String::new())]])
    );
    assert_eq!(table.columns[7].column, 8);
    assert_eq!(
        table.columns[7].defaults,
        Some(vec![vec![DbCellValue::Int(0)]])
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dbrow_meaning_spot_checks() {
    let archive = open_pack("client.config.js5");
    let files = config_group_entries(&archive, 41);

    // Row 0: raw `04 01 00` — a table link only, no columns block.
    let row = decode_dbrow(&files[&0]).expect("decode row 0");
    assert_eq!(
        row,
        DbRowType {
            table: Some(1),
            column_count: None,
            columns: Vec::new(),
        }
    );
    assert_eq!(files[&0], vec![4, 1, 0]);

    // Row 1578: raw `04 2f 03 01 ff 00` — a link plus an empty columns block
    // (size byte present, zero columns). Presence of the block is data.
    let row = decode_dbrow(&files[&1578]).expect("decode row 1578");
    assert_eq!(
        row,
        DbRowType {
            table: Some(47),
            column_count: Some(1),
            columns: Vec::new(),
        }
    );

    // Row 220: table 5 with sparse mixed int/string columns (absent columns
    // read the table defaults client-side; the bytes keep only these six).
    let row = decode_dbrow(&files[&220]).expect("decode row 220");
    assert_eq!(row.table, Some(5));
    assert_eq!(row.column_count, Some(16));
    assert_eq!(row.columns.len(), 6);
    let columns: BTreeMap<u8, &DbRowColumn> = row
        .columns
        .iter()
        .map(|column| (column.column, column))
        .collect();
    assert_eq!(columns[&0].rows, vec![vec![DbCellValue::Int(15)]]);
    assert_eq!(
        columns[&1].rows,
        vec![vec![DbCellValue::Str("Harnessed components".to_string())]]
    );
    assert_eq!(
        columns[&3].rows,
        vec![vec![DbCellValue::Str(
            "Allows you to use harnessed components in a gizmo.".to_string()
        )]]
    );
    assert_eq!(columns[&5].rows, vec![vec![DbCellValue::Int(26_225)]]);
    assert_eq!(columns[&6].rows, vec![vec![DbCellValue::Int(1)]]);
    assert_eq!(columns[&8].rows, vec![vec![DbCellValue::Int(45)]]);

    // Row 1283: the widest column in the corpus — 373 `(obj, int, int)`
    // tuples in column 0, every tuple kept (duplicate rows are data).
    let row = decode_dbrow(&files[&1283]).expect("decode row 1283");
    let column = row
        .columns
        .iter()
        .find(|column| column.column == 0)
        .expect("row 1283 column 0");
    assert_eq!(column.types, vec![33, 0, 0]);
    assert_eq!(column.rows.len(), 373);
    for tuple in &column.rows {
        assert_eq!(tuple.len(), 3);
        assert!(tuple.iter().all(|cell| matches!(cell, DbCellValue::Int(_))));
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_dbrow_columns_match_table_schemas() {
    let archive = open_pack("client.config.js5");
    // Every row column's type tuple must equal its table's schema for that
    // column — 9,096 pins in one gate that the tuple walk never desyncs.
    let tables = config_group_entries(&archive, 40);
    let mut schemas: BTreeMap<u32, BTreeMap<u8, Vec<u16>>> = BTreeMap::new();
    for (id, bytes) in &tables {
        let table = decode_dbtable(bytes).expect("decode schema");
        schemas.insert(
            *id,
            table
                .columns
                .iter()
                .map(|column| (column.column, column.types.clone()))
                .collect(),
        );
    }
    let rows = config_group_entries(&archive, 41);
    let mut checked = 0_usize;
    for (id, bytes) in &rows {
        let row = decode_dbrow(bytes).expect("decode row");
        let Some(table) = row.table else { continue };
        let schema = schemas.get(&table).expect("row links a known table");
        for column in &row.columns {
            assert_eq!(
                schema.get(&column.column),
                Some(&column.types),
                "dbrow {id} column {} diverges from table {table}",
                column.column
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 9096, "unexpected schema-match count");
    eprintln!("dbrow schema cross-check: {checked} columns match their tables");
}

#[test]
fn db_cell_bases_follow_the_script_var_table() {
    // Pins the script var type table and its base types: the boundaries
    // of every range arm, the lone string serial, every long serial, the
    // compound serial, and the gaps naming no client type.
    assert_eq!(db_cell_base(0), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(34), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(35), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(36), Some(DbCellBase::String));
    assert_eq!(db_cell_base(37), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(49), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(50), None);
    assert_eq!(db_cell_base(51), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(52), None);
    assert_eq!(db_cell_base(53), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(56), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(71), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(81), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(82), None);
    assert_eq!(db_cell_base(83), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(110), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(115), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(116), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(118), Some(DbCellBase::Long));
    assert_eq!(db_cell_base(119), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(129), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(130), None);
    assert_eq!(db_cell_base(199), None);
    assert_eq!(db_cell_base(200), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(208), Some(DbCellBase::Int));
    assert_eq!(db_cell_base(209), None);
    assert_eq!(db_cell_base(u16::MAX), None);
}

#[test]
fn handbuilt_dbtable_roundtrips() {
    // Columns deliberately out of order (2 before 0): file order is
    // preserved, never sorted. Covers a multi-tuple default, a default-less
    // column, and a present-but-empty default on empty tuple types.
    let value = DbTableType {
        column_count: Some(3),
        columns: vec![
            DbTableColumn {
                column: 2,
                types: vec![36],
                defaults: Some(vec![
                    vec![DbCellValue::Str("b".to_string())],
                    vec![DbCellValue::Str("c".to_string())],
                ]),
            },
            DbTableColumn {
                column: 0,
                types: vec![0, 110],
                defaults: None,
            },
            DbTableColumn {
                column: 1,
                types: Vec::new(),
                defaults: Some(Vec::new()),
            },
        ],
    };
    let bytes = encode_dbtable(&value).expect("encode");
    // Canonical bytes: one schema block in stored order, then terminator.
    assert_eq!(
        bytes,
        vec![
            1, 3, 130, 1, 36, 2, b'b', 0, b'c', 0, 0, 2, 0, 110, 129, 0, 0, 255, 0
        ]
    );
    let decoded = decode_dbtable(&bytes).expect("decode");
    assert_eq!(decoded, value);
    assert_eq!(encode_dbtable(&decoded).expect("re-encode"), bytes);

    // A bare table is a lone terminator, and it decodes to the default.
    assert_eq!(
        encode_dbtable(&DbTableType::default()).expect("encode"),
        vec![0]
    );
    assert_eq!(
        decode_dbtable(&[0]).expect("decode"),
        DbTableType::default()
    );
}

#[test]
fn handbuilt_dbrow_roundtrips() {
    // Table link (multi-byte varint) then columns, covering int, long and
    // string cells plus a two-row column.
    let value = DbRowType {
        table: Some(300),
        column_count: Some(2),
        columns: vec![
            DbRowColumn {
                column: 1,
                types: vec![36, 110],
                rows: vec![vec![
                    DbCellValue::Str("hi".to_string()),
                    DbCellValue::Long(-1),
                ]],
            },
            DbRowColumn {
                column: 0,
                types: vec![0],
                rows: vec![vec![DbCellValue::Int(7)], vec![DbCellValue::Int(-8)]],
            },
        ],
    };
    let bytes = encode_dbrow(&value).expect("encode");
    // Canonical bytes: link (4) before columns (3), then terminator.
    assert_eq!(
        bytes,
        vec![
            4, 172, 2, 3, 2, 1, 2, 36, 110, 1, b'h', b'i', 0, 255, 255, 255, 255, 255, 255, 255,
            255, 0, 1, 0, 2, 0, 0, 0, 7, 255, 255, 255, 248, 255, 0
        ]
    );
    let decoded = decode_dbrow(&bytes).expect("decode");
    assert_eq!(decoded, value);
    assert_eq!(encode_dbrow(&decoded).expect("re-encode"), bytes);

    // Decode accepts the blocks in either order; encode normalizes to
    // link-first (the order every corpus entry uses).
    let mut scrambled = vec![3, 2, 0, 1, 0, 1, 0, 0, 0, 7, 255, 4, 172, 2, 0];
    assert_eq!(decode_dbrow(&scrambled).expect("decode").table, Some(300));
    scrambled.pop();
    assert!(decode_dbrow(&scrambled).is_err());
}

#[test]
fn malformed_dbtables_are_rejected() {
    // Empty input is truncation, not an empty table.
    assert!(decode_dbtable(&[]).is_err());
    // Only opcode 1 carries a schema; 2 is a later build's block shape and 3
    // belongs to rows — both are hard errors in 910.
    assert!(decode_dbtable(&[2, 0]).is_err());
    assert!(decode_dbtable(&[3, 0]).is_err());
    assert!(decode_dbtable(&[9, 0]).is_err());
    // Bytes past the terminator are corrupt input.
    assert!(decode_dbtable(&[0, 0]).is_err());
    // A second schema block never merges.
    assert!(decode_dbtable(&[1, 0, 255, 1, 0, 255, 0]).is_err());
    // A column outside the count byte would crash the client on load.
    assert!(decode_dbtable(&[1, 1, 5, 0]).is_err());
    // Unterminated string (no NUL, no terminator).
    assert!(decode_dbtable(&[1, 1, 0, 1, 36, 1, b'a', b'b']).is_err());
    // Truncated schema: count byte then end, marker then end, types cut.
    assert!(decode_dbtable(&[1]).is_err());
    assert!(decode_dbtable(&[1, 2, 0]).is_err());
    assert!(decode_dbtable(&[1, 1, 0, 1]).is_err());
}

#[test]
fn malformed_dbrows_are_rejected() {
    assert!(decode_dbrow(&[]).is_err());
    // Only opcodes 3 and 4 exist in 910; 1 is the table schema opcode and 2 a
    // later build's shape.
    assert!(decode_dbrow(&[1, 0]).is_err());
    assert!(decode_dbrow(&[2, 0]).is_err());
    assert!(decode_dbrow(&[9, 0]).is_err());
    assert!(decode_dbrow(&[0, 0]).is_err());
    // A second columns block and a second table link never merge.
    assert!(decode_dbrow(&[3, 0, 255, 3, 0, 255, 0]).is_err());
    assert!(decode_dbrow(&[4, 1, 4, 2, 0]).is_err());
    // A column outside the size byte would crash the client on load.
    assert!(decode_dbrow(&[3, 1, 5, 0]).is_err());
    // A five-continuation varint cannot fit a u32 table id.
    assert!(decode_dbrow(&[4, 255, 255, 255, 255, 255, 0]).is_err());
    // Truncated link and truncated columns block.
    assert!(decode_dbrow(&[4]).is_err());
    assert!(decode_dbrow(&[3]).is_err());
    assert!(decode_dbrow(&[3, 1, 0]).is_err());
}

#[test]
fn unencodable_models_are_rejected_on_encode() {
    // Smart type ids need 15 bits at most.
    assert!(
        encode_dbtable(&DbTableType {
            column_count: Some(1),
            columns: vec![DbTableColumn {
                column: 0,
                types: vec![0x8000],
                defaults: None,
            }],
        })
        .is_err()
    );
    // A column list without a count byte has no byte form.
    assert!(
        encode_dbtable(&DbTableType {
            column_count: None,
            columns: vec![DbTableColumn {
                column: 0,
                types: vec![0],
                defaults: None,
            }],
        })
        .is_err()
    );
    assert!(
        encode_dbrow(&DbRowType {
            table: Some(1),
            column_count: None,
            columns: vec![DbRowColumn {
                column: 0,
                types: vec![0],
                rows: Vec::new(),
            }],
        })
        .is_err()
    );
    // Columns outside the count byte would crash the client on load, and the
    // marker carries only 7 id bits.
    for bad_column in [3_u8, 128] {
        assert!(
            encode_dbtable(&DbTableType {
                column_count: Some(3),
                columns: vec![DbTableColumn {
                    column: bad_column,
                    types: vec![0],
                    defaults: None,
                }],
            })
            .is_err(),
            "column {bad_column} must fail"
        );
    }
    // Tuple arity must equal the type count.
    assert!(
        encode_dbtable(&DbTableType {
            column_count: Some(1),
            columns: vec![DbTableColumn {
                column: 0,
                types: vec![0, 0],
                defaults: Some(vec![vec![DbCellValue::Int(1)]]),
            }],
        })
        .is_err()
    );
    // Cell kinds must match the column type's storage.
    assert!(
        encode_dbrow(&DbRowType {
            table: None,
            column_count: Some(1),
            columns: vec![DbRowColumn {
                column: 0,
                types: vec![36],
                rows: vec![vec![DbCellValue::Int(1)]],
            }],
        })
        .is_err()
    );
    // Compound and undeclared serials have no scalar form to encode with.
    for bad_type in [50_u16, 52] {
        assert!(
            encode_dbrow(&DbRowType {
                table: None,
                column_count: Some(1),
                columns: vec![DbRowColumn {
                    column: 0,
                    types: vec![bad_type],
                    rows: vec![vec![DbCellValue::Int(1)]],
                }],
            })
            .is_err(),
            "type {bad_type} must fail"
        );
    }
    // A type header carries at most 255 ids.
    assert!(
        encode_dbrow(&DbRowType {
            table: None,
            column_count: Some(1),
            columns: vec![DbRowColumn {
                column: 0,
                types: vec![0; 256],
                rows: Vec::new(),
            }],
        })
        .is_err()
    );
    // A tuple block carries at most 32767 tuples.
    assert!(
        encode_dbrow(&DbRowType {
            table: None,
            column_count: Some(1),
            columns: vec![DbRowColumn {
                column: 0,
                types: vec![0],
                rows: vec![vec![DbCellValue::Int(0)]; 32_768],
            }],
        })
        .is_err()
    );
}

#[test]
fn truncated_corrupt_inputs_are_rejected() {
    // Every strict prefix of a valid entry must fail (covers header, smart,
    // varint, string and tuple truncation across both formats).
    let full_table = encode_dbtable(&DbTableType {
        column_count: Some(2),
        columns: vec![DbTableColumn {
            column: 1,
            types: vec![0, 36],
            defaults: Some(vec![vec![
                DbCellValue::Int(-300),
                DbCellValue::Str("hi".to_string()),
            ]]),
        }],
    })
    .expect("encode");
    for cut in 1..full_table.len() {
        assert!(
            decode_dbtable(&full_table[..cut]).is_err(),
            "dbtable truncation at {cut} must fail"
        );
    }
    let full_row = encode_dbrow(&DbRowType {
        table: Some(70_000),
        column_count: Some(1),
        columns: vec![DbRowColumn {
            column: 0,
            types: vec![110, 36],
            rows: vec![vec![
                DbCellValue::Long(i64::MIN),
                DbCellValue::Str("s".to_string()),
            ]],
        }],
    })
    .expect("encode");
    for cut in 1..full_row.len() {
        assert!(
            decode_dbrow(&full_row[..cut]).is_err(),
            "dbrow truncation at {cut} must fail"
        );
    }
}
