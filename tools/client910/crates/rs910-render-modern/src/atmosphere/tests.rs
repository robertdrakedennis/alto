//! The scattering block's packing into the frame block's spare members.
use super::*;

#[test]
fn colours_pack_into_normal_floats_and_back() {
    for c in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [77.0 / 255.0, 77.0 / 255.0, 128.0 / 255.0],
        [0.6; 3],
    ] {
        let v = pack_colour(c);
        assert!(v.is_normal(), "{c:?} -> {v}");
        let back = unpack_colour(v);
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 0.5 / 255.0 + 1e-6);
        }
    }
}
