// Water surface data shared with the caustic ray pass. Positions use the
// engine's camera-local, downward-Y coordinates; flow stays in world XZ.
struct WaterParameters {
    inv_view_proj: mat4x4<f32>,
    plane: vec4<f32>,
    time: vec4<f32>, // seconds, flow speed, ripple strength, flow period
    size: vec4<f32>,
    rect: vec4<f32>,
    origin: vec4<f32>,
    scales: vec4<f32>,
    weights: vec4<f32>, // two detail amplitudes and long-wave amplitude
    brdf: vec4<f32>, // reflection limit, fog offset, highlight power, intensity
    reflect: vec4<f32>, // planar share, intensity, distortion, Fresnel floor
    body: vec4<f32>,
    extinction: vec4<f32>,
    sky: vec4<f32>,
    shore: vec4<f32>,
    fx: vec4<f32>,
    fx2: vec4<f32>,
};
@group(2) @binding(10) var<uniform> water: WaterParameters;
@group(2) @binding(11) var refraction_tex: texture_2d<f32>;
@group(2) @binding(13) var reflection_tex: texture_2d<f32>;
@group(2) @binding(14) var normal_map_0: texture_2d<f32>;
@group(2) @binding(15) var normal_map_1: texture_2d<f32>;
@group(2) @binding(17) var water_sampler: sampler;
@group(2) @binding(18) var clamp_sampler: sampler;

const WATER_EPSILON: f32 = 0.0001;
const WATER_TAU: f32 = 6.283185307;
const NORMAL_MIN_Z: f32 = 0.35;
const NORMAL_MAX_SLOPE: f32 = 2.0;
const NORMAL_SECOND_ROTATION: mat2x2<f32> = mat2x2<f32>(vec2<f32>(0.8, 0.6), vec2<f32>(-0.6, 0.8));
const FLOW_TIME_SCALE: f32 = 0.5;
const DETAIL_FREQUENCY: f32 = 4.0;
const LONG_WAVE_SCALE: f32 = 0.18;
const EDGE_OPACITY_PER_UNIT: f32 = 0.004;
const REFRACTION_OFFSET_LIMIT: f32 = 6.0;

struct WaterOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) bed: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) albedo: vec4<f32>,
    @location(4) @interpolate(flat) p0: vec4<f32>,
    @location(5) @interpolate(flat) p1: vec4<f32>,
    @location(6) view_depth: f32,
    @location(7) @interpolate(flat) p2: vec4<f32>,
    @location(8) water: vec4<f32>, // depth, interpolated flow, shore distance
    @location(9) uv0: vec4<f32>, // world-aligned UVs of the two normal maps
};

@vertex
fn vs_water(v: VsIn, @location(12) attrs: vec4<f32>, @location(13) slots: vec4<f32>, @location(14) bed: vec4<f32>) -> WaterOut {
    return water_vertex(v, attrs, slots, bed);
}

fn water_vertex(v: VsIn, attrs: vec4<f32>, slots: vec4<f32>, bed: vec4<f32>) -> WaterOut {
    let base = vertex(v);
    var out: WaterOut;
    out.clip = base.clip;
    out.world = base.world;
    out.bed = select(vec3<f32>(-1.0), bed.rgb, bed.r >= 0.0);
    out.uv = base.uv;
    out.albedo = base.albedo;
    out.p0 = base.p0;
    out.p1 = base.p1;
    out.p2 = base.p2;
    out.view_depth = base.view_depth;
    let corner = u32(attrs.w);
    var flow = -attrs.yz;
    if (corner == 1u) { flow = -slots.xy; }
    if (corner == 2u) { flow = -slots.zw; }
    out.water = vec4<f32>(attrs.x, flow, bed.w);
    let position = base.world.xz + water.origin.xz;
    out.uv0 = vec4<f32>(position * water.scales.x * DETAIL_FREQUENCY, position * water.scales.y * DETAIL_FREQUENCY);
    return out;
}

fn as_forward(w: WaterOut) -> VsOut {
    var out: VsOut;
    out.clip = w.clip;
    out.world = w.world;
    out.normal = vec3<f32>(0.0, -1.0, 0.0);
    out.uv = w.uv;
    out.albedo = w.albedo;
    out.p0 = w.p0;
    out.p1 = w.p1;
    out.p2 = w.p2;
    out.view_depth = w.view_depth;
    out.tangent = vec4<f32>(0.0);
    return out;
}

fn unpack_normal(texel: vec4<f32>) -> vec3<f32> {
    return texel.xyz * 2.0 - 1.0;
}

fn map_lod(map: texture_2d<f32>, uv: vec2<f32>) -> f32 {
    let texels = uv * vec2<f32>(textureDimensions(map, 0));
    let dx = dpdx(texels);
    let dy = dpdy(texels);
    return clamp(0.5 * log2(max(max(dot(dx, dx), dot(dy, dy)), WATER_EPSILON)), 0.0, f32(textureNumLevels(map)) - 1.0);
}

