
// Trilinear filtering with the mip blend done here: two bilinear samples at
// explicit integer levels (deterministic) mixed by the derivative LOD. The
// hardware's linear mip filter flips a few texels run to run on Apple GPUs.
fn sample_trilinear(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(t, 0));
    let dx = dpdx(uv * size);
    let dy = dpdy(uv * size);
    let rho = max(dot(dx, dx), dot(dy, dy));
    let top = f32(textureNumLevels(t)) - 1.0;
    let lod = clamp(0.5 * log2(max(rho, 1e-12)), 0.0, top);
    let l0 = floor(lod);
    let a = textureSampleLevel(t, material_sampler, uv, l0);
    let b = textureSampleLevel(t, material_sampler, uv, min(l0 + 1.0, top));
    return mix(a, b, lod - l0);
}

// A draw's layer of the material arrays (0: its material has its own
// textures) and whether the layer is a 64 texel map tiled twice over each way:
// the high bits of the draw's flags (`models::material_arrays`).
fn mat_layer(flags: u32) -> i32 {
    return i32((flags >> 10u) & 511u);
}
fn mat_tiled(flags: u32) -> bool {
    return ((flags >> 19u) & 1u) != 0u;
}

// The mip levels of `sample_trilinear` for a material map: the draw's layer of
// the arrays, or the material's own texture `own` (layer 0). The derivatives
// are taken here, before any branch (they need uniform control flow), and are
// those of the texture the material draws with: a tiled layer is the 64 texel
// map at half the coordinates, so the size and the levels are its own, and
// the coordinates the array is sampled at are `uv` halved (an exact scaling).
struct MatLod {
    l0: f32,
    l1: f32,
    t: f32,
};

fn material_lod(own: texture_2d<f32>, uv: vec2<f32>, flags: u32) -> MatLod {
    var size = vec2<f32>(textureDimensions(own, 0));
    var levels = textureNumLevels(own);
    if (mat_layer(flags) > 0) {
        if (mat_tiled(flags)) {
            size = vec2<f32>(64.0);
            levels = 7u;
        } else {
            size = vec2<f32>(textureDimensions(diffuse_layers, 0));
            levels = textureNumLevels(diffuse_layers);
        }
    }
    let dx = dpdx(uv * size);
    let dy = dpdy(uv * size);
    let rho = max(dot(dx, dx), dot(dy, dy));
    let top = f32(levels) - 1.0;
    let lod = clamp(0.5 * log2(max(rho, 1e-12)), 0.0, top);
    let l0 = floor(lod);
    return MatLod(l0, min(l0 + 1.0, top), lod - l0);
}

// `sample_trilinear` of a material map at `lod` (explicit levels, so this may
// sit in a branch): the draw's layer of the diffuse or aux array, or `own`.
fn sample_material(own: texture_2d<f32>, aux: bool, uv: vec2<f32>, flags: u32, lod: MatLod) -> vec4<f32> {
    var a: vec4<f32>;
    var b: vec4<f32>;
    let layer = mat_layer(flags);
    // The second level only matters with a fraction of the blend: at either
    // end of the levels it is the first (and `mix(a, b, 0.0)` is `a`).
    let blend = lod.t != 0.0;
    if (layer > 0) {
        let at = select(uv, uv * 0.5, mat_tiled(flags));
        if (aux) {
            a = textureSampleLevel(aux_layers, layer_sampler, at, layer, lod.l0);
            if (blend) {
                b = textureSampleLevel(aux_layers, layer_sampler, at, layer, lod.l1);
            }
        } else {
            a = textureSampleLevel(diffuse_layers, layer_sampler, at, layer, lod.l0);
            if (blend) {
                b = textureSampleLevel(diffuse_layers, layer_sampler, at, layer, lod.l1);
            }
        }
    } else {
        a = textureSampleLevel(own, material_sampler, uv, lod.l0);
        if (blend) {
            b = textureSampleLevel(own, material_sampler, uv, lod.l1);
        }
    }
    if (!blend) {
        return a;
    }
    return mix(a, b, lod.t);
}

