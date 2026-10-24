use super::*;

#[test]
fn position_spline_decodes_control_points_and_advances_the_camera() {
    let mut bytes = vec![1, 1];
    let mut push_float = |value: f32| bytes.extend(value.to_bits().to_be_bytes());
    for value in [
        0.0, 0.0, 0.0, 33.3333, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 66.6667, 0.0, 0.0, 100.0, 0.0,
    ] {
        push_float(value);
    }
    let mut reader = crate::ui_bytes::Cursor::new(&bytes);
    let mut spline = SplineTrack::new(CameraSplineKind::Accelerated);
    spline.decode(&mut reader).unwrap();
    assert_eq!(reader.remaining(), 0);
    assert_eq!(spline.point().unwrap(), Vec3::ZERO);

    spline.update(0.1, [100.0; 3], [f32::INFINITY; 3]).unwrap();
    let x = spline.point().unwrap().x;
    assert!(x > 0.0 && x < 100.0);
}
