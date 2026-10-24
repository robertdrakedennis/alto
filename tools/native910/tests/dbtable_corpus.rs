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
    DbCellValue, DbRowColumn, DbRowType, DbTableColumn, DbTableType, decode_dbrow, decode_dbtable,
    encode_dbrow, encode_dbtable,
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
