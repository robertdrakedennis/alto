
// ---- The caustics resolve (water_body::caustics) ----
struct Caustics {
    map: vec4<f32>,
    look: vec4<f32>,
    extra: vec4<f32>,
    proj: vec4<f32>,
};
@group(0) @binding(0) var<storage, read> rays: array<u32>;
@group(0) @binding(1) var<storage, read_write> light: array<f32>;
@group(0) @binding(2) var<uniform> caustics: Caustics;

// The bed term's kernel: the centre 5, the ring 1, over 12; over the excess of the landed rays over their origins.
@compute @workgroup_size(8, 8)
fn cs_resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = i32(caustics.map.w);
    let p = vec2<i32>(id.xy);
    if (p.x >= n || p.y >= n) {
        return;
    }
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = p + vec2<i32>(dx, dy);
            if (q.x < 0 || q.y < 0 || q.x >= n || q.y >= n) {
                continue;
            }
            let i = u32(q.y * n + q.x);
            let k = select(1.0, 5.0, dx == 0 && dy == 0);
            sum += k * (f32(rays[2u * i]) - f32(rays[2u * i + 1u]));
        }
    }
    light[u32(p.y * n + p.x)] = max(sum / 12.0, 0.0) / (caustics.extra.y * caustics.proj.w);
}
