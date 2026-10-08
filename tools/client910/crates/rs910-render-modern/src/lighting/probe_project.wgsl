
// ---- Probe projection (lighting::probes) ----

struct Project {
    // x: probes, y: face size, z: 1 = the texels are HDR (tonemap and encode
    // them to display values as an RGBA8 face holds them), w: exposure.
    params: vec4<f32>,
    // x: the slots 0..x the global probe's row also fills (a capture's
    // first frame).
    fill: vec4<f32>,
};
@group(0) @binding(0) var capture_atlas: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out_sh: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> project: Project;
// Each atlas row's SH slot.
@group(0) @binding(3) var<storage, read> targets: array<u32>;

var<workgroup> partial: array<array<vec4<f32>, 7>, 64>;
var<workgroup> partial_w: array<f32, 64>;

// The SH projection for probe `workgroup_id.x`: each thread sums a fixed
// stride of the probe's texels (its row of the atlas: six faces side by
// side), then a fixed tree reduction (deterministic), the 4π / Σw scale
// and the packing into the shader's irradiance constants.
@compute @workgroup_size(64)
fn cs_project(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let probe = wg.x;
    let res = u32(project.params.y);
    let count = 6u * res * res;
    var acc: array<vec4<f32>, 7>;   // L0..L8 per channel packed as rgb, then w
    var sums: array<vec3<f32>, 9>;
    var wsum = 0.0;
    for (var k = li; k < count; k += 64u) {
        let face = k / (res * res);
        let t = k % (res * res);
        let i = t % res;
        let j = t / res;
        let sc = f32(2u * i + 1u) / f32(res) - 1.0;
        let tc = f32(2u * j + 1u) / f32(res) - 1.0;
        let r = sc * sc + tc * tc + 1.0;
        let w = 4.0 / (r * sqrt(r));
        let d = normalize(face_dir(face, sc, tc));
        var c = textureLoad(capture_atlas, vec2<i32>(i32(face * res + i), i32(probe * res + j)), 0).rgb;
        if (project.params.z > 0.5) {
            // The display value as an RGBA8 face holds it: the byte
            // (round(255 c)) over 256.
            c = clamp(linear_to_display(tonemap(c, project.params.w)), vec3<f32>(0.0), vec3<f32>(1.0));
            c = round(c * 255.0) / 256.0;
        }
        let cw = c * w;
        sums[0] += cw * 0.282095;
        sums[1] += cw * (-0.488603 * d.y);
        sums[2] += cw * (0.488603 * d.z);
        sums[3] += cw * (-0.488603 * d.x);
        sums[4] += cw * (1.092548 * d.x * d.y);
        sums[5] += cw * (-1.092548 * d.y * d.z);
        sums[6] += cw * (0.946175 * d.z * d.z - 0.315392);
        sums[7] += cw * (-1.092548 * d.x * d.z);
        sums[8] += cw * (0.546274 * (d.x * d.x - d.y * d.y));
        wsum += w;
    }
    // Nine rgb sums into seven vec4 slots (27 floats).
    for (var q = 0u; q < 7u; q++) {
        var v = vec4<f32>(0.0);
        for (var e = 0u; e < 4u; e++) {
            let f = q * 4u + e;
            if (f < 27u) {
                v[e] = sums[f / 3u][f % 3u];
            }
        }
        acc[q] = v;
    }
    partial[li] = acc;
    partial_w[li] = wsum;
    workgroupBarrier();
    for (var s = 32u; s > 0u; s = s / 2u) {
        if (li < s) {
            for (var q = 0u; q < 7u; q++) {
                partial[li][q] = partial[li][q] + partial[li + s][q];
            }
            partial_w[li] = partial_w[li] + partial_w[li + s];
        }
        workgroupBarrier();
    }
    if (li == 0u) {
        let scale = 4.0 * 3.14159265 / partial_w[0];
        var l: array<vec3<f32>, 9>;
        for (var f = 0u; f < 27u; f++) {
            l[f / 3u][f % 3u] = partial[0][f / 4u][f % 4u] * scale;
        }
        // The packing constants C0..C4 (lighting::probes::C0..C4).
        let c0 = 0.2820765;
        let c1 = 0.3257139;
        let c2 = 0.27311942;
        let c3 = 0.07884278;
        let c4 = 0.13655971;
        var packed: array<vec4<f32>, 8>;
        for (var ch = 0u; ch < 3u; ch++) {
            packed[ch] = vec4<f32>(-c1 * l[3][ch], -c1 * l[1][ch], c1 * l[2][ch], c0 * l[0][ch] - c3 * l[6][ch]);
            packed[3u + ch] = vec4<f32>(c2 * l[4][ch], -c2 * l[5][ch], 3.0 * c3 * l[6][ch], -c2 * l[7][ch]);
        }
        packed[6] = vec4<f32>(c4 * l[8], 1.0);
        packed[7] = vec4<f32>(0.0);
        let slot = targets[probe];
        for (var k = 0u; k < 8u; k++) {
            out_sh[slot * 8u + k] = packed[k];
        }
        // The first frame: every probe starts as the global one.
        if (probe + 1u == u32(project.params.x)) {
            for (var s = 0u; s < u32(project.fill.x); s++) {
                for (var k = 0u; k < 8u; k++) {
                    out_sh[s * 8u + k] = packed[k];
                }
            }
        }
    }
}