fn normal_slope(map: texture_2d<f32>, uv: vec2<f32>, lod: f32) -> vec2<f32> {
    let normal = unpack_normal(textureSampleLevel(map, water_sampler, uv, lod));
    return clamp(normal.xy / max(normal.z, NORMAL_MIN_Z), vec2<f32>(-NORMAL_MAX_SLOPE), vec2<f32>(NORMAL_MAX_SLOPE));
}

// Two half-period phases exchange weight exactly when either UV resets.
// Interpolating world-space flow keeps neighboring triangles continuous;
// bounding advection prevents gradients from stretching as play time grows.
fn water_normal(surface: WaterOut, lods: vec4<f32>) -> vec3<f32> {
    let phase = fract(water.time.x / water.time.w);
    let other_phase = fract(phase + 0.5);
    let blend = abs(phase * 2.0 - 1.0);
    let travel = surface.water.yz * water.time.y * water.time.w * FLOW_TIME_SCALE;
    let drift_a = travel * phase;
    let drift_b = travel * other_phase;
    let first = mix(
        normal_slope(normal_map_0, surface.uv0.xy - drift_a, lods.x),
        normal_slope(normal_map_0, surface.uv0.xy - drift_b, lods.x), blend);
    let second = transpose(NORMAL_SECOND_ROTATION) * mix(
        normal_slope(normal_map_1, NORMAL_SECOND_ROTATION * (surface.uv0.zw - drift_a), lods.y),
        normal_slope(normal_map_1, NORMAL_SECOND_ROTATION * (surface.uv0.zw - drift_b), lods.y), blend);
    let position = surface.uv0.xy * LONG_WAVE_SCALE;
    let long_phase = water.time.x / (water.time.w * 4.0);
    let long_wave = vec2<f32>(
        sin(WATER_TAU * (position.x + position.y + long_phase)),
        sin(WATER_TAU * (position.y - position.x - long_phase * 2.0)));
    let strength = water.time.z + min(length(surface.water.yz), 1.0) * 0.25;
    let slope = (first * water.weights.x + second * water.weights.y) * strength
        + long_wave * water.weights.z;
    return normalize(vec3<f32>(slope, 1.0));
}

fn planar_sample(pixel: vec2<f32>) -> vec4<f32> {
    let uv = (pixel - water.rect.xy) / water.rect.zw;
    return textureSampleLevel(reflection_tex, clamp_sampler, uv, 0.0);
}

fn unproject(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec2<f32>((pixel.x - water.rect.x) / water.rect.z * 2.0 - 1.0,
        1.0 - (pixel.y - water.rect.y) / water.rect.w * 2.0);
    let position = water.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    return position.xyz / position.w;
}

// The surface gradient already contains all four filtered flow samples.
// Convert its screen derivatives back to XZ before evaluating light focus.
fn surface_divergence(position: vec2<f32>, slope: vec2<f32>) -> f32 {
    let px = dpdx(position);
    let py = dpdy(position);
    let nx = dpdx(slope);
    let ny = dpdy(slope);
    let determinant = px.x * py.y - px.y * py.x;
    if (abs(determinant) < WATER_EPSILON) { return 0.0; }
    return (nx.x * py.y - ny.x * px.y + ny.y * px.x - nx.y * py.x) / determinant;
}

@fragment
fn fs_reflect_position(input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(input.world, 1.0);
}

