//! CPU tests of the post chain's settings and blocks (renderer plan M8).
use super::*;
use crate::settings::ModernSettings;

/// By default bloom follows the faithful toolkit's bloom (the classic
/// preference) and FXAA runs only without MSAA (antialiasing mode 1); a
/// setting fixes either.
#[test]
fn effects_follow_the_options_unless_set() {
    let d = ModernSettings::DEFAULT;
    assert_eq!(
        Effects::resolve(&d, true, 4),
        Effects {
            bloom: true,
            fxaa: false
        }
    );
    assert_eq!(
        Effects::resolve(&d, false, 1),
        Effects {
            bloom: false,
            fxaa: true
        }
    );
    let set = ModernSettings {
        bloom: Some(false),
        fxaa: Some(true),
        ..d
    };
    assert_eq!(
        Effects::resolve(&set, true, 4),
        Effects {
            bloom: false,
            fxaa: true
        }
    );
}

/// The SSAO directions are unit length (the noise taps are normalised) and
/// spread round the circle.
#[test]
fn ssao_directions_are_unit_and_spread() {
    let d = ssao_directions();
    let mut sum = [0.0_f32; 2];
    for v in &d {
        assert!(((v[0] * v[0] + v[1] * v[1]).sqrt() - 1.0).abs() < 1e-5);
        sum[0] += v[0];
        sum[1] += v[1];
    }
    assert!(sum[0].abs() < 1.0 && sum[1].abs() < 1.0, "{sum:?}");
}

#[test]
fn uniform_blocks_are_std140_sized() {
    assert_eq!(std::mem::size_of::<PostFrame>() % 16, 0);
    assert_eq!(std::mem::size_of::<PassParams>(), 256);
}
