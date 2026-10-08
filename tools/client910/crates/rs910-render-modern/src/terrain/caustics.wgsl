
// ---- Caustics on the bed (water_body::caustics) ----
struct Caustics {
    map: vec4<f32>,
    look: vec4<f32>,
    extra: vec4<f32>,
    proj: vec4<f32>,
};
@group(1) @binding(12) var<uniform> caustics: Caustics;
@group(1) @binding(13) var<storage, read> caustic_light: array<f32>;

fn caustic_light_at(i: vec2<i32>) -> f32 {
    let n = i32(caustics.map.w);
    let c = clamp(i, vec2<i32>(0), vec2<i32>(n - 1));
    return caustic_light[u32(c.y * n + c.x)];
}

// The caustic light at camera-local `world` (0 above the plane, outside the
// map or with no water): the map bilinear, faded towards its edges.
fn terrain_caustics(world: vec3<f32>) -> f32 {
    // Below the plane (y up): `world.y` at or under the caustic plane height.
    if (caustics.extra.w < 0.5 || world.y < caustics.extra.z) {
        return 0.0;
    }
    let t = (world.xz - caustics.map.xy) * caustics.map.z;
    let c = t / caustics.map.w - 0.5;
    let f = smoothstep(vec2<f32>(0.4), vec2<f32>(0.5), abs(c));
    let edge = max(0.0, 1.0 - (f.x + f.y));
    if (edge <= 0.0) {
        return 0.0;
    }
    let u = t - 0.5;
    let i = vec2<i32>(floor(u));
    let fr = u - floor(u);
    let a = mix(caustic_light_at(i), caustic_light_at(i + vec2<i32>(1, 0)), fr.x);
    let b = mix(caustic_light_at(i + vec2<i32>(0, 1)), caustic_light_at(i + vec2<i32>(1, 1)), fr.x);
    return mix(a, b, fr.y) * edge;
}
