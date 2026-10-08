#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::{defaults::*, Error};
#[test]
fn truncated_defaults_are_errors_and_unknown_opcodes_are_explicit() {
    assert!(Wear::decode(&[0]).is_err());
    for bytes in [&[1, 2, 0][..], &[1, 1, 0, 3][..]] {
        assert!(Wear::decode(bytes).is_err());
    }
    assert!(matches!(
        Graphics::decode(&[255, 0]),
        Err(Error::UnsupportedContext(_))
    ));
    assert!(matches!(
        Wear::decode(&[1, 0, 255, 0]),
        Err(Error::UnsupportedContext(_))
    ));
    assert!(Graphics::decode(&[7, 0, 1, 0, 2, 0, 1]).is_err());
}
#[test]
fn absent_second_palette_does_not_partially_replace_appearance() {
    let mut bytes = vec![7];
    for _ in 0..40 {
        bytes.extend([0, 0, 0, 0]);
    }
    bytes.push(0);
    // pack_defaults::load: flat file 3 is the graphics defaults, 6 the wear.
    let wear_bytes = vec![1, 1, 2, 0];
    let d = EntityDefaults {
        graphics: Graphics::decode(&bytes).unwrap().value,
        wear: Wear::decode(&wear_bytes).unwrap().value,
        graphics_bytes: bytes,
        wear_bytes,
    };
    let mut c = protocol910::appearance::Config {
        wear: vec![9],
        colour_lengths: [7; 10],
        texture_lengths: [8; 10],
        items: Default::default(),
        npc_sizes: Default::default(),
        titles: Default::default(),
        default_titles: Default::default(),
        staff_live_override: false,
    };
    assert!(d.apply_appearance(&mut c).is_err());
    assert_eq!(c.wear, vec![9]);
    assert_eq!(c.colour_lengths, [7; 10]);
    assert_eq!(c.texture_lengths, [8; 10]);
}
#[test]
fn missing_pack_input_never_becomes_empty_defaults() {
    // A pack without the defaults archive is an error, never empty defaults.
    let empty = std::env::temp_dir().join(format!("alto-empty-pack-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    assert!(protocol910::pack_defaults::load(&crate::cache::Pack::open(&empty)).is_err());
}
