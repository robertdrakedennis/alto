
// The water extinction.
// `scene` the colour behind the surface, `path`/`vertical` the fine-unit
// distances to it (metres: x 0.002), `light` the sun diffuse without the
// albedo (m / v), `edge` the soft edge q. The visibility is the soft edge's
// (water.shore.y = 0.002 / the visibility in metres); the opaque
// colour the type's tint times 1.5 (calibrated), the depths
// vec3<f32>(4.0, 8.0, 12.0) (calibrated).
fn extinction(scene: vec3<f32>, path: f32, vertical: f32, light: vec3<f32>, albedo: vec3<f32>, edge: f32) -> vec3<f32> {
    let vis = 0.002 / max(water.shore.y, 1e-7);
    let r = path * 0.002;
    let v = vertical * 0.002;
    let s = mix(0.04, 1.0, clamp(vis, 0.0, 1.0));
    let g = albedo / max(max(max(albedo.r, albedo.g), albedo.b), 1e-4);
    let i = water.body.rgb * 1.5 * light * g;
    let n = mix(scene, i, pow(clamp(r / vis, 0.0, 1.0), s));
    let d = max(vec3<f32>(4.0, 8.0, 12.0) * g, vec3<f32>(1e-4));
    let gv = pow(clamp(vec3<f32>(v) / d, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.25));
    let m = pow(clamp(vec3<f32>(r) / d, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.25));
    return mix(scene, n * (vec3<f32>(1.0) - gv * m), edge);
}
