//! Light probes and image-based lighting (renderer plan M6, lane Q-M6):
//! the ambient of the forward programs from probes this renderer captures
//! from its own scene, the probe maths (CPU mirrors) and the WGSL. The GPU
//! half (targets, capture passes, projection, prefilter) is
//! `crate::frame::probes`.
//!
//! # The modern client's model (reference only, nothing copied)
//!
//! - **Shading.** The forward programs take the ambient as the ambient colour
//!   times the map square's SH lighting at the world position and normal
//!   (then SSAO): 2nd-order SH irradiance in Sloan's seven-`vec4` packing,
//!   interpolated bilinearly between the zone probes of the map square
//!   (zones of 4096 fine units, a 9x9 probe grid per 32768-unit map square,
//!   the position pushed along the normal by a sample bias, rounded and
//!   clamped). The metallic programs with global environment mapping add
//!   image-based lighting: the diffuse `SH * ambient colour * conservation
//!   factor(F, metal)` and the split-sum specular (the global cube sampled
//!   along the reflection at level `last level * roughness^2`, times
//!   `F * A + B` with `(A, B)` from the BRDF LUT at `(N.V, roughness^2)`);
//!   SSAO scales both, then the lighting sum adds the diffuse times the
//!   conservation factor once more, which is kept. The global cube is
//!   prefiltered with 1024 GGX importance samples (the source mip from the
//!   sample's pdf, soft-clamped HDR) and the LUT uses 1024 samples with a
//!   height-correlated Smith term.
//! - **Capture.** One probe per map square: a 16x16 cube, placed at capture
//!   time over the terrain's bounding-box centre, 5000 units above its top,
//!   rendered with the normal scene draw at a 90-degree FOV. Captures are
//!   queued when a map square is built or the environment changes, one face
//!   per frame; the faces are read back and projected to SH: the texels'
//!   bytes over 256 (no decode), each weighted by the cube texel's solid
//!   angle `4 / (u² + v² + 1)^1.5` and rescaled by `4π / Σ weights`; then
//!   Sloan's seven vectors are packed with the cosine lobe and `1/π` folded
//!   in (C0..C4 below). A new probe blends in over 750 ms. The global
//!   environment cube is 128x128, with no GGX prefilter or LUT.
//!
//! # What this crate does
//!
//! - **Probes** (the modern layout, the capture above): 9x9 probes at the
//!   zone spacing ([`ZONE`], aligned with the world's zones: the scene base
//!   is a multiple of 8 tiles), a window of 8x8 zones (64 tiles) around the
//!   camera target when captured (the modern client binds its map square's 81
//!   probes and clamps outside them, as the shader clamps outside the
//!   window), each probe a [`CAPTURE_RES`] cube captured from the scene (the
//!   sky layers and sky models, every floor level over all its tiles, every
//!   static and dynamic loc of the scene within [`CAPTURE_RADIUS`], the
//!   floors split by zone, what subtends less than a texel left out,
//!   [`SMALL_ANGLE`]; no NPCs, players or sprites) with the forward pass's
//!   shading and its hemisphere ambient (no feedback), without sun shadows or
//!   SSAO, at [`PROBE_HEIGHT`] above the highest ground around it (chosen: the
//!   modern per-square probe sits 5000 units above the terrain; a zone probe
//!   close to the ground would sit inside houses). Projection runs on the GPU
//!   (compute) as described above (display values: the HDR texel through the
//!   tonemap and the display encode, as the byte an RGBA8 face holds, over
//!   256; solid-angle weights; the same basis and packing). The shading
//!   interpolates the four probes around the normal-biased position
//!   bilinearly.
//! - **Normalisation** (this crate's): the modern client multiplies the SH by
//!   the ambient colour; the probe's display values carry the environment's
//!   own brightness, so the SH is divided by the luminance of one more
//!   probe's up-facing irradiance, the *global* probe captured with the
//!   environment cube, and scaled by the look calibration's sky ambient
//!   (`crate::lighting::environment::look`): open ground facing up keeps the Q-LOOK
//!   ambient, faces towards the ground take the captured ground's share,
//!   and occluders and coloured surroundings darken and tint it.
//! - **Environment cube** (the 128 cube, the GGX prefilter): one per
//!   region, captured at the camera target when the probes are captured,
//!   HDR, [`ENV_MIPS`] levels prefiltered with the GGX filter
//!   (roughness² = level / (levels - 1), as the IBL specular reads it);
//!   the BRDF LUT ([`LUT_RES`]) at start-up. The filtered-envmap
//!   normalisation variant is off (the cube is this scene's radiance).
//! - **When**: a new scene (the floor or entity set) or a settled change of
//!   the environment's colours or skybox (a key two frames equal), never per
//!   frame. A capture is spread over frames the way the modern client spreads
//!   its faces: the environment cube and the global probe first (its SH fills
//!   every probe), then [`PROBES_PER_FRAME`] probes a frame, nearest the
//!   camera first (the modern client also blends each new probe in over
//!   750 ms: not done).
//! - **Water** (lane Q-WATER2's follow-ups): the reflection pass draws with
//!   the forward pass's entry points, and the water's sky behind the
//!   reflected scene is the environment cube (the skybox and clouds show)
//!   instead of the gradient.
//!
//! Under the verified look the ambient is the per-square captured block
//! instead ([`crate::lighting::ambient`]); the zone grid above is then not
//! captured (only the global probe and the environment cube are), and the
//! calibrated look, whose values were fitted with it, keeps it.
//!
//! `CLIENT910_MODERN_PROBES=sh` shows the probe ambient's
//! light (the SH factor times the ambient colour), `=ibl` the RT7 IBL
//! specular, `=metal` the RT7 metalness (verification).

