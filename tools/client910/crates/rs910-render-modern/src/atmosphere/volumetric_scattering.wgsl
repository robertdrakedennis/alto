
struct Shadow {
    light_view: mat4x4<f32>,
    tex_scale: array<vec4<f32>, 4>,
    tex_offset: array<vec4<f32>, 4>,
    extents: array<vec4<f32>, 4>,
    splits: vec4<f32>,
    params: vec4<f32>,
    fade: vec4<f32>,
    lookup: vec4<f32>,
    spheres: array<vec4<f32>, 4>,
    bias_select: vec4<f32>,
};
@group(2) @binding(0) var<uniform> shadow: Shadow;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

// The sunlight shadow sample: one hardware PCF
// tap of the first cascade whose tile holds the point, lit past them.
fn vol_shadow(world: vec3<f32>) -> f32 {
    let count = i32(shadow.params.y);
    let lv = (shadow.light_view * vec4<f32>(world, 1.0)).xyz;
    var k = count;
    if (u32(shadow.bias_select.y) == 2u) {
        for (var i = 0; i < count; i++) {
            let d = lv - shadow.spheres[i].xyz;
            if (dot(d, d) < shadow.spheres[i].w) {
                k = i;
                break;
            }
        }
    } else {
        for (var i = 0; i < count; i++) {
            let c = lv * shadow.tex_scale[i].xyz + shadow.tex_offset[i].xyz;
            let e = shadow.extents[i];
            if (all(c.xy >= e.xy) && all(c.xy <= e.zw) && c.z >= 0.0 && c.z <= 1.0) {
                k = i;
                break;
            }
        }
    }
    if (k >= count) {
        return 1.0;
    }
    let c = lv * shadow.tex_scale[k].xyz + shadow.tex_offset[k].xyz;
    return textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy, c.z);
}

fn vol_phase(c: f32, m: vec4<f32>) -> f32 {
    return m.w * (m.x / pow(m.y - m.z * c, 1.5));
}

fn vol_exaggerate(v: vec2<f32>) -> vec2<f32> {
    let u = v.x + v.y;
    if (u > 0.0) {
        var s = v.x / u;
        let q = pass_block.p0.w;
        if (s > 0.0 && q > 0.0) {
            s = pow(s, q);
        }
        return u * vec2<f32>(s, 1.0 - s);
    }
    return v;
}

// The in-scattered light and the extinction of the view ray through
// camera-local pixel `px` (scene viewport coordinates) at wgpu depth `z`,
// dithered by the 4x4 tile at `ip`: the march and the scatter colour and extinction, scattering
// only.
fn vol_scatter(px: vec2<f32>, ip: vec2<i32>, z: f32) -> vec4<f32> {
    let sky = select(0.0, 1.0, z >= 1.0);
    let world = pass_unproject(px, min(z, 1.0));
    var d = world - frame.eye.xyz;
    let x = max(length(d), 1e-3);
    d = d / x;
    var dither = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let q4 = vec2<u32>(ip) % vec2<u32>(4u);
    let jitter = fract((dither[q4.y * 4u + q4.x] + 0.5) / 16.0);
    let n = max(i32(pass_block.p3.w), 1);
    let sv = pass_block.p4.w * 1.4;
    let h = min(sv, x);
    let m = 1.0 / f32(n);
    var q = jitter * m;
    var u = vec2<f32>(0.0);
    let s = pass_block.p0.x;
    let t = pass_block.p0.y;
    for (var i = 0; i < n; i++) {
        let f = fract(q);
        let a = frame.eye.xyz + d * (h * f);
        let e = exp(-h * f * t);
        let lit = min(1.0, vol_shadow(a));
        u += vec2<f32>(lit, 1.0 - lit) * e;
        q += m;
    }
    u *= s * h * m;
    let ext = exp(-x * t);
    if (x > sv) {
        u.x += s * (x - sv) * ext;
    }
    u = vol_exaggerate(u);
    let mg = mix(pass_block.p1, pass_block.p2, sky);
    u.x *= vol_phase(dot(d, frame.sun_dir.xyz), mg);
    // The sky's share.
    u = mix(u, u * pass_block.p0.z, sky);
    let out_ext = mix(ext, exp(-x * t * pass_block.p0.z), sky);
    let lit_c = frame.sun_colour.rgb * pass_block.p3.rgb;
    let unlit_c = frame.sky_ambient.rgb * pass_block.p4.rgb;
    return vec4<f32>(u.x * lit_c + u.y * unlit_c, out_ext);
}

