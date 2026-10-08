
// ---- Light probes and IBL (lighting::probes) ----

// frame::probes::ProbeUniforms.
struct Probes {
    origin: vec4<f32>,   // xyz: the scene-local camera origin
    grid: vec4<f32>,     // xy: scene-local x, z of probe (0, 0); z: 1 / ZONE; w: the normal sample bias
    dims: vec4<u32>,     // x, y: probes along x and z; z: the global probe; w: debug output
    env: vec4<f32>,      // x: the last level; y: 1 = captured; z: the least metalness (tests)
    ambient: vec4<f32>,  // x: 1 = the per-square ambient; yz: the scene-local origin's offset from the first cell (fine units)
    ambient_dims: vec4<u32>, // x: the cells per side
};
@group(3) @binding(3) var<storage, read> probe_sh: array<vec4<f32>>;
@group(3) @binding(4) var<uniform> probes: Probes;
@group(3) @binding(5) var env_cube: texture_cube<f32>;
@group(3) @binding(6) var brdf_lut: texture_2d<f32>;
@group(3) @binding(7) var env_sampler: sampler;

const PROBE_STRIDE: u32 = 8u;

// Second-order SH lighting of probe coefficients c (Sloan's seven vectors).
fn eval_sh(c: array<vec4<f32>, 7>, n: vec3<f32>) -> vec3<f32> {
    let n1 = vec4<f32>(n, 1.0);
    let x1 = vec3<f32>(dot(c[0], n1), dot(c[1], n1), dot(c[2], n1));
    let vb = n1.xyzz * n1.yzzx;
    let x2 = vec3<f32>(dot(c[3], vb), dot(c[4], vb), dot(c[5], vb));
    let x3 = c[6].rgb * (n.x * n.x - n.y * n.y);
    return x1 + x2 + x3;
}

fn probe_coefs(i: u32) -> array<vec4<f32>, 7> {
    var c: array<vec4<f32>, 7>;
    for (var k = 0u; k < 7u; k++) {
        c[k] = probe_sh[i * PROBE_STRIDE + k];
    }
    return c;
}

// The camera-local position pushed along the normal, its zone and the four
// probes' coefficients mixed bilinearly, then evaluated.
fn probe_irradiance(world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let biased = world + n * probes.grid.w;
    let nx = probes.dims.x;
    let nz = probes.dims.y;
    let p = clamp((round(biased.xz) + probes.origin.xz - probes.grid.xy) * probes.grid.z,
        vec2<f32>(0.0), vec2<f32>(f32(nx - 1u), f32(nz - 1u)));
    let i0 = min(vec2<u32>(floor(p)), vec2<u32>(max(nx, 2u) - 2u, max(nz, 2u) - 2u));
    let t = p - vec2<f32>(i0);
    let i1 = min(i0 + vec2<u32>(1u), vec2<u32>(nx - 1u, nz - 1u));
    var c: array<vec4<f32>, 7>;
    for (var k = 0u; k < 7u; k++) {
        let a = probe_sh[(i0.y * nx + i0.x) * PROBE_STRIDE + k];
        let b = probe_sh[(i0.y * nx + i1.x) * PROBE_STRIDE + k];
        let d = probe_sh[(i1.y * nx + i0.x) * PROBE_STRIDE + k];
        let e = probe_sh[(i1.y * nx + i1.x) * PROBE_STRIDE + k];
        c[k] = mix(mix(a, b, t.x), mix(d, e, t.x), t.y);
    }
    return eval_sh(c, n);
}

