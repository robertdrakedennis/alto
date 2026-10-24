//! Database tuple values and lane counts against the original client's
//! decoders, recorded once in `tests/fixtures/recorded/db-tuple.txt`.
//!
//! The fixture carries a real table schema (table 15) and two rows of it: one
//! holding a mixed string/int tuple, and the table-only row whose columns all
//! take their defaults. The recorded outputs are the tuple lane counts, the
//! int lane and the hex of the string lane as the original decoders produced
//! them.
mod common;

use native910::dbtable::{self, DbCellValue, DbRowType};
use std::collections::HashMap;
use std::fmt::Write;

fn fixture() -> HashMap<String, String> {
    let text = std::fs::read_to_string(common::fixture("recorded").join("db-tuple.txt")).unwrap();
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let (key, value) = line.split_once('\t').unwrap();
            (key.to_string(), value.replace('|', "\n"))
        })
        .collect()
}

/// The recorded output format: `ints,strings,longs`, the int lane, then the
/// hex of each string.
fn tuple_output(tuple: &[DbCellValue]) -> String {
    let ints: Vec<_> = tuple
        .iter()
        .filter_map(|value| match value {
            DbCellValue::Int(n) => Some(*n),
            _ => None,
        })
        .collect();
    let strings: Vec<_> = tuple
        .iter()
        .filter_map(|value| match value {
            DbCellValue::Str(s) => Some(s),
            _ => None,
        })
        .collect();
    let mut out = format!("{},{},0\n{ints:?}\n", ints.len(), strings.len());
    for string in strings {
        for byte in string.as_bytes() {
            write!(out, "{byte:02x}").unwrap();
        }
        out.push('\n');
    }
    out
}

#[test]
fn database_tuple_and_defaults_match_original_client() {
    let fixture = fixture();
    let table_id: u32 = fixture["table_id"].parse().unwrap();
    let column: u8 = fixture["column"].parse().unwrap();
    let tuple_index: usize = fixture["tuple_index"].parse().unwrap();
    let table = dbtable::decode_dbtable(&common::unhex(&fixture["table_hex"])).unwrap();
    let schema = table
        .columns
        .iter()
        .rev()
        .find(|s| s.column == column)
        .unwrap();
    assert!(schema.defaults.is_none());
    assert!(schema.types.contains(&36) && schema.types.iter().any(|t| *t != 36));

    // A stored row: the tuple the native decoder reads is the one the
    // original decoders read.
    let row_bytes = common::unhex(&fixture["row_hex"]);
    let row = dbtable::decode_dbrow(&row_bytes).unwrap();
    assert_eq!(row.table, Some(table_id));
    assert_eq!(dbtable::encode_dbrow(&row).unwrap(), row_bytes);
    let stored = row
        .columns
        .iter()
        .find(|c| c.column == column)
        .unwrap()
        .rows[tuple_index]
        .clone();
    assert_eq!(tuple_output(&stored), fixture["row_tuple_output"]);

    // A table-only row: every column reads its type default (0 for the
    // boolean/int-like types 0 and 1, -1 for other ints, "" for strings).
    let defaults_bytes = dbtable::encode_dbrow(&DbRowType {
        table: Some(table_id),
        ..DbRowType::default()
    })
    .unwrap();
    assert_eq!(defaults_bytes, common::unhex(&fixture["defaults_row_hex"]));
    let defaults: Vec<DbCellValue> = schema
        .types
        .iter()
        .map(|kind| {
            if *kind == 36 {
                DbCellValue::Str(String::new())
            } else {
                DbCellValue::Int(if matches!(*kind, 0 | 1) { 0 } else { -1 })
            }
        })
        .collect();
    assert_eq!(tuple_output(&defaults), fixture["defaults_tuple_output"]);
}