// The half-size targets' size (first constants slot, xy).
fn vol_half() -> vec2<f32> {
    return pass_block.c[0].xy;
}

// The half-size depth: the farthest of the 2x2 scene depths (the first sample) under each
// half-size texel.
@fragment
fn fs_vol_depth(in: FullOut) -> @location(0) vec4<f32> {
    let base = vec2<i32>(pass_block.rect.xy) + vec2<i32>(in.clip.xy) * 2;
    let hi = vec2<i32>(pass_block.rect.xy + pass_block.rect.zw) - vec2<i32>(1);
    var z = 0.0;
    for (var j = 0; j < 2; j++) {
        for (var i = 0; i < 2; i++) {
            z = max(z, textureLoad(depth_tex, min(base + vec2<i32>(i, j), hi), 0));
        }
    }
    return vec4<f32>(z, 0.0, 0.0, 1.0);
}

// The half-size march: each half-size texel's ray through its centre over the half-size depth.
@fragment
fn fs_vol_march(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    let z = textureLoad(tex1, ip, 0).r;
    let px = pass_block.rect.xy + in.clip.xy / vol_half() * pass_block.rect.zw;
    return vol_scatter(px, ip, z);
}

// The view-space depth of wgpu depth `z` (at any pixel `px`: the
// projection's depth does not depend on it).
fn vol_view_depth(px: vec2<f32>, z: f32) -> f32 {
    let p = pass_unproject(px, min(z, 1.0));
    return (frame.view * vec4<f32>(p, 1.0)).z;
}

// The nearest-depth sample (the depth test is in view space, 200 units): the half-size
// scattering at the
// pixel, bilinear unless one of the four half-size depths around it lies
// more than 200 units from the pixel's own view depth, then the texel of
// the nearest depth (the first on ties; texel `k` at offset `(k % 2,
// k / 2)`, the order the shader selects by).
fn vol_upsample(frag: vec2<f32>) -> vec4<f32> {
    let v = vol_half();
    let hi = vec2<i32>(v) - vec2<i32>(1);
    let uv = (frag - pass_block.rect.xy) / pass_block.rect.zw;
    let own = vol_view_depth(frag, textureLoad(depth_tex, vec2<i32>(frag), 0));
    let f = vec2<i32>(floor(uv * v - vec2<f32>(0.251)));
    var edge = false;
    var best = 0;
    var nearest = 3.4e38;
    for (var k = 0; k < 4; k++) {
        let q = clamp(f + vec2<i32>(k % 2, k / 2), vec2<i32>(0), hi);
        let g = abs(vol_view_depth(frag, textureLoad(tex1, q, 0).r) - own);
        edge = edge || g > 200.0;
        if (g < nearest) {
            nearest = g;
            best = k;
        }
    }
    if (edge) {
        let q = clamp(f + vec2<i32>(best % 2, best / 2), vec2<i32>(0), hi);
        return textureLoad(tex2, q, 0);
    }
    // The bilinear tap at `uv`.
    let p = uv * v - vec2<f32>(0.5);
    let i = vec2<i32>(floor(p));
    let w = p - floor(p);
    let a = textureLoad(tex2, clamp(i, vec2<i32>(0), hi), 0);
    let b = textureLoad(tex2, clamp(i + vec2<i32>(1, 0), vec2<i32>(0), hi), 0);
    let c = textureLoad(tex2, clamp(i + vec2<i32>(0, 1), vec2<i32>(0), hi), 0);
    let d = textureLoad(tex2, clamp(i + vec2<i32>(1, 1), vec2<i32>(0), hi), 0);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

// The upsampled scattering, dithered (its colour times 1 + 1/16 of an interleaved gradient),
// applied to the frame (the colour times the extinction plus the in-scattered light).
@fragment
fn fs_vol_apply(in: FullOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(in.clip.xy);
    let src = textureLoad(tex0, ip, 0);
    let s = vol_upsample(in.clip.xy);
    let noise = fract(52.9829 * fract(dot(in.clip.xy, vec2<f32>(0.0671106, 0.00583715))));
    let ins = s.rgb * (1.0 + noise * 0.0625);
    return vec4<f32>(src.rgb * s.a + ins, src.a);
}
