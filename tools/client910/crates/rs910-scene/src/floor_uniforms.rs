//! The floor/model shaders' group-0 uniform block as the CPU builds it
//! (`FloorUniforms`, the sun/fog/matrix state of the toolkit's model shaders)
//! and the fine-unit-to-world scale it applies. Split whole out of
//! client910's `floor_render` (Phase 3.2), which re-exports it. No wgpu: the
//! struct is plain `bytemuck::Pod` data. Only the faithful GPU toolkit
//! builds it: the interface model draws carry neutral inputs and the toolkit
//! derives this block when it prepares them (renderer plan A4,
//! `rs910_render_gpu::ui_model_gpu::interface_uniforms`).

/// `M`: game fine units (+Y down) → viewer world (1 tile = 1 unit, +Y up).
pub const GAME_TO_WORLD: [f32; 3] = [1.0 / 512.0, -1.0 / 512.0, 1.0 / 512.0];

/// Group-0 uniform block (`FloorUniforms` in [`FLOOR_SHADER`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FloorUniforms {
    pub wvp: [[f32; 4]; 4],
    pub sun_dir: [f32; 4],
    pub sun_colour: [f32; 4],
    pub anti_sun_colour: [f32; 4],
    pub ambient_colour: [f32; 4],
    pub height_fog_plane: [f32; 4],
    pub height_fog_colour: [f32; 4],
    pub distance_fog_plane: [f32; 4],
    pub distance_fog_colour: [f32; 4],
    pub scene_origin: [f32; 4],
    pub shadow_wvp: [[f32; 4]; 4],
    pub eye_time: [f32; 4],
    pub scene_base: [f32; 4],
    pub model_world: [[f32; 4]; 4],
    /// The raw sun RGB split out when the sun is set; read by the
    /// environment-mapped water shader.
    pub sun_rgb: [f32; 4],
}

impl FloorUniforms {
    /// Project in a shared local frame, as the scene-local coordinates do,
    /// rather than cancelling million-unit world positions in the GPU.
    /// The origin subtraction happens before adding model-local vertices.
    /// Shadows keep exactly the legacy one-fine-unit lift, composed on the CPU.
    pub fn for_camera(camera: &crate::camera::SceneCamera, env: &crate::env::EnvFrame) -> Self {
        let mut local = camera.clone();
        local.target = [0; 3];
        let vp = crate::camera::multiply(&local.view_entries(), &local.projection());
        let mut shift = crate::camera::Matrix4x3::translation(0.0, -1.0, 0.0).to_entries();
        shift = crate::camera::multiply(&shift, &vp);
        let depth = crate::camera::gl_to_wgpu_depth();
        let mut u = Self::new(local.view_proj(), env);
        u.wvp = (depth * crate::camera::to_glam(&vp)).to_cols_array_2d();
        u.shadow_wvp = (depth * crate::camera::to_glam(&shift)).to_cols_array_2d();
        let eye = local.eye();
        u.eye_time = [eye[0] as f32, eye[1] as f32, eye[2] as f32, 0.];
        u.scene_origin = [
            camera.target[0] as f32,
            camera.target[1] as f32,
            camera.target[2] as f32,
            0.0,
        ];
        u
    }

    /// Lit-path uniforms for the toolkit sun state, fog off (zeroed), with
    /// `view_proj` the viewer's matrix over Rust world units (the fine-unit to
    /// world scale is folded in here).
    #[must_use]
    pub fn new(view_proj: glam::Mat4, env: &crate::env::EnvFrame) -> Self {
        let m = glam::Mat4::from_scale(glam::Vec3::from(GAME_TO_WORLD));
        let wvp = view_proj * m;
        let sun = &env.sun;
        let sun_rgb = env.sun_rgb;
        let d = sun.diffuse_half;
        let s = sun.shadow_half;
        let a = sun.ambient;
        let fp = env.fog.distance_plane;
        let fc = env.fog.distance_colour;
        Self {
            wvp: wvp.to_cols_array_2d(),
            eye_time: [0.; 4],
            scene_base: [0.; 4],
            model_world: glam::Mat4::IDENTITY.to_cols_array_2d(),
            scene_origin: [0.0; 4],
            shadow_wvp: (wvp * glam::Mat4::from_translation(glam::Vec3::new(0.0, -1.0, 0.0)))
                .to_cols_array_2d(),
            sun_dir: [sun.dir[0], sun.dir[1], sun.dir[2], 0.0],
            sun_colour: [sun_rgb[0] * d, d * sun_rgb[1], sun_rgb[2] * d, 0.0],
            anti_sun_colour: [-s * sun_rgb[0], -s * sun_rgb[1], -s * sun_rgb[2], 0.0],
            ambient_colour: [sun_rgb[0] * a, a * sun_rgb[1], sun_rgb[2] * a, 0.0],
            // Height fog is the water fog, phase D water.
            height_fog_plane: [0.0; 4],
            height_fog_colour: [0.0; 4],
            distance_fog_plane: fp,
            distance_fog_colour: [fc[0], fc[1], fc[2], 0.0],
            sun_rgb: [sun_rgb[0], sun_rgb[1], sun_rgb[2], 0.0],
        }
    }
}
