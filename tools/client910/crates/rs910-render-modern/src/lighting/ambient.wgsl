// ---- The per-square ambient block (lighting::ambient) ----

// frame::gpu::ambient: the table's cells, the block of each map square of a
// grid around the camera (eight vec4 each, seven used), row-major by z.
@group(3) @binding(13) var<storage, read> square_sh: array<vec4<f32>>;

const SQUARE_STRIDE: u32 = 8u;
const SQUARE_SIZE_INV: f32 = 1.0 / 32768.0;

// The light of the map square under `world` (camera-local): its block
// evaluated for the normal. The block's frame is y up, the renderer's y is
// down. A position outside the grid takes the edge cell.
fn square_irradiance(world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let side = i32(probes.ambient_dims.x);
    let at = (world.xz + probes.origin.xz + probes.ambient.yz) * SQUARE_SIZE_INV;
    let cell = clamp(vec2<i32>(floor(at)), vec2<i32>(0), vec2<i32>(side - 1));
    let base = u32(cell.y * side + cell.x) * SQUARE_STRIDE;
    var c: array<vec4<f32>, 7>;
    for (var k = 0u; k < 7u; k++) {
        c[k] = square_sh[base + k];
    }
    return eval_sh(c, vec3<f32>(n.x, -n.y, n.z));
}
