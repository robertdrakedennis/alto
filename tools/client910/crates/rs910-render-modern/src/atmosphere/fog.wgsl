
// The distance fog (per vertex) and its blend.

// The distance fog of the eye distance (the length from the camera to the world position): the
// fog parameters (1 / range, end) are held here as (start, 1 / range).
fn distance_fog(world: vec3<f32>) -> f32 {
    if (frame.fog_colour.w <= 0.0) {
        return 0.0;
    }
    let d = length(world - frame.eye.xyz);
    return clamp((d - frame.fog_range.x) * frame.fog_range.y, 0.0, 1.0);
}

// The in/out-scattering blend with out = 1 - fog and in = fog colour * fog, the scattering
// itself off.
fn apply_fog(c: vec3<f32>, fog: f32) -> vec3<f32> {
    return c * (1.0 - fog) + frame.fog_colour.rgb * fog;
}
