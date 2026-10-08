//! Shader used by the faithful floor and model pipelines.

/// WGSL for the standard model shader variants and the waterfall program.
pub const FLOOR_SHADER: &str = r#"
struct FloorUniforms {
    wvp: mat4x4<f32>,              // world-view-projection
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    anti_sun_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    height_fog_plane: vec4<f32>,
    height_fog_colour: vec4<f32>,
    distance_fog_plane: vec4<f32>,
    distance_fog_colour: vec4<f32>,
    scene_origin: vec4<f32>,       // shared camera-relative origin, fine units
    shadow_wvp: mat4x4<f32>,       // CPU-composed model transform lowered by one unit (y -= 1)
    eye_time: vec4<f32>,           // relative eye, UV time in seconds
    scene_base: vec4<f32>,
    model_world: mat4x4<f32>,
    sun_rgb: vec4<f32>,            // sun colour used by the water shading
};
struct BatchUniforms {
    tex_scale: vec4<f32>,          // xy: UV scale; z: alpha ref (-1 disables); w: reflective alpha
    origin: vec4<f32>,
    shader: vec4<f32>,             // program, exponent, UV speed divisor, light count
    scroll: vec4<f32>,
    water: vec4<f32>,              // waterfall parameters; environment-mapped water: x = time fraction
    light_pos: array<vec4<f32>,4>,
    light_colour: array<vec4<f32>,4>,
    height_fog_plane: vec4<f32>,   // per batch (underwater fog)
    height_fog_colour: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(0) @binding(1) var environment_tex: texture_2d_array<f32>;
@group(0) @binding(2) var environment_sampler: sampler;
@group(0) @binding(3) var billow_tex: texture_3d<f32>;
@group(0) @binding(4) var billow_sampler: sampler;
@group(0) @binding(5) var water_normal_tex: texture_3d<f32>;   // water normal/height volume
@group(0) @binding(6) var water_normal_sampler: sampler;
@group(1) @binding(0) var diffuse_sampler_tex: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> b: BatchUniforms;

struct VsIn {
    @location(0) pos: vec3<f32>,     // position (stream 0)
    @location(1) uv: vec2<f32>,      // texture coordinates (stream 0)
    @location(2) depth: f32,         // water depth (stream 0; unused in lit mode 0)
    @location(3) normal: vec3<f32>,  // normal (stream 0)
    @location(4) colour: vec4<f32>,  // vertex colour (stream 1, batch colours)
};
struct VsOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,      // vertex colour
    @location(1) uv: vec2<f32>,          // texture coordinates
    @location(2) lighting: vec3<f32>,    // vertex lighting
    @location(3) fog: vec2<f32>,         // height and distance fog factors
    @location(4) @interpolate(flat) alpha_state: vec2<f32>,
    @location(5) @interpolate(flat) shader_state: vec2<f32>,
    @location(6) view_vector: vec3<f32>,
    @location(7) normal: vec3<f32>,
    @location(8) water_uv: vec3<f32>,
    @location(9) water_depth: f32,                         // depth stream value
    @location(10) @interpolate(flat) height_fog_colour: vec3<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    // Positions are scene-local (floors) or model-local (scene models); the
    // viewer adds `origin` so every mesh lands in the same absolute
    // fine-unit space the camera and fog planes use.
    let vertex = vec4<f32>(v.pos, 1.0) + vec4<f32>(b.origin.xyz, 0.0);
    // Camera-relative position and clip position.
    let local_vertex = vec4<f32>(v.pos + (b.origin.xyz - u.scene_origin.xyz), 1.0);
    out.clip = u.wvp * local_vertex;
    // Texture coordinates through the texture matrix (scale only), plus
    // the scroll offset.
    let offset = u.eye_time.w * b.scroll.xy / b.shader.z;
    out.uv = v.uv * b.tex_scale.xy + (offset - trunc(offset));
    let mode = i32(b.shader.x);
    out.shader_state = b.shader.xy;
    out.view_vector = vec3<f32>(0.0);
    out.normal = v.normal;
    // The view vector is needed by the specular, environment and water modes.
    if (mode == 1 || mode == 2 || mode == 7 || mode == 8 || mode == 10) { out.view_vector = normalize(u.eye_time.xyz - local_vertex.xyz); }
    let scene_vertex = (u.model_world * vec4<f32>(local_vertex.xyz + (u.scene_origin.xyz - u.scene_base.xyz), 1.0)).xyz;
    out.water_uv = vec3<f32>(select(scene_vertex.z, scene_vertex.x, b.water.x > 0.0) * abs(b.water.x), scene_vertex.y * b.water.y + b.water.z, b.water.w);
    // Environment-mapped water: the volume coordinate is (uv * texture
    // matrix, time fraction) with the texture matrix a scale of 2.4414062E-4.
    if (mode == 7 || mode == 8) { out.water_uv = vec3<f32>(v.uv * 2.4414062e-4, b.water.x); }
    out.water_depth = v.depth;
    out.height_fog_colour = b.height_fog_colour.xyz;
    // Lighting = ambient + sat(N.L) * sun + sat(-N.L) * anti-sun
    let ndotl = dot(v.normal, u.sun_dir.xyz);
    let lit = clamp(ndotl, 0.0, 1.0);
    let anti = clamp(-ndotl, 0.0, 1.0);
    out.lighting = u.ambient_colour.xyz + lit * u.sun_colour.xyz + anti * u.anti_sun_colour.xyz;
    if (mode == 3 || mode == 6) { out.lighting = vec3<f32>(1.0); }
    for (var i=0; i<i32(b.shader.w); i=i+1) {
        let vector = b.light_pos[i].xyz - v.pos;
        let distance = length(vector);
        let attenuation = 1.0 / (b.light_pos[i].w * distance * distance);
        out.lighting += b.light_colour[i].xyz * attenuation * max(0.0, dot(normalize(vector), v.normal));
    }
    // Fog factors: sat(dot(position, height fog plane)), sat(dot(position, distance fog plane))
    out.fog = vec2<f32>(
        clamp(dot(vertex, b.height_fog_plane), 0.0, 1.0),
        clamp(dot(vertex, u.distance_fog_plane), 0.0, 1.0),
    );
    // Underwater-ground modes: height fog = sat(dot((0, depth, 0, 0), height fog plane)).
    // Floors without a depth stream read the default (0,0,0,1); depth 0 gives no
    // height fog (the 0 * inf NaN of a scale-0 batch is taken as 0,
    // TODO(#gap-underwater-fog-nan)).
    if (mode == 9 || mode == 10) {
        out.fog.x = select(0.0, clamp(v.depth * b.height_fog_plane.y, 0.0, 1.0), v.depth != 0.0);
    }
    out.colour = v.colour;
    out.alpha_state = b.tex_scale.zw;
    return out;
}

