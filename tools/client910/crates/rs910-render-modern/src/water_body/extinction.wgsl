// Per-channel absorption keeps the shallow bed visible and approaches the
// cache's water pigment smoothly as the view ray travels through more water.
const WATER_COLOUR_EPSILON: f32 = 0.0001;
fn extinction(scene: vec3<f32>, path: f32, light: vec3<f32>, albedo: vec3<f32>, coverage: f32) -> vec3<f32> {
    let transmission = exp(-max(path, 0.0) / max(water.extinction.xyz, vec3<f32>(WATER_COLOUR_EPSILON)));
    let pigment = albedo / max(max(max(albedo.r, albedo.g), albedo.b), WATER_COLOUR_EPSILON);
    let deep = water.body.rgb * pigment * water.extinction.w * light;
    let absorbed = scene * transmission + deep * (vec3<f32>(1.0) - transmission);
    return mix(scene, absorbed, coverage);
}
