
// The point-light shadows.
struct PointShadowLight {
    fade: vec4<f32>,                 // enabled, fade view params
    bias: vec4<f32>,                 // filter bias params
    scale: vec4<f32>,                // atlas face UV scale, atlas face UV texel size
    faces: array<vec4<f32>, 6>,      // atlas face UV extents +X -X +Y -Y +Z -Z
};
struct PointShadows {
    lights: array<PointShadowLight, 4>,
    ids: vec4<u32>,                  // the shadowed lights' ids (1-based slots)
    params: vec4<f32>,               // x max view distance, y filter, z smooth, w count
};
@group(3) @binding(8) var<uniform> point_shadows: PointShadows;
@group(3) @binding(9) var point_shadow_map: texture_depth_2d;

struct PointCaster {
    light: vec4<f32>,                // camera-local light, w 1 / r^2
    face: vec4<f32>,                 // x face, y near, z far (the light's radius)
};
@group(2) @binding(4) var<uniform> point_caster: PointCaster;

// One hardware compare tap of the atlas.
fn shadow_tap_on(t: texture_depth_2d, s: sampler_comparison, uv: vec2<f32>, z: f32) -> f32 {
    return textureSampleCompareLevel(t, s, uv, z);
}

// The depth filter library (the offsets are whole texels): 0 the 2x2 box
// (shifted half a texel, 1.5-texel border, taps -1..0), 1 the approximate
// 4x4 box (shifted, 2.5-texel border, the corners (-2,-2) (-2,1) (1,-2)
// (1,1), all sixteen of -2..1 when they disagree), 2 the 4x4 box (5-texel
// border, -2..1), 3 the single tap, 4 the 8x8 box (5-texel border, -4..3,
// smoothstep(2, 58, total)), 5 the 8-tap disk and 6 the 12-tap disk (5-texel
// border; the offset tables times 1.75, rounded away from zero).
fn shadow_filter_on(t: texture_depth_2d, s: sampler_comparison, kind: u32, c: vec3<f32>, texel: vec2<f32>, ext: vec4<f32>) -> f32 {
    if (kind == 3u) {
        return shadow_tap_on(t, s, c.xy, c.z);
    }
    if (kind == 0u) {
        let uv = clamp(c.xy + 0.5 * texel, ext.xy + 1.5 * texel, ext.zw - 1.5 * texel);
        var total = 0.0;
        for (var i = 0; i < 4; i++) {
            let o = vec2<f32>(f32(i % 2) - 1.0, f32(i / 2) - 1.0);
            total += shadow_tap_on(t, s, uv + o * texel, c.z);
        }
        return total * 0.25;
    }
    if (kind == 1u) {
        let uv = clamp(c.xy + 0.5 * texel, ext.xy + 2.5 * texel, ext.zw - 2.5 * texel);
        var corners = 0.0;
        for (var i = 0; i < 4; i++) {
            let o = vec2<f32>(select(-2.0, 1.0, (i & 1) != 0), select(-2.0, 1.0, (i & 2) != 0));
            corners += shadow_tap_on(t, s, uv + o * texel, c.z);
        }
        if (corners == 0.0 || corners >= 3.9) {
            return corners * 0.25;
        }
        var total = 0.0;
        for (var i = 0; i < 16; i++) {
            let o = vec2<f32>(f32(i % 4) - 2.0, f32(i / 4) - 2.0);
            total += shadow_tap_on(t, s, uv + o * texel, c.z);
        }
        return total * 0.0625;
    }
    let uv = clamp(c.xy, ext.xy + 5.0 * texel, ext.zw - 5.0 * texel);
    if (kind == 2u) {
        var total = 0.0;
        for (var i = 0; i < 16; i++) {
            let o = vec2<f32>(f32(i % 4) - 2.0, f32(i / 4) - 2.0);
            total += shadow_tap_on(t, s, uv + o * texel, c.z);
        }
        return total * 0.0625;
    }
    if (kind == 4u) {
        var total = 0.0;
        for (var i = 0; i < 64; i++) {
            let o = vec2<f32>(f32(i % 8) - 4.0, f32(i / 8) - 4.0);
            total += shadow_tap_on(t, s, uv + o * texel, c.z);
        }
        return smoothstep(2.0, 58.0, total);
    }
    if (kind == 5u) {
        var disk8 = array<vec2<f32>, 8>(
            vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(-1.0, 0.0),
            vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0));
        var total = 0.0;
        for (var i = 0; i < 8; i++) {
            total += shadow_tap_on(t, s, uv + disk8[i] * texel, c.z);
        }
        return total * 0.125;
    }
    var disk12 = array<vec2<f32>, 12>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(-1.0, 0.0), vec2<f32>(-1.0, 1.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 0.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, -2.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(2.0, 1.0), vec2<f32>(-1.0, -2.0), vec2<f32>(-1.0, -1.0));
    var total = 0.0;
    for (var i = 0; i < 12; i++) {
        total += shadow_tap_on(t, s, uv + disk12[i] * texel, c.z);
    }
    return total * 0.0833333;
}