// The client keeps repeat wrapping and does not enable seamless cubemaps.
// Isolate faces in a 2D array with the same per-face wrapping:
// Metal's native cube sampling otherwise blends across their borders.
fn cube_uv(v: vec3<f32>, face: i32) -> vec2<f32> {
    var st = vec2<f32>(0.0);
    var major = 1.0;
    switch face {
        case 0: { st=vec2<f32>(-v.z,-v.y);major=v.x; }
        case 1: { st=vec2<f32>(v.z,-v.y);major=-v.x; }
        case 2: { st=vec2<f32>(v.x,v.z);major=v.y; }
        case 3: { st=vec2<f32>(v.x,-v.z);major=-v.y; }
        case 4: { st=vec2<f32>(v.x,-v.y);major=v.z; }
        default: { st=vec2<f32>(-v.x,-v.y);major=-v.z; }
    }
    return st / major * 0.5 + vec2<f32>(0.5);
}
fn sample_environment(v: vec3<f32>) -> vec3<f32> {
    let a=abs(v);
    var face=select(5,4,v.z>=0.0);
    if (a.x>=a.y && a.x>=a.z) { face=select(1,0,v.x>=0.0); }
    else if (a.y>=a.z) { face=select(3,2,v.y>=0.0); }
    let uv=cube_uv(v,face);
    let dx=cube_uv(v+dpdx(v),face)-uv;
    let dy=cube_uv(v+dpdy(v),face)-uv;
    return textureSampleGrad(environment_tex,environment_sampler,uv,face,dx,dy).xyz;
}