// The atlas lookup of an RT7 map (see the module docs): `footprint` is
// log2 of the UV's screen derivative (uniform control flow, before any
// branch), so the LOD sees the unwrapped UV, not the wrap's seams.
fn sample_atlas(t: texture_2d<f32>, uv: vec2<f32>, footprint: f32, atlas: vec4<f32>) -> vec4<f32> {
    let inner = f32(textureDimensions(t, 0).x) * atlas.x;
    let lod = clamp(footprint + log2(inner), 0.0, atlas.z);
    let bits = u32(material.params.w);
    let repeat = vec2<bool>((bits & 1u) != 0u, (bits & 2u) != 0u);
    let wrapped = select(clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), fract(uv), repeat);
    let at = wrapped * atlas.x + vec2<f32>(atlas.y);
    let l0 = floor(lod);
    let a = textureSampleLevel(t, material_sampler, at, l0);
    let b = textureSampleLevel(t, material_sampler, at, min(l0 + 1.0, atlas.z));
    return mix(a, b, lod - l0);
}

// The classic texture gamma (0.7 on the display value) for textures that
// could not take it at load (the ETC diffuse): linear in, linear out.
fn texture_gamma(c: vec3<f32>) -> vec3<f32> {
    return srgb_to_linear(pow(max(linear_to_srgb(c), vec3<f32>(0.0)), vec3<f32>(0.7)));
}

struct SurfaceBase {
    colour: vec3<f32>,
    alpha: f32,
    spec_mask: f32,
    keep: bool,
    // The tangent-space normal of the normal map (0, 0, 1 without one).
    detail: vec3<f32>,
};

fn surface_base(in: VsOut) -> SurfaceBase {
    var s: SurfaceBase;
    let flags = u32(in.p0.w);
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let footprint = 0.5 * log2(max(max(dot(duv_x, duv_x), dot(duv_y, duv_y)), 1e-24));
    var tex: vec4<f32>;
    if (material.diffuse.w > 0.5) {
        tex = sample_atlas(diffuse_tex, in.uv, footprint, material.diffuse);
        if (material.params.y > 0.5) {
            tex = vec4<f32>(texture_gamma(tex.rgb), tex.a);
        }
        if (material.params.x > 0.5) {
            // Black texels are transparent (from the filtered
            // colour; the M1 path bakes it into the alpha).
            tex.a = tex.a * select(0.0, 1.0, max(max(tex.r, tex.g), tex.b) > 0.0005);
        }
    } else {
        tex = sample_material(diffuse_tex, false, in.uv, flags, material_lod(diffuse_tex, in.uv, flags));
    }
    // The aux map only scales the colour of materials that have one.
    let aux_lod = material_lod(aux_tex, in.uv, flags);
    var aux = 0.0;
    if ((flags & 2u) != 0u) {
        aux = sample_material(aux_tex, true, in.uv, flags, aux_lod).r;
    }
    s.detail = vec3<f32>(0.0, 0.0, 1.0);
    if (material.normal.w > 0.5) {
        let texel = sample_atlas(normal_tex, in.uv, footprint, material.normal);
        // X in alpha (red in the ETC copies: w 2), Y in green; green points
        // up the image, i.e. -V.
        let x = select(texel.a, texel.r, material.normal.w > 1.5);
        var xy = (vec2<f32>(x, texel.g) * 2.0 - 1.0) * vec2<f32>(1.0, -1.0);
        let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
        s.detail = vec3<f32>(xy * material.params.z, z);
    }
    var coverage = tex.a;
    s.spec_mask = 1.0;
    if ((flags & 1u) != 0u) {
        s.spec_mask = tex.a;
        coverage = 1.0;
    }
    s.colour = texture_linear(tex.rgb) * in.albedo.rgb;
    if ((flags & 2u) != 0u) {
        // HDRScale: rgb * (1 + ratio * 31).
        s.colour = s.colour * (1.0 + aux * 31.0);
    }
    s.alpha = coverage * in.albedo.a;
    let tested = select(s.alpha, coverage, (flags & 16u) != 0u);
    s.keep = s.alpha > 0.0 && !(in.p0.z >= 0.0 && tested <= in.p0.z);
    return s;
}

struct Surface {
    // The texel times the vertex colour, linear, HDR-scaled.
    colour: vec3<f32>,
    // The bare texel (RT7's metal albedo), linear, HDR-scaled.
    tex: vec3<f32>,
    alpha: f32,
    // The specular multiplier D.
    spec_mul: f32,
    keep: bool,
    detail: vec3<f32>,
    rt7: bool,
    metal: f32,
    rough: f32,
    emissive: f32,
    // The env mask (the texel alpha).
    env_mask: f32,
};

