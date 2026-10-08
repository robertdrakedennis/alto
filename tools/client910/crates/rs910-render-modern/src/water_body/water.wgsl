
// frame::water::WaterUniforms: the frame's water constants. Names in
// the comments say what each field stands for.
struct Water {
    inv_view_proj: mat4x4<f32>,  // the inverse view-projection (camera-local)
    plane: vec4<f32>,            // x: reflection plane y, y: tolerance, z: planar on, w: debug mode
    time: vec4<f32>,             // x: the water clock in seconds, y: the flow speed, z: the still-water normal strength, w: the flow noise scale
    size: vec4<f32>,             // xy: target size, zw: 1 / target size
    rect: vec4<f32>,             // the scene viewport in target pixels (offset and scale)
    origin: vec4<f32>,           // xyz: the camera-local origin in scene-local units
    scales: vec4<f32>,           // the normal maps' texture scales (xyz)
    weights: vec4<f32>,          // the detail maps' sample weights
    distort: vec4<f32>,          // the detail maps' UV distortions
    macro_weights: vec4<f32>,    // the macro maps' sample weights
    macro_distort: vec4<f32>,    // the macro maps' UV distortions
    brdf: vec4<f32>,             // the normal BRDF parameters
    reflect: vec4<f32>,          // x: the reflection map's contribution, y: the reflection strength, z: distortion scale
    body: vec4<f32>,             // xyz: the opaque water colour's tint (linear), w: the longest path
    extinction: vec4<f32>,       // xyz: the extinction depths per channel (fine units), w: the opaque colour's brightness
    sky: vec4<f32>,              // xyz: the sky colour (HDR), w: the zenith's share
    shore: vec4<f32>,            // x: the bank's slope (depth per unit from the shore), y: the soft edge's visibility share, w: the bed colour's brightness
    fx: vec4<f32>,               // x: effect bits, y: the bed gain under the modern sun, z: the foam depth, w: the foam scale
    fx2: vec4<f32>,              // x: the caustics' strength, y: the caustics' refraction scale, zw: the fade's z and w
};
@group(2) @binding(10) var<uniform> water: Water;
@group(2) @binding(11) var refraction_tex: texture_2d<f32>;

@group(2) @binding(13) var reflection_tex: texture_2d<f32>;
@group(2) @binding(14) var normal_map_0: texture_2d<f32>;
@group(2) @binding(15) var normal_map_1: texture_2d<f32>;
@group(2) @binding(17) var water_sampler: sampler;
@group(2) @binding(18) var clamp_sampler: sampler;

struct WaterOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) bed: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) albedo: vec4<f32>,
    @location(4) @interpolate(flat) p0: vec4<f32>,
    @location(5) @interpolate(flat) p1: vec4<f32>,
    @location(6) view_depth: f32,
    // The one-hot corner, interpolated into the three flow slots' weights.
    @location(7) mask: vec4<f32>,
    @location(8) @interpolate(flat) p2: vec4<f32>,
    // The depth, slot 0's noisy flow x, z; the distance to the shore.
    @location(9) water: vec4<f32>,
    // The detail UVs of normal maps 0 and 1 per flow slot and the macro UVs.
    @location(10) uv0: vec4<f32>,
    @location(11) macro_uv: vec4<f32>,
    @location(12) uv1: vec4<f32>,
    @location(13) uv2: vec4<f32>,
    // The noisy flows of slots 1 and 2.
    @location(14) flows: vec4<f32>,
};

// The UV in the flow's frame, advanced along
// the flow by its length times the speed and time.
fn flow_oriented_uv(v: vec2<f32>, d: vec2<f32>, speed: f32, t: f32) -> vec2<f32> {
    var u = vec2<f32>(0.0, 1.0);
    if (dot(d, d) >= 1e-6) {
        u = normalize(d);
    }
    let across = vec2<f32>(u.y, -u.x);
    let g = length(d * speed * t);
    return vec2<f32>(dot(across, v), dot(u, v) + g);
}

