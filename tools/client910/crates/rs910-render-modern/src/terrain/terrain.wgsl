
fn terrain_sun(world: vec3<f32>, n: vec3<f32>, depth: f32) -> f32 {
    return sun_visibility(world, n, depth);
}
fn terrain_point(slot: u32, world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    return point_light(slot, world, n);
}
fn terrain_fog(world: vec3<f32>, depth: f32) -> f32 {
    return distance_fog(world);
}

// terrain (M10): the terrain pass.
struct Terrain {
    origin: vec4<f32>,   // scene-local camera target (world = camera-local + origin)
    params: vec4<f32>,   // x grid size (fine units per tile), y specular on
};
struct TerrainMaterial {
    // x specular power (0: none), y 1 = the texel alpha is a specular mask,
    // z 1 = textured, w grey blend nibble * 16 + brightness nibble
    params: vec4<f32>,
};
@group(1) @binding(8) var<uniform> terrain: Terrain;
@group(1) @binding(9) var terrain_layers: texture_2d_array<f32>;
@group(1) @binding(10) var terrain_sampler: sampler;
@group(1) @binding(11) var<storage, read> terrain_materials: array<TerrainMaterial>;

struct TerrainIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) colour: vec4<f32>,
    @location(3) slots: vec4<u32>,     // layer per slot (0xFFFF: none)
    @location(4) scale: vec4<f32>,     // texture scale per slot
    @location(5) weight: vec4<f32>,    // xyz this vertex's slot bits, w level / 255
};

struct TerrainOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) albedo: vec4<f32>,
    @location(3) @interpolate(flat) slots: vec4<u32>,
    @location(4) @interpolate(flat) scale: vec4<f32>,
    @location(5) weight: vec4<f32>,
    @location(6) view_depth: f32,
    @location(7) @interpolate(flat) level: f32,
    // The distance fog per vertex.
    @location(8) fog: f32,
};

fn terrain_vertex(v: TerrainIn) -> TerrainOut {
    var out: TerrainOut;
    // Camera-local: the scene-local position minus the camera target.
    let world = vec4<f32>(v.pos - terrain.origin.xyz, 1.0);
    out.world = world.xyz;
    out.normal = v.normal;
    out.clip = frame.view_proj * world;
    out.view_depth = (frame.view * world).z;
    // The display-referred classic colour, decoded.
    out.albedo = v.colour;
    out.slots = v.slots;
    out.scale = v.scale;
    out.weight = vec4<f32>(v.weight.xyz, 0.0);
    out.level = round(v.weight.w * 255.0);
    out.fog = terrain_fog(out.world, out.view_depth);
    return out;
}

@vertex
fn vs_terrain(v: TerrainIn) -> TerrainOut {
    return terrain_vertex(v);
}

struct TerrainSurface {
    albedo: vec3<f32>,
    spec_mask: f32,
    spec_power: f32,
    unlit: f32,
};

// One slot's texel at world `p`: xz planar, `p / (scale * grid)`. The
// reference renderer writes the same coordinate as `8 / (D * grid)` with its
// scale `D` in eighths of a repeat.
fn terrain_texel(layer: u32, scale: f32, p: vec2<f32>) -> vec4<f32> {
    if (layer == 0xFFFFu || terrain_materials[layer].params.z < 0.5) {
        return vec4<f32>(1.0);
    }
    let k = 1.0 / (max(scale, 1e-3) * terrain.params.x);
    let uv = p * k;
    // LOD from the unwrapped UV (the wrap's seam must not reach it).
    let size = vec2<f32>(textureDimensions(terrain_layers, 0));
    let dx = dpdx(uv * size);
    let dy = dpdy(uv * size);
    let top = f32(textureNumLevels(terrain_layers)) - 1.0;
    let lod = clamp(0.5 * log2(max(max(dot(dx, dx), dot(dy, dy)), 1e-12)), 0.0, top);
    let l0 = floor(lod);
    let w = fract(uv);
    let a = textureSampleLevel(terrain_layers, terrain_sampler, w, i32(layer), l0);
    let b = textureSampleLevel(terrain_layers, terrain_sampler, w, i32(layer), min(l0 + 1.0, top));
    return mix(a, b, lod - l0);
}

fn terrain_surface(in: TerrainOut) -> TerrainSurface {
    var s: TerrainSurface;
    // World xz for the planar UVs: scene-local (camera-local + origin).
    let p = in.world.xz + terrain.origin.xz;
    var sum = vec3<f32>(0.0);
    var mask = 0.0;
    var power = 0.0;
    var total = 0.0;
    for (var k = 0; k < 3; k++) {
        let wk = in.weight[k];
        let layer = in.slots[k];
        // Sample every slot (uniform control flow for the derivatives).
        let texel = terrain_texel(layer, in.scale[k], p);
        var props = vec4<f32>(0.0);
        if (layer != 0xFFFFu) {
            props = terrain_materials[layer].params;
        }
        // The material property byte: grey blend A (high nibble, read as
        // `(r - E) / 255`) and brightness E / 15.
        // W = -1 marks an unlit layer (atmosphere::fog).
        let byte = max(props.w, 0.0);
        let e = byte - 16.0 * floor(byte / 16.0);
        let a = (byte - e) / 255.0;
        let base = mix(in.albedo.rgb, vec3<f32>(0.580392), a) * (1.0 + e / 15.0);
        let colour = texture_linear(texel.rgb) * display_to_linear(base);
        sum += colour * wk;
        if (props.w < -0.5) {
            s.unlit += wk;
        }
        total += wk;
        if (props.y > 0.5) {
            mask += texel.a * wk;
        }
        if (layer != 0xFFFFu) {
            power = max(power, props.x);
        }
    }
    if (total <= 1e-5) {
        s.albedo = display_to_linear(in.albedo.rgb);
    } else {
        s.albedo = sum / total;
    }
    if (total > 1e-5) {
        s.unlit = s.unlit / total;
    }
    s.spec_mask = mask;
    s.spec_power = power;
    return s;
}

