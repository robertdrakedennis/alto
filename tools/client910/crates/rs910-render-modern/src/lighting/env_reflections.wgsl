
// The environment record's global cube, the
// cube faded from and to, x the fade, y the record's environment mapping parameter,
// z 1 = a cube bound.
struct GlobalEnv {
    params: vec4<f32>,
};
@group(3) @binding(10) var global_env_a: texture_cube<f32>;
@group(3) @binding(11) var global_env_b: texture_cube<f32>;
@group(3) @binding(12) var<uniform> global_env: GlobalEnv;
