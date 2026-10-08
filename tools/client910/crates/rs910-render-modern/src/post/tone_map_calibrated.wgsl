
// The composite's operator: the advanced per-channel Reinhard over the
// adapted luminance.
fn tone_map_op(v: vec3<f32>, avg: f32) -> vec3<f32> {
    return reinhard_adv_rgb(v, avg);
}
