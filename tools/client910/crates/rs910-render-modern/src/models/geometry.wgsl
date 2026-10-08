
fn vertex(v: VsIn) -> VsOut {
    var out: VsOut;
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.pos, 1.0);
    out.world = world.xyz;
    out.normal = mat3x3<f32>(v.m0.xyz, v.m1.xyz, v.m2.xyz) * v.normal;
    out.tangent = vec4<f32>(mat3x3<f32>(v.m0.xyz, v.m1.xyz, v.m2.xyz) * v.tangent.xyz, v.tangent.w);
    out.clip = frame.view_proj * world;
    out.view_depth = (frame.view * world).z;
    let scroll = v.p1.xy * frame.eye.w;
    out.uv = v.uv * v.p0.xy + (scroll - trunc(scroll));
    // The per-vertex albedo: the classic HSL colour is display-referred (the 2.2
    // display transfer, post::tonemap).
    out.albedo = vec4<f32>(display_to_linear(v.colour.rgb), v.colour.a);
    out.p0 = v.p0;
    out.p1 = v.p1;
    out.p2 = v.p2;
    return out;
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    return vertex(v);
}

// The forward varyings plus the per-vertex fog.
struct VsOutFog {
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
    @location(9) fog: f32,
};

@vertex
fn vs_forward(v: VsIn) -> VsOutFog {
    let o = vertex(v);
    var out: VsOutFog;
    out.clip = o.clip;
    out.world = o.world;
    out.normal = o.normal;
    out.uv = o.uv;
    out.albedo = o.albedo;
    out.p0 = o.p0;
    out.p1 = o.p1;
    out.view_depth = o.view_depth;
    out.tangent = o.tangent;
    out.p2 = o.p2;
    out.fog = distance_fog(o.world);
    return out;
}

fn unfog(in: VsOutFog) -> VsOut {
    var o: VsOut;
    o.clip = in.clip;
    o.world = in.world;
    o.normal = in.normal;
    o.uv = in.uv;
    o.albedo = in.albedo;
    o.p0 = in.p0;
    o.p1 = in.p1;
    o.view_depth = in.view_depth;
    o.tangent = in.tangent;
    o.p2 = in.p2;
    return o;
}

// GeometryPass: depth only, with the same coverage test.
@fragment
fn fs_depth(in: VsOut) {
    let s = surface_base(in);
    if (!s.keep) {
        discard;
    }
}
