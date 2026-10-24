
// ---- The render scale's resample (`frame::scale`): the scaled display frame
// into the scene viewport. A 4x4 Catmull-Rom filter at each output pixel's
// centre mapped into the source rectangle (sharper than bilinear at the
// same cost class), clamped to the range of the 2x2 nearest source texels
// so its negative lobes cannot ring at hard edges, with a light
// contrast-adaptive sharpen inside that range. `pp.p` is the viewport in
// the frame's pixels, `pf.rect` the scaled viewport and `pf.clip` its
// scissor in the source; `pp.q.x` 1: encode for an sRGB target. ----

fn upscale_texel(p: vec2<i32>) -> vec3<f32> {
    let lo = vec2<i32>(pf.clip.xy);
    let hi = max(vec2<i32>(pf.clip.zw) - vec2<i32>(1), lo);
    return textureLoad(t0, clamp(p, lo, hi), 0).rgb;
}

// How strongly the upscale sharpens flat, low-contrast detail.
const UPSCALE_SHARPEN: f32 = 0.6;

// The Catmull-Rom weights of the four taps around fraction `t`.
fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    );
}

@fragment
fn fs_upscale(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // The output pixel's centre in source pixels, less the texel centre.
    let p = (frag.xy - pp.p.xy) / pp.p.zw * pf.rect.zw + pf.rect.xy - vec2<f32>(0.5);
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let wx = catmull_rom(f.x);
    let wy = catmull_rom(f.y);
    var rgb = vec3<f32>(0.0);
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    for (var y = 0; y < 4; y++) {
        var row = vec3<f32>(0.0);
        for (var x = 0; x < 4; x++) {
            let c = upscale_texel(i + vec2<i32>(x - 1, y - 1));
            row += c * wx[x];
            if ((x == 1 || x == 2) && (y == 1 || y == 2)) {
                lo = min(lo, c);
                hi = max(hi, c);
            }
        }
        rgb += row * wy[y];
    }
    // A contrast-adaptive sharpen: push away from the 2x2 mean, less where
    // the neighbourhood already has strong contrast, then stay in its range.
    let mean = (upscale_texel(i) + upscale_texel(i + vec2<i32>(1, 0))
        + upscale_texel(i + vec2<i32>(0, 1)) + upscale_texel(i + vec2<i32>(1, 1))) * 0.25;
    let contrast = max(hi - lo, vec3<f32>(0.0));
    let amount = UPSCALE_SHARPEN * (1.0 - clamp(contrast * 2.0, vec3<f32>(0.0), vec3<f32>(1.0)));
    rgb = clamp(rgb + (rgb - mean) * amount, lo, hi);
    if (pp.q.x > 0.5) {
        return vec4<f32>(srgb_to_linear(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
    }
    return vec4<f32>(rgb, 1.0);
}
