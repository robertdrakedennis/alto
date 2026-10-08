
// post::PostFrame.
struct PostFrame {
    rect: vec4<f32>,        // scene viewport x, y, w, h (frame pixels)
    clip: vec4<f32>,        // its scissor l, t, r, b
    sizes: vec4<f32>,       // bloom target w, h; luminance cell w, h
    ssao: vec4<f32>,        // radius (pixels at unit depth), intensity, bias, base samples
    ssao2: vec4<f32>,       // z far, min samples, blur sharpness, -
    directions: array<vec4<f32>, 16>,
    bloom: vec4<f32>,       // threshold, scale, on, -
    exposure: vec4<f32>,    // key, min, max, adaptation on
    adapt: vec4<f32>,       // time step (s), speed multiplier, reset, -
    tonemap: vec4<f32>,     // min black, max white, key, min auto exposure
    tonemap2: vec4<f32>,    // max auto exposure, 1 = the modern operator
    fxaa: vec4<f32>,        // subpix, edge threshold, edge threshold min, FXAA follows
};
struct Pass {
    p: vec4<f32>,
    // The upscale's second block (`post::PassParams::pad[0]`).
    q: vec4<f32>,
};
// post::grading::PostUniforms (the grading of the composite).
struct Grading {
    params: vec4<f32>,      // x exposure, y 1 = encode, z remap slots, w levels on
    weights: vec4<f32>,
    levels: vec4<f32>,
    levels_max: vec4<f32>,
};
@group(0) @binding(0) var<uniform> pf: PostFrame;
@group(0) @binding(1) var<uniform> pp: Pass;
@group(0) @binding(2) var t0: texture_2d<f32>;
@group(0) @binding(3) var t1: texture_2d<f32>;
@group(0) @binding(4) var t2: texture_2d<f32>;
@group(0) @binding(5) var t3: texture_2d<f32>;
@group(0) @binding(6) var<uniform> grading: Grading;

// The luminance weights and the smallest valid luminance.
const LUM_CONVERT_SCALE: vec3<f32> = vec3<f32>(0.2125, 0.7154, 0.0721);
const MIN_VALID_LUMINANCE: f32 = 1e-5;

// The luminance of a colour, never below the smallest valid luminance.
fn calculate_luminance(c: vec3<f32>) -> f32 {
    return max(dot(c, LUM_CONVERT_SCALE) + MIN_VALID_LUMINANCE, MIN_VALID_LUMINANCE);
}

