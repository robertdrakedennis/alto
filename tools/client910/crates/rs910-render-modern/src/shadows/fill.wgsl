// The shadow maps' tile fills (shadows::cache): a full-viewport triangle
// at depth 1 (a tile's clear, with no fragment stage), or one that copies
// the static atlas's depth texel for texel into the same place of the frame
// atlas (the viewport is the tile in both).
@group(0) @binding(0) var static_map: texture_depth_2d;

@vertex
fn vs_fill(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 1.0, 1.0);
}

@fragment
fn fs_restore(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(static_map, vec2<i32>(p.xy), 0);
}
