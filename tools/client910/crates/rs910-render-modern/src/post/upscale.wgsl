
// ---- The render scale's resample (lane P4-GPU, `frame::scale`): the scaled
// display frame into the scene viewport, as the modern client stretches its
// scene into the output (the final post blit, a linear-filtered blit when
// the rectangles differ): a bilinear tap at each output pixel's centre
// mapped into the source rectangle, clamped to its edge. (The Lanczos and
// bicubic scalers are for the interface, not the scene.) `pp.p` is the
// viewport in the frame's pixels, `pf.rect` the
// scaled viewport and `pf.clip` its scissor in the source; `pp.q.x` 1:
// encode for an sRGB target. ----

fn upscale_texel(p: vec2<i32>) -> vec3<f32> {
    let lo = vec2<i32>(pf.clip.xy);
    let hi = max(vec2<i32>(pf.clip.zw) - vec2<i32>(1), lo);
    return textureLoad(t0, clamp(p, lo, hi), 0).rgb;
}

@fragment
fn fs_upscale(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // The output pixel's centre in source pixels, less the texel centre.
    let p = (frag.xy - pp.p.xy) / pp.p.zw * pf.rect.zw + pf.rect.xy - vec2<f32>(0.5);
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let a = upscale_texel(i);
    let b = upscale_texel(i + vec2<i32>(1, 0));
    let c = upscale_texel(i + vec2<i32>(0, 1));
    let d = upscale_texel(i + vec2<i32>(1, 1));
    let rgb = mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
    if (pp.q.x > 0.5) {
        return vec4<f32>(srgb_to_linear(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
    }
    return vec4<f32>(rgb, 1.0);
}
