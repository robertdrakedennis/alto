
// One point light's diffuse term:
// clamp(N.L) times the attenuation, the quadratic falloff for floors
// (1 - (d/r)^2) and the inverse square for models (r^2 / d^2).
fn point_light_base(slot: u32, world: vec3<f32>, n: vec3<f32>, on_floor: bool) -> vec3<f32> {
    if (slot == 0u || slot > point_grid.dims.w) {
        return vec3<f32>(0.0);
    }
    let l = point_lights[slot - 1u];
    let v = l.pos_radius.xyz - world;
    let d2 = dot(v, v);
    if (d2 <= 0.0) {
        return vec3<f32>(0.0);
    }
    let d = sqrt(d2);
    let r = l.pos_radius.w;
    var attenuation: f32;
    if (on_floor) {
        let ratio = clamp(d / r, 0.0, 1.0);
        attenuation = 1.0 - ratio * ratio;
    } else {
        attenuation = 1.0 / max(0.000001, d2 / (r * r));
    }
    return l.colour.rgb * clamp(dot(n, v / d), 0.0, 1.0) * attenuation;
}

// The static point lights on a fragment (lighting::point_lights): a floor's (flag 32)
// from its tile's grid entry at the floor's level, a model's from its
// entity's four slots; up to four either way.
fn point_light_sum_base(in: VsOut, n: vec3<f32>) -> vec3<f32> {
    if (point_grid.dims.w == 0u) {
        return vec3<f32>(0.0);
    }
    let flags = u32(in.p0.w);
    var slots: vec4<u32>;
    let on_floor = (flags & 32u) != 0u;
    if (on_floor) {
        let p = in.world.xz + point_grid.origin.xz;
        let tile = vec2<i32>(floor(p / 512.0));
        let level = u32(in.p2.x);
        if (tile.x < 0 || tile.y < 0 || u32(tile.x) >= point_grid.dims.x
            || u32(tile.y) >= point_grid.dims.y || level >= point_grid.dims.z) {
            return vec3<f32>(0.0);
        }
        let packed = light_grid[(level * point_grid.dims.y + u32(tile.y)) * point_grid.dims.x + u32(tile.x)];
        slots = vec4<u32>(packed.x & 0xFFFFu, packed.x >> 16u, packed.y & 0xFFFFu, packed.y >> 16u);
    } else {
        slots = vec4<u32>(in.p2);
    }
    var term = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        if (slots[i] == 0u) {
            break;
        }
        term += point_light_base(slots[i], in.world, n, on_floor);
    }
    return term;
}

// One light, the modern law: the attenuation
// pow(1 - min(1 - 1/65535, d²/r²), falloff), the wrapped N.L (wrap 0),
// the clamped colour times the intensity (the CPU record); floors and
// models alike.
fn point_light_unshadowed(slot: u32, world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if (slot == 0u || slot > point_grid.dims.w) {
        return vec3<f32>(0.0);
    }
    let l = point_lights[slot - 1u];
    let b = l.pos_radius.xyz - world;
    let d2 = dot(b, b);
    if (d2 <= 0.0) {
        return vec3<f32>(0.0);
    }
    let r = l.pos_radius.w;
    let w = pow(1.0 - min(1.0 - 1.0 / 65535.0, d2 / (r * r)), POINT_LIGHT_FALLOFF);
    let n_dot_l = clamp(dot(n, b * inverseSqrt(d2)), 0.0, 1.0);
    return l.colour.rgb * (w * n_dot_l);
}

// The pre-lane light lists (a floor's tile, a model's slots) under the
// modern law.
fn point_light_sum(in: VsOut, n: vec3<f32>) -> vec3<f32> {
    if (point_grid.dims.w == 0u) {
        return vec3<f32>(0.0);
    }
    let flags = u32(in.p0.w);
    var slots: vec4<u32>;
    if ((flags & 32u) != 0u) {
        let p = in.world.xz + point_grid.origin.xz;
        let tile = vec2<i32>(floor(p / 512.0));
        let level = u32(in.p2.x);
        if (tile.x < 0 || tile.y < 0 || u32(tile.x) >= point_grid.dims.x
            || u32(tile.y) >= point_grid.dims.y || level >= point_grid.dims.z) {
            return vec3<f32>(0.0);
        }
        let packed = light_grid[(level * point_grid.dims.y + u32(tile.y)) * point_grid.dims.x + u32(tile.x)];
        slots = vec4<u32>(packed.x & 0xFFFFu, packed.x >> 16u, packed.y & 0xFFFFu, packed.y >> 16u);
    } else {
        slots = vec4<u32>(in.p2);
    }
    var term = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        if (slots[i] == 0u) {
            break;
        }
        term += point_light(slots[i], in.world, n);
    }
    return term;
}
