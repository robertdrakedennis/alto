
@group(2) @binding(12) var scene_depth: texture_depth_multisampled_2d;
// The nearest and farthest scene depth over the pixel's samples.
fn scene_depth_range(px: vec2<i32>) -> vec2<f32> {
    let size = vec2<i32>(water.size.xy);
    let at = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    var r = vec2<f32>(1.0, 0.0);
    for (var i = 0u; i < textureNumSamples(scene_depth); i++) {
        let z = textureLoad(scene_depth, at, i32(i));
        r = vec2<f32>(min(r.x, z), max(r.y, z));
    }
    return r;
}
