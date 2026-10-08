use super::sort::quicksort_parallel;
use super::texture_mapping::dominant_axis;
use super::*;

#[test]
fn jittered_quicksort_sorts_keys_and_carries_values() {
    let mut keys = vec![9_i64, 2, 7, 2, 5, 1, i64::MAX, 3];
    let mut vals: Vec<i32> = (0..keys.len() as i32).collect();
    let expect: Vec<i64> = {
        let mut k = keys.clone();
        k.sort_unstable();
        k
    };
    quicksort_parallel(&mut keys, &mut vals);
    assert_eq!(keys, expect);
    // every value still pairs with its original key
    let orig = [9_i64, 2, 7, 2, 5, 1, i64::MAX, 3];
    for (k, v) in keys.iter().zip(&vals) {
        assert_eq!(orig[*v as usize], *k);
    }
}

/// The 7-bit lightness scales by the ambient (`* ambient >> 7`), clamps to
/// 2..=126 and keeps hue and saturation (`& 0xFF80`, which includes bit 7).
#[test]
fn scale_lightness_clamps() {
    // 0x34 * 64 >> 7 = 26, in range; 0x12B4 & 0xFF80 = 0x1280.
    assert_eq!(GpuModel::scale_lightness(0x12B4, 64), 0x129A);
    // 0x7F * 128 >> 7 = 127, clamped to 126; hue/sat 0xFF80 kept.
    assert_eq!(GpuModel::scale_lightness(0xFFFF, 128), 0xFFFE);
    // 0 lightness clamps up to 2; bit 7 alone survives the mask.
    assert_eq!(GpuModel::scale_lightness(0x0080, 0), 0x0082);
}

#[test]
fn dominant_axis_picks_the_largest_component() {
    assert_eq!(dominant_axis(0.0, 2.0, 1.0), 0);
    assert_eq!(dominant_axis(0.0, -2.0, 1.0), 1);
    assert_eq!(dominant_axis(1.0, 0.5, 3.0), 2);
    assert_eq!(dominant_axis(1.0, 0.5, -3.0), 3);
    assert_eq!(dominant_axis(2.0, 1.0, 1.0), 4);
    assert_eq!(dominant_axis(-2.0, 1.0, 1.0), 5);
}