@vertex
fn vs_full(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let x = f32((id << 1u) & 2u);
    let y = f32(id & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// A frame pixel clamped inside the scissor.
fn in_clip(p: vec2<i32>) -> vec2<i32> {
    let lo = vec2<i32>(pf.clip.xy);
    let hi = max(vec2<i32>(pf.clip.zw) - vec2<i32>(1), lo);
    return clamp(p, lo, hi);
}

fn inside_clip(p: vec2<i32>) -> bool {
    return p.x >= i32(pf.clip.x) && p.y >= i32(pf.clip.y)
        && p.x < i32(pf.clip.z) && p.y < i32(pf.clip.w);
}

// ---- SSAO and its geometry-aware blur ----

@fragment
fn fs_ssao(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let nd = textureLoad(t0, px, 0);
    // No geometry (the normal target's clear): unoccluded.
    if (nd.w < 0.5) {
        return vec4<f32>(1.0);
    }
    let n = nd.xyz;
    let e = textureLoad(t1, px, 0).xyz;
    // The screen radius shrinks with depth (the radius over z).
    let radius = pf.ssao.x / e.z;
    // The sample count falls off with depth, never below the minimum.
    let base = pf.ssao.w;
    let d = max(base * pf.ssao2.x / (pf.ssao2.x + base * e.z), pf.ssao2.y);
    let count = i32(min(base, d));
    var q = 0.0;
    for (var i = 0; i < count; i++) {
        let f = pf.directions[i].xy;
        let m = vec2<i32>(floor(frag.xy + f * radius));
        if (!inside_clip(m) || textureLoad(t0, m, 0).w < 0.5) {
            continue;
        }
        let c = textureLoad(t1, m, 0).xyz;
        let x = c - e;
        q += max(0.0, dot(x, n) - c.z * pf.ssao.z) / (dot(x, x) + 1e-5);
    }
    q = max(0.0, 1.0 - 2.0 * pf.ssao.y / d * q);
    return vec4<f32>(q, q, q, 1.0);
}

// The geometry-aware blur: 7 taps (kernel radius 3) along `pp.p.xy`, each
// weighted by its distance and its depth difference to the centre.
@fragment
fn fs_blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    var result = textureLoad(t0, px, 0).r;
    if (textureLoad(t1, px, 0).w < 0.5) {
        return vec4<f32>(1.0);
    }
    let depth = textureLoad(t2, px, 0).z;
    let dir = vec2<i32>(pp.p.xy);
    let r = 3.0 * 0.5;
    let falloff = 1.0 / (2.0 * r * r);
    var weights = 1.0;
    for (var k = -3; k <= 3; k++) {
        if (k == 0 || abs(k) > i32(pp.q.y)) {
            continue;
        }
        let q = in_clip(px + dir * k);
        if (textureLoad(t1, q, 0).w < 0.5) {
            continue;
        }
        let h = f32(k) * pp.q.x;
        let de = (textureLoad(t2, q, 0).z - depth) * pf.ssao2.z;
        let w = exp2(-h * h * falloff - de * de);
        weights += w;
        result += textureLoad(t0, q, 0).r * w;
    }
    result /= weights;
    return vec4<f32>(result, 0.0, 0.0, 1.0);
}

// ---- Scene luminance and adaptation ----

// 64x64 cells over the scissor: the mean colour of a 3x3 tap grid per cell,
// its log luminance.
@fragment
fn fs_lum_log(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = floor(frag.xy);
    var rgb = vec3<f32>(0.0);
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            let at = pf.clip.xy + (cell + (vec2<f32>(f32(i), f32(j)) + 0.5) / 3.0) * pf.sizes.zw;
            rgb += max(textureLoad(t0, in_clip(vec2<i32>(floor(at))), 0).rgb, vec3<f32>(0.0));
        }
    }
    return vec4<f32>(log(calculate_luminance(rgb / 9.0 * pf.tonemap2.z)), 0.0, 0.0, 1.0);
}

// A 16-tap box of the previous level; `pp.p.x` 1: the last (the output is
// the exponential of the mean log).
@fragment
fn fs_lum_down(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let base = vec2<i32>(frag.xy) * 4;
    var sum = 0.0;
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            sum += textureLoad(t0, base + vec2<i32>(i, j), 0).r;
        }
    }
    var v = sum * 0.0625;
    if (pp.p.x > 0.5) {
        v = exp(v);
    }
    return vec4<f32>(v, 0.0, 0.0, 1.0);
}

// The adapted luminance, blending rod and cone adaptation times
// (post::adapt).
@fragment
fn fs_adapt(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let curr = textureLoad(t0, vec2<i32>(0, 0), 0).r;
    let prev = textureLoad(t1, vec2<i32>(0, 0), 0).r;
    var next = curr;
    if (pf.adapt.z < 0.5) {
        let rods = 0.04 / (0.04 + curr);
        let tau = rods * 0.2 + (1.0 - rods) * 0.4;
        let k = clamp(1.0 - exp(-(pf.adapt.x / (tau * pf.adapt.y))), 0.0, 1.0);
        next = mix(prev, curr, k);
    }
    return vec4<f32>(max(max(next, MIN_VALID_LUMINANCE), MIN_VALID_LUMINANCE), 0.0, 0.0, 1.0);
}

// This backend's exposure of the adapted luminance (post::exposure; 1
// without adaptation).
fn exposure_of(adapted: f32) -> f32 {
    if (pf.exposure.w < 0.5) {
        return 1.0;
    }
    return clamp(pf.exposure.x / max(adapted, MIN_VALID_LUMINANCE), pf.exposure.y, pf.exposure.z);
}

// ---- Bloom (bright pass, Kawase blur, composite) ----

// The bright pass without a tone map: a colour whose luminance passes the
// brightness threshold goes through whole, clamped to the maximum auto
// exposure; the rest is black (post::bright).
fn bright_of(c: vec3<f32>) -> vec3<f32> {
    if (calculate_luminance(c) > pf.bloom.x) {
        return clamp(c, vec3<f32>(0.0), vec3<f32>(pf.tonemap2.x));
    }
    return vec3<f32>(0.0);
}

