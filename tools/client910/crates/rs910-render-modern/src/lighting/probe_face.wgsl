
// lighting::probes::face_axes: the classic direction of face `face` at (sc, tc).
fn face_dir(face: u32, sc: f32, tc: f32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, tc, -sc); }
        case 1u: { return vec3<f32>(-1.0, tc, sc); }
        case 2u: { return vec3<f32>(sc, -1.0, tc); }
        case 3u: { return vec3<f32>(sc, 1.0, -tc); }
        case 4u: { return vec3<f32>(sc, tc, 1.0); }
        default: { return vec3<f32>(-sc, tc, -1.0); }
    }
}
