
struct Layer {
    colour: vec4<f32>,   // display RGBA of a fill; tint alpha of a material in a
    rect: vec4<f32>,     // viewport x, y, w, h (target pixels)
    tile: vec4<f32>,     // x offset, y offset, tile size, mode (0 fill, 1 tiled, 2 horizon, 3 one sprite)
    top: vec4<f32>,      // horizon fill above (display RGBA)
    bottom: vec4<f32>,   // horizon fill below (display RGBA)
    params: vec4<f32>,   // x: exposure, yz: viewport pixels per target pixel (0: 1; the render scale)
};
@group(0) @binding(0) var<uniform> layer: Layer;
@group(1) @binding(0) var sky_tex: texture_2d<f32>;
@group(1) @binding(1) var sky_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let x = f32((id << 1u) & 2u);
    let y = f32(id & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn hdr(display: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(inverse_tonemap(display_to_linear(display.rgb), layer.params.x), display.a);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let mode = i32(layer.tile.w);
    let p = (frag.xy - layer.rect.xy) * select(vec2<f32>(1.0), layer.params.yz, layer.params.yz > vec2<f32>(0.0));
    let size = layer.tile.z;
    let q = (p - layer.tile.xy) / size;
    // A decor sprite (mode 3) is one square at `tile.xy`; the tiled modes wrap.
    var tex = textureSampleLevel(sky_tex, sky_sampler, q - floor(q), 0.0);
    if (mode == 3) {
        if (q.x < 0.0 || q.x >= 1.0 || q.y < 0.0 || q.y >= 1.0) { discard; }
        tex = textureSampleLevel(sky_tex, sky_sampler, q, 0.0);
    }
    if (mode == 0) {
        return hdr(layer.colour);
    }
    if (mode == 2) {
        if (p.y < layer.tile.y + 1.0) {
            if (layer.top.a <= 0.0) { discard; }
            return hdr(layer.top);
        }
        if (p.y >= layer.tile.y + size) {
            if (layer.bottom.a <= 0.0) { discard; }
            return hdr(layer.bottom);
        }
    }
    let texel = vec4<f32>(tex.rgb, tex.a * layer.colour.a);
    if (texel.a <= 0.0) { discard; }
    return vec4<f32>(inverse_tonemap(texture_linear(texel.rgb), layer.params.x), texel.a);
}