// The half-size bright pass: the source read at the half-size texel's
// centre (the mean of its 2x2 block, a bilinear read).
@fragment
fn fs_bright(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let base = vec2<i32>(pf.rect.xy) + vec2<i32>(frag.xy) * 2;
    var rgb = vec3<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        for (var i = 0; i < 2; i++) {
            rgb += textureLoad(t0, in_clip(base + vec2<i32>(i, j)), 0).rgb;
        }
    }
    return vec4<f32>(bright_of(rgb * 0.25 * pf.tonemap2.z), 1.0);
}

// The mean of the 2x2 block at `a`: a bilinear tap on the texel corner.
fn quad_mean(a: vec2<i32>) -> vec3<f32> {
    let hi = vec2<i32>(pf.sizes.xy) - vec2<i32>(1);
    let p0 = clamp(a, vec2<i32>(0), hi);
    let p1 = clamp(a + vec2<i32>(1, 1), vec2<i32>(0), hi);
    return (textureLoad(t0, p0, 0).rgb + textureLoad(t0, vec2<i32>(p1.x, p0.y), 0).rgb
        + textureLoad(t0, vec2<i32>(p0.x, p1.y), 0).rgb + textureLoad(t0, p1, 0).rgb) * 0.25;
}

// The Kawase blur at iteration `pp.p.x`: four taps (iteration + 0.5)
// texels out diagonally, averaged.
@fragment
fn fs_kawase(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec2<i32>(frag.xy);
    let o = i32(pp.p.x);
    let sum = quad_mean(c + vec2<i32>(-o - 1, o)) + quad_mean(c + vec2<i32>(o, o))
        + quad_mean(c + vec2<i32>(o, -o - 1)) + quad_mean(c + vec2<i32>(-o - 1, -o - 1));
    return vec4<f32>(sum * 0.25, 1.0);
}

