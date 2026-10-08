
// The sun shadows (shadows): one hardware-compare tap of the atlas (bilinear PCF).
fn shadow_tap(uv: vec2<f32>, depth: f32) -> f32 {
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, depth);
}

// The cascade's filter (params.w): 0 the 2x2 box (taps at texel offsets
// -1..0), 1 the approximate 4x4 box (four corner taps at +-1.5 texels, the
// sixteen half-texel taps only when the corners disagree), 2 the 4x4 box
// (taps at -2..1); the taps clamped inside the cascade's tile.
fn shadow_filter_base(c: vec3<f32>, ext: vec4<f32>) -> f32 {
    let texel = shadow.params.x;
    let kind = u32(shadow.params.w);
    if (kind == 0u) {
        let uv = clamp(c.xy, ext.xy + vec2<f32>(texel), ext.zw - vec2<f32>(texel));
        var total = 0.0;
        for (var i = 0; i < 4; i++) {
            let o = vec2<f32>(f32(i % 2) - 1.0, f32(i / 2) - 1.0);
            total += shadow_tap(uv + o * texel, c.z);
        }
        return total * 0.25;
    }
    if (kind == 1u) {
        let uv = clamp(c.xy, ext.xy + vec2<f32>(2.5 * texel), ext.zw - vec2<f32>(2.5 * texel));
        var corners = 0.0;
        for (var i = 0; i < 4; i++) {
            let o = vec2<f32>(select(-1.5, 1.5, (i & 1) != 0), select(-1.5, 1.5, (i & 2) != 0));
            corners += shadow_tap(uv + o * texel, c.z);
        }
        if (corners == 0.0 || corners >= 3.9) {
            return corners * 0.25;
        }
        var total = 0.0;
        for (var i = 0; i < 16; i++) {
            let o = vec2<f32>(f32(i % 4) - 1.5, f32(i / 4) - 1.5);
            total += shadow_tap(uv + o * texel, c.z);
        }
        return total * 0.0625;
    }
    let uv = clamp(c.xy, ext.xy + vec2<f32>(5.0 * texel), ext.zw - vec2<f32>(5.0 * texel));
    var total = 0.0;
    for (var i = 0; i < 16; i++) {
        let o = vec2<f32>(f32(i % 4) - 2.0, f32(i / 4) - 2.0);
        total += shadow_tap(uv + o * texel, c.z);
    }
    return total * 0.0625;
}

// The amount of sun reaching camera-local `world` at
// view depth `depth` (1 without shadows or outside every cascade).
fn sun_visibility_base(world: vec3<f32>, depth: f32) -> f32 {
    let count = i32(shadow.params.y);
    if (count == 0) {
        return 1.0;
    }
    let lv = (shadow.light_view * vec4<f32>(world, 1.0)).xyz;
    var k = count;
    if (shadow.lookup.x > 0.5) {
        // Select by map: the first tile that holds the point.
        for (var i = 0; i < count; i++) {
            let c = lv * shadow.tex_scale[i].xyz + shadow.tex_offset[i].xyz;
            let e = shadow.extents[i];
            if (all(c.xy >= e.xy) && all(c.xy <= e.zw) && c.z >= 0.0 && c.z <= 1.0) {
                k = i;
                break;
            }
        }
    } else {
        // Select by split: the view depth against the splits.
        k = 0;
        for (var i = 0; i < count; i++) {
            if (depth > shadow.splits[i]) {
                k = i + 1;
            }
        }
    }
    if (k >= count) {
        return 1.0;
    }
    let c = lv * shadow.tex_scale[k].xyz + shadow.tex_offset[k].xyz;
    if (c.z > 1.0) {
        return 1.0;
    }
    let ext = shadow.extents[k];
    var lit: f32;
    if (depth < shadow.lookup.y) {
        lit = shadow_filter_base(c, ext);
    } else {
        lit = shadow_tap(clamp(c.xy, ext.xy, ext.zw), c.z);
    }
    lit = min(1.0, lit + (1.0 - shadow.params.z));
    if (shadow.fade.w > 0.5) {
        lit = mix(lit, 1.0, smoothstep(shadow.fade.x, shadow.fade.y, depth));
    } else {
        lit = mix(lit, 1.0, clamp((depth - shadow.fade.x) * shadow.fade.z, 0.0, 1.0));
    }
    return lit;
}

// The caster pass (pass type 0): the forward vertex
// transform, then the cascade's orthographic projection of the light axes.
@vertex
fn vs_shadow(v: VsIn) -> VsOut {
    var out = vertex(v);
    let lv = (caster.light_view * vec4<f32>(out.world, 1.0)).xyz;
    out.clip = vec4<f32>(lv * caster.clip_scale.xyz + caster.clip_offset.xyz, 1.0);
    return out;
}

