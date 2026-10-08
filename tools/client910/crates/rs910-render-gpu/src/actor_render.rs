//! Actors retain local vertices and receive the composed model shader frame.
use crate::{
    actor_matrix::Matrix,
    camera::{self, SceneCamera},
    env::EnvFrame,
    floor_render::FloorUniforms,
};
pub fn uniforms(
    camera: &SceneCamera,
    base: (i32, i32),
    env: &EnvFrame,
    model: &Matrix,
) -> FloorUniforms {
    let mut local = camera.clone();
    local.target[0] -= base.0 * 512;
    local.target[2] -= base.1 * 512;
    let view = local.view_entries();
    let vp = camera::multiply(&view, &local.projection());
    let world = model.entries();
    let wvp = camera::multiply(&world, &vp);
    let mut u = FloorUniforms::new(glam::Mat4::IDENTITY, env);
    u.wvp = (camera::gl_to_wgpu_depth() * camera::to_glam(&wvp)).to_cols_array_2d();
    u.shadow_wvp = u.wvp;
    u.model_world = camera::to_glam(&world).to_cols_array_2d();
    u.scene_origin = [0.; 4];
    u.scene_base = [0.; 4];
    let inverse = model.inverse();
    let sun = inverse.vector(env.sun.dir);
    u.sun_dir = [sun[0], sun[1], sun[2], 0.];
    let view3 = Matrix(std::array::from_fn(|i| view[i / 3 * 4 + i % 3]));
    let inv_view = view3.inverse();
    let eye = inverse.point(inv_view.0[9], inv_view.0[10], inv_view.0[11]);
    u.eye_time = [eye[0], eye[1], eye[2], 0.];
    if let Some((start, end)) = env.fog.range {
        let mv = camera::multiply(&world, &view);
        let v = [0., 0., 1., -start];
        let scale = 1. / (end - start);
        u.distance_fog_plane = std::array::from_fn(|c| {
            (mv[c * 4 + 3] * v[3] + mv[c * 4 + 2] * v[2] + mv[c * 4] * v[0] + mv[c * 4 + 1] * v[1])
                * scale
        });
    }
    u
}
/// Transformed cylinder endpoints match the model draw even with float translation.
pub fn cylinder(model: &mut crate::gpumodel::GpuModel, matrix: &Matrix) -> Option<[f32; 7]> {
    if model.unique_count == 0 {
        return None;
    }
    let lo = matrix.point(0., model.min_y() as f32, 0.);
    let hi = matrix.point(0., model.max_y() as f32, 0.);
    Some([
        lo[0],
        lo[1],
        lo[2],
        hi[0],
        hi[1],
        hi[2],
        model.horizontal_radius() as f32,
    ])
}
