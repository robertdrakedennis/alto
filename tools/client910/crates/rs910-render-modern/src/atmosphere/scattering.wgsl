
// ---- Light scattering (atmosphere) ----
fn atmos_unpack(v: f32) -> vec3<f32> {
    let u = bitcast<u32>(v);
    return vec3<f32>(f32((u >> 16u) & 255u), f32((u >> 8u) & 255u), f32(u & 255u)) / 255.0;
}

struct Scatter {
    ext: vec3<f32>,
    ins: vec3<f32>,
};

// The extinction and in-scattering along the unit view direction `dir` over the eye distance
// `dist`; the frame's packed scattering block (atmosphere::pack).
fn in_out_scattering(dir: vec3<f32>, dist: f32) -> Scatter {
    var s: Scatter;
    s.ext = vec3<f32>(1.0);
    s.ins = vec3<f32>(0.0);
    // Off when no density is packed (`ground_ambient.w` is 0 unless the
    // scattering is on; `params.w` belongs to the modern look's flag).
    if (frame.ground_ambient.w <= 0.0) {
        return s;
    }
    let t = max(0.0, dist - frame.fog_range.z);
    let p = pow(max(0.0, dot(dir, frame.sun_dir.xyz)), 2.0);
    let u = mix(atmos_unpack(frame.sky_ambient.w), vec3<f32>(1.0), p);
    let m = -t * frame.ground_ambient.w;
    s.ext = exp(m * atmos_unpack(frame.sun_dir.w));
    s.ins = atmos_unpack(frame.sun_colour.w) * frame.fog_range.w * (vec3<f32>(1.0) - exp(m * u));
    return s;
}

// The scattering folded with the fog: `x` is the colour already fogged by `fog` (c (1 - f) +
// F f); the result is c T (1 - f) + mix(I, F, f).
fn scatter(x: vec3<f32>, fog_in: f32, world: vec3<f32>) -> vec3<f32> {
    if (frame.ground_ambient.w <= 0.0) {
        return x;
    }
    let fog = clamp(fog_in, 0.0, 1.0);
    let v = world - frame.eye.xyz;
    let a = max(length(v), 1e-3);
    let s = in_out_scattering(v / a, a);
    var f = frame.fog_colour.rgb * fog;
    if (frame.fog_colour.w <= 0.0) {
        f = vec3<f32>(0.0);
    }
    return x * s.ext + s.ins * (1.0 - fog) + f * (vec3<f32>(1.0) - s.ext);
}