/// The zone size: the probe spacing in fine units (8 tiles).
pub const ZONE: f32 = 4096.0;
/// The probe cube face size.
pub const CAPTURE_RES: u32 = 16;
/// The global environment cube size.
pub const ENV_RES: u32 = 128;
/// The environment cube's levels (128 down to 1).
pub const ENV_MIPS: u32 = 8;
/// The BRDF LUT's size (chosen).
pub const LUT_RES: u32 = 128;
/// The sample count for the prefilter and the LUT.
pub const IBL_SAMPLES: u32 = 1024;
/// A probe's height above the highest ground within half a zone (fine
/// units, chosen; see the module docs).
pub const PROBE_HEIGHT: f32 = 1024.0;
/// The environment cube's eye above the camera target (fine units, chosen:
/// about a character's eye).
pub const ENV_EYE_HEIGHT: f32 = 256.0;
/// The capture's near plane and the probes' far plane and cull radius
/// (fine units; chosen: the modern far is 100000, the probes keep the geometry
/// within one zone, the next probe's place).
pub const CAPTURE_NEAR: f32 = 16.0;
pub const CAPTURE_RADIUS: f32 = ZONE;
/// The environment cube's cull radius (fine units; chosen: four zones, the
/// scene around the camera, the fog beyond).
pub const ENV_RADIUS: f32 = 4.0 * ZONE;
/// A capture skips what subtends less than about one texel: radius over
/// distance below a texel's angle (a 16-texel face spans 90 degrees; the
/// environment cube's 128).
pub const SMALL_ANGLE: f32 = std::f32::consts::FRAC_PI_2 / 16.0;
pub const ENV_SMALL_ANGLE: f32 = std::f32::consts::FRAC_PI_2 / 128.0;
/// The probes per axis around the camera target (the modern 9x9 per map
/// square).
pub const PROBES_PER_AXIS: usize = 9;
/// Probes captured per frame after the first (chosen; the modern client
/// renders one face per frame).
pub const PROBES_PER_FRAME: usize = 6;
/// The environment cube's far plane.
pub const ENV_FAR: f32 = 100_000.0;
/// The normal sample bias of the probe lookup (chosen: a quarter tile).
pub const NORMAL_BIAS: f32 = 128.0;
/// The probe grid's largest size (probes, plus the global one).
pub const MAX_PROBES: usize = 32 * 32;
/// The coefficient vectors per probe in the storage buffer (Sloan's seven,
/// padded to eight).
pub const PROBE_STRIDE: usize = 8;

/// The irradiance packing constants: the SH
/// basis normalisation times the clamped-cosine convolution over π.
pub const C0: f32 = 0.282_076_5;
pub const C1: f32 = 0.325_713_9;
pub const C2: f32 = 0.273_119_42;
pub const C3: f32 = 0.078_842_78;
pub const C4: f32 = 0.136_559_71;

/// The probes' output (`CLIENT910_MODERN_PROBES`, a debug view,
/// `modern_debug_flags`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    On,
    /// Debug output: 1 the probe ambient's light, 2 the IBL specular, 3 the
    /// metalness.
    Debug(u32),
}