// The flow-aligned detail transform with identity
// rotations, unit scale and zero offset (the per-map parameters have no
// 910 source).
fn detail_uv(w: vec2<f32>, d: vec2<f32>, noise: vec2<f32>) -> vec2<f32> {
    let t = water.time.x * 0.5;
    let speed = water.time.y;
    return flow_oriented_uv(w, d, speed, t) + noise * speed * t;
}

// The detail UVs for one flow slot: maps 0
// and 1 with the noise signs (1, 1) and (-1, -1).
fn slot_uvs(w0: vec2<f32>, w1: vec2<f32>, d: vec2<f32>, noise: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(detail_uv(w0, d, noise), detail_uv(w1, d, -noise));
}

// The macro UVs (axis-aligned transform): a tenth of the UV, drifting by
// (0.1, -0.13) * 0.25 with the speed and time.
fn macro_uv(w: vec2<f32>) -> vec2<f32> {
    let t = water.time.x * 0.5;
    return w * 0.1 + vec2<f32>(0.1, -0.13) * 0.25 * water.time.y * t;
}

// The flow noise of a slot (the 910
// data has no source for it): a hash of the slot's patch flow in [-1, 1]^2, so
// it is constant over a patch like the flow. The noise moves the UVs by
// `noise * speed * t`, which grows with the clock: a noise that differs
// between a triangle's vertices (M7 hashed the vertex position) stretches
// the UVs by tens of repeats per tile and aliases into stripes.
fn flow_noise(d: vec2<f32>) -> vec2<f32> {
    let q = bitcast<vec2<u32>>(d);
    var h = q.x * 0x8da6b343u ^ q.y * 0xd8163841u;
    h = (h ^ (h >> 13u)) * 0x5bd1e995u;
    h = h ^ (h >> 15u);
    return vec2<f32>(f32(h & 0xffu), f32((h >> 8u) & 0xffu)) / 127.5 - 1.0;
}

// attrs: depth, slot 0 flow x, z, corner; slots: slot 1 and 2 flows
// (water_body::water_mesh).
@vertex
fn vs_water(v: VsIn, @location(12) attrs: vec4<f32>, @location(13) slots: vec4<f32>, @location(14) bed: vec4<f32>) -> WaterOut {
    return water_vertex(v, attrs, slots, bed);
}

// vs_water's body (lane Q-FIN: shared with the caustic rays' view,
// water_body::caustics).
fn water_vertex(v: VsIn, attrs: vec4<f32>, slots: vec4<f32>, bed: vec4<f32>) -> WaterOut {
    let o = vertex(v);
    var out: WaterOut;
    out.clip = o.clip;
    out.world = o.world;
    // The bed colour (linear; red below 0: none) in the normal's slot (the
    // water's normal comes from its maps).
    out.bed = select(vec3<f32>(-1.0), bed.rgb, bed.r >= 0.0);
    out.uv = o.uv;
    out.albedo = o.albedo;
    out.p0 = o.p0;
    out.p1 = o.p1;
    out.view_depth = o.view_depth;
    out.p2 = o.p2;
    let corner = u32(attrs.w);
    out.mask = vec4<f32>(
        select(0.0, 1.0, corner == 0u),
        select(0.0, 1.0, corner == 1u),
        select(0.0, 1.0, corner == 2u),
        0.0,
    );
    // The patch flows negated (the modern client stores them that way),
    // plus the noise; the maps' UVs from the world XZ.
    let p = o.world.xz + water.origin.xz;
    let d0 = -attrs.yz;
    let d1 = -slots.xy;
    let d2 = -slots.zw;
    let n0 = flow_noise(d0) * water.time.w;
    let n1 = flow_noise(d1) * water.time.w;
    let n2 = flow_noise(d2) * water.time.w;
    out.water = vec4<f32>(attrs.x, d0 + n0, bed.w);
    out.flows = vec4<f32>(d1 + n1, d2 + n2);
    let w0 = p * water.scales.x;
    let w1 = p * water.scales.y;
    out.uv0 = slot_uvs(w0, w1, d0, n0);
    out.uv1 = slot_uvs(w0, w1, d1, n1);
    out.uv2 = slot_uvs(w0, w1, d2, n2);
    out.macro_uv = vec4<f32>(macro_uv(w0), macro_uv(w1));
    return out;
}

