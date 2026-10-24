
struct Frame {
    view_proj: mat4x4<f32>,     // camera-local classic units -> clip (wgpu depth)
    view: mat4x4<f32>,          // camera-local -> classic view space (z forward)
    eye: vec4<f32>,             // camera-local eye; w: time in seconds
    sun_dir: vec4<f32>,         // towards the sun
    sun_colour: vec4<f32>,      // the sun colour (linear)
    sky_ambient: vec4<f32>,     // ambient colour, upper hemisphere
    ground_ambient: vec4<f32>,  // ambient colour, lower hemisphere
    fog_colour: vec4<f32>,      // fog colour in HDR; w: fog on
    fog_range: vec4<f32>,       // fog parameters: x start, y 1 / (end - start)
    params: vec4<f32>,          // x: exposure, y: 1 = SSAO, z: 1 = the post chain (M8)
};
@group(0) @binding(0) var<uniform> frame: Frame;
