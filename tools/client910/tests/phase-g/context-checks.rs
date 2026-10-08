#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::{
    terrain::{height, Terrain},
    varbits::{Binding, Type, ValueError},
    Error,
};
#[test]
fn relaxed_rebinding_keeps_the_existing_binding() {
    let lookup = |d, id| {
        Ok(if d == 0 {
            Some(Binding::empty(d, id))
        } else {
            None
        })
    };
    let bytes = [1, 0, 0, 7, 1, 3, 0, 9, 0];
    let b = Type::decode(1, &bytes, Some(&lookup), true).unwrap();
    assert_eq!(b.domain, Some(3));
    assert_eq!(b.base_id, 9);
    assert_eq!(b.binding.unwrap().id, 7);
    let e = Type::decode(1, &bytes, Some(&lookup), false).unwrap_err();
    assert_eq!(e.consumed, 8);
    assert!(matches!(e.cause, Error::UnsupportedContext(_)));
}
#[test]
fn varbit_failures_preserve_the_cursor_and_input() {
    assert_eq!(
        Type::decode(1, &[2, 1], None, false).unwrap_err().consumed,
        3
    );
    assert_eq!(
        Type::decode(1, &[1, 0, 128], None, false)
            .unwrap_err()
            .consumed,
        6
    );
    assert_eq!(
        Type::decode(1, &[1, 0], None, false).unwrap_err().consumed,
        2
    );
    let t = Type::decode(1, &[2, 0, 31, 0], None, false).unwrap();
    assert_eq!(t.get(i32::MIN), Ok(i32::MIN));
    for v in [i32::MIN, -1, 0, 1, i32::MAX] {
        assert_eq!(t.set(8, v), Err(ValueError::Overflow));
    }
}
#[test]
fn terrain_decode_is_atomic_and_dimensions_are_validated() {
    let s = Terrain::new(72, 72).unwrap();
    let old = s.clone();
    assert!(s.read_normal(&[15, 128], 0, 0, 0, 0).is_err());
    assert_eq!(s, old);
    assert!(Terrain::new(0, 72).is_err());
    assert!(Terrain::new(257, 72).is_err());
    let mut broken = s.clone();
    broken.heights.clear();
    assert!(height(Some(&broken), 0, 0, 0).is_err());
}
#[test]
fn no_scene_and_outside_queries_keep_the_short_circuit_order() {
    assert_eq!(height(None, 0, 0, 900), Ok(0));
    let mut s = Terrain::new(2, 2).unwrap();
    assert_eq!(height(Some(&s), -1, 0, 900), Ok(0));
    assert!(height(Some(&s), 0, 0, -1).is_err());
    let at = s.tile(1, 0, 0);
    s.tiles[at].flags = 2;
    assert_eq!(height(Some(&s), 0, 0, -1), Ok(0));
    assert_eq!(s.context(1, 10, 20).unwrap().bridges, vec![(0, 0)]);
}