// The surface: the pre-lane `surface` (same samples, same faithful alpha
// tests, so the depth pre-pass's coverage matches) with the modern client's specular map,
// normal unpack, compound map and emissive mask.
fn surface(in: VsOut) -> Surface {
    var s: Surface;
    let flags = u32(in.p0.w);
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let footprint = 0.5 * log2(max(max(dot(duv_x, duv_x), dot(duv_y, duv_y)), 1e-24));
    var tex: vec4<f32>;
    s.rt7 = material.diffuse.w > 0.5;
    if (s.rt7) {
        tex = sample_atlas(diffuse_tex, in.uv, footprint, material.diffuse);
        if (material.params.y > 0.5) {
            tex = vec4<f32>(texture_gamma(tex.rgb), tex.a);
        }
        if (material.params.x > 0.5) {
            tex.a = tex.a * select(0.0, 1.0, max(max(tex.r, tex.g), tex.b) > 0.0005);
        }
    } else {
        tex = sample_material(diffuse_tex, false, in.uv, flags, material_lod(diffuse_tex, in.uv, flags));
    }
    // The aux map only scales the colour of materials that have one.
    let aux_lod = material_lod(aux_tex, in.uv, flags);
    var aux = 0.0;
    if ((flags & 2u) != 0u) {
        aux = sample_material(aux_tex, true, in.uv, flags, aux_lod).r;
    }
    s.detail = vec3<f32>(0.0, 0.0, 1.0);
    s.emissive = 0.0;
    if (material.normal.w > 0.5) {
        let texel = sample_atlas(normal_tex, in.uv, footprint, material.normal);
        let etc = material.normal.w > 1.5;
        // The compressed-normal unpack: X from alpha (red in the ETC
        // copies, whose channels are swizzled), Y from green, `c * 255 / 127 -
        // 1.00787`, Y negated, Z rebuilt; XY times the normal scale.
        let x = select(texel.a, texel.r, etc);
        let xy = (vec2<f32>(x, texel.g) * (255.0 / 127.0) - vec2<f32>(1.00787)) * vec2<f32>(1.0, -1.0);
        let z = sqrt(1.0 - min(1.0, dot(xy, xy)));
        s.detail = vec3<f32>(xy * material.params.z, z);
        // The emissive mask: red (alpha in
        // the ETC copies).
        s.emissive = select(texel.r, texel.a, etc);
    }
    s.metal = RT7_DEFAULT_METAL;
    s.rough = RT7_DEFAULT_ROUGHNESS;
    if (material.compound.w > 0.5) {
        // Red is metalness, green roughness (the ETC copies swap green
        // and alpha).
        let c = sample_atlas(compound_tex, in.uv, footprint, material.compound);
        s.metal = c.r;
        s.rough = select(c.g, c.a, material.compound.w > 1.5);
    }
    var coverage = tex.a;
    s.spec_mul = 1.0;
    if ((flags & 1u) != 0u) {
        // A specular map in the texture alpha:
        // D = 1 + 4 * alpha, and the alpha becomes coverage 1.
        s.spec_mul = 1.0 + SPEC_MAP_SCALE * tex.a;
        coverage = 1.0;
    }
    // The modern client's vertex bits for models: D only with bit 0
    // (effect 1); the env mask = the texel alpha with bit 2, the alpha
    // then 1.
    s.env_mask = 0.0;
    if ((flags & 32u) == 0u) {
        if ((flags & 128u) == 0u) {
            s.spec_mul = 1.0;
        }
        if ((flags & 64u) != 0u) {
            s.env_mask = tex.a;
            coverage = 1.0;
        }
    }
    s.tex = texture_linear(tex.rgb);
    if ((flags & 2u) != 0u) {
        // HDR scale (unchanged): rgb * (1 + ratio * 31).
        s.tex = s.tex * (1.0 + aux * 31.0);
    }
    s.colour = s.tex * in.albedo.rgb;
    s.alpha = coverage * in.albedo.a;
    let tested = select(s.alpha, coverage, (flags & 16u) != 0u);
    s.keep = s.alpha > 0.0 && !(in.p0.z >= 0.0 && tested <= in.p0.z);
    return s;
}

// The lit normal (the pre-lane shade's): the vertex normal with the normal
// map in the vertex tangent frame (the bitangent is `cross(N, T) * w`).
fn lit_normal(in: VsOut, detail: vec3<f32>) -> vec3<f32> {
    var n = in.normal;
    if (dot(n, n) < 1e-8) {
        n = vec3<f32>(0.0, -1.0, 0.0);
    }
    n = normalize(n);
    let t = in.tangent.xyz - n * dot(n, in.tangent.xyz);
    if (detail.z < 1.0 && dot(t, t) > 1e-12) {
        let tn = normalize(t);
        let bn = cross(n, tn) * select(-1.0, 1.0, in.tangent.w >= 0.0);
        n = normalize(tn * detail.x + bn * detail.y + n * detail.z);
    }
    return n;
}
