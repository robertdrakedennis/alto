
struct Pass {
    inv_view_proj: mat4x4<f32>,
    rect: vec4<f32>,
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    p4: vec4<f32>,
    c: array<vec4<f32>, 4>,
    pad: vec4<f32>,
};
@group(1) @binding(0) var<uniform> pass_block: Pass;
@group(1) @binding(1) var tex0: texture_2d<f32>;
@group(1) @binding(2) var tex1: texture_2d<f32>;
@group(1) @binding(3) var tex2: texture_2d<f32>;
@group(1) @binding(4) var tex3: texture_2d<f32>;


// The camera-local point at pixel `px` and wgpu depth `z`.
fn pass_unproject(px: vec2<f32>, z: f32) -> vec3<f32> {
    let ndc = vec2<f32>((px.x - pass_block.rect.x) / pass_block.rect.z * 2.0 - 1.0,
                        1.0 - (px.y - pass_block.rect.y) / pass_block.rect.w * 2.0);
    let p = pass_block.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return p.xyz / p.w;
}

struct FullOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: FullOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