fn as_forward(w: WaterOut) -> VsOut {
    var o: VsOut;
    o.clip = w.clip;
    o.world = w.world;
    o.normal = vec3<f32>(0.0, -1.0, 0.0);
    o.uv = w.uv;
    o.albedo = w.albedo;
    o.p0 = w.p0;
    o.p1 = w.p1;
    o.view_depth = w.view_depth;
    o.tangent = vec4<f32>(0.0);
    o.p2 = w.p2;
    return o;
}

// Normal unpacking (uncompressed): the 910 maps are RGB tangent normals whose
// X and Y average 0.5 (no bias; nxt-data-formats.md §7.5).
fn unpack_normal(t: vec4<f32>) -> vec3<f32> {
    return t.xyz * 2.0 - 1.0;
}

fn map_lod(t: texture_2d<f32>, uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(t, 0));
    let dx = dpdx(uv * size);
    let dy = dpdy(uv * size);
    return clamp(0.5 * log2(max(max(dot(dx, dx), dot(dy, dy)), 1e-12)), 0.0, f32(textureNumLevels(t)) - 1.0);
}

// One normal map's XY and its slope
// (XY / Z) weighted by how much the flow shows (still water keeps
// the still-water normal strength of it).
fn weighted_normal(weight: f32, t: texture_2d<f32>, uv: vec2<f32>, lod: f32, flow: vec2<f32>, e: f32) -> vec4<f32> {
    if (weight <= 0.0) {
        return vec4<f32>(0.0);
    }
    let n = unpack_normal(textureSampleLevel(t, water_sampler, uv, lod));
    let slope = n.xy / n.z;
    let g = clamp(length(flow) * 6.0 + water.time.z, 0.0, 1.0);
    return vec4<f32>(n.xy, slope * g) * weight * e;
}

// The weighted sum over the two 910 maps (the modern client has a
// third): each map's UV is pushed by the XY summed so far.
fn normal_slope_sum(w: vec4<f32>, dist: vec4<f32>, uv0: vec2<f32>, uv1_in: vec2<f32>, lod: vec2<f32>, flow: vec2<f32>, e: f32) -> vec2<f32> {
    var h = weighted_normal(w.x, normal_map_0, uv0, lod.x, flow, 1.0);
    let uv1 = uv1_in + h.xy * dist.y * e;
    h += weighted_normal(w.y, normal_map_1, uv1, lod.y, flow, 1.0);
    return h.zw;
}

// One flow slot's detail
// normal, its UVs pushed by the macro normal `m`.
fn detail_normal(uv: vec4<f32>, flow: vec2<f32>, m: vec3<f32>, lods: vec2<f32>) -> vec3<f32> {
    let uv0 = uv.xy + m.xy * 0.5 * water.distort.x;
    let uv1 = uv.zw + m.xy * 0.5 * water.distort.y;
    return normalize(vec3<f32>(normal_slope_sum(water.weights, water.distort, uv0, uv1, lods, flow, 0.8), 1.0));
}

// The water normal: the macro normal plus the detail normal, one
// slot where the three slot flows agree, else the slots weighted by the
// corner mask; tangent space, z up.
fn water_normal(win: WaterOut, lods: vec4<f32>) -> vec3<f32> {
    let macro_flow = vec2<f32>(0.1, -0.13) * 0.25;
    let m = normalize(vec3<f32>(normal_slope_sum(water.macro_weights, water.macro_distort, win.macro_uv.xy, win.macro_uv.zw, lods.zw, macro_flow, 0.1), 1.0));
    let f0 = win.water.yz;
    let f1 = win.flows.xy;
    let f2 = win.flows.zw;
    let a = f1 - f0;
    let b = f2 - f0;
    if (dot(a, a) <= 1e-7 && dot(b, b) <= 1e-7) {
        return normalize(detail_normal(win.uv0, f0, m, lods.xy) + m);
    }
    let u = (detail_normal(win.uv0, f0, m, lods.xy) + m) * win.mask.x
        + (detail_normal(win.uv1, f1, m, lods.xy) + m) * win.mask.y
        + (detail_normal(win.uv2, f2, m, lods.xy) + m) * win.mask.z;
    return normalize(u);
}