// The point-light shadow attenuation after the light's normal offset: the light of slot `slot` reaching camera-local `world` of
// normal `n` (1 when the light has no shadow map).
fn point_shadow(slot: u32, world: vec3<f32>, n: vec3<f32>) -> f32 {
    let count = u32(point_shadows.params.w);
    if (count == 0u) {
        return 1.0;
    }
    let depth = (frame.view * vec4<f32>(world, 1.0)).z;
    if (depth > point_shadows.params.x) {
        return 1.0;
    }
    var k = 0u;
    loop {
        if (k >= count || k >= 4u) {
            return 1.0;
        }
        if (point_shadows.ids[k] == slot) {
            break;
        }
        k += 1u;
    }
    let s = point_shadows.lights[k];
    if (s.fade.x == 0.0) {
        return 1.0;
    }
    let l = point_lights[slot - 1u];
    let r = l.pos_radius.w;
    var b = l.pos_radius.xyz - world;
    let w = pow(1.0 - min(1.0 - 1.0 / 65535.0, dot(b, b) / (r * r)), POINT_LIGHT_FALLOFF);
    let v = normalize(b);
    b = b + n * (s.bias.y + (1.0 - w) * s.bias.y);
    // The cube-map face texture coordinates of -P, P = V with z negated.
    let d = vec3<f32>(-v.x, -v.y, v.z);
    let a = abs(d);
    var f: i32;
    var st: vec2<f32>;
    var ma: f32;
    if (a.x >= a.y && a.x >= a.z) {
        f = select(0, 1, d.x < 0.0);
        st = vec2<f32>(select(-d.z, d.z, d.x < 0.0), -d.y);
        ma = a.x;
    } else if (a.y >= a.z) {
        f = select(2, 3, d.y < 0.0);
        st = vec2<f32>(d.x, select(d.z, -d.z, d.y < 0.0));
        ma = a.y;
    } else {
        f = select(4, 5, d.z < 0.0);
        st = vec2<f32>(select(d.x, -d.x, d.z < 0.0), -d.y);
        ma = a.z;
    }
    st = st * (0.5 / ma) + 0.5;
    let ext = point_shadows.lights[k].faces[f];
    let uv = st * s.scale.xy + ext.xy;
    let z = dot(b, b) / (r * r) + s.bias.z;
    var e = shadow_filter_on(point_shadow_map, shadow_sampler, u32(point_shadows.params.y), vec3<f32>(uv, z), s.scale.zw, ext);
    if (point_shadows.params.z > 0.5) {
        e = mix(e, 1.0, smoothstep(s.fade.y, s.fade.z, depth));
    } else {
        e = mix(e, 1.0, clamp((depth - s.fade.y) * s.fade.w, 0.0, 1.0));
    }
    return min(1.0, e + (1.0 - s.bias.w));
}

// The point light with the shadow term.
fn point_light(slot: u32, world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let c = point_light_unshadowed(slot, world, n);
    if (all(c == vec3<f32>(0.0))) {
        return c;
    }
    return c * point_shadow(slot, world, n);
}

// The point-light caster pass: the forward vertex transform, then the face's
// 90 degree projection of the direction with z negated (the receiver's
// cube-map face lookup inverted), near plane point_caster.face.y and far
// plane point_caster.face.z (the radius; beyond it the depth is 1 anyway).
@vertex
fn vs_point_shadow(v: VsIn) -> VsOut {
    var out = vertex(v);
    let d = out.world - point_caster.light.xyz;
    let q = vec3<f32>(d.x, d.y, -d.z);
    let f = u32(point_caster.face.x);
    var sc: f32;
    var tc: f32;
    var ma: f32;
    switch f {
        case 0u: { ma = q.x; sc = -q.z; tc = -q.y; }
        case 1u: { ma = -q.x; sc = q.z; tc = -q.y; }
        case 2u: { ma = q.y; sc = q.x; tc = q.z; }
        case 3u: { ma = -q.y; sc = q.x; tc = -q.z; }
        case 4u: { ma = q.z; sc = q.x; tc = -q.y; }
        default: { ma = -q.z; sc = -q.x; tc = -q.y; }
    }
    let near = point_caster.face.y;
    let far = point_caster.face.z;
    out.clip = vec4<f32>(sc, -tc, far * (ma - near) / (far - near), ma);
    return out;
}

// The cutout test of the sun casters (fs_shadow), then the depth d^2 times
// the light's inverse squared far clip.
@fragment
fn fs_point_shadow(in: VsOut) -> @builtin(frag_depth) f32 {
    let flags = u32(in.p0.w);
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let footprint = 0.5 * log2(max(max(dot(duv_x, duv_x), dot(duv_y, duv_y)), 1e-24));
    var tex: vec4<f32>;
    if (material.diffuse.w > 0.5) {
        tex = sample_atlas(diffuse_tex, in.uv, footprint, material.diffuse);
        if (material.params.x > 0.5) {
            tex.a = tex.a * select(0.0, 1.0, max(max(tex.r, tex.g), tex.b) > 0.0005);
        }
    } else {
        tex = sample_material(diffuse_tex, false, in.uv, flags, material_lod(diffuse_tex, in.uv, flags));
    }
    var coverage = tex.a;
    if ((flags & 1u) != 0u) {
        coverage = 1.0;
    }
    let alpha = coverage * in.albedo.a;
    let tested = select(alpha, coverage, (flags & 16u) != 0u);
    if (alpha < 0.5 || (in.p0.z >= 0.0 && tested <= in.p0.z)) {
        discard;
    }
    let d = in.world - point_caster.light.xyz;
    return clamp(dot(d, d) * point_caster.light.w, 0.0, 1.0);
}
