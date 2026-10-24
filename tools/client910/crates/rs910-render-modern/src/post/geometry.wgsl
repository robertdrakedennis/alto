
// The ambient occlusion's geometry pass: the classic view-space face normal
// (w: 1 = geometry) of the opaque draws. View positions come back from its
// depth (`post.wgsl`'s `view_at`).
@fragment
fn fs_geometry(in: VsOut) -> @location(0) vec4<f32> {
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
    return vec4<f32>(n, 1.0);
}
