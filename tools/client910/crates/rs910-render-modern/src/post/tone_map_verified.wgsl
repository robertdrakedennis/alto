
// The Uncharted 2 / Hable filmic curve (the constants A..F).
fn filmic_op(x: vec3<f32>, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> vec3<f32> {
    return (x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f) - e / f;
}

// The tone map of `v` for the operator in pf.tonemap2.w: -1 tone mapping
// disabled, 3 Uncharted 2 filmic, 4 Hable filmic (the exposed colour, then
// the curve over the curve at the white point minus the curve at the black
// level), else the advanced per-channel Reinhard. The exposure is the key
// over `avg`, clamped to the minimum and maximum auto exposure.
fn tone_map_filmic(v: vec3<f32>, avg: f32) -> vec3<f32> {
    let op = pf.tonemap2.w;
    if (op < -0.5) {
        return v;
    }
    if (op > 2.5 && op < 4.5) {
        let x = v * clamp(pf.tonemap.z / avg, pf.tonemap.w, pf.tonemap2.x);
        let white = vec3<f32>(pf.tonemap.y);
        let black = vec3<f32>(pf.tonemap.x);
        if (op < 3.5) {
            return filmic_op(x, 0.22, 0.3, 0.1, 0.2, 0.01, 0.3) / filmic_op(white, 0.22, 0.3, 0.1, 0.2, 0.01, 0.3)
                - filmic_op(black, 0.22, 0.3, 0.1, 0.2, 0.01, 0.3);
        }
        return filmic_op(x, 0.15, 0.5, 0.1, 0.2, 0.02, 0.3) / filmic_op(white, 0.15, 0.5, 0.1, 0.2, 0.02, 0.3)
            - filmic_op(black, 0.15, 0.5, 0.1, 0.2, 0.02, 0.3);
    }
    return reinhard_adv_rgb(v, avg);
}

// The composite's operator: the record's (`pf.tonemap2.w`).
fn tone_map_op(v: vec3<f32>, avg: f32) -> vec3<f32> {
    return tone_map_filmic(v, avg);
}
