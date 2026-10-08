//! Lighting: the environment record's colour remapping in the verified
//! look.
use super::*;
use crate::settings::LookMode;

/// Under the verified look, Lumbridge grades by the record's second remap
/// slot as the NXT client reads it (sprite 27692 at 0.85, the source colour
/// at 0.15)
/// in place of the classic environment's remap, and the composite applies
/// that LUT: against the same look grading by the classic remap, the HDR
/// frame is the same (the grading is the composite's) while the displayed
/// frame moves by more than two levels on average.
#[test]
#[ignore = "needs a GPU (headless wgpu device) and server/data/pack"]
fn verified_frame_grades_by_the_records_second_slot() {
    let _clock = fixed_clock();
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let (device, queue) = crate::test_support::require_gpu();
    let size = [640, 360];
    let offline = OfflineScene::new(&pack, (3222, 3222), (640, 360));
    let snapshot = offline.snapshot(&pack);
    let verified = ModernSettings {
        look: LookMode::Verified,
        ..ModernSettings::DEFAULT
    };
    let frame = |remap: bool| {
        let mut r = renderer(&device, &queue, 4, verified);
        r.environment_record.remap = remap;
        settled(&device, &queue, &mut r, &snapshot, size);
        // Past the record's 5 s fade in.
        let mut out = None;
        for k in 0..60 {
            crate::logic_clock::set_test_now(Some(1_700_000_010_000 + 100 * k));
            out = Some(render(&device, &queue, &mut r, &snapshot, size));
        }
        crate::logic_clock::set_test_now(Some(1_700_000_000_000));
        (out.unwrap(), r.grading)
    };
    let (graded, g) = frame(true);
    let (plain, g_plain) = frame(false);
    eprintln!("graded {g:?}; without the remap {g_plain:?}");
    assert_eq!((g.luts, g.count), ([27692, -1, -1], 1));
    assert!((g.weights[0] - 0.85).abs() < 1e-6 && (g.base - 0.15).abs() < 1e-6);
    assert_ne!(g_plain.luts, g.luts, "the classic environment's remap");
    let hdr = graded
        .hdr
        .iter()
        .zip(&plain.hdr)
        .filter(|(a, b)| (*a - *b).abs() > 1e-3)
        .count();
    let moved = graded
        .pixels
        .chunks_exact(4)
        .zip(plain.pixels.chunks_exact(4))
        .map(|(a, b)| (0..3).map(|c| f64::from(a[c].abs_diff(b[c]))).sum::<f64>() / 3.0)
        .sum::<f64>()
        / f64::from(size[0] * size[1]);
    eprintln!("the slot moves the frame {moved:.2} levels; {hdr} HDR values differ");
    assert!(hdr * 1000 <= graded.hdr.len(), "{hdr} HDR values differ");
    assert!(moved > 2.0, "the LUT changes the frame: {moved}");
}
