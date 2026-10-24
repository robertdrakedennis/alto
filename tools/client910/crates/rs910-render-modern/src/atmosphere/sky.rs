//! The modern client's sky: a cube map sampled by view direction, fogged by elevation, with a
//! glow around the sun.
//!
//! # The law
//!
//! Per pixel, with `p` the unit view direction (the cube is sampled along it), `u` the same
//! direction with `y` up and `s` the unit vector towards the sun:
//!
//! ```text
//! c = sample(cube, p)                                   // one cube, decoded from sRGB
//! c = mix(sample(previous, p), sample(current, p), t)   // while one cube fades into another
//! c = mix(c, fog colour, angle_fog(u))
//! c += pow(max(dot(s, u), 0), 128 z) * min(z u.y, 1)    // the sun's glow, on all channels
//! c += exposure
//! ```
//!
//! with `angle_fog(u) = clamp(pow(1 - clamp(u.y + w, 0, 1), z), 0, 1)`, `z = pow(start / end, e) *
//! s` and `w` from the environment record's three angle-fog values (scale `s`, exponent `e`,
//! offset `w`; [`crate::lighting::env_colour_grading`]), `start` and `end` the distance fog's
//! range, and the fog colour half the linear environment colour. So the fog colour covers the
//! horizon and all below it and fades to the cube's colour above, the glow is a broad lobe
//! around the sun that exists only together with the fog, and the exposure is 0 except while the
//! ambient capture takes its pictures
//! ([`crate::frame::ModernRenderer::set_sky_exposure_offset`]).
//!
//! There is no gradient, no sun disc, no moon, no stars and no cloud layer: the sky is the
//! cube's colour, the fog and the glow. A frame without a cube shows the flat fog colour plus the
//! exposure. The cube comes from the environment record (none: no sky) and changes cross-fade
//! over 5000 ms ([`super::sky_fade`]). The sky sits at the far plane, is drawn before the shadow
//! passes and the geometry and is the frame's background.
//!
//! # This port
//!
//! Stand-ins (not proven):
//!
//! - **The cube's texels.** The 910 cache has no cube-map sky. The cube is the environment's
//!   existing sky box (its dome model with the cloud material, or its tiled material) drawn
//!   once from the origin into the six faces ([`crate::frame::gpu::sky_box`]), in the colour
//!   the classic sky layers have (display-referred, at the sky's calibrated level), over the
//!   fog colour of that moment. A decor sprite of the box stays a screen-space layer over the
//!   cube.
//! - **Distance scattering on the sky**: none (the path length of the sky's vertices is not
//!   known), so the cube's colour is only fogged.
//! - **The frame's composite**: the sky is shaded into the frame's colour target by a pass
//!   covering the scene viewport before the geometry; the geometry covers it by depth.
//!
//! The angle fog's power and offset of a frame without distance fog are the chosen
//! [`ANGLE_FOG_POWER`] and [`ANGLE_FOG_OFFSET`].

/// Chosen: the power of the sky's angle fog when the distance fog is off (the horizon's haze
/// thickens over the lowest ~15 degrees: 0.58 at 5, 0.17 at 15).
pub const ANGLE_FOG_POWER: f32 = 6.0;
/// Chosen: the angle fog's offset when the distance fog is off (the horizon itself).
pub const ANGLE_FOG_OFFSET: f32 = 0.0;

/// The angle fog of a direction whose up component is `up`, for the record's power `z` and
/// offset `w`.
#[must_use]
pub fn angle_fog(up: f32, z: f32, w: f32) -> f32 {
    (1.0 - (up + w).clamp(0.0, 1.0)).powf(z).clamp(0.0, 1.0)
}

/// The sun's glow for `sun_dot = dot(s, u)` and the direction's up component: the lobe
/// `max(sun_dot, 0)^(128 z)` scaled by `min(z up, 1)`.
#[must_use]
pub fn sun_glow(sun_dot: f32, up: f32, z: f32) -> f32 {
    sun_dot.max(0.0).powf(128.0 * z) * (z * up).min(1.0)
}

/// The atmosphere passes' uniform slot (256 bytes, one dynamic slot per pass). The sky's `p0` is
/// the angle fog power and offset and the exposure offset (`w`), `p1` its flags (x: a cube
/// shows, y: decor sprites were drawn, z: the cubes' level divisor, w: the share of the current
/// cube), `p2` (x: a previous cube exists, y: a current cube exists, z: two cubes mix) and `c[0]`
/// the flat colour of a frame without a cube; the volumetrics and the depth of field have
/// theirs (`atmosphere::volumetric_scattering`, `post::dof`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PassUniforms {
    /// Clip (wgpu depth) to camera-local.
    pub inv_view_proj: [[f32; 4]; 4],
    /// The scene viewport `[x, y, w, h]` in target pixels.
    pub rect: [f32; 4],
    pub p0: [f32; 4],
    pub p1: [f32; 4],
    pub p2: [f32; 4],
    pub p3: [f32; 4],
    pub p4: [f32; 4],
    pub c: [[f32; 4]; 4],
    pub pad: [f32; 4],
}

/// What the sky shading draws this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyShading {
    /// A cube shows (one, or two mixing).
    pub cube: bool,
    /// The sky decor sprites were drawn into the sky's source.
    pub decor: bool,
    /// The cubes hold the classic sky's display-referred colour: their samples are divided by
    /// this (the sky's calibrated level, `Look::sky_exposure`).
    pub level: f32,
    /// A previous and a current cube exist.
    pub previous: bool,
    pub current: bool,
    /// Two cubes mix, with the share `t` of the current one.
    pub mixing: bool,
    pub t: f32,
    /// The constant exposure offset added last (0 outside the ambient captures).
    pub exposure: f32,
    /// The flat colour of a frame without a cube (HDR).
    pub flat: [f32; 3],
}

/// The sky's slot.
#[must_use]
pub fn uniforms(inv_view_proj: [[f32; 4]; 4], rect: [f32; 4], sky: &SkyShading) -> PassUniforms {
    PassUniforms {
        inv_view_proj,
        rect,
        p0: [ANGLE_FOG_POWER, ANGLE_FOG_OFFSET, 0.0, sky.exposure],
        p1: [
            f32::from(sky.cube),
            f32::from(sky.decor),
            1.0 / sky.level,
            sky.t,
        ],
        p2: [
            f32::from(sky.previous),
            f32::from(sky.current),
            f32::from(sky.mixing),
            0.0,
        ],
        c: [
            [sky.flat[0], sky.flat[1], sky.flat[2], 1.0],
            [0.0; 4],
            [0.0; 4],
            [0.0; 4],
        ],
        ..PassUniforms::default()
    }
}
