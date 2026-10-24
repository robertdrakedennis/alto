
// The water body's effects.
@group(2) @binding(16) var foam_map: texture_2d<f32>;

const FX_SHADOW: u32 = 1u;
const FX_FOAM: u32 = 2u;
const FX_CAUSTICS: u32 = 4u;
const FX_CAUSTICS_FALLOFF: f32 = 48.0;
const FOAM_PERIOD_SECONDS: f32 = 8.0;

fn fx_on(bit: u32) -> bool {
    return (u32(water.fx.x) & bit) != 0u;
}

// The sun's diffuse light without the albedo: the wrapped sun towards the
// normal `n` (classic axes: both n and the sun direction point up with -y),
// less what the fresnel reflects, times the attenuation `c` the specular
// takes.
fn fx_sun_diffuse(n: vec3<f32>, l: vec3<f32>, fresnel: f32, c: f32) -> f32 {
    return (dot(n, l) * 0.5 + 0.5) * (1.0 - fresnel) * c;
}

// This crate's gradient noise in [-1, 1] (a slow phase over the water).
fn fx_hash2(p: vec2<i32>) -> vec2<f32> {
    var h = bitcast<u32>(p.x) * 0x27d4eb2du ^ bitcast<u32>(p.y) * 0x165667b1u;
    h = (h ^ (h >> 15u)) * 0x85ebca6bu;
    h = h ^ (h >> 13u);
    let a = f32(h & 0xffffu) / 65535.0 * 6.2831853;
    return vec2<f32>(cos(a), sin(a));
}

fn fx_noise(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let g00 = dot(fx_hash2(i), f);
    let g10 = dot(fx_hash2(i + vec2<i32>(1, 0)), f - vec2<f32>(1.0, 0.0));
    let g01 = dot(fx_hash2(i + vec2<i32>(0, 1)), f - vec2<f32>(0.0, 1.0));
    let g11 = dot(fx_hash2(i + vec2<i32>(1, 1)), f - vec2<f32>(1.0, 1.0));
    return clamp(mix(mix(g00, g10, u.x), mix(g01, g11, u.x), u.y) * 1.4142, -1.0, 1.0);
}

// The albedo with foam: the albedo moved
// towards the foam map near the shore in slow water. `p`: scene-local XZ,
// `depth`: the shading depth, `flow`: the combined flow's length.
fn fx_foam_albedo(albedo: vec3<f32>, depth: f32, p: vec2<f32>, flow: f32) -> vec3<f32> {
    let foam_depth = water.fx.z;
    if (foam_depth <= 0.0) {
        return albedo;
    }
    let q = clamp(1.0 - min(depth / foam_depth, 1.0), 0.0, 1.0);
    if (q <= 0.0) {
        return albedo;
    }
    let v = clamp(fx_noise(p / 512.0) * 0.125, 0.0, 1.0);
    let t = water.time.x * WATER_TAU / FOAM_PERIOD_SECONDS + p.x * 0.0001 + p.y * 0.0001;
    let a = pow(max(0.0, cos(abs(depth) * 0.005 + t + v * 4.0)), 8.0);
    let b = pow(max(0.0, cos(abs(depth) * 0.005 - t + v * 8.0)), 4.0);
    let s = min(1.0, a + b * q) * max(0.2 - flow, 0.0);
    // The foam UV: the position at the first map's scale.
    let w = textureSampleLevel(foam_map, water_sampler, p * water.scales.x, 0.0) * water.fx.w;
    return mix(albedo, w.rgb * w.a, s * q);
}

// Light focus from the filtered surface gradient, in fine world units.
// This reuses the surface samples rather than fetching three more normals.
fn fx_caustics(divergence: f32, depth: f32) -> f32 {
    let jacobian = max(1.0 + depth * water.fx2.y * divergence, 0.2);
    let fade = smoothstep(water.fx2.z, water.fx2.w, depth);
    let per_ray = min(FX_CAUSTICS_FALLOFF / max(depth, 1e-3), 7.0) * water.fx2.x;
    return per_ray * fade * max(1.0 / jacobian - 1.0, 0.0);
}
