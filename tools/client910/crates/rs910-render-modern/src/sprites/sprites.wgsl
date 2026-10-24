
// Sprites (M9: model billboards and particles, sprites::billboards,
// sprites::particles): camera-local
// quads, unlit, the display-referred vertex colour times the material
// texture, the material's HDR scale, the distance fog.
struct SpriteIn {
    @location(0) pos: vec3<f32>,
    @location(3) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) params: vec4<f32>,   // x alpha reference, y 1 = HDR scale, z 1 = a model billboard
};

// Sprites: the pre-lane quads with the per-vertex fog (the billboard and
// particle vertex programs).
struct SpriteOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
    @location(3) @interpolate(flat) params: vec4<f32>,  // x alpha reference, y HDR scale, z billboard, w premultiplied (blended pipeline)
    @location(4) world: vec3<f32>,
};

@vertex
fn vs_sprite(v: SpriteIn) -> SpriteOut {
    var out: SpriteOut;
    out.clip = frame.view_proj * vec4<f32>(v.pos, 1.0);
    out.colour = vec4<f32>(display_to_linear(v.colour.rgb), v.colour.a);
    out.uv = v.uv;
    out.fog = distance_fog(v.pos);
    out.world = v.pos;
    out.params = v.params;
    return out;
}

// Model billboards: the coverage squared and alpha-tested squared.
// Particles: discarded at alpha <= 0.01, premultiplied `(rgb * a, a * k)`
// with the alpha output factor k = 1 (no additive classic particles).
@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let footprint = 0.5 * log2(max(max(dot(duv_x, duv_x), dot(duv_y, duv_y)), 1e-24));
    var tex: vec4<f32>;
    if (material.diffuse.w > 0.5) {
        tex = sample_atlas(diffuse_tex, in.uv, footprint, material.diffuse);
        if (material.params.y > 0.5) {
            tex = vec4<f32>(texture_gamma(tex.rgb), tex.a);
        }
    } else {
        tex = sample_trilinear(diffuse_tex, in.uv);
    }
    let aux = sample_trilinear(aux_tex, in.uv).r;
    let alpha = tex.a * in.colour.a;
    var out_alpha = alpha;
    if (in.params.z > 0.5) {
        out_alpha = alpha * alpha;
        if (out_alpha <= in.params.x) {
            discard;
        }
    } else if (alpha <= max(in.params.x, PARTICLE_ALPHA_MIN)) {
        discard;
    }
    var rgb = texture_linear(tex.rgb) * in.colour.rgb;
    if (in.params.y > 0.5) {
        rgb = rgb * (1.0 + aux * 31.0);
    }
    if (in.params.z > 0.5) {
        // ModelBillboard(RT7).fs square the product
        // texel x colour (HDR scale inside): the colour's square is the 2.2
        // decode above, the texel and its HDR scale are squared here.
        var t = texture_linear(tex.rgb);
        if (in.params.y > 0.5) {
            t = t * (1.0 + aux * 31.0);
        }
        rgb = t * t * in.colour.rgb;
    }
    rgb = scatter(apply_fog(rgb, in.fog), in.fog, in.world);
    if (in.params.w > 0.5) {
        return vec4<f32>(rgb * out_alpha, out_alpha);
    }
    return vec4<f32>(rgb, out_alpha);
}