// Environment-mapped water fragment shading (`waves` = the sea variant);
// the multiply/add fog terms are rebuilt from the fog factors.
fn shade_water(in: VsOut, waves: bool) -> vec4<f32> {
    let n4 = textureSample(water_normal_tex, water_normal_sampler, in.water_uv);
    let normal = 2.0 * n4.xyz - vec3<f32>(1.0);
    let eye = normalize(in.view_vector);
    let reflected = eye - (2.0 * normal) * dot(normal, eye);
    let env = sample_environment(reflected);
    // Sun direction (negated), specular exponent 32; sun colour with wave
    // exponent 5; wave intensity 0.1, wave base 0.4, break depth 256, break
    // offset 0.3.
    let sun = -u.sun_dir.xyz;
    let specular = pow(clamp(dot(sun, reflected), 0.0, 1.0), 32.0);
    let depth = in.water_depth;
    let shore = clamp(depth / 256.0 - 0.3 * n4.w, 0.0, 1.0);
    var wave = pow(1.0001 - shore, 5.0) - 0.5;
    wave = -4.0 * wave * wave + 1.0;
    let fresnel = 1.0 - abs(dot(reflected, normal));
    let diffuse_intensity = dot(sun, normal) * 2.0 * (1.0 - fresnel);
    let shallow = clamp(depth / 40.0, 0.0, 1.0);
    let main_alpha = fresnel * shore * shallow * shallow * (3.0 - 2.0 * shallow);
    var surface: vec4<f32>;
    if (waves) {
        let main = vec4<f32>(env, main_alpha);
        let crest = vec4<f32>(0.1 * n4.w + 0.4);
        let spec = specular * u.sun_rgb.xyz * shore;
        surface = main + wave * (crest - main) + vec4<f32>(spec, 0.0);
    } else {
        let water = vec4<f32>(u.distance_fog_colour.xyz, 1.0);
        let lit = env + (diffuse_intensity + specular) * u.sun_rgb.xyz;
        let tinted = water * vec4<f32>(lit, 1.0);
        surface = water + fresnel * (tinted - water);
    }
    let h = in.fog.x;
    let d = in.fog.y;
    let mul = vec4<f32>(vec3<f32>((1.0 - h) * (1.0 - d)), 1.0);
    let add = vec4<f32>(h * in.height_fog_colour * (1.0 - d) + u.distance_fog_colour.xyz * d, 0.0);
    return surface * mul + add;
}

fn shade(in: VsOut) -> vec4<f32> {
    let mode = i32(in.shader_state.x);
    if (mode == 7 || mode == 8) { return shade_water(in, mode == 8); }
    // Diffuse colour from the material texture.
    var diffuse = textureSample(diffuse_sampler_tex, diffuse_sampler, in.uv);
    var specular = vec3<f32>(0.0);
    let environment_factor = diffuse.www;
    if (mode == 1 || mode == 10) {
        if (length(in.normal) > 0.0) {
            let reflected = u.sun_dir.xyz - (2.0 * in.normal) * dot(in.normal, u.sun_dir.xyz);
            specular = pow(clamp(dot(reflected, -in.view_vector), 0.0, 1.0), in.shader_state.y) * diffuse.w * u.sun_colour.xyz;
        }
    }
    if (mode == 1 || mode == 2 || mode == 6 || mode == 10 || in.alpha_state.y != 0.0) { diffuse.w = 1.0; }
    if (mode == 5) {
        let noise = textureSample(billow_tex, billow_sampler, in.water_uv);
        return vec4<f32>(noise.rrr, noise.g) * in.colour;
    }
    // Times the vertex colour.
    diffuse = diffuse * in.colour;
    // Times the vertex lighting.
    diffuse = vec4<f32>(diffuse.xyz * in.lighting, diffuse.w);
    if (mode == 1 || mode == 10) { diffuse = vec4<f32>(diffuse.xyz + specular, diffuse.w); }
    if (mode == 2) {
        let reflected = in.view_vector - (2.0 * in.normal) * dot(in.normal, in.view_vector);
        // Negate local X/Z before the world rotation.
        let direction = reflected * vec3<f32>(-1.0,1.0,-1.0);
        let world_direction = direction.x*u.model_world[0].xyz + direction.y*u.model_world[1].xyz + direction.z*u.model_world[2].xyz;
        let env = sample_environment(world_direction);
        diffuse = vec4<f32>(diffuse.xyz + environment_factor * (env - diffuse.xyz), diffuse.w);
    }
    // height fog then distance fog, alpha untouched
    let hf = vec4<f32>(in.height_fog_colour, diffuse.w);
    diffuse = diffuse + in.fog.x * (hf - diffuse);
    let df = vec4<f32>(u.distance_fog_colour.xyz, diffuse.w);
    diffuse = diffuse + in.fog.y * (df - diffuse);
    return diffuse;
}

// Keep discard out of the ordinary entry point: a uniform false branch
// still permits fragment rejection and can inhibit early depth testing.
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let diffuse = shade(in);
    if (diffuse.w <= 0.0) { discard; }
    return diffuse;
}

@fragment
fn fs_cutout(in: VsOut) -> @location(0) vec4<f32> {
    let diffuse = shade(in);
    // Strict GREATER, after texture * vertex alpha.
    if (diffuse.w <= in.alpha_state.x) { discard; }
    return diffuse;
}

// Diagnostic A/B only: reproduce the old shared discard branch with the
// same colour/alpha result. Selected once at startup, never per frame.
@fragment
fn fs_shared_discard(in: VsOut) -> @location(0) vec4<f32> {
    let diffuse = shade(in);
    if (in.alpha_state.x >= 0.0 && diffuse.w <= in.alpha_state.x) { discard; }
    return diffuse;
}
"#;
