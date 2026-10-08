//! Model matrix (4x3/4x4) and model shader-frame input comparison.
use std::io::Write;
#[test]
fn shader_frames() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("actor-frames");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut result = std::fs::File::create(out.join("rust.bin"))?;
    input.write_all(&192i32.to_be_bytes())?;
    let put = |f: &mut std::fs::File, values: &[f32]| -> anyhow::Result<()> {
        for v in values {
            f.write_all(&v.to_bits().to_be_bytes())?;
        }
        Ok(())
    };
    for i in 0..192 {
        let base = (3100, 3200);
        let mut camera =
            crate::camera::SceneCamera::new([base.0 * 512 + 8400, -733, base.1 * 512 + 8600]);
        camera.yaw = ((i * 701) & 16383) as f32;
        camera.pitch = (1077 + i % 32 * 53) as f32;
        camera.viewport = (1280, 720);
        camera.map_size_x = 104;
        let mut local = camera.clone();
        local.target[0] -= base.0 * 512;
        local.target[2] -= base.1 * 512;
        let view = local.view_entries();
        let projection = local.projection();
        let q = crate::actor::rotation(i * 139, -127 + i % 4 * 193, 217 - i % 7 * 113);
        let position = [8192.25 + i as f32 * 0.3, -371.75 + i as f32 * 1.3, 8666.125];
        let offset = -5. - i as f32;
        let matrix = crate::actor_matrix::Matrix::actor(q, position, offset);
        let mut env = crate::env::EnvFrame::default_for(10000., 10., &view);
        env.sun.dir = [0.3, -0.9, 0.1];
        env.fog.range = if i % 3 == 0 {
            None
        } else {
            Some((311.75, 4997.5))
        };
        env.fog.distance_plane = [0.; 4];
        put(&mut input, &view)?;
        put(&mut input, &projection)?;
        put(&mut input, &q)?;
        put(&mut input, &position)?;
        put(&mut input, &[offset])?;
        put(&mut input, &env.sun.dir)?;
        put(
            &mut input,
            &[
                if env.fog.range.is_some() { 311.75 } else { 0. },
                if env.fog.range.is_some() { 4997.5 } else { 0. },
            ],
        )?;
        let u = crate::actor_render::uniforms(&camera, base, &env, &matrix);
        put(&mut result, &matrix.0)?;
        put(&mut result, &u.wvp.concat())?;
        put(&mut result, &u.model_world.concat())?;
        put(&mut result, &u.sun_dir)?;
        put(&mut result, &u.eye_time)?;
        put(&mut result, &u.distance_fog_plane)?;
    }
    scratch.finish("actor-frames", &[("rust.bin", "recording")]);
    Ok(())
}