@fragment
fn fs_terrain(in: TerrainOut) -> @location(0) vec4<f32> {
    let s = terrain_surface(in);
    var n = in.normal;
    if (dot(n, n) < 1e-8) {
        n = vec3<f32>(0.0, -1.0, 0.0);
    }
    n = normalize(n);
    let l = frame.sun_dir.xyz;
    let n_dot_l_raw = max(dot(n, l), 0.0);
    var n_dot_l = n_dot_l_raw;
    if (n_dot_l > 0.0) {
        n_dot_l = n_dot_l * terrain_sun(in.world, n, in.view_depth);
    }
    // Ambient (this backend's hemisphere for the irradiance), SSAO.
    // The probe ambient (lighting::probes).
    var ambient = probe_ambient(in.world, n);
    if (frame.params.y > 0.5) {
        ambient = ambient * ambient_occlusion(in.clip.xy);
    }
    if (probes.dims.w == 1u) {
        return vec4<f32>(ambient, 1.0);
    }
    if (probes.dims.w >= 2u) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // The sun's diffuse and (when specular is on) the specular: Schlick
    // fresnel (F0 0.65, exponent 5) times the Blinn-Phong distribution.
    let diffuse = frame.sun_colour.rgb * n_dot_l;
    var specular = vec3<f32>(0.0);
    if (terrain.params.y > 0.5 && s.spec_power > 0.0 && n_dot_l > 0.0) {
        let v = normalize(frame.eye.xyz - in.world);
        let h = normalize(v + l);
        let f = 0.65 + 0.35 * pow(1.0 - clamp(dot(l, h), 0.0, 1.0), 5.0);
        let ndf = (s.spec_power + 2.0) * 0.125 * pow(max(dot(h, n), 0.0), s.spec_power);
        specular = frame.sun_colour.rgb * (f * ndf * n_dot_l);
    }
    // Point lights: diffuse only, the classic floor law from the tile grid.
    var point = vec3<f32>(0.0);
    if (point_grid.dims.w > 0u) {
        let q = in.world.xz + point_grid.origin.xz;
        let tile = vec2<i32>(floor(q / 512.0));
        let level = u32(in.level);
        if (tile.x >= 0 && tile.y >= 0 && u32(tile.x) < point_grid.dims.x
            && u32(tile.y) < point_grid.dims.y && level < point_grid.dims.z) {
            let packed = light_grid[(level * point_grid.dims.y + u32(tile.y)) * point_grid.dims.x + u32(tile.x)];
            let slots = vec4<u32>(packed.x & 0xFFFFu, packed.x >> 16u, packed.y & 0xFFFFu, packed.y >> 16u);
            for (var i = 0; i < 4; i++) {
                if (slots[i] == 0u) {
                    break;
                }
                point += terrain_point(slots[i], in.world, n);
            }
        }
    }
    // Everything times the albedo.
    // The caustics on the bed (water_body::caustics).
    var caustic = terrain_caustics(in.world);
    if (caustic > 0.0) {
        caustic = caustic * step(0.999, terrain_sun(in.world, n, in.view_depth));
        if (frame.params.y > 0.5) {
            caustic = caustic * ambient_occlusion(in.clip.xy);
        }
    }
    var lit = (ambient + diffuse + point + specular * (1.0 + 4.0 * s.spec_mask)
        + frame.sun_colour.rgb * caustic) * s.albedo;
    // The classic unlit program for effect-6 layers.
    lit = mix(lit, s.albedo, s.unlit);
    // In/out scattering with the scattering off.
    lit = lit * (1.0 - in.fog) + frame.fog_colour.rgb * in.fog;
    lit = scatter(lit, in.fog, in.world);
    return vec4<f32>(lit, 1.0);
}

// The caster pass (pass type 0; the terrain casts opaque): the terrain
// transform, then the cascade's projection.
@vertex
fn vs_terrain_shadow(v: TerrainIn) -> @builtin(position) vec4<f32> {
    let out = terrain_vertex(v);
    let lv = (caster.light_view * vec4<f32>(out.world, 1.0)).xyz;
    return vec4<f32>(lv * caster.clip_scale.xyz + caster.clip_offset.xyz, 1.0);
}

// The normal/depth pre-pass (M8's SSAO geometry): the classic view-space face
// normal and position.
struct TerrainGeometryOut {
    @location(0) normal: vec4<f32>,
    @location(1) position: vec4<f32>,
};

@fragment
fn fs_terrain_geometry(in: TerrainOut) -> TerrainGeometryOut {
    let vp = (frame.view * vec4<f32>(in.world, 1.0)).xyz;
    var n = cross(dpdx(vp), dpdy(vp));
    if (dot(n, n) < 1e-12) {
        n = -vp;
    }
    n = normalize(n);
    if (dot(n, vp) > 0.0) {
        n = -n;
    }
    var out: TerrainGeometryOut;
    out.normal = vec4<f32>(n, 1.0);
    out.position = vec4<f32>(vp, 1.0);
    return out;
}
