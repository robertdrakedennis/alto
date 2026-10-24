//! The packet tables against the recording of the original client's tables.

use super::{client, login, server};

const RECORDING: &str =
    include_str!("../../../../fixtures/recorded/protocol-910/protocol-tables.json");

/// One recorded table's `(name, opcode, size)` rows. The recording writes one
/// `"key": value` pair per line, `name` first, so a line scan reads it.
fn recorded(table: &str) -> Vec<(String, u8, i32)> {
    let start = RECORDING
        .find(&format!("\"{table}\": ["))
        .unwrap_or_else(|| panic!("the recording has no {table} table"));
    let body = &RECORDING[start..];
    let body = &body[..body.find(']').expect("the table closes")];
    let mut rows = Vec::new();
    let mut lines = body.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if !line.starts_with("\"name\"") {
            continue;
        }
        let name = value(line).trim_matches('"').to_owned();
        let opcode = value(lines.next().expect("an opcode line"))
            .parse()
            .expect("an opcode");
        let size = value(lines.next().expect("a size line"))
            .parse()
            .expect("a size");
        rows.push((name, opcode, size));
    }
    rows
}

fn value(line: &str) -> &str {
    line.split_once(':')
        .expect("a key line")
        .1
        .trim()
        .trim_end_matches(',')
}

fn check(table: &str, count: usize, size: fn(u8) -> Option<i32>, name: fn(u8) -> &'static str) {
    let rows = recorded(table);
    assert_eq!(rows.len(), count, "{table}: packet count");
    for (recorded_name, opcode, recorded_size) in rows {
        assert_eq!(name(opcode), recorded_name, "{table} opcode {opcode}: name");
        assert_eq!(
            size(opcode),
            Some(recorded_size),
            "{table} {recorded_name}: size"
        );
    }
}

#[test]
fn tables_match_the_recorded_client() {
    check("LoginProt", login::COUNT, login::size, login::name);
    check("ServerProt", server::COUNT, server::size, server::name);
    check("ClientProt", client::COUNT, client::size, client::name);
}