// The stand-in for the environment reflection: the sky the
// reflected ray `r` sees. The classic environment cubes are material maps (a
// lava cave at Draynor, a jungle at Lumbridge), not the sky of the place,
// so the sky here is a gradient from the frame's clear colour (the fog
// colour the classic sky is painted with; the modern angle-based fog takes the
// horizon to it) up to a deeper zenith (`water.sky.w`: its share).
fn sky_reflection(r: vec3<f32>) -> vec3<f32> {
    let up = clamp(-r.y, 0.0, 1.0);
    let zenith = water.sky.rgb * vec3<f32>(0.55, 0.75, 1.0);
    return mix(water.sky.rgb, zenith, sqrt(up) * water.sky.w);
}

fn planar_sample(frag: vec2<f32>) -> vec4<f32> {
    let uv = (frag - water.rect.xy) / water.rect.zw;
    return textureSampleLevel(reflection_tex, clamp_sampler, uv, 0.0);
}

// The camera-local point the depth `z` shows at framebuffer position `frag`.
fn unproject(frag: vec2<f32>, z: f32) -> vec3<f32> {
    let ndc = vec2<f32>(
        (frag.x - water.rect.x) / water.rect.z * 2.0 - 1.0,
        1.0 - (frag.y - water.rect.y) / water.rect.w * 2.0,
    );
    let p = water.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return p.xyz / p.w;
}

// The extinction's shape over the fine-unit path `path`
// through the water: the light that crosses it per channel (red is taken
// first), the modern per-channel depths as `water.extinction.xyz`.
fn transmittance(path: f32) -> vec3<f32> {
    return exp(-max(path, 0.0) / water.extinction.xyz);
}

// Debug output 5's reflection pass: the fragment's camera-local position.
@fragment
fn fs_reflect_position(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.world, 1.0);
}

