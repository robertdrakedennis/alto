
// ---- Prefilter, LUT (lighting::probes) ----

// The fullscreen passes: a face of a cube level, its texel's cube-space
// direction (wgpu's convention: lighting::probes::face_axes in cube space).
struct Filter {
    // x: face, y: the source's face size, z: roughness, w: the source level to read (downsample).
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> filt: Filter;
@group(0) @binding(1) var source_cube: texture_cube<f32>;
@group(0) @binding(2) var source_sampler: sampler;

@vertex
fn vs_full(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let x = f32((id << 1u) & 2u);
    let y = f32(id & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The cube-space direction of pixel `frag` of a `size` face.
fn cube_dir(face: u32, frag: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let sc = frag.x / size.x * 2.0 - 1.0;
    let tc = frag.y / size.y * 2.0 - 1.0;
    let d = face_dir(face, sc, tc);
    return normalize(vec3<f32>(d.x, -d.y, d.z));
}

// The mip chain of the captured cube: the level above (the bound view
// holds just that level) at this texel's direction (bilinear, a 2x2 box).
@fragment
fn fs_downsample(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(source_cube, 0)) * 0.5;
    let d = cube_dir(u32(filt.params.x), frag.xy, size);
    return vec4<f32>(textureSampleLevel(source_cube, source_sampler, d, 0.0).rgb, 1.0);
}

// Soft clamp of an HDR sample's luminance (the clamp scale is 8).
fn soft_clamp(f: vec3<f32>) -> vec3<f32> {
    let v = max(0.0001, dot(vec3<f32>(0.2627, 0.678, 0.0593), f));
    let m = min(v, 8.0 * 3.0);
    let p = m / 8.0;
    let t = m / sqrt(1.0 + p * p);
    return f * (t / v);
}

fn inv_soft_clamp(f: vec3<f32>) -> vec3<f32> {
    let v = max(0.0001, dot(vec3<f32>(0.2627, 0.678, 0.0593), f));
    let p = v / 8.0;
    let m = v / sqrt(max(1.0 - p * p, 1e-6));
    return f * (m / v);
}

fn reverse_bits(x: u32) -> u32 {
    return reverseBits(x);
}

// The Hammersley sequence and GGX importance sampling.
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), f32(reverse_bits(i)) * 2.32831e-10);
}

fn ggx_sample(u: vec2<f32>, t: vec3<f32>, p: f32) -> vec3<f32> {
    let v = p * p;
    let f = 6.28318 * u.x;
    let c = (1.0 - u.y) / (1.0 + (v - 1.0) * u.y);
    let e = sqrt(c);
    let s = sqrt(1.0 - c);
    let n = vec3<f32>(s * cos(f), s * sin(f), e);
    var r = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(t.z) < 0.999) {
        r = vec3<f32>(0.0, 0.0, 1.0);
    }
    let i = normalize(cross(r, t));
    let o = cross(i, t);
    return normalize(i * n.x + o * n.y + t * n.z);
}

// The environment prefilter (without the filtered-envmap normalisation
// variant): 1024 GGX samples of roughness `params.z` about this
// texel's direction, each read at the source level its pdf asks for
// (0.5 log2(1 / (N pdf) / (4π / 6 size²))), soft-clamped, weighted by N.L.
@fragment
fn fs_prefilter(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let out_size = vec2<f32>(textureDimensions(source_cube, 0)) / exp2(filt.params.w);
    let p = cube_dir(u32(filt.params.x), frag.xy, out_size);
    let v = filt.params.y * filt.params.y;
    let u = filt.params.z;
    let d = u * u;
    let top = f32(textureNumLevels(source_cube)) - 1.0;
    var total = vec3<f32>(0.0);
    var weight = 0.0;
    for (var k = 0u; k < 1024u; k++) {
        let e = hammersley(k, 1024u);
        let l = ggx_sample(e, p, d);
        let r = normalize(2.0 * dot(p, l) * l - p);
        let n = max(0.0, dot(p, r));
        if (n > 0.0) {
            let g = max(0.0, dot(p, l));
            let pl = max(0.0, dot(p, l));
            // The GGX (Trowbridge-Reitz) normal distribution at (g, u).
            let gg = g * g;
            let uu = u * u;
            let tt = uu * (gg - 1.0) + 1.0;
            let ndf = gg / (3.14159 * (tt * tt + 0.0001));
            let a = ndf * g / (4.0 * pl + 0.0001);
            let b = 4.0 * 3.14159 / (6.0 * v);
            let o = 1.0 / (1024.0 * a + 0.0001);
            var lod = 0.0;
            if (u != 0.0) {
                lod = clamp(0.5 * log2(o / b), 0.0, top);
            }
            let l0 = floor(lod);
            let s0 = textureSampleLevel(source_cube, source_sampler, r, l0).rgb;
            let s1 = textureSampleLevel(source_cube, source_sampler, r, min(l0 + 1.0, top)).rgb;
            total += soft_clamp(mix(s0, s1, lod - l0)) * n;
            weight += n;
        }
    }
    return vec4<f32>(inv_soft_clamp(total / max(weight, 1e-6)), 1.0);
}

// The BRDF LUT integration (1024 samples; height-correlated Smith
// geometry term) at (N.V, roughness) = the texel centre.
fn smith_lambda(c: f32, rough: f32) -> f32 {
    let s = rough * rough;
    let a = sqrt(1.0 - min(1.0, c * c));
    let t = a / max(1e-7, c);
    return (sqrt(1.0 + s * s * t * t) - 1.0) / 2.0;
}

@fragment
fn fs_lut(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(filt.params.y);
    let uv = frag.xy / size;
    let nv = uv.x;
    let m = uv.y;
    let s = m * m;
    let v = vec3<f32>(sqrt(max(1.0 - nv * nv, 0.0)), 0.0, nv);
    var a = 0.0;
    var b = 0.0;
    for (var k = 0u; k < 1024u; k++) {
        let h = ggx_sample(hammersley(k, 1024u), vec3<f32>(0.0, 0.0, 1.0), s);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let nl = l.z;
        let nvv = max(0.0, v.z);
        let nh = max(0.0, h.z);
        let vh = max(0.0, dot(v, h));
        if (nl > 0.0) {
            let g = 1.0 / (1.0 + smith_lambda(nvv, m) + smith_lambda(nl, m));
            let vis = clamp(g * vh / max(1e-7, nh * nvv), 0.0, 1.0);
            let fc = pow(1.0 - vh, 5.0);
            a += (1.0 - fc) * vis;
            b += fc * vis;
        }
    }
    return vec4<f32>(a / 1024.0, b / 1024.0, 0.0, 1.0);
}