// A caster's coverage: its diffuse coverage and vertex alpha (the forward
// pass's cutouts), kept where at least half opaque.
@fragment
fn fs_shadow(in: VsOut) {
    let flags = u32(in.p0.w);
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let footprint = 0.5 * log2(max(max(dot(duv_x, duv_x), dot(duv_y, duv_y)), 1e-24));
    var tex: vec4<f32>;
    if (material.diffuse.w > 0.5) {
        tex = sample_atlas(diffuse_tex, in.uv, footprint, material.diffuse);
        if (material.params.x > 0.5) {
            tex.a = tex.a * select(0.0, 1.0, max(max(tex.r, tex.g), tex.b) > 0.0005);
        }
    } else {
        tex = sample_material(diffuse_tex, false, in.uv, flags, material_lod(diffuse_tex, in.uv, flags));
    }
    var coverage = tex.a;
    if ((flags & 1u) != 0u) {
        coverage = 1.0;
    }
    let alpha = coverage * in.albedo.a;
    let tested = select(alpha, coverage, (flags & 16u) != 0u);
    if (alpha < 0.5 || (in.p0.z >= 0.0 && tested <= in.p0.z)) {
        discard;
    }
}

// The 2x2 box: shifted half a texel, clamped 1.5 texels inside the tile, taps (-1,-1)..(0,0).
fn shadow_box2(c: vec3<f32>, ext: vec4<f32>) -> f32 {
    let texel = shadow.params.x;
    let uv = clamp(c.xy + vec2<f32>(0.5 * texel), ext.xy + vec2<f32>(1.5 * texel), ext.zw - vec2<f32>(1.5 * texel));
    var total = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<f32>(f32(i % 2) - 1.0, f32(i / 2) - 1.0);
        total += shadow_tap(uv + o * texel, c.z);
    }
    return total * 0.25;
}

// The quality's filter: the 2x2 box above; the approximate 4x4 box (shifted
// half a texel, 2.5-texel border, four corner taps and all sixteen only when
// they disagree; here the offsets are the 4x4 box's -2..1); the 4x4 box.
fn shadow_filter(c: vec3<f32>, ext: vec4<f32>) -> f32 {
    let texel = shadow.params.x;
    let kind = u32(shadow.params.w);
    if (kind == 0u) {
        return shadow_box2(c, ext);
    }
    if (kind == 1u) {
        let uv = clamp(c.xy + vec2<f32>(0.5 * texel), ext.xy + vec2<f32>(2.5 * texel), ext.zw - vec2<f32>(2.5 * texel));
        var corners = 0.0;
        for (var i = 0; i < 4; i++) {
            let o = vec2<f32>(select(-2.0, 1.0, (i & 1) != 0), select(-2.0, 1.0, (i & 2) != 0));
            corners += shadow_tap(uv + o * texel, c.z);
        }
        if (corners == 0.0 || corners >= 3.9) {
            return corners * 0.25;
        }
        var total = 0.0;
        for (var i = 0; i < 16; i++) {
            let o = vec2<f32>(f32(i % 4) - 2.0, f32(i / 4) - 2.0);
            total += shadow_tap(uv + o * texel, c.z);
        }
        return total * 0.0625;
    }
    return shadow_filter_base(c, ext);
}

// The sun's shadow attenuation after the receiver's normal offset: the
// sun reaching camera-local `world` of lit normal `n` at view depth `depth`.
fn sun_visibility(world: vec3<f32>, n: vec3<f32>, depth: f32) -> f32 {
    let count = i32(shadow.params.y);
    if (count == 0 || depth > shadow.splits[3]) {
        return 1.0;
    }
    let p = world + n * shadow.bias_select.x;
    let lv = (shadow.light_view * vec4<f32>(p, 1.0)).xyz;
    let mode = u32(shadow.bias_select.y);
    var k = count;
    if (mode == 1u) {
        // Select by map: the first tile holding the point.
        for (var i = 0; i < count; i++) {
            let c = lv * shadow.tex_scale[i].xyz + shadow.tex_offset[i].xyz;
            let e = shadow.extents[i];
            if (all(c.xy >= e.xy) && all(c.xy <= e.zw) && c.z >= 0.0 && c.z <= 1.0) {
                k = i;
                break;
            }
        }
    } else if (mode == 2u) {
        // Select by sphere: the first bounding sphere holding
        // the point.
        for (var i = 0; i < count; i++) {
            let d = lv - shadow.spheres[i].xyz;
            if (dot(d, d) < shadow.spheres[i].w) {
                k = i;
                break;
            }
        }
    } else {
        // Select by split.
        k = 0;
        for (var i = 0; i < count; i++) {
            if (depth > shadow.splits[i]) {
                k = i + 1;
            }
        }
    }
    if (k >= count) {
        return 1.0;
    }
    let c = lv * shadow.tex_scale[k].xyz + shadow.tex_offset[k].xyz;
    if (c.z > 1.0) {
        return 1.0;
    }
    let ext = shadow.extents[k];
    var lit: f32;
    if (depth < shadow.lookup.y) {
        lit = shadow_filter(c, ext);
    } else {
        lit = shadow_box2(c, ext);
    }
    lit = min(1.0, lit + (1.0 - shadow.params.z));
    if (shadow.fade.w > 0.5) {
        lit = mix(lit, 1.0, smoothstep(shadow.fade.x, shadow.fade.y, depth));
    } else {
        lit = mix(lit, 1.0, clamp((depth - shadow.fade.x) * shadow.fade.z, 0.0, 1.0));
    }
    return lit;
}
