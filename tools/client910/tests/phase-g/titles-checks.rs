#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::titles::{Defaults, Enum, Value};

#[test]
fn corrupt_enum_inputs_fail() {
    for bytes in [
        &[][..],
        &[250, 0][..],
        &[7, 0, 0, 0, 1, 0, 0, b'x', 0, 0][..],
    ] {
        assert!(Enum::decode(bytes).is_err());
    }
    assert!(Defaults::decode(&[1, 128]).is_err());
}

#[test]
fn typed_lookup_and_array_precedence() {
    let mut e = Enum {
        values: Some([(3, Value::Int(7))].into_iter().collect()),
        ..Default::default()
    };
    assert!(e.string(3).is_err());
    e.indexed = Some(vec![]);
    assert_eq!(e.string(3).unwrap(), "null");
    assert_eq!(e.string(-1).unwrap(), "null");
}
