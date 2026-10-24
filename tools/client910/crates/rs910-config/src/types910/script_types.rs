//! The script variable types a config can name by a one-byte id: the base
//! type each belongs to (0 integer, 1 long, 2 string, 3 fine coordinate) and
//! its default value. Ids without an entry are not byte-addressable types.
use super::variables::Value;

pub fn script_type(id: u8) -> Option<(u8, Value)> {
    Some(match id {
        52 | 82 | 130..=199 | 209..=255 => return None,
        0 | 1 | 126 => (0, Value::Int(0)),
        35 | 49 | 56 | 71 | 115 | 116 | 118 => (1, Value::Long(-1)),
        110 => (1, Value::Long(0)),
        36 => (2, Value::String(String::new())),
        50 => (3, Value::FineCoord([-1, 0, 0, 0])),
        _ => (0, Value::Int(-1)),
    })
}
