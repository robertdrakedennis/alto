#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::{
    bas_types::Bas,
    combat_types::{Bar, Hit},
    effect_types::Effect,
    sequence_types::{Group, Sequence},
    Error,
};
#[test]
fn unsupported_config_opcodes_keep_kind_and_id() {
    assert!(matches!(
        Sequence::decode(72, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "sequence",
            id: 72,
            opcode: 250
        })
    ));
    assert!(matches!(
        Bas::decode(72, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "BAS",
            id: 72,
            opcode: 250
        })
    ));
    assert!(matches!(
        Hit::decode(72, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "hitmark",
            id: 72,
            opcode: 250
        })
    ));
    assert!(matches!(
        Bar::decode(72, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "headbar",
            id: 72,
            opcode: 250
        })
    ));
    assert!(matches!(
        Effect::decode(72, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "effect",
            id: 72,
            opcode: 250
        })
    ));
    assert!(Group::decode(&[250, 0]).is_err());
}
#[test]
fn sequence_array_dependencies_and_bounds_fail() {
    for bytes in [
        &[19, 0, 1, 0][..],
        &[20, 0, 0, 1, 0, 1, 0][..],
        &[13, 0, 0, 19, 0, 1, 0][..],
        &[1, 0, 1, 0, 1, 0][..],
    ] {
        assert!(Sequence::decode(1, bytes).is_err());
    }
    assert!(Group::decode(&[2, 1, 0x81, 0x90, 0]).is_err());
}
#[test]
fn truncated_or_invalid_records_never_produce_constructor_defaults() {
    assert!(Bas::decode(1, &[27, 255, 0]).is_err());
    assert!(Hit::decode(1, &[8, 1, 0]).is_err());
    assert!(Hit::decode(1, &[18, 0, 0]).is_err());
    assert!(Bar::decode(1, &[7, 128]).is_err());
    assert!(Effect::decode(1, &[40, 1, 0, 0, 0]).is_err());
    for b in [&[][..], &[1][..]] {
        assert!(Sequence::decode(1, b).is_err());
        assert!(Bas::decode(1, b).is_err());
        assert!(Hit::decode(1, b).is_err());
        assert!(Bar::decode(1, b).is_err());
        assert!(Effect::decode(1, b).is_err());
    }
}

#[test]
fn variable_definitions_reject_invalid_inputs_without_inventing_defaults() {
    use protocol910::variable_types::{Domain, Variable};
    for bytes in [
        &[2, 0][..],
        &[3, 254, 0][..],
        &[1, 1, 0][..],
        &[110, 0][..],
        &[250, 0][..],
    ] {
        assert!(Variable::decode(Domain::Player, 77, bytes).is_err());
    }
    let missing = Variable::empty(Domain::Npc, 77);
    assert!(missing.base_type().is_none());
}
