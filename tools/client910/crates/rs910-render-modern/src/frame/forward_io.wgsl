// The ambient occlusion (M8, post): frame pixels.
@group(0) @binding(1) var ssao_map: texture_2d<f32>;
// Scene pixels per occlusion texel: full (1), half (2); off uses white.
fn ambient_occlusion(pixel: vec2<f32>) -> f32 {
    let extent = vec2<i32>(textureDimensions(ssao_map));
    let texel = vec2<i32>(floor(pixel / max(frame.params.y, 1.0)));
    return textureLoad(ssao_map, clamp(texel, vec2<i32>(0), extent - vec2<i32>(1)), 0).r;
}

@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var aux_tex: texture_2d<f32>;
@group(1) @binding(2) var material_sampler: sampler;
@group(1) @binding(3) var normal_tex: texture_2d<f32>;
@group(1) @binding(4) var compound_tex: texture_2d<f32>;

// models::material_arrays: the RT5 materials' diffuse and aux maps as two 2D
// arrays (128 texel layers) and their repeat-both sampler. A draw names its
// layer in the high bits of its flags (`p0.w`), and whether the layer is a 64
// texel map tiled twice over each way; layer 0 is a material with its own
// textures (group 1).
@group(4) @binding(0) var diffuse_layers: texture_2d_array<f32>;
@group(4) @binding(1) var aux_layers: texture_2d_array<f32>;
@group(4) @binding(2) var layer_sampler: sampler;

// models::materials::MaterialMeta. Each map's atlas transform: x scale, y
// offset (inner uv = wrap(uv) * x + y), z mip limit, w 1 = atlas lookup
// (2: a normal map with X in red).
struct MaterialMeta {
    diffuse: vec4<f32>,
    normal: vec4<f32>,
    compound: vec4<f32>,   // bound, not sampled: its channels are unknown
    params: vec4<f32>,     // x black cutout, y gamma in shader, z normal strength, w repeat bits
};
@group(1) @binding(5) var<uniform> material: MaterialMeta;

// shadows::ShadowUniforms (the sunlight shadow block).
struct Shadow {
    light_view: mat4x4<f32>,              // camera-local -> light axes
    tex_scale: array<vec4<f32>, 4>,       // per-cascade texture-matrix scale
    tex_offset: array<vec4<f32>, 4>,      // per-cascade texture-matrix offset (+ the bias in z)
    extents: array<vec4<f32>, 4>,         // per-cascade atlas extents
    splits: vec4<f32>,                    // the cascade frustum view depths
    params: vec4<f32>,                    // mapping parameters: x texel, y cascades, z strength, w filter
    fade: vec4<f32>,                      // sunlight fade: x start, y end, z 1/range, w smooth
    lookup: vec4<f32>,                    // x select by map, y the low-quality shadow depth
    spheres: array<vec4<f32>, 4>,        // cascade bounding spheres (light axes, radius squared)
    bias_select: vec4<f32>,                  // x the position-normal bias, y selection (0 split, 1 map, 2 sphere)
};
@group(2) @binding(0) var<uniform> shadow: Shadow;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

// shadows::CasterUniforms: one cascade of the caster pass.
struct Caster {
    light_view: mat4x4<f32>,
    clip_scale: vec4<f32>,
    clip_offset: vec4<f32>,
};
@group(2) @binding(3) var<uniform> caster: Caster;

// lighting::point_lights (M4): the frame's static point lights (camera-local), the
// per-tile index grid (tile (level * nz + z) * nx + x: up to four ids plus
// one as u16 pairs, 0 none) and its origin and size.
struct PointLight {
    pos_radius: vec4<f32>,   // camera-local position, radius
    colour: vec4<f32>,       // clamp(colour * intensity, 0, 1) * strength
};
struct PointGrid {
    origin: vec4<f32>,       // scene-local camera origin
    dims: vec4<u32>,         // x tiles, z tiles, levels, light count
};
@group(3) @binding(0) var<storage, read> point_lights: array<PointLight>;
@group(3) @binding(1) var<storage, read> light_grid: array<vec2<u32>>;
@group(3) @binding(2) var<uniform> point_grid: PointGrid;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(10) tangent: vec4<f32>,  // xyz along +U, w: the bitangent's side
    @location(3) colour: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
    @location(8) p0: vec4<f32>,  // uv scale xy, alpha ref z, flags w
    @location(9) p1: vec4<f32>,  // uv scroll xy, specular power z, strength w
    @location(11) p2: vec4<f32>, // point lights: a model's four slots, a floor's level (flag 32)
};

struct VsOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) albedo: vec4<f32>,
    @location(4) @interpolate(flat) p0: vec4<f32>,
    @location(5) @interpolate(flat) p1: vec4<f32>,
    @location(6) view_depth: f32,
    @location(7) tangent: vec4<f32>,
    @location(8) @interpolate(flat) p2: vec4<f32>,
};
