
@group(2) @binding(12) var scene_depth: texture_depth_2d;
// The nearest and farthest scene depth over the pixel's samples.
fn scene_depth_range(px: vec2<i32>) -> vec2<f32> {
    let size = vec2<i32>(water.size.xy);
    let at = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    let z = textureLoad(scene_depth, at, 0);
    return vec2<f32>(z);
}