@fragment
fn fs_water(win: WaterOut) -> @location(0) vec4<f32> {
    let in = as_forward(win);
    let s = surface_base(in);
    let lods = vec4<f32>(
        map_lod(normal_map_0, win.uv0.xy),
        map_lod(normal_map_1, win.uv0.zw),
        map_lod(normal_map_0, win.macro_uv.xy),
        map_lod(normal_map_1, win.macro_uv.zw),
    );
    let debug = u32(water.plane.w);
    // The depth the shading sees: the floor's, but no deeper than a bank
    // sloping down from the shoreline (the 910 depths do not reach 0 at
    // the shore; this port's addition, `water.shore.x` per fine unit).
    var depth = min(win.water.x, win.water.w * water.shore.x);
    if (water.shore.z > 0.5) {
        let bed_here = scene_depth_range(vec2<i32>(win.clip.xy));
        if (bed_here.x >= win.clip.z && bed_here.y < 1.0) {
            let bed_point = unproject(win.clip.xy, bed_here.x);
            depth = max(bed_point.y - in.world.y, 0.0);
        }
    }
    // The eye's view vector: v, d, f.
    let from_eye = in.world - frame.eye.xyz;
    let dist = length(from_eye);
    let f = from_eye / max(dist, 1e-4);
    var tn = water_normal(win, lods);
    if (debug == 1u) {
        tn = vec3<f32>(0.0, 0.0, 1.0);
    }
    // Tangent z up -> modern world (y up) -> classic (y down).
    let h = normalize(vec3<f32>(tn.x, -tn.z, tn.y));
    if (debug == 7u) {
        return vec4<f32>(tn * 0.5 + 0.5, 1.0);
    }
    if (debug == 8u) {
        return lods;
    }
    // Albedo: the vertex colour (without foam): the classic water
    // batch's colour and texture.
    var albedo = s.colour;
    if (fx_on(FX_FOAM)) {
        let flow = win.water.yz * win.mask.x + win.flows.xy * win.mask.y + win.flows.zw * win.mask.z;
        albedo = fx_foam_albedo(albedo, depth, in.world.xz + water.origin.xz, length(flow));
    }
    let offset = vec2<f32>(h.x, h.z) * min(depth, 32.0) * water.reflect.z;
    // The soft edge's alpha with extinction: 0.004 per unit
    // plus the visibility's share.
    let q = clamp(depth * (0.004 + water.shore.y), 0.0, 1.0);
    let cos_eye = max(0.0, dot(h, -f));
    let fresnel = clamp(0.28 + 0.72 * pow(1.0 - cos_eye, 5.0), 0.0, 0.6);
    let n_mix = clamp(fresnel + water.brdf.x, 0.0, 0.6);
    if (debug == 1u) {
        return vec4<f32>(fresnel, 1.0, cos_eye, 1.0);
    }
    if (debug == 6u) {
        return vec4<f32>(depth / 1024.0, q, fract(depth / 64.0), 1.0);
    }
    if (debug == 2u) {
        let g = length(-win.water.yz * water.time.y * water.time.x * 0.5);
        return vec4<f32>(fract(g), length(win.water.yz), 0.0, 1.0);
    }
    var r = vec4<f32>(0.0, 0.0, 0.0, q * fresnel);
    // The reflection: the planar image at the pixel
    // pushed by the normal, over the sky where it is empty (the modern client retries the
    // unpushed pixel and then the environment map there: an empty texel is
    // the sky either way).
    let tint = mix(albedo, vec3<f32>(1.0), n_mix);
    let planar_on = water.plane.z > 0.5 && abs(in.world.y - water.plane.x) <= water.plane.y;
    if (debug == 5u) {
        // Alpha 0.5: a surface off the plane (no planar image).
        return select(vec4<f32>(0.0, 0.0, 0.0, 0.5), planar_sample(win.clip.xy), planar_on);
    }
    // The reflection target holds the reflected scene premultiplied over
    // transparent black: the sky fills what it leaves uncovered.
    var refl = env_reflection(reflect(f, h));
    if (planar_on) {
        let p = planar_sample(win.clip.xy + offset);
        refl = p.rgb + refl * (1.0 - p.a);
    }
    if (debug == 3u) {
        return vec4<f32>(refl, 1.0);
    }
    r = vec4<f32>(refl * tint * water.reflect.x * water.reflect.y, r.a);
    // The sun's shadow attenuation, offset by the normal.
    let l = frame.sun_dir.xyz;
    let shadow = sun_visibility_base(in.world + vec3<f32>(offset.x, 0.0, offset.y), in.view_depth);
    // The sun's specular: half vector, the
    // sun's colour squared, its elevation, the modern power and intensity law.
    let half_vector = normalize(l - f);
    let hq = clamp(dot(half_vector, h), 0.0, 1.0);
    let spec = frame.sun_colour.rgb * frame.sun_colour.rgb * n_mix * clamp(-l.y, 0.0, 1.0)
        * pow(hq, water.brdf.z * 0.25) * (water.brdf.w * 1.8 + 0.2) * clamp(water.brdf.w - 0.05, 0.0, 1.0) * 25.0;
    r = vec4<f32>(r.rgb + spec * shadow, r.a);
    // The distance fog (the modern client applies it in the vertex stage through the
    // in/out scattering, and pulls the alpha to 1).
    var fog = 0.0;
    if (frame.fog_colour.w > 0.0) {
        // The distance-based fog of the eye distance.
        fog = distance_fog(in.world);
        fog = fog + fog * water.brdf.y - water.brdf.y;
        r = vec4<f32>(mix(r.rgb, frame.fog_colour.rgb, fog), mix(r.a, 1.0, fog));
    }
    // The water body (refraction with extinction). The
    // light under the surface: the sun's diffuse (the
    // wrapped sun, less what the fresnel reflects) over the ambient and the
    // point lights.
    var sun = (dot(-h, l) * 0.5 + 0.5) * (1.0 - fresnel) * sun_visibility_base(in.world, in.view_depth);
    if (fx_on(FX_SHADOW)) {
        // The modern wrap towards the normal under the specular's
        // attenuation (water_body::effects).
        sun = fx_sun_diffuse(h, l, fresnel, shadow);
    }
    let light = probe_ambient(in.world, vec3<f32>(0.0, -1.0, 0.0)) + frame.sun_colour.rgb * sun
        + point_light_sum_base(in, vec3<f32>(0.0, -1.0, 0.0));
    // The opaque water colour: the classic water colour under the type's
    // colour; the bed seen through shallow water: the classic water colour
    // itself.
    let deep = albedo * water.body.rgb * water.extinction.w * light;
    // The bed seen through shallow water (the modern client sees the seabed; the 910
    // floor builds none): the nearest bank's colour (`water_body::
    // bed_colours`) under the water's light and the land texture's mean
    // brightness, else the deep colour.
    var bed = deep;
    if (win.bed.r >= 0.0) {
        bed = win.bed * water.shore.w * light;
        if (fx_on(FX_SHADOW | FX_CAUSTICS)) {
            // The bed gain under the modern sun; the caustics as sun
            // light on the bed (light += sun colour * caustics * shadow,
            // before the albedo).
            var gain = water.shore.w;
            if (fx_on(FX_SHADOW)) {
                gain = water.fx.y;
            }
            var bed_light = light;
            if (fx_on(FX_CAUSTICS)) {
                bed_light += frame.sun_colour.rgb * fx_caustics(win.uv0.xy, lods.x, depth) * shadow;
            }
            bed = win.bed * gain * bed_light;
        }
    }
    // The path through the water along the view ray to the bed (a flat bed
    // `depth` below the surface; steep views see through less water).
    let path = min(depth / max(-f.y, 0.2), water.body.w);
    // Refraction: the scene copy behind the surface, nudged by the normal,
    // when every sample there shows geometry behind the water (at the
    // default water detail nothing is: no bed is built); then the path
    // runs to that geometry.
    let nudge = vec2<f32>(h.x, h.z) * min(sqrt(depth) * q * 2.0, 128.0) * fresnel * 2.0;
    var at = vec2<i32>(win.clip.xy + nudge);
    var behind = scene_depth_range(at);
    if (behind.x < win.clip.z || behind.y >= 1.0) {
        at = vec2<i32>(win.clip.xy);
        behind = scene_depth_range(at);
    }
    var body: vec3<f32>;
    if (behind.x >= win.clip.z && behind.y < 1.0) {
        let size = vec2<i32>(water.size.xy);
        let seen = textureLoad(refraction_tex, clamp(at, vec2<i32>(0), size - vec2<i32>(1)), 0).rgb;
        let far = unproject(vec2<f32>(at) + 0.5, behind.y);
        let through = min(length(far - in.world), water.body.w);
        // The extinction of the
        // refracted scene, the path and the vertical distance to it.
        body = extinction(seen, through, abs(far.y - in.world.y), frame.sun_colour.rgb * sun, albedo, q);
    } else {
        // The extinction over the synthesised bed
        // (the scene the modern client would see there).
        body = extinction(bed, path, depth, frame.sun_colour.rgb * sun, albedo, q);
    }
    if (debug == 4u) {
        return vec4<f32>(body, 1.0);
    }
    // The scattering on the surface term only, the
    // body blended under it unscattered.
    return vec4<f32>(mix(body, scatter(r.rgb, max(fog, 0.0), in.world), r.a), 1.0);
}