// The blurred bloom at a frame pixel (bilinear from the half-size target).
fn bloom_at(frag: vec2<f32>) -> vec3<f32> {
    let h = (frag - pf.rect.xy) * 0.5 - 0.5;
    let i = vec2<i32>(floor(h));
    let f = h - floor(h);
    let hi = vec2<i32>(pf.sizes.xy) - vec2<i32>(1);
    let p0 = clamp(i, vec2<i32>(0), hi);
    let p1 = clamp(i + vec2<i32>(1, 1), vec2<i32>(0), hi);
    let a = textureLoad(t1, p0, 0).rgb;
    let b = textureLoad(t1, vec2<i32>(p1.x, p0.y), 0).rgb;
    let c = textureLoad(t1, vec2<i32>(p0.x, p1.y), 0).rgb;
    let d = textureLoad(t1, p1, 0).rgb;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// The remap LUTs (post::grading; the composite of shaders::TONEMAP_WGSL).
fn lut_texel(slot: i32, r: i32, g: i32, b: i32) -> vec3<f32> {
    return textureLoad(t3, vec2<i32>(16 * b + r, 16 * slot + g), 0).rgb;
}

fn lut_sample(slot: i32, c: vec3<f32>) -> vec3<f32> {
    let t = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * 15.0;
    let i0 = min(vec3<i32>(floor(t)), vec3<i32>(15));
    let i1 = min(i0 + vec3<i32>(1), vec3<i32>(15));
    let f = t - vec3<f32>(i0);
    var out = vec3<f32>(0.0);
    for (var k = 0; k < 8; k++) {
        let hi = vec3<bool>((k & 1) != 0, (k & 2) != 0, (k & 4) != 0);
        let i = select(i0, i1, hi);
        let w = select(vec3<f32>(1.0) - f, f, hi);
        out += lut_texel(slot, i.x, i.y, i.z) * (w.x * w.y * w.z);
    }
    return out;
}

// The advanced per-channel Reinhard tone map (post::reinhard_adv_rgb).
fn reinhard_adv_rgb(v: vec3<f32>, avg: f32) -> vec3<f32> {
    let r = max(calculate_luminance(v) - pf.tonemap.x, 0.0);
    let c = clamp(r * (pf.tonemap.z / avg), pf.tonemap.w, pf.tonemap2.x);
    let white = pf.tonemap.y * pf.tonemap.y;
    return v * (c * (1.0 + c / white) / (1.0 + c));
}

// The composite: the bloom added to the scene, the tone mapping (this
// backend's exposure and curve, or with `reinhard` the modern operator and
// its display encode), the clamp, then the colour correction
// (post::grading: levels and the remap LUTs).
@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let hdr = textureLoad(t0, vec2<i32>(frag.xy), 0).rgb * pf.tonemap2.z;
    let adapted = textureLoad(t2, vec2<i32>(0, 0), 0).r;
    var e = hdr;
    if (pf.bloom.z > 0.5) {
        e = e + bloom_at(frag.xy) * pf.bloom.y;
    }
    var c: vec3<f32>;
    if (pf.tonemap2.y > 0.5) {
        c = sqrt(clamp(tone_map_op(e, adapted), vec3<f32>(0.0), vec3<f32>(1.0)));
    } else {
        c = clamp(linear_to_display(tonemap(e * exposure_of(adapted), grading.params.x)), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if (grading.params.w > 0.5) {
        let x = clamp((c - grading.levels.y) / (grading.levels.z - grading.levels.y), vec3<f32>(0.0), vec3<f32>(1.0));
        let g = clamp(pow(x, vec3<f32>(grading.levels.x)), vec3<f32>(0.0), vec3<f32>(1.0));
        c = vec3<f32>(grading.levels.w) + g * (grading.levels_max.x - grading.levels.w);
    }
    let slots = i32(grading.params.z);
    if (slots > 0) {
        var out = c * grading.weights.x;
        out += lut_sample(0, c) * grading.weights.y;
        if (slots > 1) { out += lut_sample(1, c) * grading.weights.z; }
        if (slots > 2) { out += lut_sample(2, c) * grading.weights.w; }
        c = out;
    }
    if (grading.params.y < 0.5 && pf.fxaa.w < 0.5) {
        c = srgb_to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return vec4<f32>(c, 1.0);
}

// ---- FXAA (FXAA 3.11 PC quality with green as luma; the default quality is
// preset 15) ----

fn fx_load(p: vec2<i32>) -> vec3<f32> {
    return textureLoad(t0, in_clip(p), 0).rgb;
}

// FxaaTexTop: bilinear at a position in pixels (texel centres at +0.5).
fn fx_top(pos: vec2<f32>) -> vec3<f32> {
    let q = pos - 0.5;
    let i = vec2<i32>(floor(q));
    let f = q - floor(q);
    let a = fx_load(i);
    let b = fx_load(i + vec2<i32>(1, 0));
    let c = fx_load(i + vec2<i32>(0, 1));
    let d = fx_load(i + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn fx_out(c: vec3<f32>) -> vec4<f32> {
    if (grading.params.y < 0.5) {
        return vec4<f32>(srgb_to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
    }
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_fxaa(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let rgb_m = fx_load(p);
    // FXAA_GREEN_AS_LUMA: luma is the green channel.
    let luma_m = rgb_m.g;
    var luma_s = fx_load(p + vec2<i32>(0, 1)).g;
    let luma_e = fx_load(p + vec2<i32>(1, 0)).g;
    var luma_n = fx_load(p + vec2<i32>(0, -1)).g;
    let luma_w = fx_load(p + vec2<i32>(-1, 0)).g;
    let range_max = max(max(luma_n, luma_w), max(luma_e, max(luma_s, luma_m)));
    let range_min = min(min(luma_n, luma_w), min(luma_e, min(luma_s, luma_m)));
    let luma_range = range_max - range_min;
    if (luma_range < max(pf.fxaa.z, range_max * pf.fxaa.y)) {
        return fx_out(rgb_m);
    }
    let luma_nw = fx_load(p + vec2<i32>(-1, -1)).g;
    let luma_se = fx_load(p + vec2<i32>(1, 1)).g;
    let luma_ne = fx_load(p + vec2<i32>(1, -1)).g;
    let luma_sw = fx_load(p + vec2<i32>(-1, 1)).g;
    let luma_ns = luma_n + luma_s;
    let luma_we = luma_w + luma_e;
    let edge_horz = abs(-2.0 * luma_w + luma_nw + luma_sw)
        + abs(-2.0 * luma_m + luma_ns) * 2.0 + abs(-2.0 * luma_e + luma_ne + luma_se);
    let edge_vert = abs(-2.0 * luma_s + luma_sw + luma_se)
        + abs(-2.0 * luma_m + luma_we) * 2.0 + abs(-2.0 * luma_n + luma_nw + luma_ne);
    let horz_span = edge_horz >= edge_vert;
    let subpix_a = (luma_ns + luma_we) * 2.0 + (luma_nw + luma_sw + luma_ne + luma_se);
    if (!horz_span) {
        luma_n = luma_w;
        luma_s = luma_e;
    }
    // One pixel across the edge (y for a horizontal span), signed.
    var length_sign = 1.0;
    let subpix_b = subpix_a * (1.0 / 12.0) - luma_m;
    let gradient_n = luma_n - luma_m;
    let gradient_s = luma_s - luma_m;
    var luma_nn = luma_n + luma_m;
    let luma_ss = luma_s + luma_m;
    let pair_n = abs(gradient_n) >= abs(gradient_s);
    let gradient = max(abs(gradient_n), abs(gradient_s));
    if (pair_n) {
        length_sign = -length_sign;
    }
    let subpix_c = clamp(abs(subpix_b) / luma_range, 0.0, 1.0);
    var across = vec2<f32>(1.0, 0.0);
    var along = vec2<f32>(0.0, 1.0);
    if (horz_span) {
        across = vec2<f32>(0.0, 1.0);
        along = vec2<f32>(1.0, 0.0);
    }
    let pos_b = frag.xy + across * (length_sign * 0.5);
    // FXAA_QUALITY_PRESET 15: 8 steps.
    var steps = array<f32, 8>(1.0, 1.5, 2.0, 2.0, 2.0, 2.0, 4.0, 12.0);
    var pos_n = pos_b - along * steps[0];
    var pos_p = pos_b + along * steps[0];
    let subpix_d = -2.0 * subpix_c + 3.0;
    var luma_end_n = fx_top(pos_n).g;
    let subpix_e = subpix_c * subpix_c;
    var luma_end_p = fx_top(pos_p).g;
    if (!pair_n) {
        luma_nn = luma_ss;
    }
    let gradient_scaled = gradient * 1.0 / 4.0;
    let luma_mm = luma_m - luma_nn * 0.5;
    let subpix_f = subpix_d * subpix_e;
    let luma_m_lt_zero = luma_mm < 0.0;
    luma_end_n -= luma_nn * 0.5;
    luma_end_p -= luma_nn * 0.5;
    var done_n = abs(luma_end_n) >= gradient_scaled;
    var done_p = abs(luma_end_p) >= gradient_scaled;
    if (!done_n) { pos_n -= along * steps[1]; }
    if (!done_p) { pos_p += along * steps[1]; }
    // Each further step reads the ends still searching, then moves them
    // (the last move is not read: the unrolled shader's shape).
    for (var i = 2; i < 8; i++) {
        if (done_n && done_p) {
            break;
        }
        if (!done_n) { luma_end_n = fx_top(pos_n).g - luma_nn * 0.5; }
        if (!done_p) { luma_end_p = fx_top(pos_p).g - luma_nn * 0.5; }
        done_n = abs(luma_end_n) >= gradient_scaled;
        done_p = abs(luma_end_p) >= gradient_scaled;
        if (!done_n) { pos_n -= along * steps[i]; }
        if (!done_p) { pos_p += along * steps[i]; }
    }
    let dst_n = dot(frag.xy - pos_n, along);
    let dst_p = dot(pos_p - frag.xy, along);
    let good_span_n = (luma_end_n < 0.0) != luma_m_lt_zero;
    let span_length = dst_p + dst_n;
    let good_span_p = (luma_end_p < 0.0) != luma_m_lt_zero;
    let direction_n = dst_n < dst_p;
    let dst = min(dst_n, dst_p);
    let good_span = select(good_span_p, good_span_n, direction_n);
    let subpix_g = subpix_f * subpix_f;
    let pixel_offset = dst * (-1.0 / span_length) + 0.5;
    let subpix_h = subpix_g * pf.fxaa.x;
    let pixel_offset_good = select(0.0, pixel_offset, good_span);
    let pixel_offset_subpix = max(pixel_offset_good, subpix_h);
    return fx_out(fx_top(frag.xy + across * (pixel_offset_subpix * length_sign)));
}
