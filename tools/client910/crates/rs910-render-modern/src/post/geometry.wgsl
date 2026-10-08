
// post (M8): the normal/depth pre-pass, here the classic view-space face
// normal (w: 1 = geometry) and position of the opaque draws.
struct GeometryOut {
    @location(0) normal: vec4<f32>,
    @location(1) position: vec4<f32>,
};

@fragment
fn fs_geometry(in: VsOut) -> GeometryOut {
    // The face normal: the triangle's normal from the view-space position's
    // derivatives (taken before the cutout's discard; classic floors and
    // models interpolate smoothed normals that tilt away from the faces).
    let vp = (frame.view * vec4<f32>(in.world, 1.0)).xyz;
    var n = cross(dpdx(vp), dpdy(vp));
    let s = surface_base(in);
    if (!s.keep) {
        discard;
    }
    if (dot(n, n) < 1e-12) {
        n = -vp;
    }
    n = normalize(n);
    // Towards the eye (the view origin).
    if (dot(n, vp) > 0.0) {
        n = -n;
    }
    var out: GeometryOut;
    out.normal = vec4<f32>(n, 1.0);
    out.position = vec4<f32>(vp, 1.0);
    return out;
}
