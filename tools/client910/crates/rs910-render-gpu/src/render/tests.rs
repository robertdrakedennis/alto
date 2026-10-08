use super::*;

#[test]
fn camera_orbits_target_and_builds_valid_view_proj() {
    let target = glam::Vec3::new(3222.0, 1.0, 3222.0);
    let camera = OrbitCamera::new(target);
    let eye = camera.position();
    // Yaw 0: the eye sits south (-z) of and above the target
    // (yaw 0 faces north).
    assert!(eye.z < target.z && eye.y > target.y && (eye.x - target.x).abs() < 1e-3);
    assert!(camera.distance_tiles() > 1.0);
    // W pans along the view direction: at yaw 0 that is +Z (north).
    let mut panned = camera.clone();
    panned.yaw = 0.0;
    panned.pan(2.0, 0.0);
    assert!((panned.target.z - (target.z + 2.0)).abs() < 1e-4);
    assert!((panned.target.x - target.x).abs() < 1e-4);
    // Strafe right at yaw 0 is east (+x).
    let mut strafed = camera.clone();
    strafed.pan(0.0, 2.0);
    assert!((strafed.target.x - (target.x + 2.0)).abs() < 1e-4);
    // Pitch/dist clamps hold (`clampCamera` bounds).
    let mut clamped = camera.clone();
    clamped.rotate(0.0, 1000.0);
    assert!(clamped.pitch <= crate::camera::ORBIT_PITCH_MAX);
    clamped.zoom(0.0);
    assert!(clamped.dist >= 0.25);
    // View-projection must be invertible (determinant != 0) and finite.
    let view_proj = camera.scene_camera((1600, 900)).view_proj();
    assert!(view_proj.determinant().abs() > 1e-6);
    assert!(view_proj.to_cols_array().iter().all(|v| v.is_finite()));
}
