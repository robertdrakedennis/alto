
// The environment reflection: the sample direction made upward (the modern
// client takes abs(y); the classic up is -y), then the angle-based fog blend
// of the reflected ray towards the fog colour (the sky's power and offset).
fn env_reflection(r: vec3<f32>) -> vec3<f32> {
    var c = probe_sky(vec3<f32>(r.x, -abs(r.y), r.z));
    if (frame.fog_colour.w > 0.0) {
        let up = -r.y / max(length(r), 1e-6);
        let a = clamp(pow(1.0 - clamp(up + 0.0, 0.0, 1.0), 6.0), 0.0, 1.0);
        c = mix(c, frame.fog_colour.rgb, a);
    }
    return c;
}
