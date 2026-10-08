
// Horizon-based occlusion: 4 directions x 4 steps.
const HBAO_STEPS: f32 = 4.0;
const HBAO_DIRECTIONS: f32 = 4.0;

// ComputeAO: the sample's elevation over the tangent plane less the bias,
// times the falloff 1 - d^2 / R^2 (pp.p.y = -1 / R^2).
fn hbao_term(p: vec3<f32>, n: vec3<f32>, s: vec3<f32>) -> f32 {
    let u = s - p;
    let d2 = dot(u, u);
    // The pixel itself (a clamped tap): 0 here (the shader's 0 / 0).
    if (d2 < 1e-8) {
        return 0.0;
    }
    let f = dot(n, u) * inverseSqrt(d2);
    return clamp(f - pp.p.z, 0.0, 1.0) * clamp(d2 * pp.p.y + 1.0, 0.0, 1.0);
}

@fragment
fn fs_hbao(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let nd = textureLoad(t0, px, 0);
    // The normal target's clear: unoccluded.
    if (nd.w < 0.5) {
        return vec4<f32>(1.0);
    }
    let n = nd.xyz;
    let p = textureLoad(t1, px, 0).xyz;
    // The noise texture is indexed by the pixel position / 4: the 4x4 tile repeats.
    let tile = vec2<u32>(px) % vec2<u32>(4u);
    let noise = pf.directions[tile.y * 4u + tile.x];
    // The pixel radius min(H, H R / z) (H is the viewport height).
    let h = pf.rect.w;
    let radius = min(h, h * pp.p.x / p.z);
    let step = radius / (HBAO_STEPS + 1.0);
    var r = 0.0;
    for (var c = 0.0; c < HBAO_DIRECTIONS; c += 1.0) {
        let a = 6.2831853 / HBAO_DIRECTIONS * c;
        let rot = vec2<f32>(cos(a), sin(a));
        // RotateDirection(noise.xy, rot).
        let dir = vec2<f32>(noise.x * rot.x - noise.y * rot.y, noise.x * rot.y + noise.y * rot.x);
        var ray = noise.z * step + 1.0;
        for (var k = 0; k < 4; k++) {
            let q = in_clip(px + vec2<i32>(floor(ray * dir + 0.5)));
            ray += step;
            // A tap without geometry reads the far plane: no term.
            if (textureLoad(t0, q, 0).w < 0.5) {
                continue;
            }
            r += hbao_term(p, n, textureLoad(t1, q, 0).xyz);
        }
    }
    r *= 1.0 / ((1.0 - pp.p.z) * HBAO_DIRECTIONS * HBAO_STEPS);
    let ao = clamp(1.0 - r * 2.0, 0.0, 1.0);
    let v = pow(ao, pp.p.w);
    return vec4<f32>(v, v, v, 1.0);
}
