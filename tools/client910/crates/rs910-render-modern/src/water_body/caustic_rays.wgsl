
// ---- The caustic rays (water_body::caustics) ----
struct Caustics {
    map: vec4<f32>,     // xy: the map corner (camera-local x, z); z: 1 / texel; w: texels per side
    look: vec4<f32>,    // x: strength (the fade's y); y: the refraction scale; zw: the fade's z and w (the depth ramp)
    extra: vec4<f32>,   // x: the fade's x (the depth numerator); y: the fixed-point scale; z: the plane height; w: on
    proj: vec4<f32>,    // xy: the ray raster's centre; z: 1 / its half side; w: rays per texel
};
@group(2) @binding(19) var<storage, read_write> caustic_rays: array<atomic<u32>>;
@group(2) @binding(20) var<uniform> caustics: Caustics;

// The caustics view: the water seen from straight above, the map's square.
@vertex
fn vs_caustics(v: VsIn, @location(12) attrs: vec4<f32>, @location(13) slots: vec4<f32>, @location(14) bed: vec4<f32>) -> WaterOut {
    var out = water_vertex(v, attrs, slots, bed);
    let c = (out.world.xz - caustics.proj.xy) * caustics.proj.z;
    out.clip = vec4<f32>(c.x, c.y, 0.5, 1.0);
    return out;
}

// The map texel under camera-local x, z (-1: outside).
fn caustic_texel(p: vec2<f32>) -> i32 {
    let t = floor((p - caustics.map.xy) * caustics.map.z);
    let n = caustics.map.w;
    if (t.x < 0.0 || t.y < 0.0 || t.x >= n || t.y >= n) {
        return -1;
    }
    return i32(t.y * n + t.x);
}

// One ray's weight: min(fade.x / depth * fade.y, 7 fade.y) *
// smoothstep(fade.z, fade.w, depth).
fn caustic_weight(depth: f32) -> f32 {
    let per_ray = min(caustics.extra.x / max(depth, 1e-3), 7.0) * caustics.look.x;
    return per_ray * smoothstep(caustics.look.z, caustics.look.w, depth);
}

// The ray pass: one ray per fragment, into the texel it lands in and (the
// even light it would have been) the texel under the fragment.
@fragment
fn fs_caustics(win: WaterOut) -> @location(0) vec4<f32> {
    let lods = vec4<f32>(
        map_lod(normal_map_0, win.uv0.xy),
        map_lod(normal_map_1, win.uv0.zw),
        map_lod(normal_map_0, win.macro_uv.xy),
        map_lod(normal_map_1, win.macro_uv.zw),
    );
    let depth = min(win.water.x, win.water.w * water.shore.x);
    let w = caustic_weight(depth) * caustics.extra.y;
    if (w >= 1.0) {
        let tn = water_normal(win, lods);
        let h = normalize(vec3<f32>(tn.x, -tn.z, tn.y));
        let landed = caustic_texel(win.world.xz + h.xz * depth * caustics.look.y);
        let origin = caustic_texel(win.world.xz);
        if (landed >= 0) {
            atomicAdd(&caustic_rays[2 * landed], u32(w));
        }
        if (origin >= 0) {
            atomicAdd(&caustic_rays[2 * origin + 1], u32(w));
        }
    }
    return vec4<f32>(0.0);
}
