use super::*;

fn light() -> StaticLight {
    StaticLight {
        level: 0,
        above: false,
        below: false,
        x: 0,
        y: 0,
        z: 0,
        radius: 1024,
        colour: 0xffffff,
        flicker: 2,
        phase: 0,
        wave: 1,
        offset: 0,
        amplitude: 2048,
        speed: 2048,
        group: -1,
        span_runs: vec![1],
    }
}
#[test]
fn toggling_flicker_preserves_phase_and_uses_logic_cycle() {
    let l = light();
    let wave = intensity(&l, 13, false);
    assert_ne!(wave, intensity(&l, 14, false));
    assert_eq!(wave, intensity(&l, 13, false));
    assert_eq!(intensity(&l, 13, true), 1.);
    assert_eq!(intensity(&l, 999, true), 1.);
    assert_eq!(intensity(&l, 13, false), wave);
}