/// The probes' output (read once).
#[must_use]
pub fn mode() -> Mode {
    crate::modern_debug_flags::flags()
        .probe_debug
        .map_or(Mode::On, Mode::Debug)
}

/// The SH basis at direction `d` (`d` in this crate's
/// classic axes, which the shader evaluates in too).
#[must_use]
pub fn basis([x, y, z]: [f32; 3]) -> [f32; 9] {
    [
        0.282_095,
        -0.488_603 * y,
        0.488_603 * z,
        -0.488_603 * x,
        1.092_548 * x * y,
        -1.092_548 * y * z,
        0.946_175 * z * z - 0.315_392,
        -1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// A cube face's axes in classic coordinates (y down): the major axis `m`,
/// and the texture's `s` (right) and `t` (down) axes, for wgpu's cube
/// convention in the cube space `c = (x, -y, z)` (the direction `m + s sc
/// + t tc` at `sc`, `tc` in -1..1 is texel `((sc + 1) / 2, (tc + 1) / 2)`).
#[must_use]
pub fn face_axes(face: usize) -> [[f32; 3]; 3] {
    match face {
        0 => [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
        1 => [[-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
        2 => [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        3 => [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        4 => [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        _ => [[0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
    }
}

/// The (unnormalised) classic direction of face `face` at `sc`, `tc`.
#[must_use]
pub fn face_direction(face: usize, sc: f32, tc: f32) -> [f32; 3] {
    let [m, s, t] = face_axes(face);
    std::array::from_fn(|i| m[i] + s[i] * sc + t[i] * tc)
}

/// Classic direction `d` in the cube space the cube textures are sampled in.
#[must_use]
pub fn to_cube([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, -y, z]
}

/// The camera-local clip matrix of face `face` seen from camera-local
/// `eye` (wgpu depth, `near`..`far` along the major axis): clip `(sc, -tc,
/// depth, ma)`, so the face's image is the cube face's texels.
#[must_use]
pub fn face_view_proj(face: usize, eye: [f32; 3], near: f32, far: f32) -> glam::Mat4 {
    let [m, s, t] = face_axes(face).map(glam::Vec3::from);
    let e = glam::Vec3::from(eye);
    let a = far / (far - near);
    let b = -far * near / (far - near);
    // Rows over (x, y, z, 1): (sc, -tc, a ma + b, ma).
    let row = |v: glam::Vec3, k: f32| glam::Vec4::new(v.x, v.y, v.z, -v.dot(e) + k);
    let rx = row(s, 0.0);
    let ry = row(-t, 0.0);
    let rw = row(m, 0.0);
    let rz = rw * a + glam::Vec4::new(0.0, 0.0, 0.0, b);
    glam::Mat4::from_cols_array_2d(&[
        [rx.x, ry.x, rz.x, rw.x],
        [rx.y, ry.y, rz.y, rw.y],
        [rx.z, ry.z, rz.z, rw.z],
        [rx.w, ry.w, rz.w, rw.w],
    ])
}

/// The face's view matrix for the forward pass's `view` (its z is the
/// distance along the major axis, the view depth of the fog and shadows).
#[must_use]
pub fn face_view(face: usize, eye: [f32; 3]) -> glam::Mat4 {
    let [m, s, t] = face_axes(face).map(glam::Vec3::from);
    let e = glam::Vec3::from(eye);
    glam::Mat4::from_cols(
        glam::Vec4::new(s.x, t.x, m.x, 0.0),
        glam::Vec4::new(s.y, t.y, m.y, 0.0),
        glam::Vec4::new(s.z, t.z, m.z, 0.0),
        glam::Vec4::new(-s.dot(e), -t.dot(e), -m.dot(e), 1.0),
    )
}

/// The classic pitch and yaw (14-bit angles) of a camera looking along face
/// `face` (the orbit camera's forward: `(-sin yaw cos pitch, sin pitch,
/// cos yaw cos pitch)`), for the 2D sky layers.
#[must_use]
pub fn face_angles(face: usize) -> (i32, i32) {
    let [d, _, _] = face_axes(face);
    let unit = 16384.0 / std::f32::consts::TAU;
    let pitch = (d[1].clamp(-1.0, 1.0).asin() * unit).round() as i32 & 0x3FFF;
    let yaw = if d[0] == 0.0 && d[2] == 0.0 {
        0
    } else {
        ((-d[0]).atan2(d[2]) * unit).round() as i32 & 0x3FFF
    };
    (pitch, yaw)
}

/// The texel's solid-angle weight at texel centre `sc`, `tc`.
#[must_use]
pub fn texel_weight(sc: f32, tc: f32) -> f32 {
    let r = sc * sc + tc * tc + 1.0;
    4.0 / (r * r.sqrt())
}

/// The SH projection of a cube given per face (wgpu face
/// order, row-major `res x res` display colours) to 9 coefficients per
/// channel.
#[must_use]
pub fn project(faces: &[Vec<[f32; 3]>; 6], res: usize) -> [[f32; 9]; 3] {
    let mut sh = [[0.0_f64; 9]; 3];
    let mut total = 0.0_f64;
    for (face, texels) in faces.iter().enumerate() {
        for j in 0..res {
            for i in 0..res {
                let sc = (2 * i + 1) as f32 / res as f32 - 1.0;
                let tc = (2 * j + 1) as f32 / res as f32 - 1.0;
                let w = texel_weight(sc, tc);
                let d = glam::Vec3::from(face_direction(face, sc, tc)).normalize();
                let y = basis(d.to_array());
                let c = texels[j * res + i];
                for ch in 0..3 {
                    for k in 0..9 {
                        sh[ch][k] += f64::from(c[ch] * y[k] * w);
                    }
                }
                total += f64::from(w);
            }
        }
    }
    let scale = 4.0 * std::f64::consts::PI / total;
    sh.map(|ch| ch.map(|v| (v * scale) as f32))
}

/// The irradiance packing: Sloan's seven vectors
/// `cAr, cAg, cAb, cBr, cBg, cBb, cC` from the coefficients.
#[must_use]
pub fn pack(sh: &[[f32; 9]; 3]) -> [[f32; 4]; 7] {
    let mut out = [[0.0; 4]; 7];
    for (ch, l) in sh.iter().enumerate() {
        out[ch] = [-C1 * l[3], -C1 * l[1], C1 * l[2], C0 * l[0] - C3 * l[6]];
        out[3 + ch] = [C2 * l[4], -C2 * l[5], 3.0 * C3 * l[6], -C2 * l[7]];
    }
    out[6] = [C4 * sh[0][8], C4 * sh[1][8], C4 * sh[2][8], 1.0];
    out
}

/// The second-order SH lighting: the irradiance
/// over π for normal `n`.
#[must_use]
pub fn evaluate(c: &[[f32; 4]; 7], [x, y, z]: [f32; 3]) -> [f32; 3] {
    let n1 = [x, y, z, 1.0];
    let vb = [x * y, y * z, z * z, z * x];
    let dot = |a: &[f32; 4], b: &[f32; 4]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let vc = x * x - y * y;
    std::array::from_fn(|ch| dot(&c[ch], &n1) + dot(&c[3 + ch], &vb) + c[6][ch] * vc)
}

/// The probe grid of a capture: probe `(i, j)` at scene-local `(x0 + i
/// ZONE, z0 + j ZONE)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeGrid {
    pub nx: usize,
    pub nz: usize,
    pub x0: f32,
    pub z0: f32,
    /// Scene-local probe positions (fine units, classic y), row-major by z.
    pub positions: Vec<[f32; 3]>,
}

impl ProbeGrid {
    /// [`PROBES_PER_AXIS`] probes per axis over the zones around scene-local
    /// `centre` (x, z), inside the `tiles_x` x `tiles_z` scene where it is
    /// large enough, each probe [`PROBE_HEIGHT`] above the highest ground
    /// `height(fx, fz)` (classic y, down) within half a zone.
    #[must_use]
    pub fn new(
        tiles_x: usize,
        tiles_z: usize,
        centre: [f32; 2],
        height: impl Fn(i32, i32) -> i32,
    ) -> Self {
        let n = PROBES_PER_AXIS;
        let span = (n - 1) as f32 * ZONE;
        let start = |c: f32, tiles: usize| {
            let zone = (c / ZONE).floor() - ((n - 1) / 2) as f32;
            let last = ((tiles * 512) as f32 - span).max(0.0);
            (zone * ZONE).clamp(0.0, (last / ZONE).floor() * ZONE)
        };
        let (x0, z0) = (start(centre[0], tiles_x), start(centre[1], tiles_z));
        let half = (ZONE / 2.0) as i32;
        let mut positions = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let (x, z) = ((x0 + i as f32 * ZONE) as i32, (z0 + j as f32 * ZONE) as i32);
                let mut top = i32::MAX;
                for dz in (-half..=half).step_by(256) {
                    for dx in (-half..=half).step_by(256) {
                        top = top.min(height(x + dx, z + dz));
                    }
                }
                positions.push([x as f32, top as f32 - PROBE_HEIGHT, z as f32]);
            }
        }
        Self {
            nx: n,
            nz: n,
            x0,
            z0,
            positions,
        }
    }

    /// The four probes and bilinear weights the modern zone interpolation takes
    /// at scene-local `(x, z)` (clamped into the grid).
    #[must_use]
    pub fn weights(&self, x: f32, z: f32) -> [(usize, f32); 4] {
        let p = |v: f32, n: usize| (v / ZONE).clamp(0.0, (n.max(2) - 1) as f32);
        let (px, pz) = (p(x - self.x0, self.nx), p(z - self.z0, self.nz));
        let ix = (px.floor() as usize).min(self.nx.saturating_sub(2));
        let iz = (pz.floor() as usize).min(self.nz.saturating_sub(2));
        let (tx, tz) = (px - ix as f32, pz - iz as f32);
        let at = |i: usize, j: usize| (j.min(self.nz - 1)) * self.nx + i.min(self.nx - 1);
        [
            (at(ix, iz), (1.0 - tx) * (1.0 - tz)),
            (at(ix + 1, iz), tx * (1.0 - tz)),
            (at(ix, iz + 1), (1.0 - tx) * tz),
            (at(ix + 1, iz + 1), tx * tz),
        ]
    }
}

/// The Hammersley sequence (bit-reversed radical inverse).
#[must_use]
pub fn hammersley(i: u32, n: u32) -> [f32; 2] {
    [i as f32 / n as f32, i.reverse_bits() as f32 * 2.328_31e-10]
}

/// GGX importance sampling: the half vector
/// of sample `u` about `n` for GGX parameter `p` (its alpha is `p²`).
#[must_use]
pub fn ggx_importance_sample(u: [f32; 2], n: [f32; 3], p: f32) -> [f32; 3] {
    let v = p * p;
    let f = std::f32::consts::TAU * u[0];
    let c = (1.0 - u[1]) / (1.0 + (v - 1.0) * u[1]);
    let (e, s) = (c.sqrt(), (1.0 - c).sqrt());
    let h = glam::Vec3::new(s * f.cos(), s * f.sin(), e);
    let t = glam::Vec3::from(n);
    let r = if t.z.abs() < 0.999 {
        glam::Vec3::Z
    } else {
        glam::Vec3::X
    };
    let i = r.cross(t).normalize();
    let o = i.cross(t);
    (i * h.x + o * h.y + t * h.z).normalize().to_array()
}

/// The Smith GGX lambda and the height-correlated Smith geometry term.
fn smith_correlated(n_dot_v: f32, n_dot_l: f32, rough: f32) -> f32 {
    let lambda = |c: f32| {
        let s = rough * rough;
        let a = (1.0 - (c * c).min(1.0)).sqrt();
        let t = a / c.max(1e-7);
        ((1.0 + s * s * t * t).sqrt() - 1.0) / 2.0
    };
    1.0 / (1.0 + lambda(n_dot_v) + lambda(n_dot_l))
}

/// The BRDF integration: the split-sum scale and bias at
/// `n_dot_v` and roughness `m` over `samples` samples.
#[must_use]
pub fn integrate_brdf(n_dot_v: f32, m: f32, samples: u32) -> [f32; 2] {
    let s = m * m;
    let v = glam::Vec3::new((1.0 - n_dot_v * n_dot_v).max(0.0).sqrt(), 0.0, n_dot_v);
    let (mut a, mut b) = (0.0, 0.0);
    for i in 0..samples {
        let h = glam::Vec3::from(ggx_importance_sample(
            hammersley(i, samples),
            [0.0, 0.0, 1.0],
            s,
        ));
        let l = (2.0 * v.dot(h) * h - v).normalize();
        let (nl, nv, nh, vh) = (l.z, v.z.max(0.0), h.z.max(0.0), v.dot(h).max(0.0));
        if nl > 0.0 {
            let g = smith_correlated(nv, nl, m);
            let vis = (g * vh / (nh * nv).max(1e-7)).clamp(0.0, 1.0);
            let fc = (1.0 - vh).powi(5);
            a += (1.0 - fc) * vis;
            b += fc * vis;
        }
    }
    [a / samples as f32, b / samples as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each face's clip matrix puts a direction on its face at the cube
    /// texel `face_direction` names, in front (w > 0) and in depth range.
    #[test]
    fn face_matrices_follow_the_cube_convention() {
        let eye = [100.0, -200.0, 300.0];
        for face in 0..6 {
            let m = face_view_proj(face, eye, CAPTURE_NEAR, 1000.0);
            for (sc, tc) in [(0.0, 0.0), (0.5, -0.25), (-0.9, 0.9)] {
                let d = glam::Vec3::from(face_direction(face, sc, tc)).normalize();
                let p = glam::Vec3::from(eye) + d * 500.0;
                let c = m * p.extend(1.0);
                assert!(c.w > 0.0);
                let ndc = c.truncate() / c.w;
                assert!(
                    (ndc.x - sc).abs() < 1e-4,
                    "face {face}: {ndc} vs ({sc}, {tc})"
                );
                assert!(
                    (ndc.y + tc).abs() < 1e-4,
                    "face {face}: {ndc} vs ({sc}, {tc})"
                );
                assert!((0.0..=1.0).contains(&ndc.z));
                let view = face_view(face, eye) * p.extend(1.0);
                assert!((view.z - c.w).abs() < 1e-3);
            }
            // The cube-space direction of the face's centre is the face's
            // own axis in wgpu's order (+X, -X, +Y, -Y, +Z, -Z).
            let c = to_cube(face_direction(face, 0.0, 0.0));
            let axis = face / 2;
            assert_eq!(c[axis], if face % 2 == 0 { 1.0 } else { -1.0 });
        }
    }

    /// The side faces look along the horizon at the classic yaws of their axes;
    /// up and down look straight up and down.
    #[test]
    fn face_angles_follow_the_orbit_camera() {
        assert_eq!(face_angles(4), (0, 0)); // +Z: yaw 0
        assert_eq!(face_angles(0), (0, 12288)); // +X: -sin(yaw) = 1
        assert_eq!(face_angles(1), (0, 4096));
        assert_eq!(face_angles(5), (0, 8192));
        assert_eq!(face_angles(2).0, 12288); // up (classic -y): pitch -90°
        assert_eq!(face_angles(3).0, 4096);
    }

    /// The grid is the modern 9x9 at its zone spacing around the camera,
    /// inside the scene, probes above the ground, and its weights are the
    /// modern bilinear ones: continuous, summing to one, the probe itself at its
    /// position, clamped outside.
    #[test]
    fn grid_covers_the_camera_and_blends_bilinearly() {
        let grid = ProbeGrid::new(104, 104, [26_000.0, 30_000.0], |x, z| -(x + z) / 64);
        assert_eq!((grid.nx, grid.nz), (9, 9));
        // Zone 6 (x) and 7 (z) hold the centre: four zones either side.
        assert_eq!((grid.x0, grid.z0), (2.0 * ZONE, 3.0 * ZONE));
        let p = grid.positions[10];
        assert_eq!((p[0], p[2]), (3.0 * ZONE, 4.0 * ZONE));
        assert!(p[1] < -((7.0 * ZONE) as i32 / 64) as f32 - PROBE_HEIGHT + 1.0);
        // Near the scene's edge the window stays inside.
        let edge = ProbeGrid::new(104, 104, [1000.0, 52_000.0], |_, _| 0);
        assert_eq!(edge.x0, 0.0);
        assert!(edge.z0 + 8.0 * ZONE <= 104.0 * 512.0);
        let at = |x: f32, z: f32| {
            let w = grid.weights(x, z);
            assert!((w.iter().map(|p| p.1).sum::<f32>() - 1.0).abs() < 1e-5);
            w
        };
        let w = at(grid.x0 + 3.0 * ZONE, grid.z0 + 5.0 * ZONE);
        assert!(w
            .iter()
            .any(|&(i, f)| i == 5 * 9 + 3 && (f - 1.0).abs() < 1e-6));
        // Continuity across a zone edge.
        let value = |x: f32| {
            at(x, grid.z0 + 7000.0)
                .iter()
                .map(|&(i, f)| i as f32 * f)
                .sum::<f32>()
        };
        let edge_x = grid.x0 + 2.0 * ZONE;
        assert!((value(edge_x - 0.1) - value(edge_x + 0.1)).abs() < 0.01);
        // Outside: clamped to the edge probes.
        assert_eq!(at(-5000.0, -5000.0)[0], (0, 1.0));
    }
}