// The probe ambient (lighting::probes "Normalisation"): the look's sky ambient
// times the probes' irradiance over the global probe's up-facing
// luminance (the modern client: the ambient colour times the SH).
fn probe_ambient(world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if (probes.ambient.x > 0.5) {
        // The captured ambient (lighting::ambient): the ambient colour times
        // the map square's block, as it is.
        return frame.sky_ambient.rgb * max(square_irradiance(world, n), vec3<f32>(0.0));
    }
    if (probes.env.y < 0.5) {
        // Nothing captured yet (no scene): the hemisphere of the pass
        // before M6.
        return mix(frame.ground_ambient.rgb, frame.sky_ambient.rgb, 0.5 - 0.5 * n.y);
    }
    let up = eval_sh(probe_coefs(probes.dims.z), vec3<f32>(0.0, -1.0, 0.0));
    // Per channel, so open ground facing
    // up keeps the look's ambient colour and the sky's chroma is not
    // applied twice.
    let lum = max(dot(up, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-3);
    let norm = max(up, vec3<f32>(0.05 * lum));
    // The modern ambient colour * SH(n), the
    // captured light as it is.
    if (frame.params.w > 0.5) {
        return frame.sky_ambient.rgb * max(probe_irradiance(world, n), vec3<f32>(0.0));
    }
    return frame.sky_ambient.rgb * max(probe_irradiance(world, n), vec3<f32>(0.0)) / norm;
}

// The environment cube at classic direction r, level `lod`, the mip blend
// done here (sample_trilinear's reason: deterministic on Apple GPUs).
fn env_sample(r: vec3<f32>, lod: f32) -> vec3<f32> {
    let c = vec3<f32>(r.x, -r.y, r.z);
    let l = clamp(lod, 0.0, probes.env.x);
    let l0 = floor(l);
    let a = textureSampleLevel(env_cube, env_sampler, c, l0).rgb;
    let b = textureSampleLevel(env_cube, env_sampler, c, min(l0 + 1.0, probes.env.x)).rgb;
    return mix(a, b, l - l0);
}

// The environment reflection for the water (lane Q-WATER2's sky gradient
// replaced): the environment cube, one level down (the 128 cube's level 0
// is sharper than the planar image it fills in for).
fn probe_sky(r: vec3<f32>) -> vec3<f32> {
    return env_sample(r, 1.0);
}

// The IBL specular: the prefiltered cube along the reflected view at level
// (the last level) * roughness^2, times F * A + B from the LUT at
// (N.V, roughness^2).
fn ibl_specular(v: vec3<f32>, n: vec3<f32>, f: vec3<f32>, rough: f32) -> vec3<f32> {
    let r = reflect(-v, n);
    let c = clamp(rough, 0.0, 1.0);
    let c2 = c * c;
    let e = env_sample(r, probes.env.x * c2);
    let size = vec2<f32>(textureDimensions(brdf_lut, 0));
    let uv = clamp(vec2<f32>(max(dot(v, n), 0.0), c2), vec2<f32>(0.0), vec2<f32>(1.0));
    let ab = textureSampleLevel(brdf_lut, env_sampler, (uv * (size - 1.0) + 0.5) / size, 0.0).rg;
    return e * (f * ab.x + ab.y);
}

// shade_ambient (models::shading) with the modern probe ambient and, for RT7,
// the IBL terms.
fn shade(in: VsOut, s: Surface, fog: f32) -> vec3<f32> {
    let flags = u32(in.p0.w);
    if ((flags & 8u) != 0u) {
        return inverse_tonemap(s.colour, frame.params.x);
    }
    var lit = s.colour;
    let debug = probes.dims.w;
    if ((flags & 4u) == 0u) {
        let n = lit_normal(in, s.detail);
        let l = frame.sun_dir.xyz;
        let n_dot_l = max(dot(n, l), 0.0);
        var e = n_dot_l;
        if (n_dot_l > 0.0) {
            e = n_dot_l * sun_visibility(in.world, n, in.view_depth);
        }
        var ao = 1.0;
        if (frame.params.y > 0.5) {
            ao = ambient_occlusion(in.clip.xy);
        }
        // The diffuse ambient is the ambient colour times the map square's SH
        // lighting, times the SSAO.
        let ambient = probe_ambient(in.world, n) * ao;
        if (debug == 1u) {
            return ambient;
        }
        let v = normalize(frame.eye.xyz - in.world);
        let h = normalize(v + l);
        let sun = frame.sun_colour.rgb * e;
        let point = point_light_sum(in, n);
        if (s.rt7) {
            var s = s;
            s.metal = max(s.metal, probes.env.z);
            let albedo = mix(s.colour, s.tex, s.metal);
            let rough = clamp(s.rough * RT7_ROUGHNESS_MULTIPLIER, 0.0, 1.0);
            let gloss = 1.0 - rough;
            let power = pow(8192.0, gloss);
            let f0 = mix(vec3<f32>(RT7_NON_METAL_SPECULAR), albedo, s.metal);
            let n_dot_v = max(dot(n, v), 0.0);
            let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(dot(l, h), 0.0, 1.0), RT7_FRESNEL_EXPONENT);
            let brdf = ggx(rough, max(dot(n, h), 0.0)) * schlick_smith(power, n_dot_l, n_dot_v);
            let spec = fresnel * brdf * sun;
            let fr = f0 + (max(vec3<f32>(gloss), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
            let conserve = (vec3<f32>(1.0) - clamp(fr, vec3<f32>(0.0), vec3<f32>(1.0))) * (1.0 - s.metal);
            // The IBL diffuse adds SH * ambient colour * conserve, the
            // specular is the IBL specular; both times the SSAO; then the
            // diffuse is times conserve again (the modern client applies the
            // factor twice).
            let ibl_diffuse = ambient * conserve;
            let ibl_spec = ibl_specular(v, n, fr, rough) * ao;
            if (debug == 2u) {
                return ibl_spec;
            }
            if (debug == 3u) {
                return vec3<f32>(s.metal);
            }
            lit = (ibl_diffuse * conserve + (sun + point) * conserve) * albedo + spec + ibl_spec;
            lit = mix(lit, s.colour * (1.0 - s.metal), s.emissive);
        } else {
            if (debug >= 2u) {
                return vec3<f32>(0.0);
            }
            var spec = vec3<f32>(0.0);
            if (in.p1.z > 0.0) {
                let fresnel = RT5_F0 + (1.0 - RT5_F0) * pow(1.0 - clamp(dot(l, h), 0.0, 1.0), RT5_FRESNEL_POWER);
                let ndf = (in.p1.z + 2.0) * 0.125 * pow(max(dot(h, n), 0.0), in.p1.z);
                spec = sun * (fresnel * ndf * in.p1.w);
            }
            // The env mask blend:
            // the albedo towards env * SSAO * h * params.w by g *
            // FresnelSchlick(0.8, N.V, 5), before the lighting. The cube is
            // the record's (or the stand-in, the probes' cube over the open
            // ground's light), h the SH irradiance relative to the global
            // probe's up-facing one.
            var colour = s.colour;
            if ((flags & 96u) == 64u && s.env_mask > 0.0 && probes.env.y > 0.5) {
                let fr = 0.8 + 0.2 * pow(1.0 - max(dot(v, n), 0.0), 5.0);
                let up = eval_sh(probe_coefs(probes.dims.z), vec3<f32>(0.0, -1.0, 0.0));
                let up_lum = max(dot(up, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-3);
                let open = frame.sky_ambient.rgb * max(up, vec3<f32>(0.0))
                    + frame.sun_colour.rgb * max(-frame.sun_dir.y, 0.0);
                let norm = max(dot(open, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-3);
                let h = max(probe_irradiance(in.world, n), vec3<f32>(0.0)) / up_lum;
                let r = reflect(-v, n);
                var cube = env_sample(r, 0.0) / norm;
                if (global_env.params.z > 0.5) {
                    // The record's cube (the classic direction convention), faded.
                    let a = textureSampleLevel(global_env_a, env_sampler, r, 0.0).rgb;
                    let b = textureSampleLevel(global_env_b, env_sampler, r, 0.0).rgb;
                    cube = texture_linear(mix(a, b, global_env.params.x));
                }
                let env = cube * ao * h * global_env.params.y;
                colour = mix(colour, env, s.env_mask * fr);
            }
            lit = (ambient + sun + point + spec * s.spec_mul) * colour;
        }
    } else if (debug != 0u) {
        return vec3<f32>(0.0);
    }
    return scatter(apply_fog(lit, fog), fog, in.world);
}

@fragment
fn fs_forward(inf: VsOutFog) -> @location(0) vec4<f32> {
    let in = unfog(inf);
    let s = surface(in);
    if (!s.keep) {
        discard;
    }
    return vec4<f32>(shade(in, s, inf.fog), s.alpha);
}