@fragment
fn fs_water(surface: WaterOut) -> @location(0) vec4<f32> {
    let input = as_forward(surface);
    let lods = vec4<f32>(map_lod(normal_map_0, surface.uv0.xy), map_lod(normal_map_1, surface.uv0.zw), 0.0, 0.0);
    var tangent_normal = water_normal(surface, lods);
    let divergence = surface_divergence(surface.world.xz, tangent_normal.xy / max(tangent_normal.z, NORMAL_MIN_Z));
    let debug = u32(water.plane.w);
    var depth = min(surface.water.x, surface.water.w * water.shore.x);
    if (water.shore.z > 0.5) {
        let bed_depth = scene_depth_range(vec2<i32>(surface.clip.xy));
        if (bed_depth.x >= surface.clip.z && bed_depth.y < 1.0) {
            depth = max(unproject(surface.clip.xy, bed_depth.x).y - surface.world.y, 0.0);
        }
    }
    depth = max(depth, 0.0);
    if (debug == 1u) { tangent_normal = vec3<f32>(0.0, 0.0, 1.0); }
    if (debug == 7u) { return vec4<f32>(tangent_normal * 0.5 + 0.5, 1.0); }
    if (debug == 8u) { return lods; }
    let normal = vec3<f32>(tangent_normal.x, -tangent_normal.z, tangent_normal.y);
    let eye_vector = surface.world - frame.eye.xyz;
    let view = eye_vector / max(length(eye_vector), WATER_EPSILON);
    let eye_cosine = clamp(dot(normal, -view), 0.0, 1.0);
    let grazing = 1.0 - eye_cosine;
    let grazing_squared = grazing * grazing;
    let fresnel = min(water.reflect.w + (1.0 - water.reflect.w) * grazing_squared * grazing_squared * grazing, water.brdf.x);
    let edge = clamp(depth * (EDGE_OPACITY_PER_UNIT + water.shore.y), 0.0, 1.0);
    let coverage = edge * edge * (3.0 - 2.0 * edge);
    if (debug == 1u) { return vec4<f32>(fresnel, 1.0, eye_cosine, 1.0); }
    if (debug == 6u) { return vec4<f32>(depth / 1024.0, coverage, fract(depth / 64.0), 1.0); }
    if (debug == 2u) { return vec4<f32>(fract(water.time.x / water.time.w), length(surface.water.yz), 0.0, 1.0); }
    // Water pigment is independent of the legacy floor texture and HDR aux.
    var albedo = surface.albedo.rgb;
    if (fx_on(FX_FOAM)) {
        albedo = fx_foam_albedo(albedo, depth, surface.world.xz + water.origin.xz, length(surface.water.yz));
    }
    let screen_normal = (frame.view * vec4<f32>(normal - vec3<f32>(0.0, -1.0, 0.0), 0.0)).xy * vec2<f32>(1.0, -1.0);
    let pixel_scale = water.rect.w / DISTORTION_REFERENCE_HEIGHT;
    let offset = screen_normal * min(depth, REFLECTION_OFFSET_LIMIT) * water.reflect.z * pixel_scale * coverage;
    let planar_on = water.plane.z > 0.5 && abs(surface.world.y - water.plane.x) <= water.plane.y;
    if (debug == 5u) { return select(vec4<f32>(0.0, 0.0, 0.0, 0.5), planar_sample(surface.clip.xy), planar_on); }
    var reflection = env_reflection(reflect(view, normal));
    if (planar_on) {
        // A small symmetric footprint softens reflected silhouette aliasing.
        let blur = vec2<f32>(REFLECTION_FILTER_RADIUS, REFLECTION_FILTER_RADIUS) * pixel_scale;
        let planar = (planar_sample(surface.clip.xy + offset - blur)
            + planar_sample(surface.clip.xy + offset + blur)) * 0.5;
        reflection = planar.rgb + reflection * (1.0 - planar.a);
    }
    if (debug == 3u) { return vec4<f32>(reflection, 1.0); }
    reflection *= mix(albedo, vec3<f32>(1.0), fresnel) * water.reflect.x * water.reflect.y;
    let sun_direction = frame.sun_dir.xyz;
    let shadow = sun_visibility_base(surface.world, surface.view_depth);
    let half_vector = normalize(sun_direction - view);
    let highlight = pow(max(dot(half_vector, normal), 0.0), water.brdf.z) * water.brdf.w * fresnel;
    reflection += frame.sun_colour.rgb * highlight * shadow;
    let sun = fx_sun_diffuse(normal, sun_direction, fresnel, select(1.0, shadow, fx_on(FX_SHADOW)));
    let light = probe_ambient(surface.world, vec3<f32>(0.0, -1.0, 0.0))
        + frame.sun_colour.rgb * sun + point_light_sum_base(input, vec3<f32>(0.0, -1.0, 0.0));
    var bed = albedo * water.body.rgb * water.extinction.w * light;
    if (surface.bed.r >= 0.0) {
        let gain = select(water.shore.w, water.fx.y, fx_on(FX_SHADOW));
        var bed_light = light;
        if (fx_on(FX_CAUSTICS)) { bed_light += frame.sun_colour.rgb * fx_caustics(divergence, depth) * shadow; }
        bed = surface.bed * gain * bed_light;
    }
    let path = min(depth / max(-view.y, 0.2), water.body.w);
    let nudge = screen_normal * min(sqrt(depth), REFRACTION_OFFSET_LIMIT) * pixel_scale * coverage;
    var at = vec2<i32>(surface.clip.xy + nudge);
    var behind = scene_depth_range(at);
    if (behind.x < surface.clip.z || behind.y >= 1.0) {
        at = vec2<i32>(surface.clip.xy);
        behind = scene_depth_range(at);
    }
    var body: vec3<f32>;
    let body_light = frame.sun_colour.rgb * sun;
    if (behind.x >= surface.clip.z && behind.y < 1.0) {
        let seen = textureLoad(refraction_tex, clamp(at, vec2<i32>(0), vec2<i32>(water.size.xy) - vec2<i32>(1)), 0).rgb;
        let far_point = unproject(vec2<f32>(at) + 0.5, behind.y);
        body = extinction(seen, min(length(far_point - surface.world), water.body.w), body_light, albedo, coverage);
    } else {
        body = extinction(bed, path, body_light, albedo, coverage);
    }
    if (debug == 4u) { return vec4<f32>(body, 1.0); }
    let fog = max(distance_fog(surface.world), 0.0);
    // Refraction already contains the scene atmosphere; only the surface
    // reflection travels from this surface to the eye.
    let surface_colour = scatter(apply_fog(reflection, fog), fog, surface.world);
    return vec4<f32>(mix(body, surface_colour, coverage * fresnel), 1.0);
}
